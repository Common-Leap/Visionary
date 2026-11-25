use super::*;
use crate::eff_attrs::AttrValue;
use crate::mod_project::{EffMod, EmitterFieldEdits};

const MARIO: &str = "effect/fighter/mario/ef_mario.eff";

fn mario() -> Option<Vec<u8>> {
    let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT") else {
        eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
        return None;
    };
    Some(std::fs::read(std::path::Path::new(&root).join(MARIO)).unwrap())
}

fn load(bytes: &[u8]) -> effect_library::PtclFile {
    let file = effect_library::NamcoEffectFile::load(bytes).unwrap();
    file.ptcl_file.unwrap()
}

fn programs(ptcl: &effect_library::PtclFile) -> bnsh::ShaderFile {
    let info = ptcl.shader_info.as_ref().unwrap();
    bnsh::ShaderFile::read(info.binary_data.as_ref().unwrap()).unwrap()
}

/// The file with one part's attributes changed.
fn edited(bytes: &[u8], set: usize, part: usize, attrs: &[(&str, AttrValue)]) -> Vec<u8> {
    let ptcl = load(bytes);
    let set_name = ptcl.emitter_list.emitter_sets[set].name.clone();
    let eff = EffMod {
        source_rel: MARIO.into(),
        authored: vec![AuthoredEdit {
            emitter_name: ptcl.emitter_list.emitter_sets[set].emitters[part]
                .data
                .display_name(),
            entry_name: String::new(),
            set_name,
            set_idx: set,
            emitter_idx: part,
            fields: EmitterFieldEdits {
                attrs: attrs.iter().map(|(id, v)| (id.to_string(), *v)).collect(),
                ..Default::default()
            },
        }],
        ..Default::default()
    };
    crate::eff_export::rebuild_eff_bytes(bytes, &eff, None).unwrap()
}

/// A part on its own whose opacity runs through `keys` keys, with no part made from it.
fn part_with(ptcl: &effect_library::PtclFile, keys: u32) -> (usize, usize) {
    let sets = &ptcl.emitter_list.emitter_sets;
    let shaders = programs(ptcl);
    (0..sets.len())
        .flat_map(|s| (0..sets[s].emitters.len()).map(move |e| (s, e)))
        .find(|&(s, e)| {
            let data = &sets[s].emitters[e].data;
            let index = data.shader_references.shader_index as usize;
            let stage = shaders.programs[index].stages[bnsh::STAGE_VERTEX].as_ref();
            stage.is_some_and(|s| keys::keys_read(&s.code)[Table::Alpha0 as usize].keys == keys)
                && data.emitter_static.num_alpha0_keys == keys
                && sets[s].emitters.len() > 1
        })
        .expect("a part with that many keys")
}

#[test]
fn a_part_given_keys_gets_a_shader_that_runs_through_them() {
    let Some(bytes) = mario() else { return };
    let before = load(&bytes);
    let (set, part) = part_with(&before, 2);
    let out = edited(
        &bytes,
        set,
        part,
        &[
            ("emitter_static.num_alpha0_keys", AttrValue::Int(3)),
            ("emitter_static.alpha0.keys[2].x", AttrValue::Float(0.5)),
            ("emitter_static.alpha0.keys[2].time", AttrValue::Float(1.0)),
        ],
    );
    let after = load(&out);
    let (was, now) = (programs(&before), programs(&after));
    assert_eq!(now.programs.len(), was.programs.len() + 1);
    let index = |ptcl: &effect_library::PtclFile, e: usize| {
        ptcl.emitter_list.emitter_sets[set].emitters[e]
            .data
            .shader_references
            .shader_index as usize
    };
    assert_eq!(index(&after, part), was.programs.len());
    let read = |shaders: &bnsh::ShaderFile, index: usize| {
        keys::keys_read(
            &shaders.programs[index].stages[bnsh::STAGE_VERTEX]
                .as_ref()
                .unwrap()
                .code,
        )
    };
    let (old, new) = (
        read(&was, index(&before, part)),
        read(&now, index(&after, part)),
    );
    for table in Table::ALL {
        let want = if table == Table::Alpha0 {
            8
        } else {
            old[table as usize].keys
        };
        assert_eq!(new[table as usize].keys, want, "{table:?}");
    }
    // Its pixels are drawn as they were, and so is every other part.
    let pixels = |shaders: &bnsh::ShaderFile, index: usize| {
        shaders.programs[index].stages[bnsh::STAGE_FRAGMENT]
            .as_ref()
            .unwrap()
            .code
            .clone()
    };
    assert_eq!(
        pixels(&was, index(&before, part)),
        pixels(&now, index(&after, part))
    );
    for (a, b) in was.programs.iter().zip(&now.programs) {
        for (a, b) in a.stages.iter().zip(&b.stages) {
            assert_eq!(a.as_ref().map(|s| &s.code), b.as_ref().map(|s| &s.code));
        }
    }
    for e in 0..before.emitter_list.emitter_sets[set].emitters.len() {
        if e != part {
            assert_eq!(index(&before, e), index(&after, e));
        }
    }
    // Written again from what was written, it is not widened twice.
    let again = load(&edited(
        &out,
        set,
        part,
        &[("emitter_static.num_alpha0_keys", AttrValue::Int(5))],
    ));
    let index = index(&again, part);
    assert_eq!(read(&programs(&again), index)[1].keys, 8);
    // The shader no part uses any more is dropped on the way.
    assert_eq!(programs(&again).programs.len(), was.programs.len());
}

#[test]
fn a_constant_given_keys_gets_a_shader_that_runs_through_them() {
    let Some(bytes) = mario() else { return };
    let before = load(&bytes);
    let sets = &before.emitter_list.emitter_sets;
    let shaders = programs(&before);
    let (set, part) = (0..sets.len())
        .flat_map(|s| (0..sets[s].emitters.len()).map(move |e| (s, e)))
        .find(|&(s, e)| {
            let data = &sets[s].emitters[e].data;
            let stage = shaders.programs[data.shader_references.shader_index as usize].stages[0]
                .as_ref()
                .unwrap();
            matches!(
                data.particle_color.color0_type,
                effect_library::ColorType::Constant
            ) && keys::keys_read(&stage.code)[0].keys == 1
        })
        .unwrap();
    let after = load(&edited(
        &bytes,
        set,
        part,
        &[
            ("particle_color.color0_type", AttrValue::Int(2)),
            ("emitter_static.num_color0_keys", AttrValue::Int(2)),
        ],
    ));
    let index = after.emitter_list.emitter_sets[set].emitters[part]
        .data
        .shader_references
        .shader_index as usize;
    let code = &programs(&after).programs[index].stages[0]
        .as_ref()
        .unwrap()
        .code
        .clone();
    assert_eq!(keys::keys_read(code)[0].keys, 8);
}

/// Fewer keys than the shader runs through need no other shader: the game fills the rest with
/// copies of the last.
#[test]
fn fewer_keys_keep_the_shader() {
    let Some(bytes) = mario() else { return };
    let before = load(&bytes);
    let (set, part) = part_with(&before, 3);
    let after = load(&edited(
        &bytes,
        set,
        part,
        &[("emitter_static.num_alpha0_keys", AttrValue::Int(2))],
    ));
    assert_eq!(
        programs(&after).programs.len(),
        programs(&before).programs.len()
    );
}
