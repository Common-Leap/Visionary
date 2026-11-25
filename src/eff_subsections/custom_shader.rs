//! The custom shader data of a part (`CSDP`): 32 numbers its shader is handed.
//!
//! What each is was read from the game's own shaders. Which of them a part uses depends on its
//! custom shader and on the features that was compiled with, so the rows shown are the ones the
//! part's shader reads.

use egui::Ui;

use super::{read_f32, write_f32};
use crate::effects::EmitterDef;

/// How a row of numbers is edited.
#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Number,
    /// X, Y and Z.
    Direction,
    /// Red, green and blue, which can be brighter than 1.
    Colour,
}

impl Shape {
    fn words(self) -> u32 {
        match self {
            Shape::Number => 1,
            _ => 3,
        }
    }
}

struct Row {
    at: u32,
    shape: Shape,
    name: &'static str,
    hover: &'static str,
}

const fn row(at: u32, shape: Shape, name: &'static str, hover: &'static str) -> Row {
    Row {
        at,
        shape,
        name,
        hover,
    }
}

/// The numbers of each custom shader. A shader that reads none of its part's has no rows.
fn rows(custom_shader: i64) -> &'static [Row] {
    use Shape::*;
    match custom_shader {
        4 => {
            const ROWS: &[Row] = &[
                row(
                    0,
                    Number,
                    "Tint",
                    "How far the part takes the colour the game gives its effects, such as the \
                     stage's: 0 is none, 1 is all. Below 0 the game chooses.",
                ),
                row(
                    8,
                    Number,
                    "Roundness",
                    "How round the light takes the part to be: 0 lights it as it is, 1 as a ball \
                     around its centre",
                ),
                row(
                    12,
                    Direction,
                    "Light direction",
                    "Which way the light shines. Only its direction counts, not its length.",
                ),
                row(
                    24,
                    Colour,
                    "Light colour",
                    "What the side facing the light is multiplied by, on top of the shadow colour",
                ),
                row(
                    36,
                    Colour,
                    "Shadow colour",
                    "What the side away from the light is multiplied by",
                ),
                row(
                    48,
                    Number,
                    "Edge fade",
                    "How far in from the edge of the area the game draws effects in the part \
                     starts to fade out",
                ),
                row(
                    52,
                    Number,
                    "Darkest shade",
                    "How dark the side away from the light gets: 0 is black, 1 is as bright as \
                     the lit side",
                ),
                row(
                    56,
                    Number,
                    "Reflection",
                    "How strongly the part's third texture is reflected in it",
                ),
                row(
                    60,
                    Number,
                    "Screen pattern tiles",
                    "How often the first texture repeats over the screen: it is laid over the \
                     screen, not over the part",
                ),
                row(
                    64,
                    Number,
                    "Screen pattern movement",
                    "How far the screen pattern moves with the part",
                ),
            ];
            ROWS
        }
        5 => {
            const ROWS: &[Row] = &[row(
                0,
                Number,
                "Bend strength",
                "How far the picture behind the part is moved, in pixels of a 1920 by 1080 screen",
            )];
            ROWS
        }
        6 => {
            const ROWS: &[Row] = &[
                row(
                    0,
                    Number,
                    "Blur length",
                    "How far the picture behind the part is smeared, in pixels of a 1920 by 1080 \
                     screen",
                ),
                row(
                    4,
                    Number,
                    "Blur value",
                    "Handed to the game's blur. The game's own files all have 1.",
                ),
            ];
            ROWS
        }
        8 => {
            const ROWS: &[Row] = &[row(
                0,
                Number,
                "Dissolve softness",
                "How softly the edge fades where the part dissolves: 0 is a hard edge",
            )];
            ROWS
        }
        _ => &[],
    }
}

/// The part's custom shader, and the bytes its shader reads of this section. All of them when
/// the shader can't be read.
pub(super) struct Reads {
    custom_shader: i64,
    words: Option<std::collections::BTreeSet<u32>>,
}

impl Reads {
    pub(super) fn of(em: &EmitterDef) -> Self {
        let id = "shader_references.custom_shader_index";
        Self {
            custom_shader: crate::eff_attrs::index_of(id)
                .and_then(|at| em.attrs.get(at).copied().flatten())
                .map_or(-1, |v| v.as_i64()),
            words: em.shader_use().map(|shader| shader.custom.clone()),
        }
    }

    fn uses(&self, row: &Row) -> bool {
        self.words
            .as_ref()
            .is_none_or(|words| (0..row.shape.words()).any(|k| words.contains(&(row.at + 4 * k))))
    }
}

fn values(data: &[u8], row: &Row) -> [f32; 3] {
    std::array::from_fn(|k| read_f32(data, row.at as usize + 4 * k).unwrap_or(0.0))
}

pub(super) fn draw(ui: &mut Ui, data: &mut [u8], reads: &Reads) -> bool {
    let mut changed = false;
    let known = rows(reads.custom_shader);
    let used: Vec<&Row> = known.iter().filter(|row| reads.uses(row)).collect();
    if !used.is_empty() {
        egui::Grid::new("custom_shader_rows")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for row in used {
                    ui.label(row.name).on_hover_text(format!(
                        "{}\n\nNumber {}",
                        row.hover,
                        row.at / 4 + 1
                    ));
                    changed |= draw_row(ui, data, row);
                    ui.end_row();
                }
            });
    }
    // What the shader reads that has no name above, and what it does not read.
    let named = |at: u32| {
        known
            .iter()
            .any(|row| reads.uses(row) && (row.at..row.at + 4 * row.shape.words()).contains(&at))
    };
    let count = (data.len() / 4) as u32;
    let read = |at: u32| {
        reads
            .words
            .as_ref()
            .is_some_and(|words| words.contains(&at))
    };
    let unnamed: Vec<u32> = (0..count)
        .map(|k| k * 4)
        .filter(|at| read(*at) && !named(*at))
        .collect();
    changed |= draw_numbers(ui, "custom_shader_unnamed", data, &unnamed);
    let idle: Vec<u32> = (0..count)
        .map(|k| k * 4)
        .filter(|at| !read(*at) && !named(*at))
        .collect();
    egui::CollapsingHeader::new(format!("Not used by this part's shader ({})", idle.len()))
        .id_salt("custom_shader_idle")
        .show(ui, |ui| {
            changed |= draw_numbers(ui, "custom_shader_idle_numbers", data, &idle);
        });
    changed
}

fn draw_row(ui: &mut Ui, data: &mut [u8], row: &Row) -> bool {
    let mut v = values(data, row);
    let edited = match row.shape {
        Shape::Number => ui
            .add(egui::DragValue::new(&mut v[0]).speed(0.01).max_decimals(3))
            .changed(),
        Shape::Direction => {
            ui.horizontal(|ui| {
                let mut edited = false;
                for (value, axis) in v.iter_mut().zip(["X ", "Y ", "Z "]) {
                    let drag = egui::DragValue::new(value).speed(0.01).max_decimals(3);
                    edited |= ui.add(drag.prefix(axis)).changed();
                }
                edited
            })
            .inner
        }
        Shape::Colour => {
            ui.horizontal(|ui| {
                crate::ui::colour_field_with(ui, ("custom_shader", row.at), &mut v, true)
            })
            .inner
        }
    };
    let mut changed = false;
    if edited {
        for (k, value) in v.iter().enumerate().take(row.shape.words() as usize) {
            if value.is_finite() {
                changed |= write_f32(data, row.at as usize + 4 * k, *value);
            }
        }
    }
    changed
}

/// Numbers by their place, four to a row.
fn draw_numbers(ui: &mut Ui, salt: &str, data: &mut [u8], at: &[u32]) -> bool {
    let mut changed = false;
    if at.is_empty() {
        return false;
    }
    egui::Grid::new(salt)
        .num_columns(8)
        .striped(true)
        .show(ui, |ui| {
            for row in at.chunks(4) {
                for &at in row {
                    ui.label(format!("{}", at / 4 + 1))
                        .on_hover_text(format!("Byte {at:#04x}"));
                    let mut value = read_f32(data, at as usize).unwrap_or(0.0);
                    let drag = egui::DragValue::new(&mut value).speed(0.01).max_decimals(3);
                    if ui.add(drag).changed() && value.is_finite() {
                        changed |= write_f32(data, at as usize, value);
                    }
                }
                ui.end_row();
            }
        });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every number the game's shaders read of a part's custom data has a name, and every named
    /// one is read by some shader.
    #[test]
    fn every_number_a_shader_reads_is_named() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let mut read = std::collections::BTreeSet::new();
        for file in [
            "fighter/mario",
            "fighter/krool",
            "fighter/shulk",
            "fighter/simon",
            "fighter/rockman",
            "fighter/samus",
            "fighter/murabito",
            "fighter/sonic",
            "stage/wreckingcrew",
        ] {
            let name = file.rsplit('/').next().unwrap();
            let path = root.join(format!("effect/{file}/ef_{name}.eff"));
            let Ok(effect) = crate::effects::load_effect(&path) else {
                continue;
            };
            for em in effect.ptcl.emitter_sets.iter().flat_map(|s| &s.emitters) {
                let reads = Reads::of(em);
                if !em.subsections.iter().any(|s| s.magic == "CSDP") {
                    continue;
                }
                for &at in reads.words.as_ref().expect("its shader can be read") {
                    let named = rows(reads.custom_shader)
                        .iter()
                        .any(|row| (row.at..row.at + 4 * row.shape.words()).contains(&at));
                    assert!(
                        named,
                        "{file} {}: {} reads {at}",
                        em.name, reads.custom_shader
                    );
                    read.insert((reads.custom_shader, at));
                }
            }
        }
        for shader in [4, 5, 6, 8] {
            for row in rows(shader) {
                assert!(read.contains(&(shader, row.at)), "{shader} {}", row.name);
            }
        }
    }
}
