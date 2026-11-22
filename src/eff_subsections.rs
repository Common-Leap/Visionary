//! Editors for EMTR child sections documented by EffectResearch.
//!
//! These blocks are little-endian in real files (and in EffectLibraryRust's parser). The
//! research converter uses native little-endian too, despite an older prose note saying big.

use egui::Ui;

use crate::effects::{EmitterDef, EmitterSubsectionDef};

#[derive(Clone, Copy)]
enum Kind {
    U8,
    U32,
    F32,
    /// An on/off byte. Shown as a box to tick while it holds 0 or 1, as a number otherwise.
    Flag8,
    /// An on/off word, shown as [`Kind::Flag8`].
    Flag32,
    /// A float the game's files only fill with whole numbers: a count.
    Count,
    /// A word choosing the X, Y or Z axis.
    Axis,
}

impl Kind {
    /// Bytes the field takes in the section.
    fn width(self) -> usize {
        match self {
            Kind::U8 | Kind::Flag8 => 1,
            Kind::U32 | Kind::F32 | Kind::Flag32 | Kind::Count | Kind::Axis => 4,
        }
    }
}

const AXES: [&str; 3] = ["X", "Y", "Z"];

#[derive(Clone, Copy)]
struct Field {
    offset: usize,
    kind: Kind,
    label: &'static str,
}

macro_rules! fields {
    ($($offset:expr, $kind:ident, $label:expr);* $(;)?) => {
        &[$(Field { offset: $offset, kind: Kind::$kind, label: $label }),*]
    };
}

fn documented_fields(magic: &str) -> &'static [Field] {
    match magic {
        "FMAG" => fields![
            0x00,Flag8,"follow emitter"; 0x01,Flag8,"affect X"; 0x02,Flag8,"affect Y";
            0x03,Flag8,"affect Z";
            0x04,F32,"magnetic force"; 0x08,F32,"position X"; 0x0c,F32,"position Y";
            0x10,F32,"position Z"
        ],
        "FCOV" => fields![
            0x00,U8,"convergence type"; 0x04,F32,"position X"; 0x08,F32,"position Y";
            0x0c,F32,"position Z"; 0x10,F32,"ratio"
        ],
        "FCOL" => fields![
            0x00,U8,"collision type"; 0x01,Flag8,"process in world"; 0x02,Flag8,"common plane";
            0x04,F32,"coordinates"; 0x08,F32,"bounce rate"; 0x0c,U32,"collision count";
            0x10,F32,"friction"
        ],
        "FCLN" => fields![
            0x00,U8,"interpolation"; 0x01,Flag8,"random noise offset";
            0x02,Flag8,"world coordinates";
            0x04,F32,"animation speed X"; 0x08,F32,"animation speed Y"; 0x0c,F32,"animation speed Z";
            0x14,F32,"influence X"; 0x18,F32,"influence Y"; 0x1c,F32,"influence Z";
            0x20,F32,"table scale"; 0x24,F32,"noise offset"
        ],
        "FSPN" => fields![
            0x00,F32,"rotation force"; 0x04,Axis,"axis"; 0x08,F32,"outer velocity"
        ],
        "FRND" => fields![
            0x00,Flag8,"fixed random seed"; 0x01,Flag8,"detailed options";
            0x02,Flag8,"air resistance";
            0x04,F32,"velocity X"; 0x08,F32,"velocity Y"; 0x0c,F32,"velocity Z";
            0x10,U32,"apply timing"; 0x14,F32,"phase speed"; 0x18,F32,"phase width";
            0x1c,F32,"wave weight 0"; 0x20,F32,"wave weight 1"; 0x24,F32,"wave weight 2";
            0x28,F32,"wave weight 3"; 0x2c,F32,"frequency ratio 0"; 0x30,F32,"frequency ratio 1";
            0x34,F32,"frequency ratio 2"; 0x38,F32,"frequency ratio 3"
        ],
        "FRN1" => fields![
            0x00,F32,"random width X"; 0x04,F32,"random width Y"; 0x08,F32,"random width Z";
            0x0c,U32,"interval"
        ],
        "FPAD" => fields![
            0x00,Flag8,"world coordinates"; 0x04,F32,"position X"; 0x08,F32,"position Y";
            0x0c,F32,"position Z"
        ],
        "FCSF" => fields![
            0x00,U32,"flag"; 0x04,F32,"value 0"; 0x08,F32,"value 1"; 0x0c,F32,"value 2";
            0x10,F32,"value 3"; 0x14,F32,"value 4"; 0x18,F32,"value 5"; 0x1c,F32,"value 6";
            0x20,F32,"value 7"; 0x24,F32,"value 8"; 0x28,F32,"value 9"; 0x2c,F32,"value 10";
            0x30,F32,"value 11"; 0x34,F32,"value 12"; 0x38,F32,"value 13";
            0x3c,F32,"value 14"; 0x40,F32,"value 15"
        ],
        "EP01" => fields![
            0x00,U32,"calculation type"; 0x04,Flag32,"follow emitter"; 0x08,Flag32,"option";
            0x0c,Flag32,"texturing"; 0x10,U32,"divisions"; 0x14,U32,"connection type";
            0x18,F32,"head alpha"; 0x1c,F32,"tail alpha"; 0x20,F32,"history interpolation";
            0x24,F32,"direction interpolation"
        ],
        "EP02" => fields![
            0x00,U32,"calculation type"; 0x04,Flag32,"follow emitter"; 0x08,Flag32,"option";
            0x0c,Flag32,"texturing"; 0x10,Count,"partitions"; 0x14,Count,"histories";
            0x18,F32,"sample interval"; 0x1c,F32,"head alpha"; 0x20,F32,"tail alpha";
            0x24,F32,"history interpolation"; 0x28,F32,"direction interpolation"
        ],
        "EP03" => fields![
            0x00,U32,"calculation type"; 0x04,Flag32,"follow emitter"; 0x08,Flag32,"option";
            0x0c,Flag32,"UV0 texturing"; 0x10,Flag32,"UV1 texturing";
            0x14,Flag32,"UV2 texturing"; 0x18,Count,"history entries"; 0x1c,U32,"connection type"; 0x20,F32,"head alpha";
            0x24,F32,"tail alpha"; 0x28,U32,"partitions"; 0x2c,F32,"history interpolation";
            0x30,F32,"direction interpolation"; 0x34,F32,"air resistance";
            0x38,F32,"acceleration X"; 0x3c,F32,"acceleration Y"; 0x40,F32,"acceleration Z";
            0x58,U32,"UV mapping type"; 0x5c,F32,"starting scale"; 0x60,F32,"ending scale"
        ],
        "EP04" => fields![
            0x00,F32,"repeat offset X"; 0x04,F32,"repeat offset Y"; 0x08,F32,"repeat offset Z";
            0x0c,F32,"repeat count"; 0x10,F32,"size X"; 0x14,F32,"size Y"; 0x18,F32,"size Z";
            0x1c,F32,"clipping plane height"; 0x20,F32,"position X"; 0x24,F32,"position Y";
            0x28,F32,"position Z"; 0x2c,U32,"clipping type"; 0x30,F32,"edge fade X";
            0x34,F32,"edge fade Y"; 0x38,F32,"edge fade Z"; 0x3c,F32,"fix before camera";
            0x40,F32,"rotation X"; 0x44,F32,"rotation Y"; 0x48,F32,"rotation Z"
        ],
        _ => &[],
    }
}

/// The plain name a section is shown under; its four-letter code goes in the hover text.
fn section_title(magic: &str) -> String {
    let named = match magic {
        "FMAG" => "Magnet",
        "FCOV" => "Pull toward a point",
        "FCOL" => "Bounce off a plane",
        "FCLN" => "Swirl noise",
        "FSPN" => "Spin field",
        "FRND" => "Random drift",
        "FRN1" => "Random jitter",
        "FPAD" => "Steady move",
        "FCSF" => "Custom field",
        "EP01" => "Joined stripe",
        "EP02" => "Stripe",
        "EP03" => "Trail stripe",
        "EP04" => "Repeating area",
        "CADP" => "Custom action data",
        "CSDP" => "Custom shader data",
        "CUDP" => "Custom user data",
        "EAA0" => "Opacity animation",
        "EAA1" => "Second opacity animation",
        "EAC0" => "Colour animation",
        "EAC1" => "Second colour animation",
        "EATR" => "Position animation",
        "EARO" => "Rotation animation",
        "EASL" => "Size animation",
        "EAER" => "Spawn rate animation",
        "EAPL" => "Particle life animation",
        "EAOV" => "Outward speed animation",
        "EADV" => "Direction speed animation",
        "EAGV" => "Gravity animation",
        _ if magic.starts_with("EA") => "Other animation",
        _ => "Extra data",
    };
    named.to_owned()
}

/// A field value animated over each particle's life (ResAnim8KeyParamSet): enabled, loop and
/// random-start words, the key count, the loop length in frames, then up to eight keys of X, Y,
/// Z and time. Time runs 0 to 1 over the particle's life, or over the loop when it loops. Field
/// sections keep room for all eight keys and zero the unused ones.
#[derive(Clone, Copy)]
struct KeyedSet {
    offset: usize,
    label: &'static str,
}

const MAX_FIELD_KEYS: usize = 8;
const KEY_STRIDE: usize = 0x10;
const KEYS_AT: usize = 0x14;

fn keyed_sets(magic: &str) -> &'static [KeyedSet] {
    match magic {
        "FMAG" => &[KeyedSet {
            offset: 0x14,
            label: "magnetic force keys",
        }],
        "FCOV" => &[KeyedSet {
            offset: 0x14,
            label: "ratio keys",
        }],
        "FSPN" => &[
            KeyedSet {
                offset: 0x0c,
                label: "rotation force keys",
            },
            KeyedSet {
                offset: 0xa0,
                label: "outer velocity keys",
            },
        ],
        "FRND" => &[KeyedSet {
            offset: 0x3c,
            label: "velocity keys",
        }],
        "FRN1" => &[KeyedSet {
            offset: 0x10,
            label: "random width keys",
        }],
        "FPAD" => &[KeyedSet {
            offset: 0x10,
            label: "position keys",
        }],
        _ => &[],
    }
}

/// How many keys a set uses: its count word, capped at eight and at the keys the section holds.
fn key_count(data: &[u8], set: KeyedSet) -> usize {
    let stored = read_u32(data, set.offset + 0xc).unwrap_or(0) as usize;
    let room = data.len().saturating_sub(set.offset + KEYS_AT) / KEY_STRIDE;
    stored.min(MAX_FIELD_KEYS).min(room)
}

fn key_offset(set: KeyedSet, key: usize) -> usize {
    set.offset + KEYS_AT + key * KEY_STRIDE
}

/// Append a key copied from the last one, placed at the end of the life. Returns false when the
/// set is full.
fn add_key(data: &mut [u8], set: KeyedSet) -> bool {
    let count = key_count(data, set);
    let room = data.len().saturating_sub(set.offset + KEYS_AT) / KEY_STRIDE;
    if count >= MAX_FIELD_KEYS.min(room) {
        return false;
    }
    let new = key_offset(set, count);
    if count > 0 {
        let last = key_offset(set, count - 1);
        data.copy_within(last..last + KEY_STRIDE, new);
        write_f32(data, new + 12, 1.0);
    } else {
        data[new..new + KEY_STRIDE].fill(0);
    }
    write_u32(data, set.offset + 0xc, count as u32 + 1)
}

/// Remove one key, moving the later keys up and zeroing the freed slot as the game's files do.
fn remove_key(data: &mut [u8], set: KeyedSet, key: usize) -> bool {
    let count = key_count(data, set);
    if key >= count {
        return false;
    }
    let from = key_offset(set, key + 1);
    let end = key_offset(set, count);
    data.copy_within(from..end, key_offset(set, key));
    data[end - KEY_STRIDE..end].fill(0);
    write_u32(data, set.offset + 0xc, count as u32 - 1)
}

fn draw_keyed_set(ui: &mut Ui, section: &mut EmitterSubsectionDef, set: KeyedSet) -> bool {
    if section.data.len() < set.offset + KEYS_AT {
        return false;
    }
    let mut changed = false;
    ui.add_space(4.0);
    ui.label(egui::RichText::new(sentence(set.label)).strong());
    let id = (section.magic.clone(), set.offset);
    egui::Grid::new(("field_keys_header", id.clone()))
        .num_columns(2)
        .show(ui, |ui| {
            for (offset, label, hover) in [
                (
                    set.offset,
                    "Animated",
                    "Use the keys below instead of the fixed value",
                ),
                (set.offset + 4, "Loop", "Repeat the keys every loop length"),
                (
                    set.offset + 8,
                    "Random start",
                    "Start each particle at a random point in the loop",
                ),
            ] {
                let mut on = read_u32(&section.data, offset).unwrap_or(0) != 0;
                ui.label(label).on_hover_text(hover);
                if ui.checkbox(&mut on, "").changed() {
                    changed |= write_f32(&mut section.data, offset, if on { 1.0 } else { 0.0 });
                }
                ui.end_row();
            }
            ui.label("Loop length").on_hover_text("Frames in one loop");
            let mut frames = read_u32(&section.data, set.offset + 0x10).unwrap_or(0);
            if ui.add(egui::DragValue::new(&mut frames)).changed() {
                changed |= write_u32(&mut section.data, set.offset + 0x10, frames);
            }
            ui.end_row();
        });
    let count = key_count(&section.data, set);
    let mut remove = None;
    if count > 0 {
        egui::Grid::new(("field_keys", id))
            .striped(true)
            .show(ui, |ui| {
                ui.label("Key");
                ui.label("X");
                ui.label("Y");
                ui.label("Z");
                ui.label("Time").on_hover_text(
                    "0 is the particle's birth and 1 its end, or the start and end of the loop",
                );
                ui.end_row();
                for key in 0..count {
                    ui.label((key + 1).to_string());
                    for component in 0..4 {
                        let offset = key_offset(set, key) + component * 4;
                        let mut value = read_f32(&section.data, offset).unwrap_or(0.0);
                        let drag = egui::DragValue::new(&mut value).speed(0.01);
                        let drag = if component == 3 {
                            drag.range(0.0..=1.0)
                        } else {
                            drag
                        };
                        if ui.add(drag).changed() && value.is_finite() {
                            changed |= write_f32(&mut section.data, offset, value);
                        }
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(key);
                    }
                    ui.end_row();
                }
            });
    }
    if let Some(key) = remove {
        changed |= remove_key(&mut section.data, set, key);
    }
    let room = section.data.len().saturating_sub(set.offset + KEYS_AT) / KEY_STRIDE;
    if count < MAX_FIELD_KEYS.min(room) && ui.small_button("Add key").clicked() {
        changed |= add_key(&mut section.data, set);
    }
    changed
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) -> bool {
    let Some(dst) = data.get_mut(offset..offset + 4) else {
        return false;
    };
    dst.copy_from_slice(&value.to_le_bytes());
    true
}

fn read_f32(data: &[u8], offset: usize) -> Option<f32> {
    read_u32(data, offset).map(f32::from_bits)
}

fn write_f32(data: &mut [u8], offset: usize, value: f32) -> bool {
    write_u32(data, offset, value.to_bits())
}

fn draw_field(ui: &mut Ui, section: &mut EmitterSubsectionDef, field: Field) -> bool {
    match field.kind {
        Kind::U8 => {
            let Some(value) = section.data.get_mut(field.offset) else {
                return false;
            };
            let mut v = u32::from(*value);
            let changed = ui
                .add(egui::DragValue::new(&mut v).range(0..=255))
                .changed();
            if changed {
                *value = v as u8;
            }
            changed
        }
        Kind::U32 => {
            let Some(mut value) = read_u32(&section.data, field.offset) else {
                return false;
            };
            let changed = ui.add(egui::DragValue::new(&mut value)).changed();
            changed && write_u32(&mut section.data, field.offset, value)
        }
        Kind::F32 => {
            let Some(mut value) = read_f32(&section.data, field.offset) else {
                return false;
            };
            let changed = ui
                .add(egui::DragValue::new(&mut value).speed(0.01))
                .changed();
            changed && value.is_finite() && write_f32(&mut section.data, field.offset, value)
        }
        Kind::Flag8 | Kind::Flag32 => {
            let Some(value) = read_flag(&section.data, field) else {
                return false;
            };
            // Only 0 and 1 are on and off; any other value stays a number, so ticking the box
            // never rewrites a value nobody chose.
            if value > 1 {
                let raw = Field {
                    kind: if field.kind.width() == 1 {
                        Kind::U8
                    } else {
                        Kind::U32
                    },
                    ..field
                };
                return draw_field(ui, section, raw);
            }
            let mut on = value == 1;
            let changed = ui.checkbox(&mut on, "").changed();
            changed && write_flag(&mut section.data, field, u32::from(on))
        }
        Kind::Count => {
            let Some(value) = read_f32(&section.data, field.offset) else {
                return false;
            };
            if value.fract() != 0.0 || !(0.0..=1.0e6).contains(&value) {
                return draw_field(
                    ui,
                    section,
                    Field {
                        kind: Kind::F32,
                        ..field
                    },
                );
            }
            let mut count = value as u32;
            let changed = ui
                .add(egui::DragValue::new(&mut count).range(0..=1_000_000))
                .changed();
            changed && write_f32(&mut section.data, field.offset, count as f32)
        }
        Kind::Axis => {
            let Some(value) = read_u32(&section.data, field.offset) else {
                return false;
            };
            let Some(&shown) = AXES.get(value as usize) else {
                return draw_field(
                    ui,
                    section,
                    Field {
                        kind: Kind::U32,
                        ..field
                    },
                );
            };
            let mut picked = value;
            egui::ComboBox::from_id_salt(("field_axis", &section.magic, field.offset))
                .selected_text(shown)
                .width(60.0)
                .show_ui(ui, |ui| {
                    for (i, axis) in AXES.iter().enumerate() {
                        ui.selectable_value(&mut picked, i as u32, *axis);
                    }
                })
                .response
                .on_hover_text(format!("Stored as {value}"));
            picked != value && write_u32(&mut section.data, field.offset, picked)
        }
    }
}

/// An on/off field's stored number, whatever its width.
fn read_flag(data: &[u8], field: Field) -> Option<u32> {
    match field.kind.width() {
        1 => data.get(field.offset).map(|&b| u32::from(b)),
        _ => read_u32(data, field.offset),
    }
}

fn write_flag(data: &mut [u8], field: Field, value: u32) -> bool {
    match field.kind.width() {
        1 => match data.get_mut(field.offset) {
            Some(byte) => {
                *byte = value as u8;
                true
            }
            None => false,
        },
        _ => write_u32(data, field.offset, value),
    }
}

/// A row label as the rest of the page writes them: first letter capital.
fn sentence(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn draw_animation(ui: &mut Ui, section: &mut EmitterSubsectionDef) -> bool {
    if section.data.len() < 12 {
        return false;
    }
    let mut changed = false;
    egui::Grid::new(("ea_header", &section.magic))
        .num_columns(2)
        .show(ui, |ui| {
            for (offset, label) in [(0, "Enabled"), (1, "Loop"), (2, "Random start")] {
                let mut on = section.data[offset] != 0;
                ui.label(label);
                if ui.checkbox(&mut on, "").changed() {
                    section.data[offset] = u8::from(on);
                    changed = true;
                }
                ui.end_row();
            }
            ui.label("Loop count");
            let mut loops = read_u32(&section.data, 8).unwrap_or(0);
            if ui.add(egui::DragValue::new(&mut loops)).changed() {
                changed |= write_u32(&mut section.data, 8, loops);
            }
            ui.end_row();
        });
    let count = read_u32(&section.data, 4).unwrap_or(0) as usize;
    let available = section.data.len().saturating_sub(12) / 16;
    let count = count.min(available);
    ui.label(
        egui::RichText::new(format!("{count} keyframe(s)"))
            .small()
            .color(egui::Color32::GRAY),
    );
    egui::Grid::new(("ea_keys", &section.magic))
        .striped(true)
        .show(ui, |ui| {
            ui.label("Key");
            ui.label("X");
            ui.label("Y");
            ui.label("Z");
            ui.label("Frame");
            ui.end_row();
            for key in 0..count {
                ui.label(key.to_string());
                for component in 0..4 {
                    let offset = 12 + key * 16 + component * 4;
                    let mut value = read_f32(&section.data, offset).unwrap_or(0.0);
                    if ui
                        .add(egui::DragValue::new(&mut value).speed(0.01))
                        .changed()
                        && value.is_finite()
                    {
                        changed |= write_f32(&mut section.data, offset, value);
                    }
                }
                ui.end_row();
            }
        });
    changed
}

fn raw_editor(ui: &mut Ui, section: &mut EmitterSubsectionDef) -> bool {
    let mut changed = false;
    egui::Grid::new(("subsection_raw", &section.magic))
        .striped(true)
        .show(ui, |ui| {
            for row in 0..section.data.len().div_ceil(8) {
                ui.label(format!("{:04X}", row * 8));
                for offset in row * 8..((row + 1) * 8).min(section.data.len()) {
                    let mut value = u32::from(section.data[offset]);
                    if ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .range(0..=255)
                                .hexadecimal(2, false, true),
                        )
                        .changed()
                    {
                        section.data[offset] = value as u8;
                        changed = true;
                    }
                }
                ui.end_row();
            }
        });
    changed
}

/// Draw every subsection on an emitter. Known layouts get named controls; every block retains
/// an advanced byte editor so newly discovered and intentionally opaque fields remain editable.
pub fn draw(ui: &mut Ui, emitter: &mut EmitterDef, pristine: &EmitterDef) -> bool {
    if emitter.subsections.is_empty() {
        return false;
    }
    let mut changed = false;
    ui.add_space(10.0);
    ui.separator();
    ui.label(egui::RichText::new("Animations & extra settings").strong());
    for index in 0..emitter.subsections.len() {
        let magic = emitter.subsections[index].magic.clone();
        let was_edited = pristine
            .subsections
            .get(index)
            .is_some_and(|p| p.magic == magic && p.data != emitter.subsections[index].data);
        let title = section_title(&magic);
        let heading = if was_edited {
            format!("{title} •")
        } else {
            title
        };
        // Keyed by the part as well: the caller pushes the part's id, so one part's open
        // sections do not open another part's section at the same position.
        egui::CollapsingHeader::new(heading)
            .id_salt(("subsection", index, &magic))
            .default_open(was_edited)
            .show(ui, |ui| {
                let section = &mut emitter.subsections[index];
                if magic.starts_with("EA") {
                    changed |= draw_animation(ui, section);
                } else {
                    let fields = documented_fields(&magic);
                    if !fields.is_empty() {
                        egui::Grid::new(("subsection_fields", index))
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                for &field in fields {
                                    if field.offset < section.data.len() {
                                        ui.label(sentence(field.label));
                                        changed |= draw_field(ui, section, field);
                                        ui.end_row();
                                    }
                                }
                            });
                    }
                    for &set in keyed_sets(&magic) {
                        changed |= draw_keyed_set(ui, section, set);
                    }
                }
                egui::CollapsingHeader::new("Raw bytes")
                    .id_salt(("raw", index))
                    .show(ui, |ui| {
                        changed |= raw_editor(ui, section);
                    })
                    .header_response
                    .on_hover_text("Change only values whose binary meaning you know");
                if was_edited && ui.small_button("Reset section").clicked() {
                    if let Some(original) =
                        pristine.subsections.get(index).filter(|p| p.magic == magic)
                    {
                        section.data = original.data.clone();
                        changed = true;
                    }
                }
            })
            .header_response
            .on_hover_text(format!("Stored in the file as {magic}"));
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ea_layout_is_little_endian_and_round_trips() {
        let mut data = vec![1, 0, 1, 0];
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&7u32.to_le_bytes());
        for value in [1.0f32, -2.5, 3.25, 9.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(read_u32(&data, 4), Some(1));
        assert_eq!(read_u32(&data, 8), Some(7));
        assert_eq!(read_f32(&data, 16), Some(-2.5));
        assert!(write_f32(&mut data, 16, 6.5));
        assert_eq!(read_f32(&data, 16), Some(6.5));
    }

    fn fspn(keys: &[[f32; 4]]) -> Vec<u8> {
        let mut data = vec![0u8; 0x134];
        write_f32(&mut data, 0x0c, 1.0);
        write_u32(&mut data, 0x18, keys.len() as u32);
        for (k, key) in keys.iter().enumerate() {
            for (c, v) in key.iter().enumerate() {
                write_f32(&mut data, 0x20 + k * 0x10 + c * 4, *v);
            }
        }
        data
    }

    #[test]
    fn field_keys_sit_where_the_count_says() {
        let rotation = keyed_sets("FSPN")[0];
        let outer = keyed_sets("FSPN")[1];
        let data = fspn(&[[0.1, 0.0, 0.0, 0.0], [0.5, 0.0, 0.0, 1.0]]);
        assert_eq!(key_count(&data, rotation), 2);
        assert_eq!(read_f32(&data, key_offset(rotation, 1)), Some(0.5));
        assert_eq!(read_f32(&data, key_offset(rotation, 1) + 12), Some(1.0));
        // The second set's keys follow its own header at 0xa0, not the first set's.
        assert_eq!(key_offset(outer, 0), 0xb4);
        assert_eq!(key_count(&data, outer), 0);
    }

    #[test]
    fn adding_and_removing_keys_keeps_the_unused_slots_zero() {
        let set = keyed_sets("FSPN")[0];
        let mut data = fspn(&[[0.1, 0.0, 0.0, 0.0], [0.5, 0.0, 0.0, 0.6]]);
        assert!(add_key(&mut data, set));
        assert_eq!(key_count(&data, set), 3);
        assert_eq!(read_f32(&data, key_offset(set, 2)), Some(0.5));
        assert_eq!(read_f32(&data, key_offset(set, 2) + 12), Some(1.0));
        assert!(remove_key(&mut data, set, 0));
        assert_eq!(key_count(&data, set), 2);
        assert_eq!(read_f32(&data, key_offset(set, 0)), Some(0.5));
        assert!(data[key_offset(set, 2)..key_offset(set, 3)]
            .iter()
            .all(|&b| b == 0));
        // A key past the count is not removed.
        assert!(!remove_key(&mut data, set, 2));
        assert_eq!(key_count(&data, set), 2);
        // Eight is the most a set holds.
        while add_key(&mut data, set) {}
        assert_eq!(key_count(&data, set), MAX_FIELD_KEYS);
        assert!(!add_key(&mut data, set));
        // The outer velocity set right after it is untouched.
        assert!(data[0xa0..].iter().all(|&b| b == 0));
    }

    /// The layout above is checked against the game's own field sections: every used key has a
    /// time from 0 to 1 in order, no count runs past eight, and every slot past the count is
    /// zero. Point `VISIONARY_EFF_ROOT` at the parent of an extracted `effect/` tree.
    #[test]
    fn game_field_sections_match_the_key_layout() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "eff")
                    && !path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('_'))
                {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&root.join("effect"), &mut files);
        assert!(files.len() > 300, "only {} corpus files", files.len());
        let (mut sets, mut keyed, mut multi) = (0usize, 0usize, 0usize);
        let mut bad = Vec::new();
        for path in &files {
            let Ok(effect) = crate::effects::load_effect(path) else {
                continue;
            };
            for emitter in effect.ptcl.emitter_sets.iter().flat_map(|s| &s.emitters) {
                for section in &emitter.subsections {
                    for &set in keyed_sets(&section.magic) {
                        if section.data.len() < set.offset + KEYS_AT {
                            continue;
                        }
                        sets += 1;
                        let stored = read_u32(&section.data, set.offset + 0xc).unwrap_or(0);
                        let count = key_count(&section.data, set);
                        let mut why = None;
                        if stored as usize > MAX_FIELD_KEYS {
                            why = Some(format!("count {stored}"));
                        }
                        let times: Vec<f32> = (0..count)
                            .map(|k| read_f32(&section.data, key_offset(set, k) + 12).unwrap())
                            .collect();
                        if times.iter().any(|t| !(0.0..=1.0).contains(t))
                            || times.windows(2).any(|w| w[1] < w[0])
                        {
                            why = Some(format!("times {times:?}"));
                        }
                        let room = (section.data.len() - set.offset - KEYS_AT) / KEY_STRIDE;
                        let tail =
                            key_offset(set, count)..key_offset(set, room.min(MAX_FIELD_KEYS));
                        if section.data[tail].iter().any(|&b| b != 0) {
                            why = Some("bytes past the count".into());
                        }
                        if count > 0 {
                            keyed += 1;
                        }
                        if count > 1 {
                            multi += 1;
                        }
                        if let Some(why) = why {
                            bad.push(format!(
                                "{} {} {}: {why}",
                                path.display(),
                                section.magic,
                                set.label
                            ));
                        }
                    }
                }
            }
        }
        eprintln!("{sets} keyed sets, {keyed} with keys, {multi} with more than one");
        assert!(multi > 0, "no multi-key set found; the walk read nothing");
        assert!(
            bad.is_empty(),
            "{} mismatches:\n{}",
            bad.len(),
            bad.join("\n")
        );
    }

    #[test]
    fn documented_rows_never_run_past_their_storage_width() {
        for magic in [
            "FMAG", "FCOV", "FCOL", "FCLN", "FSPN", "FRND", "FRN1", "FPAD", "FCSF", "EP01", "EP02",
            "EP03", "EP04",
        ] {
            for field in documented_fields(magic) {
                let width = field.kind.width();
                assert!(field.offset + width <= 0x100, "{magic} {}", field.label);
            }
        }
    }

    /// Every documented section in the game's files gets a plain title rather than its code,
    /// and every value fits the control drawn for it: on/off fields hold 0 or 1, axes 0 to 2,
    /// counts whole numbers. Point `VISIONARY_EFF_ROOT` at the parent of an extracted `effect/`.
    #[test]
    fn game_sections_get_plain_titles_and_fitting_controls() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "eff")
                    && !path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('_'))
                {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&root.join("effect"), &mut files);
        assert!(files.len() > 300, "only {} corpus files", files.len());
        let mut magics = std::collections::BTreeSet::new();
        let (mut flags, mut axes, mut counts) = (0usize, 0usize, 0usize);
        let mut bad = Vec::new();
        // A count that is not whole is drawn as a plain number instead; it must stay rare, or
        // the field is not a count.
        let mut fractional_counts = 0usize;
        for path in &files {
            let Ok(effect) = crate::effects::load_effect(path) else {
                continue;
            };
            for emitter in effect.ptcl.emitter_sets.iter().flat_map(|s| &s.emitters) {
                for section in &emitter.subsections {
                    magics.insert(section.magic.clone());
                    for &field in documented_fields(&section.magic) {
                        let fits = match field.kind {
                            Kind::Flag8 | Kind::Flag32 => {
                                read_flag(&section.data, field).map(|v| {
                                    flags += 1;
                                    v <= 1
                                })
                            }
                            Kind::Axis => read_u32(&section.data, field.offset).map(|v| {
                                axes += 1;
                                (v as usize) < AXES.len()
                            }),
                            Kind::Count => {
                                if let Some(v) = read_f32(&section.data, field.offset) {
                                    counts += 1;
                                    if v.fract() != 0.0 || v < 0.0 {
                                        fractional_counts += 1;
                                    }
                                }
                                None
                            }
                            Kind::U8 | Kind::U32 | Kind::F32 => None,
                        };
                        if fits == Some(false) {
                            bad.push(format!(
                                "{} {} {}",
                                path.display(),
                                section.magic,
                                field.label
                            ));
                        }
                    }
                }
            }
        }
        eprintln!("{flags} on/off, {axes} axis, {counts} count values; sections {magics:?}");
        assert!(
            flags > 1000 && axes > 100 && counts > 100,
            "the walk read too little"
        );
        assert!(bad.is_empty(), "{} misfits:\n{}", bad.len(), bad.join("\n"));
        assert!(
            fractional_counts * 100 < counts,
            "{fractional_counts} of {counts} counts are not whole"
        );
        // No section is titled by its code, and every section that is not an animation has a
        // name of its own rather than the catch-all.
        for magic in &magics {
            let title = section_title(magic);
            assert_ne!(&title, magic);
            if !magic.starts_with("EA") {
                assert_ne!(title, "Extra data", "{magic} has no plain title");
            }
        }
    }

    #[test]
    fn on_off_fields_tick_only_zero_and_one() {
        let field = documented_fields("FMAG")[1];
        assert!(matches!(field.kind, Kind::Flag8));
        let mut data = vec![0u8; 0x100];
        assert_eq!(read_flag(&data, field), Some(0));
        assert!(write_flag(&mut data, field, 1));
        assert_eq!(data[field.offset], 1);
        // A word-wide flag writes all four bytes, and leaves the next field alone.
        let word = documented_fields("EP03")[1];
        assert!(matches!(word.kind, Kind::Flag32));
        data[word.offset + 4] = 7;
        assert!(write_flag(&mut data, word, 1));
        assert_eq!(read_u32(&data, word.offset), Some(1));
        assert_eq!(data[word.offset + 4], 7);
        assert_eq!(sentence("follow emitter"), "Follow emitter");
        assert_eq!(section_title("FSPN"), "Spin field");
        assert_eq!(section_title("CADP"), "Custom action data");
        assert_eq!(section_title("ZZZZ"), "Extra data");
    }
}
