//! The model a part draws: taking it out of the file, and putting another in its place.

use super::*;
use crate::mod_project::ModelImport;
use crate::model_import::{ModelInfo, ModelPool};

/// The models of the open file.
#[derive(Default)]
pub(super) struct Models {
    pool: Option<ModelPool>,
    /// The project's replacements, as the app last gave them.
    imports: Vec<ModelImport>,
    /// Replacements made here, for the app to keep.
    pending: Vec<ModelImport>,
    /// What the last button did, and whether it went well.
    note: Option<(String, bool)>,
}

impl Models {
    pub(super) fn load(&mut self, pool: Option<ModelPool>) {
        self.pool = pool;
        self.note = None;
    }
}

const MODEL_ATTR: &str = "particle_data.primitive_id";

impl EffEditor {
    pub fn take_model_imports(&mut self) -> Vec<ModelImport> {
        std::mem::take(&mut self.models.pending)
    }

    pub fn set_model_imports(&mut self, imports: Vec<ModelImport>) {
        self.models.imports = imports;
    }

    /// How many parts of the file draw a model.
    fn model_users(&self, id: u64) -> usize {
        let sets = self.ptcl.iter().flat_map(|p| &p.emitter_sets);
        sets.flat_map(|set| &set.emitters)
            .filter(|em| attr(em, MODEL_ATTR).is_some_and(|v| v.as_u64() == id))
            .count()
    }

    /// The Model section of a part that draws one of the file's models.
    pub(super) fn draw_model_panel(&mut self, ui: &mut Ui, set: usize, part: usize) {
        let id = self
            .ptcl
            .as_ref()
            .and_then(|p| p.emitter_sets.get(set)?.emitters.get(part))
            .and_then(|em| attr(em, MODEL_ATTR))
            .map(AttrValue::as_u64);
        let Some(info) = id.and_then(|id| self.models.pool.as_ref()?.info(id).cloned()) else {
            return;
        };
        ui.add_space(6.0);
        egui::CollapsingHeader::new(egui::RichText::new("Model").strong())
            .id_salt("part_model")
            .default_open(true)
            .show(ui, |ui| self.draw_model(ui, &info));
    }

    fn draw_model(&mut self, ui: &mut Ui, info: &ModelInfo) {
        let replaced = self
            .models
            .imports
            .iter()
            .find(|import| import.model_id == info.id)
            .cloned();
        ui.label(&info.name);
        if replaced.is_none() {
            ui.label(
                egui::RichText::new(format!(
                    "{} vertices, {} triangles",
                    info.vertices, info.triangles
                ))
                .small()
                .color(egui::Color32::GRAY),
            );
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button("Export model")
                .on_hover_text(
                    "Save this model as .glb or .obj. GLB keeps everything; OBJ has no opacity \
                     per vertex.",
                )
                .clicked()
            {
                if let Some(path) = model_dialog()
                    .set_title("Export model")
                    .set_file_name(format!("{}.glb", info.name))
                    .save_file()
                {
                    self.export_model(info, &path);
                }
            }
            let users = match self.model_users(info.id) {
                1 => "1 part".to_owned(),
                n => format!("{n} parts"),
            };
            if ui
                .small_button("Replace model")
                .on_hover_text(format!(
                    "Use your own .glb or .obj for this model, at the size it is in the file. \
                     Changes it for every part that draws it ({users})."
                ))
                .clicked()
            {
                if let Some(path) = model_dialog().set_title("Choose a model").pick_file() {
                    self.replace_model(info, &path);
                }
            }
            if let Some(existing) = &replaced {
                if ui
                    .small_button("Restore original")
                    .on_hover_text(format!("Stop using {}", existing.path))
                    .clicked()
                {
                    self.models.note = None;
                    self.change_model(info, String::new());
                }
            }
        });
        if let Some(existing) = &replaced {
            ui.label(
                egui::RichText::new(format!("replaced by {}", existing.path))
                    .small()
                    .color(egui::Color32::from_rgb(140, 200, 255)),
            );
        }
        if let Some((note, ok)) = &self.models.note {
            ui.label(egui::RichText::new(note).small().color(if *ok {
                egui::Color32::from_rgb(0x70, 0xB0, 0x70)
            } else {
                egui::Color32::from_rgb(0xD0, 0x80, 0x60)
            }));
        }
    }

    fn export_model(&mut self, info: &ModelInfo, path: &Path) {
        let Some(pool) = &self.models.pool else {
            return;
        };
        let path = match path.extension() {
            Some(_) => path.to_path_buf(),
            None => path.with_extension("glb"),
        };
        let done = pool
            .model(info.id)
            .and_then(|model| crate::model_import::save(&path, &info.name, &model));
        self.models.note = Some(match done {
            Ok(()) => (format!("saved {}", path.display()), true),
            Err(e) => (format!("export failed: {e:#}"), false),
        });
    }

    /// Put a model file in a model's place. It is tried now, so one that can't be used is
    /// refused where it was picked.
    pub(super) fn replace_model(&mut self, info: &ModelInfo, path: &Path) {
        let Some(pool) = &self.models.pool else {
            return;
        };
        let tried = crate::model_import::load(path).and_then(|model| pool.check(info.id, &model));
        match tried {
            Ok(now) => {
                self.models.note = Some((
                    format!("{} vertices, {} triangles", now.vertices, now.triangles),
                    true,
                ));
                self.change_model(info, path.to_string_lossy().into_owned());
            }
            Err(e) => self.models.note = Some((format!("import failed: {e:#}"), false)),
        }
    }

    fn change_model(&mut self, info: &ModelInfo, path: String) {
        self.models.pending.push(ModelImport {
            model_id: info.id,
            name: info.name.clone(),
            path,
        });
        touch(&mut self.eff_dirty_at, &mut self.edit_rev);
    }
}

fn model_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new()
        .add_filter("Model", &["glb", "obj"])
        .add_filter("glTF", &["glb"])
        .add_filter("OBJ", &["obj"])
}
