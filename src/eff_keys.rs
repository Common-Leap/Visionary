//! Shaders for parts given more keys than theirs run through.
//!
//! The game's effect shaders are each compiled for the key counts of the part they draw, and read
//! no more keys than that whatever the file says. Where an edit gives a part's colour, opacity or
//! size more, the part is pointed at a copy of its shader that runs through all eight
//! (`eff_wgpu::maxwell::keys`), added to the file's.

use std::collections::{BTreeSet, HashMap};

use anyhow::{anyhow, Context, Result};
use eff_wgpu::bnsh;
use eff_wgpu::maxwell::keys::{self, KeysRead, StageCode, Table};

use crate::mod_project::AuthoredEdit;

/// Whether an edit changes how many keys a table has, or whether it runs through them.
pub fn changes_keys(edit: &AuthoredEdit) -> bool {
    edit.fields.attrs.keys().any(|id| {
        crate::eff_attrs::is_key_count(id)
            || (id.starts_with("particle_color.") && id.ends_with("_type"))
    })
}

/// The tables of a part that have more keys than its shader runs through, as a bit for each.
fn short_tables(
    data: &effect_library::EmitterData,
    read: &[KeysRead; 5],
) -> Result<Vec<Table>, Table> {
    use effect_library::ColorType::Animated8Key;
    let (stat, colour) = (&data.emitter_static, &data.particle_color);
    let runs = |kind| matches!(kind, Animated8Key);
    let mut short = Vec::new();
    for (table, count, animated) in [
        (
            Table::Colour0,
            stat.num_color0_keys,
            runs(colour.color0_type),
        ),
        (
            Table::Alpha0,
            stat.num_alpha0_keys,
            runs(colour.alpha0_type),
        ),
        (
            Table::Colour1,
            stat.num_color1_keys,
            runs(colour.color1_type),
        ),
        (
            Table::Alpha1,
            stat.num_alpha1_keys,
            runs(colour.alpha1_type),
        ),
        (Table::Scale, stat.num_scale_keys, true),
    ] {
        let read = read[table as usize];
        if animated && count > read.keys && read.keys > 0 {
            if read.picks {
                return Err(table);
            }
            short.push(table);
        }
    }
    Ok(short)
}

fn each_mut(
    emitters: &mut [effect_library::structs::Emitter],
    f: &mut impl FnMut(&mut effect_library::structs::Emitter),
) {
    for emitter in emitters {
        f(emitter);
        each_mut(&mut emitter.children, f);
    }
}

/// Give the parts of `sets` shaders that run through all the keys they have. What could not be
/// done is told in `warnings`; the part then runs through the keys its own shader has.
pub fn widen(
    ptcl: &mut effect_library::PtclFile,
    sets: &BTreeSet<usize>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let Some(binary) = ptcl
        .shader_info
        .as_ref()
        .and_then(|info| info.binary_data.as_ref())
        .filter(|binary| !sets.is_empty() && !binary.is_empty())
    else {
        return Ok(());
    };
    let shaders =
        bnsh::ShaderFile::read(binary).map_err(|e| anyhow!("reading the shaders: {e}"))?;
    let mut container: Option<effect_library::bnsh::BnshFile> = None;
    // Each program's reading of the tables, and the copies made of it.
    let mut reads: HashMap<i32, Option<[KeysRead; 5]>> = HashMap::new();
    let mut copies: HashMap<(i32, Vec<Table>), Option<i32>> = HashMap::new();
    let mut failed: Option<anyhow::Error> = None;
    for &set in sets {
        let Some(set) = ptcl.emitter_list.emitter_sets.get_mut(set) else {
            continue;
        };
        each_mut(&mut set.emitters, &mut |emitter| {
            let data = &mut emitter.data;
            let mut changed = false;
            let mut refs = data.shader_references.clone();
            for index in [
                &mut refs.shader_index,
                &mut refs.user_shader_index1,
                &mut refs.user_shader_index2,
            ] {
                let stage = |index: i32| {
                    let program = shaders.programs.get(usize::try_from(index).ok()?)?;
                    program.stages[bnsh::STAGE_VERTEX].as_ref()
                };
                let Some(read) = *reads
                    .entry(*index)
                    .or_insert_with(|| stage(*index).map(|s| keys::keys_read(&s.code)))
                else {
                    continue;
                };
                let tables = match short_tables(data, &read) {
                    Ok(tables) if tables.is_empty() => continue,
                    Ok(tables) => tables,
                    Err(_) => {
                        warnings.push(format!(
                            "keys:{}: its shader picks one key for each particle",
                            data.display_name()
                        ));
                        continue;
                    }
                };
                let copy = *copies.entry((*index, tables.clone())).or_insert_with(|| {
                    let stage = stage(*index)?;
                    if container.is_none() {
                        match effect_library::bnsh::BnshFile::read(binary) {
                            Ok(file) => container = Some(file),
                            Err(e) => {
                                failed = Some(anyhow!(e).context("reading the shaders"));
                                return None;
                            }
                        }
                    }
                    let file = container.as_mut()?;
                    let mut variation = file.variations.get(*index as usize)?.clone();
                    let code = variation.binary_program.stages[bnsh::STAGE_VERTEX].as_mut()?;
                    let wide = keys::animate(
                        &StageCode {
                            control: std::mem::take(&mut code.control_code),
                            code: std::mem::take(&mut code.byte_code),
                        },
                        &stage.reflection.inputs,
                        &tables,
                    );
                    match wide {
                        Ok(wide) => {
                            code.control_code = wide.control;
                            code.byte_code = wide.code;
                            file.variations.push(variation);
                            Some(file.variations.len() as i32 - 1)
                        }
                        Err(why) => {
                            warnings.push(format!("keys:{}: {why}", data.display_name()));
                            None
                        }
                    }
                });
                if let Some(copy) = copy {
                    *index = copy;
                    changed = true;
                }
            }
            if changed {
                data.shader_references = refs;
                // Serializer prefers the cached EMTR blob; clearing it re-encodes `data`.
                emitter.cached_binary = None;
            }
        });
    }
    if let Some(failed) = failed {
        return Err(failed);
    }
    if !copies.values().any(Option::is_some) {
        return Ok(());
    }
    let container = container.context("no shaders were read")?;
    let info = ptcl.shader_info.as_mut().context("the file has shaders")?;
    info.binary_data = Some(container.write());
    Ok(())
}

#[cfg(test)]
mod tests;
