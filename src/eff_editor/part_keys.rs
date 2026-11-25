//! Adding and removing the keys of a part's colours, opacities and size.
//!
//! A part's shader is compiled for the keys it shipped with; one given more is extended as the
//! file is written (`eff_export::widen_shaders`). What is edited here is the part's own data: how
//! many keys a table has, where each is, and whether it runs through them at all.

use eff_wgpu::maxwell::keys::{KeysRead, Table};

use super::*;

/// The slots a key table has.
const SLOTS: usize = crate::eff_attrs::KEY_SLOTS as usize;

/// The attributes one table is kept in.
struct Names {
    count: &'static str,
    keys: &'static str,
    /// The numbers of a key, besides its time.
    numbers: &'static [&'static str],
    /// How the value is chosen, and the constant it is when that says so.
    constant: Option<(&'static str, &'static [&'static str])>,
}

/// A colour type's stored values: the part's constant, its keys over a particle's life, or one
/// key picked for each particle.
const CONSTANT: i64 = 0;
const OVER_LIFE: i64 = 2;

fn names(table: Table) -> Names {
    const XYZ: &[&str] = &["x", "y", "z"];
    match table {
        Table::Colour0 => Names {
            count: "emitter_static.num_color0_keys",
            keys: "emitter_static.color0",
            numbers: XYZ,
            constant: Some((
                "particle_color.color0_type",
                &[
                    "particle_color.color0_r",
                    "particle_color.color0_g",
                    "particle_color.color0_b",
                ],
            )),
        },
        Table::Alpha0 => Names {
            count: "emitter_static.num_alpha0_keys",
            keys: "emitter_static.alpha0",
            numbers: &XYZ[..1],
            constant: Some(("particle_color.alpha0_type", &["particle_color.alpha0"])),
        },
        Table::Colour1 => Names {
            count: "emitter_static.num_color1_keys",
            keys: "emitter_static.color1",
            numbers: XYZ,
            constant: Some((
                "particle_color.color1_type",
                &[
                    "particle_color.color1_r",
                    "particle_color.color1_g",
                    "particle_color.color1_b",
                ],
            )),
        },
        Table::Alpha1 => Names {
            count: "emitter_static.num_alpha1_keys",
            keys: "emitter_static.alpha1",
            numbers: &XYZ[..1],
            constant: Some(("particle_color.alpha1_type", &["particle_color.alpha1"])),
        },
        Table::Scale => Names {
            count: "emitter_static.num_scale_keys",
            keys: "emitter_static.scale_anim",
            numbers: XYZ,
            constant: None,
        },
    }
}

/// The table an attribute is the key count or the type of.
fn table_of(id: &str) -> Option<Table> {
    Table::ALL.into_iter().find(|table| {
        let names = names(*table);
        names.count == id || names.constant.is_some_and(|(kind, _)| kind == id)
    })
}

/// Whether an attribute says how many keys a table has.
pub(super) fn sets_keys(id: &str) -> bool {
    table_of(id).is_some()
}

/// The table a row of the attribute list belongs to: its count, its type, or one of its keys.
pub(super) fn row_table(id: &str) -> Option<Table> {
    table_of(id).or_else(|| {
        Table::ALL
            .into_iter()
            .find(|table| id.starts_with(names(*table).keys) && id.contains(".keys["))
    })
}

/// How the part's shader uses a table; as one that runs through it when the shader can't be read.
pub(super) fn used(em: &EmitterDef, table: Table) -> KeysRead {
    em.shader_use().map_or(
        KeysRead {
            keys: 1,
            picks: false,
        },
        |shader| shader.keys[table as usize],
    )
}

/// Why a row of a table does nothing, when the part's shader has no use for it.
pub(super) fn idle_reason(em: &EmitterDef, id: &str) -> Option<&'static str> {
    let read = used(em, row_table(id)?);
    (read.keys == 0 && !read.picks).then_some("This part's shader does not use this.")
}

fn number(em: &EmitterDef, id: &str) -> f32 {
    attr_f32(em, id)
}

fn slot_id(table: &str, key: usize, number: &str) -> String {
    format!("{table}.keys[{key}].{number}")
}

/// One key: its numbers and its time.
type Key = ([f32; 3], f32);

fn key_at(em: &EmitterDef, names: &Names, key: usize) -> Key {
    let mut value = [0.0; 3];
    for (slot, name) in value.iter_mut().zip(names.numbers) {
        *slot = number(em, &slot_id(names.keys, key, name));
    }
    (value, number(em, &slot_id(names.keys, key, "time")))
}

fn set_key(em: &mut EmitterDef, names: &Names, key: usize, (value, time): Key) {
    for (value, name) in value.iter().zip(names.numbers) {
        set_attr(
            em,
            &slot_id(names.keys, key, name),
            AttrValue::Float(*value),
        );
    }
    set_attr(
        em,
        &slot_id(names.keys, key, "time"),
        AttrValue::Float(time),
    );
}

fn is_constant(em: &EmitterDef, names: &Names) -> bool {
    names
        .constant
        .is_some_and(|(kind, _)| attr(em, kind).is_some_and(|v| v.as_i64() == CONSTANT))
}

/// How many keys a table has; none while its value is a constant.
pub(super) fn count(em: &EmitterDef, table: Table) -> usize {
    let names = names(table);
    if is_constant(em, &names) {
        return 0;
    }
    attr(em, names.count).map_or(0, |v| v.as_i64().clamp(0, SLOTS as i64) as usize)
}

fn set_count(em: &mut EmitterDef, names: &Names, count: usize) {
    set_attr(em, names.count, AttrValue::Int(count as i64));
}

/// Make a constant the table's one key, so that others can be added to it.
fn from_constant(em: &mut EmitterDef, names: &Names) {
    let Some((kind, constant)) = names.constant else {
        return;
    };
    let mut value = [0.0; 3];
    for (slot, id) in value.iter_mut().zip(constant) {
        *slot = number(em, id);
    }
    set_key(em, names, 0, (value, 0.0));
    set_count(em, names, 1);
    set_attr(em, kind, AttrValue::Int(OVER_LIFE));
}

/// Add a key after the last. Nothing looks different until it is edited: at the end of the life
/// it holds the last key's value, and before a key already there it is half way to it.
pub(super) fn add(em: &mut EmitterDef, table: Table) -> bool {
    let names = names(table);
    if count(em, table) == 0 {
        from_constant(em, &names);
    }
    let n = count(em, table);
    if n == 0 || n >= SLOTS {
        return false;
    }
    let last = key_at(em, &names, n - 1);
    if used(em, table).picks {
        set_key(em, &names, n, last);
    } else if n == 1 {
        set_key(em, &names, 0, (last.0, 0.0));
        set_key(em, &names, 1, (last.0, 1.0));
    } else if last.1 < 0.999 {
        set_key(em, &names, n, (last.0, 1.0));
    } else {
        let before = key_at(em, &names, n - 2);
        let half = std::array::from_fn(|i| (before.0[i] + last.0[i]) / 2.0);
        set_key(em, &names, n - 1, (half, (before.1 + last.1) / 2.0));
        set_key(em, &names, n, last);
    }
    set_count(em, &names, n + 1);
    true
}

/// Remove a key, moving the later ones up. The last one left stays.
pub(super) fn remove(em: &mut EmitterDef, table: Table, key: usize) -> bool {
    let names = names(table);
    let n = count(em, table);
    if n < 2 || key >= n {
        return false;
    }
    for k in key..n - 1 {
        let next = key_at(em, &names, k + 1);
        set_key(em, &names, k, next);
    }
    set_count(em, &names, n - 1);
    true
}

/// Carry a change of a table's key count or type through to its keys. The change has been made;
/// `was` is the row's value before it.
pub(super) fn follow(em: &mut EmitterDef, pr: &EmitterSnapshot, id: &str, was: AttrValue) {
    let Some(table) = table_of(id) else {
        return;
    };
    let names = names(table);
    let Some(now) = attr(em, id) else {
        return;
    };
    if id == names.count {
        let want = now.as_i64().clamp(0, SLOTS as i64) as usize;
        set_attr(em, id, was);
        while count(em, table) < want && add(em, table) {}
        // The other way round from adding: the first and the last key stay where they are.
        while count(em, table) > want.max(1) {
            let n = count(em, table);
            let picks = used(em, table).picks;
            remove(em, table, if picks || n == 2 { n - 1 } else { n - 2 });
        }
    } else if now.as_i64() == CONSTANT {
        // A shader that runs through keys still does: the game fills them all from the first,
        // which it makes the constant, when the table has the one.
        if used(em, table).keys > 1 {
            set_count(em, &names, 1);
        }
    } else if was.as_i64() == CONSTANT {
        if pr.attr(id).is_some_and(|v| v.as_i64() != CONSTANT) {
            put_back(em, pr, &names);
        } else {
            from_constant(em, &names);
        }
        set_attr(em, id, now);
    }
}

/// Put back the whole table an attribute is the key count or the type of, as the file has it.
pub(super) fn reset(em: &mut EmitterDef, pr: &EmitterSnapshot, id: &str) {
    if let Some(table) = table_of(id) {
        put_back(em, pr, &names(table));
    }
}

/// Put a table's type, count and keys back to the file's.
fn put_back(em: &mut EmitterDef, pr: &EmitterSnapshot, names: &Names) {
    let mut ids = vec![names.count.to_owned()];
    ids.extend(names.constant.map(|(kind, _)| kind.to_owned()));
    for key in 0..SLOTS {
        for number in names.numbers.iter().chain(&["time"]) {
            ids.push(slot_id(names.keys, key, number));
        }
    }
    for id in ids {
        if let Some(value) = pr.attr(&id) {
            set_attr(em, &id, value);
        }
    }
}

/// The Add key button under a table's keys.
pub(super) fn add_button(ui: &mut Ui, em: &mut EmitterDef, table: Table) -> bool {
    let read = used(em, table);
    let n = count(em, table).max(1);
    let button = ui.add_enabled(
        (read.keys > 0 || read.picks) && n < SLOTS,
        egui::Button::new("Add key").small(),
    );
    let button = button.on_disabled_hover_text(if n >= SLOTS {
        "A part has eight keys at most."
    } else {
        "This part's shader does not use this."
    });
    button.clicked() && add(em, table)
}

/// The end of a table drawn from the part's own colours: drop the key its row was asked to, and
/// offer another.
pub(super) fn end_table(
    ui: &mut Ui,
    em: &mut EmitterDef,
    table: Table,
    removed: Option<usize>,
) -> bool {
    effective_keys_to_attrs(em);
    let changed = removed.is_some_and(|key| remove(em, table, key)) | add_button(ui, em, table);
    if changed {
        attrs_to_effective_keys(em);
    }
    changed
}

/// The button that removes one key, while there is another.
pub(super) fn remove_button(ui: &mut Ui, keys: usize) -> bool {
    keys > 1
        && ui
            .small_button("✖")
            .on_hover_text("Remove this key")
            .clicked()
}

/// The keys of a table that has no controls of its own: the part's size or second opacity over
/// each particle's life.
pub(super) fn draw(ui: &mut Ui, em: &mut EmitterDef, pr: &EmitterSnapshot, table: Table) -> bool {
    let read = used(em, table);
    let names = names(table);
    if read.keys == 0 || attr(em, names.count).is_none() {
        return false;
    }
    let (title, hover) = match table {
        Table::Scale => (
            "Size over its life",
            "What each particle's size is multiplied by, from the start of its life to its end",
        ),
        _ => (
            "Second opacity over its life",
            "A second opacity the part's shader can use, over each particle's life",
        ),
    };
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong())
        .on_hover_text(hover);
    let mut changed = false;
    let mut removed = None;
    let n = count(em, table);
    let shown: Vec<Key> = if n == 0 {
        // The constant, at no time of its own.
        let constant = names.constant.map_or(&[][..], |(_, ids)| ids);
        let mut value = [0.0; 3];
        for (slot, id) in value.iter_mut().zip(constant) {
            *slot = number(em, id);
        }
        vec![(value, 0.0)]
    } else {
        (0..n).map(|k| key_at(em, &names, k)).collect()
    };
    for (i, mut key) in shown.into_iter().enumerate() {
        ui.horizontal(|ui| {
            let mut edited = false;
            for (value, axis) in key
                .0
                .iter_mut()
                .zip(["X ", "Y ", "Z "])
                .take(names.numbers.len())
            {
                let mut drag = egui::DragValue::new(value).speed(0.01).max_decimals(3);
                if names.numbers.len() > 1 {
                    drag = drag.prefix(axis);
                }
                edited |= ui.add(drag).changed();
            }
            if n > 0 {
                ui.label("at").on_hover_text(
                    "When in the particle's life this is reached: 0% when it appears, 100% when \
                     it is gone",
                );
                edited |= life_time_field(ui, &mut key.1);
            }
            if edited {
                if n == 0 {
                    for (value, id) in key.0.iter().zip(names.constant.map_or(&[][..], |c| c.1)) {
                        set_attr(em, id, AttrValue::Float(*value));
                    }
                } else {
                    set_key(em, &names, i, key);
                }
                changed = true;
            }
            if let Some(was) = pr
                .attr(&slot_id(names.keys, i, "x"))
                .filter(|_| n > 0 && pr.count(&names) > i)
            {
                let was = was.as_f32();
                if (was - key.0[0]).abs() > 1e-6 {
                    ui.label(
                        egui::RichText::new(format!("was {was:.2}"))
                            .small()
                            .color(egui::Color32::GRAY),
                    );
                }
            }
            if remove_button(ui, n) {
                removed = Some(i);
            }
        });
    }
    if let Some(key) = removed {
        changed |= remove(em, table, key);
    }
    changed | add_button(ui, em, table)
}

impl EmitterSnapshot {
    fn count(&self, names: &Names) -> usize {
        self.attr(names.count)
            .map_or(0, |v| v.as_i64().max(0) as usize)
    }
}

#[cfg(test)]
mod tests;
