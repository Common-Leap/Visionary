//! Portable chart selection; a file or clipboard never changes the project on its own.

use std::collections::BTreeMap;

use egui::Ui;

use crate::gimmicks::controller_asset::ControllerAsset;
use crate::mod_project::ControllerChart;

#[derive(Default)]
pub(super) struct ControllerLibrary {
    clipboard: Option<ControllerAsset>,
    pending: Option<ControllerAsset>,
    target_id: String,
    status: Option<Result<String, String>>,
}

impl ControllerLibrary {
    pub fn clear_pending(&mut self) {
        self.pending = None;
        self.target_id.clear();
        self.status = None;
    }

    pub fn applied(&mut self) {
        self.pending = None;
        self.status = Some(Ok("Controller added · save the project to keep it".into()));
    }

    fn stage(
        &mut self,
        asset: ControllerAsset,
        charts: Option<&BTreeMap<String, ControllerChart>>,
    ) {
        self.target_id = suggested_id(asset.source_chart_id(), charts);
        self.pending = Some(asset);
        self.status = None;
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        selected: &str,
        charts: Option<&BTreeMap<String, ControllerChart>>,
    ) {
        let source = charts.and_then(|charts| charts.get(selected));
        ui.menu_button("Library", |ui| {
            if ui.add_enabled(source.is_some(), egui::Button::new("Copy controller")).clicked() {
                match capture(fighter, selected, source) {
                    Ok(asset) => {
                        self.status = Some(Ok(format!("Copied {}", asset.label())));
                        self.clipboard = Some(asset);
                    }
                    Err(error) => self.status = Some(Err(error)),
                }
                ui.close();
            }
            if ui.add_enabled(self.clipboard.is_some(), egui::Button::new("Paste controller")).clicked() {
                self.stage(self.clipboard.as_ref().expect("enabled clipboard").clone(), charts);
                ui.close();
            }
            ui.separator();
            if ui.add_enabled(source.is_some(), egui::Button::new("Export controller")).clicked() {
                match capture(fighter, selected, source) {
                    Ok(asset) => {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("Export portable controller")
                            .add_filter("Controller library", &["json"])
                            .set_file_name("controller.json").save_file()
                        {
                            self.status = Some(crate::gimmicks::controller_asset_io::save_new(&path, &asset)
                                .map(|()| format!("Exported {}", asset.label())));
                        }
                    }
                    Err(error) => self.status = Some(Err(error)),
                }
                ui.close();
            }
            if ui.button("Import controller").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Import portable controller")
                    .add_filter("Controller library", &["json"]).pick_file()
                {
                    match crate::gimmicks::controller_asset_io::load(&path) {
                        Ok(asset) => self.stage(asset, charts),
                        Err(error) => { self.pending = None; self.status = Some(Err(error)); }
                    }
                }
                ui.close();
            }
        }).response.on_hover_text("Reuse an additional controller's applied graph and card layout. Main and unapplied edits are not copied. Native character resources keep their exact ownership.");
    }

    /// Render outside the toolbar so the preview has the field's full width.
    pub fn preview(&mut self, ui: &mut Ui) -> Option<(ControllerAsset, String)> {
        let mut requested = None;
        let mut cancel = false;
        if let Some(asset) = &self.pending {
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(asset.label());
                    ui.weak(format!("from {} / {}", asset.donor_fighter(), asset.source_chart_id()));
                });
                let graph = &asset.chart().graph;
                ui.weak(format!("{} states · {} variables · {} named events", graph.states.len(), graph.vars.len(), graph.emission_plan.event_bindings.len()));
                ui.horizontal_wrapped(|ui| {
                    ui.label("Controller ID");
                    ui.add(egui::TextEdit::singleline(&mut self.target_id).desired_width(180.0).char_limit(48));
                    if ui.button("Add controller").on_hover_text("Validate the complete character, then add this applied controller. Existing controllers are never replaced. Compatible live preview updates automatically.").clicked() {
                        requested = Some((asset.clone(), self.target_id.clone()));
                    }
                    cancel = ui.small_button("Cancel").clicked();
                });
                ui.weak("New independent state; native identifiers are not remapped. Save the project after adding.");
            });
        }
        if cancel {
            self.clear_pending();
        }
        if let Some(status) = &self.status {
            match status {
                Ok(message) => {
                    ui.weak(message);
                }
                Err(error) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
            }
        }
        requested
    }
}

fn capture(
    fighter: &str,
    selected: &str,
    source: Option<&ControllerChart>,
) -> Result<ControllerAsset, String> {
    ControllerAsset::capture(
        fighter,
        selected,
        selected,
        source.ok_or("Select an additional controller first")?,
    )
}

fn suggested_id(source: &str, charts: Option<&BTreeMap<String, ControllerChart>>) -> String {
    let occupied = |id: &str| charts.is_some_and(|charts| charts.contains_key(id));
    if !occupied(source) {
        return source.to_owned();
    }
    for number in 1..=9 {
        let suffix = format!("_{number}");
        let mut stem = source;
        while stem.len() + suffix.len() > crate::gimmicks::controller_protocol::MAX_CHART_ID_BYTES {
            stem = &stem[..stem.char_indices().last().expect("bounded suffix fits").0];
        }
        let id = format!("{stem}{suffix}");
        if !occupied(&id) {
            return id;
        }
    }
    source.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::{behavior::BehaviorGraph, templates::Preset};

    fn asset() -> ControllerAsset {
        ControllerAsset::capture(
            "cloud",
            "timer",
            "Timer",
            &ControllerChart {
                graph: Preset::Timer.graph(),
                layout: Default::default(),
            },
        )
        .unwrap()
    }

    #[test]
    fn idle_library_and_pending_preview_do_not_change_project_or_emit_import() {
        let ctx = egui::Context::default();
        let mut library = ControllerLibrary::default();
        let original = BTreeMap::from([("timer".into(), asset().chart().clone())]);
        let charts = original.clone();
        library.stage(asset(), Some(&charts));
        assert_eq!(library.target_id, "timer_1");
        let _ = ctx.run_ui(Default::default(), |ui| {
            library.ui(ui, "mario", "timer", Some(&charts));
            assert!(library.preview(ui).is_none());
        });
        assert_eq!(charts, original);
        assert!(library.pending.is_some());
        assert!(asset()
            .candidate("mario", &BehaviorGraph::default(), &charts, "timer_1")
            .is_ok());
    }

    #[test]
    fn switching_character_discards_preview_but_preserves_clipboard_for_reuse() {
        let mut library = ControllerLibrary {
            clipboard: Some(asset()),
            ..Default::default()
        };
        library.stage(asset(), None);
        library.clear_pending();
        assert!(library.pending.is_none());
        assert!(library.target_id.is_empty());
        assert_eq!(library.clipboard.unwrap().donor_fighter(), "cloud");
    }

    #[test]
    fn suggested_collision_id_preserves_utf8_bound_and_never_overwrites() {
        let source = "é".repeat(24);
        let chart = asset().chart().clone();
        let charts = BTreeMap::from([
            (source.clone(), chart.clone()),
            (format!("{}_1", "é".repeat(23)), chart),
        ]);
        let id = suggested_id(&source, Some(&charts));
        assert_eq!(id, format!("{}_2", "é".repeat(23)));
        assert_eq!(id.len(), 48);
        assert!(!charts.contains_key(&id));
    }
}
