//! Character-aware gimmick editing in the main workspace.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use egui::{RichText, Ui};

use super::behavior::{BehaviorGraph, ConstValue, Expr, VarDef, VarType};
use super::controller::{ControllerAction, ControllerActionKind};
use super::native_inputs::{
    CombatChannel, CombatOp, Facing, ItemAction, MovementAction, MovementStat, NativeInput,
    NativeKind, ObjectOp, PolicyScope,
};
use super::templates::{BehaviorTemplate, Preset};
use super::view::BaseState;
use crate::game_link::GameLink;
use crate::mod_project::{ParamMod, ParamValue};

mod controller_library;
mod controllers;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterContext {
    pub fighter: String,
    pub name: String,
    pub costume_slots: Vec<u8>,
}

impl CharacterContext {
    pub fn new(fighter: &str, name: &str, costume_slots: Vec<u8>) -> Self {
        Self {
            fighter: fighter.to_string(),
            name: name.to_string(),
            costume_slots,
        }
    }

    fn scope_description(&self) -> String {
        if self.costume_slots.is_empty() {
            format!(
                "Native parameters and the saved behavior apply to all costumes of {}.",
                self.fighter
            )
        } else {
            format!("{} uses {} as its engine fighter. Gimmicks are shared with all donor costumes; the costume-scoped move target does not isolate parameters, meters, or behavior.", self.name, self.fighter)
        }
    }
}

pub struct EditorData<'a> {
    pub roots: &'a [PathBuf],
    pub labels: &'a HashMap<u64, String>,
    pub params: &'a mut BTreeMap<String, ParamMod>,
    pub behaviors: &'a mut BTreeMap<String, BehaviorGraph>,
    pub behavior_layouts: &'a mut BTreeMap<String, super::field_canvas::FieldLayout>,
    pub controller_charts:
        &'a mut BTreeMap<String, BTreeMap<String, crate::mod_project::ControllerChart>>,
    pub meters: &'a mut BTreeMap<String, Vec<super::meter::MeterBinding>>,
    pub link: &'a GameLink,
    pub recipes: Option<&'a BTreeMap<String, crate::move_nodes::model::MoveGraph>>,
}

struct SavedGimmicks {
    behavior: Option<BehaviorGraph>,
    personal: Option<ParamMod>,
    meters: Option<Vec<super::meter::MeterBinding>>,
    layout: Option<super::field_canvas::FieldLayout>,
    charts: Option<BTreeMap<String, crate::mod_project::ControllerChart>>,
}

impl SavedGimmicks {
    fn capture(fighter: &str, data: &EditorData<'_>) -> Self {
        Self {
            behavior: data.behaviors.get(fighter).cloned(),
            personal: data.params.get(fighter).cloned(),
            meters: data.meters.get(fighter).cloned(),
            layout: data.behavior_layouts.get(fighter).cloned(),
            charts: data.controller_charts.get(fighter).cloned(),
        }
    }

    fn validate_change(self, fighter: &str, data: &mut EditorData<'_>) -> Result<(), String> {
        let main_unchanged = self.behavior.as_ref() == data.behaviors.get(fighter);
        let old_charts = self.charts.as_ref();
        let new_charts = data.controller_charts.get(fighter);
        // Layout-only edits and removal of bad charts are safe repair operations.
        // Adding or changing a graph must validate the complete remaining owner.
        let only_layout_or_removal = new_charts.into_iter().flatten().all(|(id, chart)| {
            old_charts
                .and_then(|charts| charts.get(id))
                .is_some_and(|old| old.graph == chart.graph)
        });
        let validation = if main_unchanged && only_layout_or_removal {
            old_charts
                .into_iter()
                .flatten()
                .filter(|(id, _)| !new_charts.is_some_and(|charts| charts.contains_key(*id)))
                .try_for_each(|(id, _)| controllers::can_remove_chart(id, data.recipes))
        } else {
            controllers::effective_main(fighter, data.behaviors, data.recipes, new_charts).and_then(
                |main| {
                    if let Some(charts) = new_charts.filter(|charts| !charts.is_empty()) {
                        super::controller_project::compile(fighter, &main, charts)?;
                    }
                    Ok(())
                },
            )
        };
        if let Err(error) = validation {
            restore_saved(data.behaviors, fighter, self.behavior);
            restore_saved(data.params, fighter, self.personal);
            restore_saved(data.meters, fighter, self.meters);
            restore_saved(data.behavior_layouts, fighter, self.layout);
            restore_saved(data.controller_charts, fighter, self.charts);
            return Err(format!("Controller not applied: {error}"));
        }
        Ok(())
    }
}

fn restore_saved<T>(store: &mut BTreeMap<String, T>, fighter: &str, saved: Option<T>) {
    if let Some(saved) = saved {
        store.insert(fighter.to_owned(), saved);
    } else {
        store.remove(fighter);
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Section {
    #[default]
    Parameters,
    Behavior,
    Meters,
}

#[derive(Default)]
pub struct GimmicksEditor {
    /// Set when the user asks to edit controllers in the node workspace; the app consumes it.
    pub open_workspace: bool,
    live: super::live_sync::LiveSync,
    action_drafts: HashMap<String, ControllerAction>,
    action_test_boid: Option<u32>,
    preview_frames: u32,
    preview_cursor: usize,
    preview: Vec<super::controller::ControllerFrame>,
    preview_graph: Option<BehaviorGraph>,
    fighter: String,
    roots: Vec<PathBuf>,
    base: Option<BaseState>,
    profile: Option<super::catalog::FighterProfile>,
    section: Section,
    search: String,
    status: String,
    pending: Option<(String, BehaviorGraph)>,
    pending_setup: Option<(String, super::blueprint::PreparedGimmicks)>,
    character: Option<crate::roster::RosterKey>,
    advanced_params: super::view::PersonalView,
    chart: String,
    chart_sessions: BTreeMap<String, controllers::ControllerSession>,
    pending_chart_delete: Option<String>,
    controller_library: controller_library::ControllerLibrary,
    meter: super::meter_view::MeterView,
    operations: super::operations_view::OperationsView,
}

impl GimmicksEditor {
    pub fn set_recipe_errors(&mut self, errors: BTreeMap<String, String>) {
        self.live.set_recipe_errors(errors);
    }

    pub fn sync_live(
        &mut self,
        params: &BTreeMap<String, ParamMod>,
        graphs: &BTreeMap<String, BehaviorGraph>,
        link: &GameLink,
    ) {
        self.live.update(params, graphs, link);
    }

    pub fn pack_messages(
        &mut self,
        params: &BTreeMap<String, ParamMod>,
        graphs: &BTreeMap<String, BehaviorGraph>,
        charts: &BTreeMap<String, BTreeMap<String, crate::mod_project::ControllerChart>>,
    ) -> Result<Vec<String>, String> {
        self.live.pack_messages(params, graphs, charts)
    }

    pub fn sync_live_controllers(
        &mut self,
        params: &BTreeMap<String, ParamMod>,
        graphs: &BTreeMap<String, BehaviorGraph>,
        charts: &BTreeMap<String, BTreeMap<String, crate::mod_project::ControllerChart>>,
        link: &GameLink,
    ) {
        self.live
            .update_with_controllers(params, graphs, charts, link);
    }

    fn live_status(&mut self, ui: &mut Ui, fighter: &str, link: &GameLink) {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.live.enabled, "Live preview").on_hover_text("Automatically preview supported saved parameters and behavior. Turning this off removes preview overrides and restores the installed mod; project edits remain saved.");
            if ui.small_button("Retry").clicked() { self.live.retry(); }
            crate::ui::game_connection(ui, link.status());
        });
        if link.gimmick_live_snapshot().is_none() {
            ui.weak("Live preview needs the updated plugin and a running match.");
        } else {
            ui.horizontal_wrapped(|ui| {
                for (family, title) in [("parameters", "Parameters"), ("behavior", "Behavior")] {
                    if self.live.error_for(family).is_some() {
                        ui.weak(format!("{title}: not sent; see details below"));
                        continue;
                    }
                    let status = match link.gimmick_live_ack(family) {
                        Some(ack) if ack.accepted => format!(
                            "{title}: {}",
                            if self.live.enabled {
                                if family == "parameters" {
                                    "table loaded"
                                } else {
                                    "accepted"
                                }
                            } else {
                                "preview cleared"
                            }
                        ),
                        Some(ack) => format!("{title}: {}", ack.message),
                        None => format!("{title}: awaiting acknowledgement"),
                    };
                    ui.weak(status);
                }
            });
        }
        if let Some(warnings) = self.live.warnings.get(fighter) {
            egui::CollapsingHeader::new(format!("{} live-preview issue(s)", warnings.len()))
                .id_salt("live_gimmick_warnings")
                .show(ui, |ui| {
                    for warning in warnings {
                        ui.weak(warning);
                    }
                });
        }
        if !self.live.error.is_empty() {
            ui.weak(&self.live.error);
        }
    }
    /// Donor-backed custom characters use the donor's capabilities without inventing an ID.
    pub fn character_context(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        name: &str,
        authored: &[crate::mod_project::AuthoredEntry],
        suggested_slots: &[u8],
    ) -> CharacterContext {
        if self.fighter != fighter {
            self.character = authored
                .iter()
                .find(|entry| entry.donor == fighter && entry.all_slots() == suggested_slots)
                .map(|entry| entry.key.clone());
        }
        if self.character.as_ref().is_some_and(|key| {
            !authored
                .iter()
                .any(|entry| &entry.key == key && entry.donor == fighter)
        }) {
            self.character = None;
        }
        let selected = self
            .character
            .as_ref()
            .and_then(|key| authored.iter().find(|entry| &entry.key == key));
        let selected_name = selected.map_or(name, |entry| entry.display_name.as_str());
        ui.horizontal_wrapped(|ui| {
            ui.label("Character");
            egui::ComboBox::from_id_salt("main_gimmick_character")
                .selected_text(selected_name)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.character, None, name);
                    for entry in authored.iter().filter(|entry| entry.donor == fighter) {
                        ui.selectable_value(
                            &mut self.character,
                            Some(entry.key.clone()),
                            &entry.display_name,
                        );
                    }
                });
        });
        let selected = self
            .character
            .as_ref()
            .and_then(|key| authored.iter().find(|entry| &entry.key == key));
        CharacterContext::new(
            fighter,
            selected.map_or(name, |entry| entry.display_name.as_str()),
            selected.map_or_else(Vec::new, |entry| entry.all_slots()),
        )
    }

    pub fn ui(&mut self, ui: &mut Ui, context: &CharacterContext, mut data: EditorData<'_>) {
        let saved = SavedGimmicks::capture(&context.fighter, &data);
        if self.fighter != context.fighter || self.roots != data.roots {
            self.fighter = context.fighter.clone();
            self.roots = data.roots.to_vec();
            self.base = None;
            self.profile = super::catalog::profile(&context.fighter);
            self.search.clear();
            self.status.clear();
            self.pending = None;
            self.pending_setup = None;
            // Nested drafts belong to the exact current project/character context.
            self.clear_controller_sessions();
            self.meter = Default::default();
            self.operations = Default::default();
            self.preview.clear();
        }
        let profile = &self.profile;
        ui.horizontal_wrapped(|ui| {
            ui.strong(&context.name);
            if let Some(profile) = profile {
                if !profile.mechanics.is_empty() {
                    ui.weak(profile.mechanics.join(" · ")).on_hover_text(
                        "Character mechanics are descriptive. The controls below distinguish editable parameters, authored behavior, and validated native event sources."
                    );
                }
            }
            ui.label(RichText::new(if context.costume_slots.is_empty() { "Fighter-wide" } else { "Donor-wide" }).small().weak())
                .on_hover_text(context.scope_description());
        });
        let additional_count = data
            .controller_charts
            .get(&context.fighter)
            .map_or(0, BTreeMap::len);
        if additional_count > 0 {
            self.pending_setup = None;
            ui.weak(format!("{additional_count} additional controller{}", if additional_count == 1 { "" } else { "s" }))
                .on_hover_text("Live preview requires a chart-capable plugin. Normal packages include every controller with a compatible selected SLight source. Standalone source and setup files cannot carry additional charts.");
        }
        ui.horizontal_wrapped(|ui| {
            let setup_available = data.controller_charts.get(&context.fighter).is_none_or(BTreeMap::is_empty);
            let setup_hint = if setup_available {
                "Export only this donor's authored personal values, behavior, and meters as a reusable setup. No base-game files are included."
            } else {
                "This character has additional controllers. Save the project to retain them; the current setup format cannot include them."
            };
            if ui.add_enabled(setup_available, egui::Button::new("Save setup")).on_hover_text(setup_hint).clicked() {
                let empty = ParamMod::default();
                let blueprint = super::blueprint::GimmickBlueprint::from_fighter(
                    &format!("{} gimmicks", context.name), &context.fighter,
                    data.params.get(&context.fighter).unwrap_or(&empty),
                    data.behaviors.get(&context.fighter).cloned().unwrap_or_default(),
                    data.meters.get(&context.fighter).cloned().unwrap_or_default(),
                );
                match blueprint {
                    Ok(blueprint) => {
                        if let Some(path) = rfd::FileDialog::new().add_filter("Gimmick setup", &["json"]).set_file_name(format!("{}-gimmicks.json", context.fighter)).save_file() {
                            self.status = match blueprint.encode().and_then(|text| write_text(&path, &text)) {
                                Ok(()) => format!("Setup saved to {}", path.display()), Err(error) => error,
                            };
                        }
                    }
                    Err(error) => self.status = error,
                }
            }
            if ui.add_enabled(setup_available, egui::Button::new("Load setup")).on_hover_text(if setup_available { "Validate all paths, types, source ownership, and behavior before replacing any saved gimmick edits." } else { setup_hint }).clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("Gimmick setup", &["json"]).pick_file() {
                    self.pending_setup = None;
                    if self.base.is_none() { self.base = Some(super::view::load_base(data.roots, &context.fighter)); }
                    let base = match self.base.as_ref() { Some(BaseState::Present(tree)) => Some(tree), _ => None };
                    match read_text(&path).and_then(|text| super::blueprint::GimmickBlueprint::decode(&text)).and_then(|blueprint| {
                        let prepared = blueprint.prepare(&context.fighter, base)?;
                        Ok((blueprint.name, prepared))
                    }) {
                        Ok(prepared) => self.pending_setup = Some(prepared), Err(error) => self.status = error,
                    }
                }
            }
        });
        if let Some((name, _)) = &self.pending_setup {
            let mut apply = false;
            let mut cancel = false;
            ui.group(|ui| {
                ui.label(format!("Load {name}?"));
                ui.weak("Replace this donor's personal overrides, behavior, and meters, including advanced drafts. Other fighter and trait edits are preserved.");
                ui.horizontal(|ui| { apply = ui.button("Apply setup").clicked(); cancel = ui.button("Cancel").clicked(); });
            });
            if apply {
                let (_, prepared) = self.pending_setup.take().unwrap();
                self.status = match apply_setup(
                    &context.fighter,
                    &prepared,
                    data.params,
                    data.behaviors,
                    data.meters,
                ) {
                    Ok(()) => {
                        self.meter = Default::default();
                        self.advanced_params = Default::default();
                        self.pending = None;
                        "Setup saved in the project; export the mod to apply it.".into()
                    }
                    Err(error) => error,
                };
            } else if cancel {
                self.pending_setup = None;
            }
        }
        ui.horizontal_wrapped(|ui| {
            for (section, label) in [
                (Section::Parameters, "Parameters"),
                (Section::Behavior, "Behavior"),
                (Section::Meters, "Meters"),
            ] {
                let available = match section {
                    Section::Parameters => true,
                    Section::Behavior => profile.is_some(),
                    Section::Meters => profile.as_ref().is_some_and(|profile| {
                        profile.work_floats + profile.work_ints + profile.work_flags > 0
                    }),
                };
                if !available && self.section == section {
                    self.section = Section::Parameters;
                }
                if ui
                    .add_enabled(
                        available,
                        egui::Button::selectable(self.section == section, label),
                    )
                    .clicked()
                {
                    self.section = section;
                    self.status.clear();
                }
            }
        });
        // Capability details stay collapsible; they are not promises of native mechanic control.
        if self.section == Section::Parameters {
            egui::CollapsingHeader::new("Available surfaces").id_salt("gimmick_surfaces").show(ui, |ui| {
            if let Some(profile) = profile {
                ui.weak(format!("{} articles · {} weapon statuses · {} fighter statuses", profile.articles, profile.weapon_statuses, profile.fighter_statuses));
                ui.weak(format!("Work values: {} decimals · {} integers · {} flags", profile.work_floats, profile.work_ints, profile.work_flags));
                if profile.native_sources.is_empty() {
                    ui.weak("No validated native resource event adapter yet. Authored state/timers and discovered personal values remain available.");
                } else {
                    for source in &profile.native_sources {
                        ui.label(source.label()).on_hover_text(source.description());
                    }
                }
            } else {
                ui.weak("Unknown engine fighter: personal-file discovery only. Native identifiers and generated behavior stay guarded.");
            }
        });
        }
        ui.separator();
        let has_articles = profile
            .as_ref()
            .is_some_and(|profile| profile.articles + profile.weapon_statuses > 0);
        self.live_status(ui, &context.fighter, data.link);
        egui::ScrollArea::vertical()
            .id_salt(("main_gimmicks", &context.fighter))
            .show(ui, |ui| {
                match self.section {
                    Section::Parameters => self.parameters(ui, &mut data),
                    Section::Behavior => {
                        self.controller_field(ui, &context.fighter, &mut data);
                        // Direct article/weapon operations are a live test aid, so they sit
                        // under the controller instead of in a tab of their own.
                        if has_articles {
                            self.operations.ui(ui, &context.fighter, data.link);
                        }
                    }
                    Section::Meters => self.meter.ui(ui, &context.fighter, data.meters, data.link),
                }
                if let Err(error) = saved.validate_change(&context.fighter, &mut data) {
                    self.status = error;
                }
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(&self.status).small());
                }
            });
    }

    pub fn invalidate(&mut self) {
        let enabled = self.live.enabled;
        *self = Self::default();
        self.live.enabled = enabled;
    }

    fn parameters(&mut self, ui: &mut Ui, data: &mut EditorData<'_>) {
        ui.weak("Supported numeric values preview on subsequent engine reads. Cached values may need the move or fighter to restart; other paths require export.");
        if let Some(health) = data
            .link
            .gimmick_live_snapshot()
            .and_then(|snapshot| snapshot.parameter_health)
            .filter(|health| {
                data.link
                    .gimmick_live_ack("parameters")
                    .is_some_and(|ack| ack.accepted && ack.revision == health.revision)
            })
        {
            egui::CollapsingHeader::new("Live parameter diagnostics").id_salt("parameter_getter_health").show(ui, |ui| {
                match health.getter_backend {
                    Some(super::live_params::ParameterGetterBackend::NativeParamData) => { ui.weak("Native parameter getters"); }
                    Some(super::live_params::ParameterGetterBackend::WorkModuleExport) => { ui.weak("Exported-getter fallback: native mechanics may bypass this preview."); }
                    None => { ui.weak("Getter backend not reported by this plugin."); }
                }
                ui.weak("Game-wide counters since the accepted parameter snapshot; field read counts remain fighter-specific.");
                ui.weak(format!("Getter reads: {} float · {} integer; {} overrides returned", health.float_wrapper_calls, health.int_wrapper_calls, health.overrides_returned));
                ui.weak(format!("Fighter identity: {} matches · {} misses; parameter keys: {} matches · {} misses", health.identity_matches, health.identity_misses, health.parameter_key_matches, health.parameter_key_misses));
                ui.weak(format!("{} type mismatches · {} lock contentions · {} disabled-table reads", health.type_mismatches, health.identity_lock_busy.saturating_add(health.overlay_lock_busy), health.overlay_disabled));
                if health.float_wrapper_calls == 0 && health.int_wrapper_calls == 0 { ui.weak("No reads reached the installed getters. An accepted table alone does not prove live application."); }
            });
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Personal values").strong()).on_hover_text(
                "Typed scalar controls discovered from this character's actual fighter/<name>/param/vl.prc. Only changed values are saved; the source file is never edited."
            );
            if ui.small_button("Reload").clicked() {
                self.base = None;
                self.advanced_params = Default::default();
            }
        });
        if self.base.is_none() {
            self.base = Some(super::view::load_base(data.roots, &self.fighter));
        }
        crate::ui::search_field(
            ui,
            "gimmick_parameter_search",
            &mut self.search,
            "Filter parameters…",
        );
        match self.base.as_ref().unwrap() {
            BaseState::Missing => {
                ui.weak("No personal parameter file found in the active data or mod roots.");
            }
            BaseState::Unreadable(error) => {
                ui.label(error);
            }
            BaseState::Present(tree) => {
                let empty = ParamMod::default();
                match super::controls::controls(
                    tree,
                    &self.fighter,
                    data.labels,
                    data.params.get(&self.fighter).unwrap_or(&empty),
                ) {
                    Ok(controls) => {
                        let live_snapshot = data.link.gimmick_live_snapshot().filter(|snapshot| {
                            self.live.error_for("parameters").is_none()
                                && data.link.gimmick_live_ack("parameters").is_some_and(|ack| {
                                    ack.accepted && ack.revision == snapshot.parameter_revision
                                })
                        });
                        let query = self.search.to_lowercase();
                        let mut groups = BTreeMap::<String, Vec<_>>::new();
                        for control in controls.into_iter().filter(|control| {
                            query.is_empty()
                                || control.path.to_lowercase().contains(&query)
                                || control.label.to_lowercase().contains(&query)
                                || control.group.to_lowercase().contains(&query)
                        }) {
                            groups
                                .entry(control.group.clone())
                                .or_default()
                                .push(control);
                        }
                        if groups.is_empty() {
                            ui.weak(if data.labels.is_empty() { "Parameter labels are still loading; unresolved fields are not editable." } else { "No named scalar parameters match the filter." });
                        }
                        for (group, controls) in groups {
                            egui::CollapsingHeader::new(&group)
                                .id_salt(("gimmick_parameter_group", &self.fighter, &group))
                                .default_open(!query.is_empty())
                                .show(ui, |ui| {
                                    for control in controls {
                                        ui.push_id(&control.path, |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label(&control.label)
                                                    .on_hover_text(&control.path);
                                                let support = super::live_sync::parameter(&self.fighter, &control.path, control.value);
                                                ui.weak(if support.is_ok() { "preview" } else { "export" })
                                                    .on_hover_text(support.err().unwrap_or_else(|| "Eligible for live preview, not proof of application. A matching hooked getter must read this value; native mechanics can bypass these getters and cached values are not recomputed.".into()));
                                                if control.overridden {
                                                    if let Some(row) = live_snapshot.as_ref().and_then(|snapshot| snapshot.parameters.iter().find(|row| row.get("fighter").and_then(|v| v.as_str()) == Some(&self.fighter) && row.get("path").and_then(|v| v.as_str()) == Some(&control.path))) {
                                                        let hits = row.get("hits").and_then(|v| v.as_u64()).unwrap_or(0);
                                                        ui.weak(match hits { 0 => "not read yet".into(), 1 => "1 read".into(), _ => format!("{hits} reads") });
                                                    }
                                                }
                                                let mut value = control.value;
                                                if param_control(ui, &mut value) {
                                                    self.status = match super::controls::save(
                                                        data.params
                                                            .entry(self.fighter.clone())
                                                            .or_default(),
                                                        &self.fighter,
                                                        &control,
                                                        value,
                                                    ) {
                                                        Ok(()) => {
                                                            "Personal value saved in the project."
                                                                .into()
                                                        }
                                                        Err(error) => error,
                                                    };
                                                }
                                                if control.overridden
                                                    && ui
                                                        .small_button("↺")
                                                        .on_hover_text(
                                                            "Return to the base-file value",
                                                        )
                                                        .clicked()
                                                {
                                                    self.status = match super::controls::save(
                                                        data.params
                                                            .entry(self.fighter.clone())
                                                            .or_default(),
                                                        &self.fighter,
                                                        &control,
                                                        control.base,
                                                    ) {
                                                        Ok(()) => {
                                                            "Personal override removed.".into()
                                                        }
                                                        Err(error) => error,
                                                    };
                                                }
                                            });
                                        });
                                    }
                                });
                        }
                    }
                    Err(error) => {
                        ui.label(error);
                    }
                }
            }
        }
        egui::CollapsingHeader::new("Individual parameter / saved edits")
            .id_salt("gimmick_individual_parameter")
            .show(ui, |ui| {
                self.advanced_params.ui_for_fighter(
                    ui,
                    &self.fighter,
                    data.roots,
                    data.labels,
                    data.params,
                );
            });
    }

    fn behaviors(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        graphs: &mut BTreeMap<String, BehaviorGraph>,
        link: &GameLink,
        recipes: Option<&BTreeMap<String, crate::move_nodes::model::MoveGraph>>,
    ) {
        // Every way to fill the controller from somewhere else lives in one menu; each stages
        // a confirmation below instead of replacing the graph immediately.
        let has_graph = graphs.get(fighter).is_some_and(|graph| !graph.is_empty());
        ui.menu_button("Start from", |ui| {
            ui.menu_button("Preset", |ui| {
                for preset in Preset::ALL {
                    if ui.button(preset.label()).clicked() {
                        self.pending = Some((preset.label().to_string(), preset.graph()));
                        ui.close();
                    }
                }
                ui.separator();
                for defense in super::status_recipes::Defense::ALL {
                    if ui.button(defense.label()).on_hover_text(defense.hover()).clicked() {
                        match defense.graph(fighter) {
                            Ok(graph) => self.pending = Some((defense.label().to_string(), graph)),
                            Err(error) => self.status = error,
                        }
                        ui.close();
                    }
                }
                if fighter == "cloud" && ui.button("Limit events").clicked() {
                    self.pending = Some(("Limit event counters".to_string(), super::templates::limit_event_starter()));
                    ui.close();
                }
            });
            if self.selected_chart() == super::controller_project::MAIN_CHART {
                let candidates: Vec<_> = graphs
                    .iter()
                    .filter(|(name, graph)| name.as_str() != fighter && !graph.is_empty())
                    .map(|(name, _)| name.clone())
                    .collect();
                ui.add_enabled_ui(!candidates.is_empty(), |ui| {
                    ui.menu_button("Copy another character", |ui| {
                        for name in candidates {
                            if ui.button(&name).clicked() {
                                self.pending = None;
                                if let Some(source) = graphs.get(&name) {
                                    match super::templates::copy_for_fighter(source, &name, fighter) {
                                        Ok(graph) => self.pending = Some((format!("Copy of {name}"), graph)),
                                        Err(error) => self.status = error,
                                    }
                                }
                                ui.close();
                            }
                        }
                    });
                });
            }
            ui.separator();
            if ui.button("Import template").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Gimmick template", &["json"])
                    .pick_file()
                {
                    self.pending = None;
                    match read_template(&path).and_then(|template| {
                        let graph = template.instantiate(fighter)?;
                        Ok((template.name, graph))
                    }) {
                        Ok(pending) => self.pending = Some(pending),
                        Err(error) => self.status = error,
                    }
                }
                ui.close();
            }
            if ui.add_enabled(has_graph, egui::Button::new("Export template")).clicked() {
                if let Some(graph) = graphs.get(fighter) {
                    match BehaviorTemplate::from_graph(
                        &format!("{fighter} behavior"),
                        Some(fighter),
                        graph.clone(),
                    ) {
                        Ok(template) => {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Gimmick template", &["json"])
                                .set_file_name(format!("{fighter}-gimmick.json"))
                                .save_file()
                            {
                                self.status = match write_template(&path, &template) {
                                    Ok(()) => format!("Template saved to {}", path.display()),
                                    Err(error) => error,
                                };
                            }
                        }
                        Err(error) => self.status = error,
                    }
                }
                ui.close();
            }
        })
        .response
        .on_hover_text("Fill this controller from a preset, another character or a template file. You confirm before anything is replaced.");
        if let Some((name, _)) = &self.pending {
            let mut apply = false;
            let mut cancel = false;
            ui.group(|ui| {
                ui.label(format!("Use {name}?"));
                if graphs.get(fighter).is_some_and(|graph| !graph.is_empty()) {
                    ui.weak(
                        "This replaces the selected controller's saved graph and unapplied draft.",
                    );
                }
                ui.horizontal(|ui| {
                    apply = ui.button("Apply").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
            if apply {
                let (_, graph) = self.pending.take().unwrap();
                match BehaviorTemplate::from_graph("Controller", Some(fighter), graph)
                    .and_then(|template| template.instantiate(fighter))
                {
                    Ok(graph) => {
                        graphs.insert(fighter.to_string(), graph);
                        self.status =
                            "Controller saved; supported live preview updates automatically."
                                .into();
                    }
                    Err(error) => self.status = error,
                }
            } else if cancel {
                self.pending = None;
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("Edit in Move nodes").clicked() {
                self.open_workspace = true;
            }
            ui.weak("States, events and actions are edited beside the move scripts.");
        });
        if let Some(graph) = graphs
            .get(fighter)
            .filter(|graph| !graph.is_empty())
            .cloned()
        {
            self.action_test_target(ui, fighter, link);
            self.timer_controls(ui, fighter, graph, graphs, link);
        }
        self.behavior_live_view(ui, fighter, graphs.get(fighter), link, recipes);
    }

    fn timer_controls(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        mut graph: BehaviorGraph,
        graphs: &mut BTreeMap<String, BehaviorGraph>,
        link: &GameLink,
    ) -> BehaviorGraph {
        let Ok(mut config) = super::controller::extract_timer(&graph) else {
            return graph;
        };
        let mut changed = false;
        let mut add_variable = None;
        ui.scope(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Duration");
                changed |= ui.add(egui::DragValue::new(&mut config.duration_frames).range(1..=super::live_model::MAX_TIMER_FRAMES as u32).suffix(" frames")).changed();
                ui.weak(format!("{:.2} seconds at 60 fps", config.duration_frames as f32 / 60.0));
                changed |= ui.checkbox(&mut config.repeats, "Repeat").changed();
            });
            ui.weak("Counts down automatically. Each expiry increments elapsed_events and raises ready; stock loss resets its state.");
            egui::CollapsingHeader::new("When the timer expires").id_salt(("timer_expiry", fighter)).default_open(true).show(ui, |ui| {
                let mut command = None;
                let count = config.expiry_actions.len();
                for (index, action) in config.expiry_actions.iter_mut().enumerate() {
                    ui.push_id(("expiry", index), |ui| {
                        ui.group(|ui| {
                            changed |= action_picker(ui, fighter, &graph, action, true, super::value_edit::ValueScope::default());
                            self.test_article_action(ui, fighter, action, link);
                            if let Some(edit) = action_row_buttons(ui, index, count) { command = Some((index, edit)); }
                        });
                    });
                }
                if let Some((index, edit)) = command {
                    match edit {
                        ActionRowEdit::Delete => { config.expiry_actions.remove(index); }
                        ActionRowEdit::Duplicate => { config.expiry_actions.insert(index + 1, config.expiry_actions[index].clone()); }
                        ActionRowEdit::Move(to) => { let action = config.expiry_actions.remove(index); config.expiry_actions.insert(to, action); }
                    }
                    changed = true;
                }
                let key = format!("{fighter}:expiry");
                let draft = self.action_drafts.entry(key).or_insert_with(|| default_action(fighter, &graph, ControllerActionKind::GenerateArticle, true));
                ui.push_id("expiry_draft", |ui| {
                    action_picker(ui, fighter, &graph, draft, true, super::value_edit::ValueScope::default());
                    if ui.add_enabled(action_available(fighter, &graph, draft, true), egui::Button::new("Add expiry action")).clicked() { config.expiry_actions.push(draft.clone()); changed = true; }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Add numeric variable").clicked() { add_variable = Some(VarType::Float); }
                    if ui.small_button("Add flag variable").clicked() { add_variable = Some(VarType::Flag); }
                });
            });
        });
        if let Some(kind) = add_variable {
            let prefix = if kind == VarType::Flag {
                "active"
            } else {
                "value"
            };
            let name = (1..)
                .map(|index| format!("{prefix}_{index}"))
                .find(|name| !graph.vars.iter().any(|var| &var.name == name))
                .unwrap();
            graph.vars.push(super::behavior::VarDef {
                keep_across_form_switch: false,
                name,
                kind,
                initial: if kind == VarType::Flag {
                    ConstValue::Flag(false)
                } else {
                    ConstValue::Float(0.0)
                },
            });
            changed = true;
        }
        if !changed {
            return graph;
        }
        match super::controller::configure_timer(&graph, fighter, config) {
            Ok(applied) => {
                graphs.insert(fighter.into(), applied.graph.clone());
                self.preview.clear();
                self.status = "Timer updated; live preview synchronizes automatically.".into();
                applied.graph
            }
            Err(error) => {
                self.status = error;
                graph
            }
        }
    }

    fn action_test_target(&mut self, ui: &mut Ui, fighter: &str, link: &GameLink) {
        let live = link.gimmick_agents();
        let players: Vec<_> = live
            .iter()
            .filter(|agent| agent.category == 0 && agent.fighter.as_deref() == Some(fighter))
            .collect();
        if self
            .action_test_boid
            .is_some_and(|boid| !players.iter().any(|agent| agent.boid == boid))
        {
            self.action_test_boid = None;
        }
        if self.action_test_boid.is_none() && players.len() == 1 {
            self.action_test_boid = Some(players[0].boid);
        }
        ui.horizontal_wrapped(|ui| {
            ui.weak("Test actions on");
            egui::ComboBox::from_id_salt(("controller_test_player", fighter))
                .selected_text(
                    players
                        .iter()
                        .find(|agent| Some(agent.boid) == self.action_test_boid)
                        .map_or_else(
                            || "Choose a live player…".into(),
                            |agent| format!("Player {}", agent.entry_id + 1),
                        ),
                )
                .show_ui(ui, |ui| {
                    for agent in &players {
                        ui.selectable_value(
                            &mut self.action_test_boid,
                            Some(agent.boid),
                            format!("Player {}", agent.entry_id + 1),
                        );
                    }
                });
            if players.is_empty() {
                ui.weak("Start a match with this fighter to test articles.");
            }
        });
    }

    fn test_article_action(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        action: &ControllerAction,
        link: &GameLink,
    ) {
        let (symbol, generate) = match action {
            ControllerAction::GenerateArticle { symbol } => (symbol, true),
            ControllerAction::RemoveArticle { symbol } => (symbol, false),
            _ => return,
        };
        let current = link.status() == crate::game_link::LinkStatus::Connected
            && self.action_test_boid.is_some_and(|boid| {
                link.gimmick_agents().iter().any(|agent| {
                    agent.category == 0
                        && agent.fighter.as_deref() == Some(fighter)
                        && agent.boid == boid
                })
            });
        if ui.add_enabled(current, egui::Button::new(if generate { "Spawn now" } else { "Remove now" }).small()).on_hover_text("One live test on the selected player. Does not start the timer or change saved behavior. Projectiles may disappear automatically.").clicked() {
            if let Some(boid) = self.action_test_boid {
                let result = if generate { link.generate_gimmick_article(fighter, boid, symbol) } else { link.remove_gimmick_article(fighter, boid, symbol) };
                self.status = match result { Ok(()) => format!("{} request queued for the selected player.", if generate { "Spawn" } else { "Remove" }), Err(error) => error.to_string() };
            }
        }
    }

    fn behavior_live_view(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        graph: Option<&BehaviorGraph>,
        link: &GameLink,
        recipes: Option<&BTreeMap<String, crate::move_nodes::model::MoveGraph>>,
    ) {
        let chart_id = self.selected_chart().to_owned();
        if self.preview_graph.as_ref() != graph {
            self.preview.clear();
            self.preview_graph = graph.cloned();
        }
        egui::CollapsingHeader::new("Live state / offline preview").id_salt(("behavior_state", fighter)).default_open(true).show(ui, |ui| {
            if let Some(snapshot) = link.gimmick_controller_snapshot().filter(|snapshot| self.live.error_for("behavior").is_none() && link.gimmick_live_ack("behavior")
                .is_some_and(|ack| ack.accepted && ack.revision == snapshot.behavior_revision)) {
                for row in snapshot.behavior.iter().filter(|row| row.get("fighter").and_then(|v| v.as_str()) == Some(fighter)
                    && row.get("chart_id").and_then(|value| value.as_str()).unwrap_or(super::controller_project::MAIN_CHART) == chart_id) {
                    ui.push_id(("live_actor", row.get("boid").and_then(|value| value.as_u64()), row.get("entry_id").and_then(|value| value.as_i64())), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("Player {} · {}", row.get("entry_id").and_then(|v| v.as_i64()).map_or(0, |id| id + 1), row.get("state").and_then(|v| v.as_str()).unwrap_or("unknown")));
                        if ui.small_button("Restart").on_hover_text("Restart only this player's saved controller; does not refill a native resource.").clicked() {
                            if let Some(boid) = row.get("boid").and_then(|value| value.as_u64()).and_then(|value| u32::try_from(value).ok()) {
                                let result = if self.selected_chart() == super::controller_project::MAIN_CHART {
                                    link.restart_gimmick_controller(fighter, boid, snapshot.behavior_revision)
                                } else {
                                    link.restart_gimmick_chart(fighter, boid, self.selected_chart(), snapshot.behavior_revision)
                                };
                                self.status = match result {
                                    Ok(()) => "Controller restart requested.".into(), Err(error) => error,
                                };
                            }
                        }
                    });
                    if let Some(values) = row.get("vars").and_then(|v| v.as_object()) {
                        live_values_ui(ui, values, graph, recipes);
                    }
                    });
                }
            }
            let Some(graph) = graph else { return; };
            ui.horizontal_wrapped(|ui| {
                if self.preview_frames == 0 { self.preview_frames = 180; }
                ui.add(egui::DragValue::new(&mut self.preview_frames).range(1..=3600).suffix(" preview frames"));
                if ui.button("Simulate").on_hover_text("Pure offline evaluation: article actions are listed, never executed in the game.").clicked() {
                    match super::controller::simulate_frames(fighter, graph, self.preview_frames) {
                        Ok(rows) => { self.preview = rows; self.preview_cursor = 0; }, Err(error) => self.status = error,
                    }
                }
            });
            if !self.preview.is_empty() {
                self.preview_cursor = self.preview_cursor.min(self.preview.len() - 1);
                ui.add(egui::Slider::new(&mut self.preview_cursor, 0..=self.preview.len()-1).text("Frame"));
                let row = &self.preview[self.preview_cursor];
                ui.weak(format!("Offline frame {} · {}", row.frame, row.step.after.state));
                for (name, value) in &row.step.after.values {
                    let variable = graph.vars.iter().find(|var| var.name == *name);
                    live_value_row(ui, name, &serde_json::to_value(value).unwrap_or_default(), variable);
                }
                ui.weak(format!("{} transition(s) · {} action(s)", row.step.fired.len(), row.step.fired.iter().map(|transition| transition.actions.len()).sum::<usize>()));
                for transition in &row.step.fired { for action in &transition.actions { ui.weak(preview_action_label(action)); } }
            }
        });
    }
}

fn is_call_capture(name: &str) -> bool {
    name.strip_prefix("__mv_arg_")
        .or_else(|| name.strip_prefix("__mv_out_"))
        .is_some_and(|suffix| {
            suffix.len() == 16 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn live_variable_metadata<'a>(
    name: &str,
    graph: Option<&'a BehaviorGraph>,
    _recipes: Option<&'a BTreeMap<String, crate::move_nodes::model::MoveGraph>>,
) -> (String, Option<&'a VarDef>) {
    if let Some(variable) = graph.and_then(|graph| graph.vars.iter().find(|var| var.name == name)) {
        return (name.into(), Some(variable));
    }
    (name.into(), None)
}

fn live_values_ui(
    ui: &mut Ui,
    values: &serde_json::Map<String, serde_json::Value>,
    graph: Option<&BehaviorGraph>,
    recipes: Option<&BTreeMap<String, crate::move_nodes::model::MoveGraph>>,
) {
    for (name, value) in values.iter().filter(|(name, _)| !is_call_capture(name)) {
        let (label, variable) = live_variable_metadata(name, graph, recipes);
        live_value_row(ui, &label, value, variable);
    }
    let captures = values.keys().filter(|name| is_call_capture(name)).count();
    if captures > 0 {
        ui.collapsing(format!("Call storage ({captures})"), |ui| {
            ui.weak("Private argument snapshots and results; not editable character state.");
            for (name, value) in values.iter().filter(|(name, _)| is_call_capture(name)) {
                live_value_row(ui, name, value, None);
            }
        });
    }
}

fn live_value_row(ui: &mut Ui, name: &str, value: &serde_json::Value, variable: Option<&VarDef>) {
    ui.horizontal_wrapped(|ui| {
        ui.label(name);
        let timer = variable.filter(|var| var.kind == VarType::Timer);
        if let Some(duration) = timer
            .and_then(|var| {
                if let ConstValue::Int(frames) = var.initial {
                    Some(frames)
                } else {
                    None
                }
            })
            .filter(|frames| *frames > 0)
        {
            if let Some(remaining) = value.get("Int").and_then(|value| value.as_i64()) {
                ui.add(
                    egui::ProgressBar::new((remaining as f32 / duration as f32).clamp(0.0, 1.0))
                        .desired_width(180.0)
                        .text(format!(
                            "{:.2}s · {remaining} game ticks",
                            remaining as f32 / 60.0
                        )),
                );
                return;
            }
        }
        if timer.is_some() {
            if let Some(remaining) = value.get("Int").and_then(|value| value.as_i64()) {
                ui.monospace(format!("{:.2}s · {remaining} game ticks", remaining as f32 / 60.0))
                    .on_hover_text("Game time at 60 ticks per second; pauses and slow training speed change wall-clock time.");
                return;
            }
        }
        ui.monospace(display_live_value(value));
    });
}

fn display_live_value(value: &serde_json::Value) -> String {
    value
        .as_object()
        .and_then(|object| object.values().next())
        .map_or_else(
            || value.to_string(),
            |scalar| {
                if let Some(flag) = scalar.as_bool() {
                    if flag { "on" } else { "off" }.into()
                } else {
                    scalar.to_string()
                }
            },
        )
}

pub(crate) fn article_label(symbol: &str) -> String {
    if symbol == "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE" {
        return "Blade Beam".into();
    }
    match symbol.split("_GENERATE_ARTICLE_").nth(1) {
        Some("FIREBALL") => return "Fireball".into(),
        Some("FUSIONSWORD") => return "Fusion sword (model)".into(),
        Some("WATERPUMP") => return "FLUDD".into(),
        Some("MANTLE") => return "Cape".into(),
        _ => {}
    }
    symbol
        .split("_GENERATE_ARTICLE_")
        .nth(1)
        .unwrap_or(symbol)
        .replace('_', " ")
        .to_lowercase()
}

fn preview_action_label(action: &super::live_model::Action) -> String {
    use super::live_model::Action;
    match action {
        Action::AddHeldOpponentDamage { .. } => "Add held opponent damage %".into(),
        Action::ThrowHeldOpponent { .. } => "Throw held opponent now".into(),
        Action::ReleaseHeldOpponent => "Release held opponent".into(),
        Action::AddDamage { .. } => "Add to damage %".into(),
        Action::CombatPolicy { scope, op } => combat_text(op, *scope, |e| match e {
            super::live_model::Expr::Const(super::live_model::Value::Float(v)) => v.to_string(),
            _ => "…".into(),
        }),
        Action::CancelIncomingHit => "Cancel incoming hit".into(),
        Action::Movement(op) => op.label().into(),
        Action::Item(op) => super::graph_view::item_text(op).0.into(),
        Action::MeshVisibility { visible, .. } => {
            if *visible { "Show mesh" } else { "Hide mesh" }.into()
        }
        Action::Status(c) => c.label().into(),
        Action::SetMotionRate { .. } => "Set animation speed".into(),
        Action::AddSpeed { .. } => "Add speed".into(),
        super::live_model::Action::SetNative { .. }
        | super::live_model::Action::AddNative { .. }
        | super::live_model::Action::SetNativeFlag { .. } => "Game value write".into(),
        Action::Resource(command) => command.label(),
        Action::ChooseHeroMenuSpell { slot } => format!("Choose Hero menu spell · slot {slot}"),
        Action::EmitEvent { name } => format!("Emit {name}"),
        Action::EmitEventArguments { arguments, .. } => {
            format!("Emit values · {} fields", arguments.len())
        }
        Action::EmitEventValues { arguments, .. } => {
            format!("Emit calculated values · {} fields", arguments.len())
        }
        Action::GenerateArticle { symbol } => format!("Spawn {}", article_label(symbol)),
        Action::RequestArticleStatus { symbol, status } => format!(
            "{}: {}",
            article_label(symbol),
            article_status_label(status)
        ),
        Action::RemoveNewestArticle { symbol } => {
            format!("Remove newest {}", article_label(symbol))
        }
        Action::ArticleMovement { symbol, op } => {
            format!("{} · {}", article_label(symbol), op.label())
        }
        Action::Object { symbol, op } => format!("{} · {}", article_label(symbol), op.label()),
        Action::RemoveArticle { symbol } => format!("Remove {}", article_label(symbol)),
        Action::Raise { var } => format!("Enable {var}"),
        Action::Clear { var } => format!("Disable {var}"),
        Action::Reset { var } => format!("Reset {var}"),
        Action::Set { var, .. } => format!("Set {var}"),
        Action::SetFlag { var, .. } => format!("Set flag {var}"),
        Action::Add { var, .. } => format!("Add to {var}"),
        Action::RequestFighterStatus { symbol } => format!(
            "Change status: {}",
            super::field_canvas::status_name(symbol)
        ),
        Action::RequestWeaponStatus { symbol, .. } => format!(
            "Request {}",
            symbol
                .split("_STATUS_KIND_")
                .last()
                .unwrap_or(symbol)
                .to_lowercase()
        ),
    }
}

#[derive(Clone, Copy)]
enum ActionRowEdit {
    Delete,
    Duplicate,
    Move(usize),
}

fn action_row_buttons(ui: &mut Ui, index: usize, count: usize) -> Option<ActionRowEdit> {
    let mut edit = None;
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(index > 0, egui::Button::new("Up").small())
            .on_hover_text("Move earlier")
            .clicked()
        {
            edit = Some(ActionRowEdit::Move(index - 1));
        }
        if ui
            .add_enabled(index + 1 < count, egui::Button::new("Down").small())
            .on_hover_text("Move later")
            .clicked()
        {
            edit = Some(ActionRowEdit::Move(index + 1));
        }
        if ui.small_button("Duplicate").clicked() {
            edit = Some(ActionRowEdit::Duplicate);
        }
        if ui.small_button("Remove action").clicked() {
            edit = Some(ActionRowEdit::Delete);
        }
    });
    edit
}

fn action_kind(action: &ControllerAction) -> ControllerActionKind {
    match action {
        ControllerAction::Status(c) => match c {
            super::status_commands::Command::Reflector { .. } => ControllerActionKind::Reflector,
            super::status_commands::Command::Absorber { .. } => ControllerActionKind::Absorber,
            super::status_commands::Command::GrabBox { .. } => ControllerActionKind::GrabBox,
            super::status_commands::Command::SetAttackDamage { .. } => {
                ControllerActionKind::SetAttackDamage
            }
            super::status_commands::Command::RequestStatus { .. } => {
                ControllerActionKind::RequestStatus
            }
            super::status_commands::Command::TransitionTerm { .. } => {
                ControllerActionKind::TransitionTerm
            }
            super::status_commands::Command::TransitionGroup { .. } => {
                ControllerActionKind::TransitionGroup
            }
            super::status_commands::Command::EnableCancel => ControllerActionKind::EnableCancel,
            super::status_commands::Command::TryCommonTransition { .. } => {
                ControllerActionKind::TryCommonTransition
            }
            super::status_commands::Command::ChangeKinetic { .. } => {
                ControllerActionKind::ChangeKinetic
            }
            super::status_commands::Command::ConfigureEnergy { .. } => {
                ControllerActionKind::ConfigureEnergy
            }
            super::status_commands::Command::SetSituation { .. } => {
                ControllerActionKind::SetSituation
            }
            super::status_commands::Command::SetGroundPolicy { .. } => {
                ControllerActionKind::SetGroundPolicy
            }
            super::status_commands::Command::PlayMotion { .. } => ControllerActionKind::PlayMotion,
            super::status_commands::Command::InitializeStatus(_) => {
                ControllerActionKind::InitializeStatus
            }
            super::status_commands::Command::ChangeStatusNow { .. } => {
                ControllerActionKind::ChangeStatusNow
            }
        },
        ControllerAction::Set { .. } => ControllerActionKind::Set,
        ControllerAction::Add { .. } => ControllerActionKind::Add,
        ControllerAction::Raise { .. } => ControllerActionKind::Raise,
        ControllerAction::Clear { .. } => ControllerActionKind::Clear,
        ControllerAction::RequestFighterStatus { .. } => ControllerActionKind::RequestFighterStatus,
        ControllerAction::GenerateArticle { .. } => ControllerActionKind::GenerateArticle,
        ControllerAction::RemoveArticle { .. } => ControllerActionKind::RemoveArticle,
        ControllerAction::RemoveNewestArticle { .. } => ControllerActionKind::RemoveNewestArticle,
        ControllerAction::ArticleMovement { .. } => ControllerActionKind::MoveArticle,
        ControllerAction::Object {
            op: ObjectOp::SetSpeed { .. },
            ..
        } => ControllerActionKind::SetObjectSpeed,
        ControllerAction::Object {
            op: ObjectOp::Remove,
            ..
        } => ControllerActionKind::RemoveObject,
        ControllerAction::RequestArticleStatus { .. } => ControllerActionKind::RequestArticleStatus,
        ControllerAction::Resource(_) => ControllerActionKind::Resource,
        ControllerAction::ChooseHeroMenuSpell { .. } => ControllerActionKind::ChooseHeroMenuSpell,
        ControllerAction::AddHeldOpponentDamage { .. } => {
            ControllerActionKind::AddHeldOpponentDamage
        }
        ControllerAction::ThrowHeldOpponent { .. } => ControllerActionKind::ThrowHeldOpponent,
        ControllerAction::ReleaseHeldOpponent => ControllerActionKind::ReleaseHeldOpponent,
        ControllerAction::AddDamage { .. } => ControllerActionKind::AddDamage,
        ControllerAction::CombatPolicy { .. } => ControllerActionKind::CombatPolicy,
        ControllerAction::CancelIncomingHit => ControllerActionKind::CancelIncomingHit,
        ControllerAction::SetMotionRate { .. } => ControllerActionKind::SetMotionRate,
        ControllerAction::AddSpeed { .. } => ControllerActionKind::AddSpeed,
        ControllerAction::SetNative { input, .. } if input.projectile_article().is_some() => {
            ControllerActionKind::SetProjectileValue
        }
        ControllerAction::SetNativeFlag { input, .. } if input.projectile_article().is_some() => {
            ControllerActionKind::SetProjectileValue
        }
        ControllerAction::SetNative { .. } => ControllerActionKind::SetNative,
        ControllerAction::AddNative { .. } => ControllerActionKind::AddNative,
        ControllerAction::SetNativeFlag { .. } => ControllerActionKind::SetNativeFlag,
        ControllerAction::Movement(op) => match op {
            MovementAction::SetSpeed { .. } => ControllerActionKind::SetSpeed,
            MovementAction::SetGravity { .. } => ControllerActionKind::SetGravity,
            MovementAction::MoveBy { .. } => ControllerActionKind::MoveBy,
            MovementAction::SetFacing(_) => ControllerActionKind::SetFacing,
        },
        ControllerAction::MeshVisibility { .. } => ControllerActionKind::SetMeshVisibility,
        ControllerAction::Item(op) => match op {
            ItemAction::Hold { .. } => ControllerActionKind::HoldItem,
            ItemAction::Throw { .. } => ControllerActionKind::ThrowItem,
            ItemAction::Drop => ControllerActionKind::DropItem,
            ItemAction::Remove => ControllerActionKind::RemoveItem,
        },
    }
}

/// Game values this fighter's controllers may write, of one kind.
fn native_var_type(input: &NativeInput) -> VarType {
    if input.kind() == Some(NativeKind::Int) {
        VarType::Int
    } else {
        VarType::Float
    }
}

fn writable_inputs(fighter: &str, kind: NativeKind) -> Vec<NativeInput> {
    NativeInput::catalog_for(fighter, kind)
        .into_iter()
        .filter(|input| input.validate_write(fighter).is_ok())
        .collect()
}

fn native_kind_for(kind: ControllerActionKind) -> NativeKind {
    if kind == ControllerActionKind::SetNativeFlag {
        NativeKind::Flag
    } else {
        NativeKind::Float
    }
}

/// Numeric game values: floats first, then integers.
fn writable_numbers(fighter: &str) -> Vec<NativeInput> {
    let mut inputs = writable_inputs(fighter, NativeKind::Float);
    inputs.extend(writable_inputs(fighter, NativeKind::Int));
    inputs
}

fn action_variables(
    graph: &BehaviorGraph,
    kind: ControllerActionKind,
    timer: bool,
) -> impl Iterator<Item = &super::behavior::VarDef> {
    graph.vars.iter().filter(move |var| {
        !(timer && ["remaining", "ready", "elapsed_events"].contains(&var.name.as_str()))
            && match kind {
                ControllerActionKind::Raise | ControllerActionKind::Clear => {
                    var.kind == VarType::Flag
                }
                ControllerActionKind::Add | ControllerActionKind::Set => {
                    matches!(var.kind, VarType::Float | VarType::Int)
                }
                _ => var.kind != VarType::Timer,
            }
    })
}

pub(super) fn default_action(
    fighter: &str,
    graph: &BehaviorGraph,
    kind: ControllerActionKind,
    timer: bool,
) -> ControllerAction {
    let symbol = || {
        let preferred = match fighter {
            "cloud" => "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
            "mario" => "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL",
            _ => "",
        };
        if super::registry::resolve(fighter, preferred, super::registry::IdentifierKind::Article)
            .is_ok()
        {
            return preferred.into();
        }
        super::registry::entries_for(fighter, super::registry::IdentifierKind::Article)
            .next()
            .map_or_else(String::new, |entry| entry.symbol.into())
    };
    let var = action_variables(graph, kind, timer).next();
    let variable = var.map_or_else(String::new, |var| var.name.clone());
    let value = super::value_edit::zero(var.map_or(VarType::Int, |var| var.kind));
    let float = |value: f32| Expr::Const(ConstValue::Float(value));
    match kind {
        ControllerActionKind::ChangeKinetic => {
            ControllerAction::Status(super::status_commands::Command::ChangeKinetic {
                kind: super::status_commands::Kinetic::Fall,
            })
        }
        ControllerActionKind::ConfigureEnergy => {
            ControllerAction::Status(super::status_commands::Command::ConfigureEnergy {
                energy: super::status_commands::Energy::Gravity,
                op: super::status_commands::EnergyOp::Enable,
            })
        }
        ControllerActionKind::SetSituation => {
            ControllerAction::Status(super::status_commands::Command::SetSituation {
                situation: super::status_commands::Situation::Air,
            })
        }
        ControllerActionKind::SetGroundPolicy => {
            ControllerAction::Status(super::status_commands::Command::SetGroundPolicy {
                correction: super::status_commands::Correction::Air,
                cliff: super::status_commands::Cliff::None,
            })
        }
        ControllerActionKind::GrabBox => {
            ControllerAction::Status(super::status_commands::Command::GrabBox {
                enabled: true,
                size: 6.0,
                height: 8.0,
                forward: 10.0,
            })
        }
        ControllerActionKind::Reflector => {
            ControllerAction::Status(super::status_commands::Command::Reflector {
                enabled: true,
                size: 6.0,
                damage: 1.5,
                speed: 1.5,
            })
        }
        ControllerActionKind::Absorber => {
            ControllerAction::Status(super::status_commands::Command::Absorber {
                enabled: true,
                size: 8.0,
            })
        }
        ControllerActionKind::SetAttackDamage => {
            ControllerAction::Status(super::status_commands::Command::SetAttackDamage {
                id: 0,
                damage: float(8.0),
            })
        }
        ControllerActionKind::PlayMotion => {
            ControllerAction::Status(super::status_commands::Command::PlayMotion {
                motion: "special_s".into(),
                frame: super::status_commands::MotionFrame::Start(float(0.0)),
                rate: float(1.0),
            })
        }
        ControllerActionKind::InitializeStatus => ControllerAction::Status(
            super::status_commands::Command::InitializeStatus(Default::default()),
        ),
        ControllerActionKind::RequestStatus => {
            ControllerAction::Status(super::status_commands::Command::RequestStatus {
                target: "FIGHTER_STATUS_KIND_WAIT".into(),
            })
        }
        ControllerActionKind::TransitionTerm => {
            ControllerAction::Status(super::status_commands::Command::TransitionTerm {
                symbol: "FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_SQUAT".into(),
                enabled: true,
            })
        }
        ControllerActionKind::TransitionGroup => {
            ControllerAction::Status(super::status_commands::Command::TransitionGroup {
                symbol: "FIGHTER_STATUS_TRANSITION_GROUP_CHK_GROUND".into(),
                enabled: true,
            })
        }
        ControllerActionKind::EnableCancel => {
            ControllerAction::Status(super::status_commands::Command::EnableCancel)
        }
        ControllerActionKind::TryCommonTransition => {
            ControllerAction::Status(super::status_commands::Command::TryCommonTransition {
                kind: super::status_commands::CommonTransition::GroundWait,
            })
        }
        ControllerActionKind::ChangeStatusNow => {
            ControllerAction::Status(super::status_commands::Command::ChangeStatusNow {
                symbol: "FIGHTER_STATUS_KIND_WAIT".into(),
            })
        }
        ControllerActionKind::RequestFighterStatus => ControllerAction::RequestFighterStatus {
            symbol: "FIGHTER_STATUS_KIND_SPECIAL_HI".into(),
        },
        ControllerActionKind::GenerateArticle => {
            ControllerAction::GenerateArticle { symbol: symbol() }
        }
        ControllerActionKind::RemoveArticle => ControllerAction::RemoveArticle { symbol: symbol() },
        ControllerActionKind::RemoveNewestArticle => {
            ControllerAction::RemoveNewestArticle { symbol: symbol() }
        }
        ControllerActionKind::MoveArticle => ControllerAction::ArticleMovement {
            symbol: symbol(),
            op: MovementAction::MoveBy { dx: 0.0, dy: 1.0 },
        },
        ControllerActionKind::SetObjectSpeed => ControllerAction::Object {
            symbol: symbol(),
            op: ObjectOp::SetSpeed {
                x: float(1.0),
                y: float(0.0),
            },
        },
        ControllerActionKind::RemoveObject => ControllerAction::Object {
            symbol: symbol(),
            op: ObjectOp::Remove,
        },
        ControllerActionKind::RequestArticleStatus => {
            let symbol = symbol();
            let status = super::registry::article_statuses(fighter, &symbol)
                .next()
                .map_or_else(String::new, |entry| entry.symbol.into());
            ControllerAction::RequestArticleStatus { symbol, status }
        }
        ControllerActionKind::Set => ControllerAction::Set { variable, value },
        ControllerActionKind::Add => ControllerAction::Add { variable, value },
        ControllerActionKind::Raise => ControllerAction::Raise { variable },
        ControllerActionKind::Clear => ControllerAction::Clear { variable },
        ControllerActionKind::Resource => {
            use super::resource_commands::{Command, Op, Resource};
            let resource = Resource::for_fighter(fighter)
                .next()
                .unwrap_or_else(|| Resource::named("cloudLimit").unwrap());
            ControllerAction::Resource(Command {
                op: Op::Add,
                resource,
                amount: Expr::Const(ConstValue::Float(1.0)),
            })
        }
        ControllerActionKind::ChooseHeroMenuSpell => {
            ControllerAction::ChooseHeroMenuSpell { slot: 0 }
        }
        ControllerActionKind::AddHeldOpponentDamage => {
            ControllerAction::AddHeldOpponentDamage { amount: float(1.0) }
        }
        ControllerActionKind::ThrowHeldOpponent => ControllerAction::ThrowHeldOpponent {
            direction: super::native_inputs::ThrowDirection::Forward,
        },
        ControllerActionKind::ReleaseHeldOpponent => ControllerAction::ReleaseHeldOpponent,
        ControllerActionKind::AddDamage => ControllerAction::AddDamage {
            amount: float(-10.0),
        },
        ControllerActionKind::CombatPolicy => ControllerAction::CombatPolicy {
            scope: PolicyScope::Status,
            op: CombatOp::SuperArmor,
        },
        ControllerActionKind::CancelIncomingHit => ControllerAction::CancelIncomingHit,
        ControllerActionKind::SetMotionRate => ControllerAction::SetMotionRate { rate: float(1.0) },
        ControllerActionKind::AddSpeed => ControllerAction::AddSpeed {
            x: float(1.0),
            y: float(0.0),
        },
        ControllerActionKind::SetProjectileValue => {
            let input = projectile_inputs(fighter)
                .into_iter()
                .next()
                .unwrap_or(NativeInput::RobotBeamEnergy);
            if input.kind() == Some(NativeKind::Flag) {
                ControllerAction::SetNativeFlag { input, on: true }
            } else {
                ControllerAction::SetNative {
                    value: super::value_edit::zero(native_var_type(&input)),
                    input,
                }
            }
        }
        ControllerActionKind::SetNative | ControllerActionKind::AddNative => {
            let input = writable_numbers(fighter)
                .into_iter()
                .next()
                .unwrap_or(NativeInput::RobotBeamEnergy);
            let value = super::value_edit::zero(native_var_type(&input));
            if kind == ControllerActionKind::SetNative {
                ControllerAction::SetNative { input, value }
            } else {
                ControllerAction::AddNative { input, value }
            }
        }
        ControllerActionKind::SetNativeFlag => ControllerAction::SetNativeFlag {
            input: writable_inputs(fighter, NativeKind::Flag)
                .into_iter()
                .next()
                .unwrap_or(NativeInput::RefletThunderSword),
            on: true,
        },
        ControllerActionKind::SetSpeed => ControllerAction::Movement(MovementAction::SetSpeed {
            x: None,
            y: Some(0.0),
        }),
        ControllerActionKind::SetGravity => {
            ControllerAction::Movement(MovementAction::SetGravity { enabled: false })
        }
        ControllerActionKind::MoveBy => {
            ControllerAction::Movement(MovementAction::MoveBy { dx: 0.0, dy: 0.0 })
        }
        ControllerActionKind::SetFacing => {
            ControllerAction::Movement(MovementAction::SetFacing(Facing::Turn))
        }
        ControllerActionKind::HoldItem => ControllerAction::Item(ItemAction::Hold {
            item: "ITEM_KIND_BEAMSWORD".into(),
        }),
        ControllerActionKind::ThrowItem => ControllerAction::Item(ItemAction::Throw {
            angle: 45.0,
            power: 2.0,
        }),
        ControllerActionKind::DropItem => ControllerAction::Item(ItemAction::Drop),
        ControllerActionKind::RemoveItem => ControllerAction::Item(ItemAction::Remove),
        ControllerActionKind::SetMeshVisibility => ControllerAction::MeshVisibility {
            mesh: String::new(),
            visible: false,
        },
    }
}

pub(super) fn action_available(
    fighter: &str,
    graph: &BehaviorGraph,
    action: &ControllerAction,
    timer: bool,
) -> bool {
    match action {
        ControllerAction::Status(_) => true,
        ControllerAction::Resource(command) => command.check_owner(fighter).is_ok(),
        ControllerAction::ChooseHeroMenuSpell { slot } => {
            (super::hero_menu_commands::Command::Choose { slot: *slot })
                .validate(fighter)
                .is_ok()
        }
        ControllerAction::SetMotionRate {
            rate: Expr::Const(ConstValue::Float(rate)),
        } => (0.05..=20.0).contains(rate),
        ControllerAction::AddHeldOpponentDamage { .. }
        | ControllerAction::ThrowHeldOpponent { .. }
        | ControllerAction::ReleaseHeldOpponent
        | ControllerAction::AddDamage { .. }
        | ControllerAction::CancelIncomingHit => true,
        ControllerAction::CombatPolicy { scope, .. } => {
            super::native_inputs::validate_policy_scope(*scope).is_ok()
        }
        ControllerAction::Movement(op) => op.validate().is_ok(),
        ControllerAction::Item(op) => op.validate().is_ok(),
        ControllerAction::MeshVisibility { mesh, .. } => {
            super::native_inputs::validate_mesh_name(mesh).is_ok()
        }
        ControllerAction::SetMotionRate { .. } | ControllerAction::AddSpeed { .. } => true,
        ControllerAction::SetNative { input, .. }
        | ControllerAction::AddNative { input, .. }
        | ControllerAction::SetNativeFlag { input, .. } => input.validate_write(fighter).is_ok(),
        ControllerAction::RequestFighterStatus { symbol } => super::registry::resolve(
            fighter,
            symbol,
            super::registry::IdentifierKind::FighterStatus,
        )
        .is_ok(),
        ControllerAction::GenerateArticle { symbol }
        | ControllerAction::RemoveArticle { symbol }
        | ControllerAction::RemoveNewestArticle { symbol } => {
            super::registry::resolve(fighter, symbol, super::registry::IdentifierKind::Article)
                .is_ok()
        }
        ControllerAction::ArticleMovement { symbol, op } => {
            op.validate_for_article().is_ok()
                && super::registry::resolve(
                    fighter,
                    symbol,
                    super::registry::IdentifierKind::Article,
                )
                .is_ok()
        }
        ControllerAction::Object { symbol, .. } => {
            super::registry::resolve(fighter, symbol, super::registry::IdentifierKind::Article)
                .is_ok()
        }
        ControllerAction::RequestArticleStatus { symbol, status } => {
            super::registry::article_statuses(fighter, symbol).any(|entry| entry.symbol == status)
        }
        ControllerAction::Set { variable, .. }
        | ControllerAction::Add { variable, .. }
        | ControllerAction::Raise { variable }
        | ControllerAction::Clear { variable } => {
            action_variables(graph, action_kind(action), timer).any(|var| var.name == *variable)
        }
    }
}

pub(super) fn action_picker(
    ui: &mut Ui,
    fighter: &str,
    graph: &BehaviorGraph,
    action: &mut ControllerAction,
    timer: bool,
    scope: super::value_edit::ValueScope,
) -> bool {
    use super::value_edit::value_editor;
    let before = action.clone();
    let mut kind = action_kind(action);
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("action_type")
            .selected_text(action_label(kind))
            .height(520.0)
            .show_ui(ui, |ui| {
                let mut group = "";
                // Keep each heading's choices together even where ALL interleaves them.
                let mut choices = ControllerActionKind::ALL;
                let first = |g: &str| ControllerActionKind::ALL.iter().position(|c| c.group() == g);
                choices.sort_by_key(|choice| first(choice.group()));
                for choice in choices {
                    if choice == ControllerActionKind::Resource && super::resource_commands::Resource::for_fighter(fighter).next().is_none() { continue; }
                    if choice == ControllerActionKind::ChooseHeroMenuSpell && fighter != "brave" { continue; }
                    let no_targets = match choice {
                        ControllerActionKind::SetProjectileValue => projectile_inputs(fighter).is_empty(),
                        ControllerActionKind::SetNative | ControllerActionKind::AddNative => writable_numbers(fighter).is_empty(),
                        // The held opponent is sampled by the frame tick and by status phases.
                        ControllerActionKind::AddHeldOpponentDamage | ControllerActionKind::ThrowHeldOpponent | ControllerActionKind::ReleaseHeldOpponent => !scope.every_frame && !scope.status_phase,
                        // Status commands only mean something inside an owned status's phases.
                        _ if choice.group() == "Status phases" => !scope.status_phase && choice != action_kind(&before),
                        ControllerActionKind::SetNativeFlag => writable_inputs(fighter, NativeKind::Flag).is_empty(),
                        _ => false,
                    };
                    if no_targets { continue; }
                    if choice.group() != group {
                        group = choice.group();
                        ui.label(egui::RichText::new(group).small().weak());
                    }
                    ui.selectable_value(&mut kind, choice, action_label(choice));
                }
            });
        if kind != action_kind(action) {
            *action = default_action(fighter, graph, kind, timer);
        }
        match action {
            ControllerAction::Resource(command) => {
                use super::resource_commands::{Op, Resource};
                egui::ComboBox::from_id_salt("native_resource").selected_text(command.resource.label()).show_ui(ui, |ui| {
                    for resource in Resource::for_fighter(fighter) {
                        ui.selectable_value(&mut command.resource, resource, resource.label());
                    }
                });
                egui::ComboBox::from_id_salt("native_resource_op").selected_text(format!("{:?}", command.op)).show_ui(ui, |ui| {
                    for op in [Op::Add, Op::Set] {
                        ui.selectable_value(&mut command.op, op, format!("{op:?}"));
                    }
                });
                ui.end_row();
                ui.label("Amount").on_hover_text(format!("Native units, 0 to {}. Helper-backed rows (Limit, MP, ink, rebel gauge, Steve's materials) need a stable status; Limit also takes jab.", command.resource.entry().max));
                value_editor(ui, fighter, graph, &mut command.amount, VarType::Float, scope);
            }
            ControllerAction::ChooseHeroMenuSpell { slot } => {
                ui.add(egui::DragValue::new(slot).speed(1.0).range(0..=(super::hero_menu_commands::Command::SLOTS - 1)))
                    .on_hover_text("Menu slot 0..=3 while Hero's Command Selection menu is open. Requires fighter brave.");
            }

            ControllerAction::Status(c) => {
                use super::status_commands::{Command,MotionFrame,Energy,EnergyOp,Reset,Kinetic,Situation,Correction,Cliff};
                match c {
                    Command::ChangeKinetic {kind}=>status_choice(ui,"kinetic",kind,&[Kinetic::Unique,Kinetic::GroundStop,Kinetic::Fall]),
                    Command::SetSituation {situation}=>status_choice(ui,"situation",situation,&[Situation::Keep,Situation::Ground,Situation::Air]),
                    Command::SetGroundPolicy {correction,cliff}=>{
                        status_choice(ui,"correction",correction,&[Correction::Keep,Correction::Ground,Correction::Air]);
                        ui.end_row();ui.label("Cliff");status_choice(ui,"cliff",cliff,&[Cliff::None,Cliff::Always,Cliff::AlwaysBothSides]);
                    }
                    Command::ConfigureEnergy {energy,op}=>{
                        status_choice(ui,"energy",energy,&[Energy::Stop,Energy::Gravity,Energy::Control,Energy::Motion]);
                        let zero=||Expr::Const(ConstValue::Float(0.0));
                        let reset=match energy {Energy::Stop=>Reset::StopAir,Energy::Gravity=>Reset::Gravity,Energy::Control=>Reset::ControlFallAdjust,Energy::Motion=>Reset::MotionAir};
                        let choices=[EnergyOp::Enable,EnergyOp::Disable,EnergyOp::Reset {kind:reset,x:zero(),y:zero()},EnergyOp::Speed {x:zero(),y:zero()},EnergyOp::Accel {x:zero(),y:zero()},EnergyOp::Brake {x:zero(),y:zero()},EnergyOp::Limit {x:zero(),y:zero()},EnergyOp::Stable {x:zero(),y:zero()}];
                        let label=|op:&EnergyOp<Expr>|match op {EnergyOp::Enable=>"Enable",EnergyOp::Disable=>"Disable",EnergyOp::Reset {..}=>"Reset",EnergyOp::Speed {..}=>"Speed",EnergyOp::Accel {..}=>"Acceleration",EnergyOp::Brake {..}=>"Brake",EnergyOp::Limit {..}=>"Limit",EnergyOp::Stable {..}=>"Stable speed"};
                        if op.validate(*energy).is_err() {*op=EnergyOp::Enable;}
                        ui.end_row();ui.label("Operation");
                        egui::ComboBox::from_id_salt("energy_op").selected_text(label(op)).show_ui(ui,|ui|{for choice in choices {if choice.validate(*energy).is_ok() && ui.selectable_label(std::mem::discriminant(op)==std::mem::discriminant(&choice),label(&choice)).clicked(){*op=choice;}}});
                        if let EnergyOp::Reset {kind,..}=op {
                            let modes=match energy {Energy::Stop=>vec![Reset::StopGround,Reset::StopAir],Energy::Gravity=>vec![Reset::Gravity],Energy::Control=>vec![Reset::ControlFallAdjust],Energy::Motion=>vec![Reset::MotionGround,Reset::MotionAir]};
                            ui.end_row();ui.label("Reset mode");status_choice(ui,"reset",kind,&modes);
                        }
                        for (axis,expr) in ["X","Y"].into_iter().zip(op.expressions_mut()) {
                            if *energy==Energy::Gravity && axis=="X" {*expr=zero();continue;}
                            ui.end_row();ui.label(axis);ui.push_id(axis,|ui|value_editor(ui,fighter,graph,expr,VarType::Float,scope));
                        }
                    }
                    Command::Reflector {enabled,size,damage,speed} => {
                        ui.checkbox(enabled,"Enabled");
                        for (label,value,range) in [("Size",size,0.1..=50.0),("Damage multiplier",damage,0.1..=10.0),("Speed multiplier",speed,0.1..=10.0)] {
                            ui.end_row();ui.label(label);ui.add(egui::DragValue::new(value).range(range).speed(0.1));
                        }
                    }
                    Command::Absorber {enabled,size} => {
                        ui.checkbox(enabled,"Enabled");
                        ui.end_row();ui.label("Size");ui.add(egui::DragValue::new(size).range(0.1..=50.0).speed(0.1));
                    }
                    Command::GrabBox {enabled,size,height,forward} => {
                        ui.checkbox(enabled,"Enabled").on_hover_text("Only a Catch or Catch turn status accepts the capture, so bind this state to one of those.");
                        ui.end_row();ui.label("Size");ui.add(egui::DragValue::new(size).range(0.1..=50.0).speed(0.1));
                        for (label,value) in [("Height",height),("Forward",forward)] {
                            ui.end_row();ui.label(label);ui.add(egui::DragValue::new(value).range(-100.0..=100.0).speed(0.1));
                        }
                    }
                    Command::SetAttackDamage {id,damage} => {
                        ui.add(egui::DragValue::new(id).range(0..=7));
                        ui.end_row();ui.label("Damage");value_editor(ui,fighter,graph,damage,VarType::Float,scope);
                    }
                    Command::PlayMotion {motion,frame,rate} => {
                        ui.text_edit_singleline(motion);
                        ui.end_row(); ui.label("Speed"); value_editor(ui,fighter,graph,rate,VarType::Float,scope);
                        let mut inherit=matches!(frame,MotionFrame::Inherit);
                        if ui.checkbox(&mut inherit,"Inherit frame").changed() { *frame=if inherit {MotionFrame::Inherit} else {MotionFrame::Start(Expr::Const(ConstValue::Float(0.0)))}; }
                        if let MotionFrame::Start(frame)=frame {ui.end_row();ui.label("Start frame");value_editor(ui,fighter,graph,frame,VarType::Float,scope);}
                    }
                    Command::InitializeStatus(settings) => {ui.checkbox(&mut settings.jostle,"Jostle");ui.checkbox(&mut settings.status_data.grab_invulnerable,"Grab immune").on_hover_text("FighterStatusModuleImpl::set_fighter_status_data grab flag, for this status only");ui.weak("Pre settings");}
                    Command::TransitionTerm {symbol,enabled} => {
                        super::registry::transition_picker(ui,fighter,symbol,false);
                        ui.checkbox(enabled,"Enabled");
                    }
                    Command::TransitionGroup {symbol,enabled} => {
                        super::registry::transition_picker(ui,fighter,symbol,true);
                        ui.checkbox(enabled,"Enabled");
                    }
                    Command::EnableCancel => {ui.label("Enable native cancel checks");}
                    Command::TryCommonTransition {kind} => {
                        use super::status_commands::CommonTransition as C;
                        egui::ComboBox::from_id_salt("common transition").selected_text(format!("{kind:?}")).show_ui(ui,|ui| {
                            for (value,label) in [(C::AirCliff,"Air cliff"),(C::GroundWait,"Ground wait"),(C::AirFall,"Air fall"),(C::AirLanding,"Air landing"),(C::AirSpecial,"Air special")] {ui.selectable_value(kind,value,label);}
                        });
                    }
                    Command::ChangeStatusNow {symbol} | Command::RequestStatus {target:symbol} => {
                        match symbol {super::status_model::StatusRef::Native(native)=>super::registry::fighter_status_picker(ui,fighter,native),super::status_model::StatusRef::Custom {key,..}=>{ui.label(format!("Custom: {key}"));}}
                    },
                }
            }
            ControllerAction::RequestFighterStatus { symbol } => {
                super::registry::fighter_status_picker(ui, fighter, symbol);
            }
            ControllerAction::RequestArticleStatus { symbol, status } => {
                let before = symbol.clone();
                article_picker(ui, fighter, symbol);
                if *symbol != before {
                    *status = super::registry::article_statuses(fighter, symbol)
                        .next()
                        .map_or_else(String::new, |entry| entry.symbol.into());
                }
                egui::ComboBox::from_id_salt("article_status_target")
                    .selected_text(article_status_label(status))
                    .show_ui(ui, |ui| {
                        for entry in super::registry::article_statuses(fighter, symbol) {
                            ui.selectable_value(status, entry.symbol.into(), article_status_label(entry.symbol))
                                .on_hover_text(entry.symbol);
                        }
                    });
            }
            ControllerAction::RemoveNewestArticle { symbol } => article_picker(ui, fighter, symbol),
            ControllerAction::Object { symbol, op } => {
                article_picker(ui, fighter, symbol);
                if let ObjectOp::SetSpeed { x, y } = op {
                    ui.end_row();
                    ui.label("x");
                    ui.push_id("object_speed_x", |ui| value_editor(ui, fighter, graph, x, VarType::Float, scope));
                    ui.end_row();
                    ui.label("y");
                    ui.push_id("object_speed_y", |ui| value_editor(ui, fighter, graph, y, VarType::Float, scope));
                }
                ui.weak("Acts on the newest live one, on its own next frame; run it every frame to steer.");
            }
            ControllerAction::ArticleMovement { symbol, op } => {
                article_picker(ui, fighter, symbol);
                movement_fields(ui, op);
                ui.weak("Every live one moves by this much each time the rule runs; run it every frame to steer.");
            }
            ControllerAction::GenerateArticle { symbol }
            | ControllerAction::RemoveArticle { symbol } => {
                egui::ComboBox::from_id_salt("article_target")
                    .selected_text(if symbol.is_empty() {
                        "No supported articles".into()
                    } else {
                        article_label(symbol)
                    })
                    .show_ui(ui, |ui| {
                        for entry in super::registry::entries_for(
                            fighter,
                            super::registry::IdentifierKind::Article,
                        ) {
                            ui.selectable_value(
                                symbol,
                                entry.symbol.into(),
                                article_label(entry.symbol),
                            )
                            .on_hover_text(entry.symbol);
                        }
                    })
                    .response
                    .on_hover_text(symbol.as_str());
            }
            ControllerAction::SetNative { input, value } | ControllerAction::AddNative { input, value } => {
                let old = native_var_type(input);
                let projectile = input.projectile_article().is_some();
                native_target_picker(ui, input, if projectile { projectile_inputs(fighter) } else { writable_numbers(fighter) });
                if projectile { ui.label("ⓘ").on_hover_text("Writes every owned live projectile. Prefer charge, counters and flags the weapon reads; weapons may overwrite state every frame. Reads sample the newest live projectile each tick; no projectile reads as zero."); }
                if input.kind() == Some(NativeKind::Flag) {
                    *action = ControllerAction::SetNativeFlag { input: input.clone(), on: true };
                    return;
                }
                let kind = native_var_type(input);
                if kind != old {
                    *value = super::value_edit::zero(kind);
                }
                ui.end_row();
                ui.label("to");
                value_editor(ui, fighter, graph, value, kind, scope);
            }
            ControllerAction::AddHeldOpponentDamage { amount } => {
                ui.end_row();
                ui.label("%").on_hover_text("Added to the opponent this fighter is holding; nothing happens when no one is held.");
                value_editor(ui, fighter, graph, amount, VarType::Float, scope);
            }
            ControllerAction::ThrowHeldOpponent { direction } => {
                use super::native_inputs::ThrowDirection;
                ui.label("Direction").on_hover_text("Requires holding on the ground, in standard catch wait with its throw transition enabled or in a status this chart owns.");
                egui::ComboBox::from_id_salt("throw_direction").selected_text(format!("{direction:?}")).show_ui(ui, |ui| {
                    for value in [ThrowDirection::Forward, ThrowDirection::Back, ThrowDirection::Up, ThrowDirection::Down] {
                        ui.selectable_value(direction, value, format!("{value:?}"));
                    }
                });
            }
            ControllerAction::ReleaseHeldOpponent => {
                ui.weak("Lets go of the held opponent");
            }
            ControllerAction::AddDamage { amount } => {
                ui.end_row();
                ui.label("%").on_hover_text("Negative heals; healing stops at 0%.");
                value_editor(ui, fighter, graph, amount, VarType::Float, scope);
            }
            ControllerAction::CombatPolicy { scope: lasts, op } => {
                let float = |value: f32| Expr::Const(ConstValue::Float(value));
                let value = op.value().cloned().unwrap_or_else(|| float(1.0));
                let ops = [CombatOp::DamageTaken(value.clone()), CombatOp::KnockbackTaken(value.clone()), CombatOp::DamageDealt(value.clone()), CombatOp::KnockbackDealt(value.clone()), CombatOp::Stat(MovementStat::RunSpeed, value.clone()), CombatOp::Stat(MovementStat::AirSpeed, value.clone()), CombatOp::Stat(MovementStat::Gravity, value.clone()), CombatOp::Stat(MovementStat::JumpHeight, value.clone()), CombatOp::Armor(float(10.0)), CombatOp::SuperArmor, CombatOp::Intangible, CombatOp::Invincible, CombatOp::Clear(op.channel())];
                egui::ComboBox::from_id_salt("combat_op").selected_text(op.label()).show_ui(ui, |ui| {
                    for choice in ops {
                        let label = choice.label();
                        if ui.selectable_label(std::mem::discriminant(op) == std::mem::discriminant(&choice), label).clicked() {
                            *op = choice;
                        }
                    }
                });
                if let CombatOp::Clear(channel) = op {
                    egui::ComboBox::from_id_salt("combat_channel").selected_text(channel.label()).show_ui(ui, |ui| {
                        for choice in CombatChannel::ALL {
                            ui.selectable_value(channel, choice, choice.label());
                        }
                    });
                } else {
                    let frames = match lasts { PolicyScope::Frames(n) => *n, _ => 60 };
                    egui::ComboBox::from_id_salt("combat_scope").selected_text(lasts.label()).show_ui(ui, |ui| {
                        for choice in [PolicyScope::Status, PolicyScope::Frames(frames), PolicyScope::UntilCleared] {
                            ui.selectable_value(lasts, choice, choice.label());
                        }
                    });
                    if let PolicyScope::Frames(n) = lasts {
                        ui.add(egui::DragValue::new(n).range(1..=PolicyScope::MAX_FRAMES).suffix(" frames"));
                    }
                }
                if let Some(value) = op.value_mut() {
                    ui.end_row();
                    ui.label("value").on_hover_text("Multipliers: 1 is normal (0..=10). Every jump's height is Jump height ÷ Gravity. Armor: hits up to this much damage cause no flinch.");
                    value_editor(ui, fighter, graph, value, VarType::Float, scope);
                }
                ui.weak("A later policy on the same channel replaces this one; the channel returns to normal when the scope ends.");
            }
            ControllerAction::CancelIncomingHit => {}
            ControllerAction::Movement(op) => movement_fields(ui, op),
            ControllerAction::Item(ItemAction::Hold { item }) => {
                super::registry::item_picker(ui, item);
            }
            ControllerAction::Item(ItemAction::Throw { angle, power }) => {
                ui.end_row();
                ui.label("angle").on_hover_text("Degrees: 0 throws forward, 90 straight up, 270 down.");
                ui.add(egui::DragValue::new(angle).speed(1.0).range(0.0..=360.0).suffix("°"));
                ui.label("power");
                ui.add(egui::DragValue::new(power).speed(0.05).range(0.0..=10.0));
            }
            ControllerAction::Item(ItemAction::Drop | ItemAction::Remove) => {}
            ControllerAction::MeshVisibility { mesh, visible } => {
                ui.label("Mesh").on_hover_text("The model's mesh name in lowercase, without _VIS_O_OBJShape: edge_wing");
                ui.text_edit_singleline(mesh);
                ui.checkbox(visible, "Visible");
            }
            ControllerAction::SetMotionRate { rate } => {
                ui.end_row();
                ui.label("×").on_hover_text("Animation speed: below 1 slows the move, above 1 speeds it up.");
                value_editor(ui, fighter, graph, rate, VarType::Float, scope);
            }
            ControllerAction::AddSpeed { x, y } => {
                ui.end_row();
                ui.label("forward");
                ui.push_id("speed_x", |ui| value_editor(ui, fighter, graph, x, VarType::Float, scope));
                ui.end_row();
                ui.label("up");
                ui.push_id("speed_y", |ui| value_editor(ui, fighter, graph, y, VarType::Float, scope));
            }
            ControllerAction::SetNativeFlag { input, on } => {
                if input.projectile_article().is_some() { ui.label("ⓘ").on_hover_text("Writes every owned live projectile. Prefer flags the weapon reads; weapons may overwrite state every frame."); }
                native_target_picker(ui, input, if input.projectile_article().is_some() { projectile_inputs(fighter) } else { writable_inputs(fighter, native_kind_for(kind)) });
                if input.kind() != Some(NativeKind::Flag) {
                    *action = ControllerAction::SetNative { value: super::value_edit::zero(native_var_type(input)), input: input.clone() };
                    return;
                }
                ui.checkbox(on, "On");
            }
            ControllerAction::Set { variable, .. }
            | ControllerAction::Add { variable, .. }
            | ControllerAction::Raise { variable }
            | ControllerAction::Clear { variable } => {
                egui::ComboBox::from_id_salt("variable_target")
                    .selected_text(if variable.is_empty() {
                        "Add a variable first"
                    } else {
                        variable.as_str()
                    })
                    .show_ui(ui, |ui| {
                        for var in action_variables(graph, kind, timer) {
                            ui.selectable_value(variable, var.name.clone(), &var.name);
                        }
                    });
            }
        }
        if let ControllerAction::Set { variable, value }
        | ControllerAction::Add { variable, value } = action
        {
            if let Some(var) =
                action_variables(graph, kind, timer).find(|var| var.name == *variable)
            {
                if action_variable(&before).map(String::as_str) != Some(variable.as_str()) {
                    *value = super::value_edit::zero(var.kind);
                }
                ui.end_row();
                ui.label(if matches!(action_kind(&before), ControllerActionKind::Add) { "by" } else { "to" });
                value_editor(ui, fighter, graph, value, var.kind, scope);
            }
        }
    });
    *action != before
}

pub(super) fn article_picker(ui: &mut Ui, fighter: &str, symbol: &mut String) {
    egui::ComboBox::from_id_salt("article_target")
        .selected_text(if symbol.is_empty() {
            "No supported articles".into()
        } else {
            article_label(symbol)
        })
        .show_ui(ui, |ui| {
            for entry in
                super::registry::entries_for(fighter, super::registry::IdentifierKind::Article)
            {
                ui.selectable_value(symbol, entry.symbol.into(), article_label(entry.symbol))
                    .on_hover_text(entry.symbol);
            }
        });
}

/// `WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR` reads as "Regular".
pub(crate) fn article_status_label(symbol: &str) -> String {
    let name = symbol
        .rsplit_once("_STATUS_KIND_")
        .map_or(symbol, |(_, name)| name);
    let mut label = name.replace('_', " ").to_lowercase();
    if let Some(first) = label.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    label
}

fn native_target_picker(ui: &mut Ui, input: &mut NativeInput, choices: Vec<NativeInput>) {
    egui::ComboBox::from_id_salt("native_target")
        .selected_text(input.label())
        .show_ui(ui, |ui| {
            for choice in choices {
                let label = choice.label().to_owned();
                ui.selectable_value(input, choice, label);
            }
        })
        .response
        .on_hover_text(input.symbol().to_owned());
}

fn action_variable(action: &ControllerAction) -> Option<&String> {
    match action {
        ControllerAction::Set { variable, .. }
        | ControllerAction::Add { variable, .. }
        | ControllerAction::Raise { variable }
        | ControllerAction::Clear { variable } => Some(variable),
        _ => None,
    }
}

fn movement_fields(ui: &mut Ui, op: &mut MovementAction) {
    match op {
        MovementAction::SetSpeed { x, y } => {
            for (label, axis) in [("X", x), ("Y", y)] {
                let mut enabled = axis.is_some();
                if ui.checkbox(&mut enabled, label).changed() {
                    *axis = enabled.then_some(0.0);
                }
                if let Some(value) = axis {
                    ui.add(egui::DragValue::new(value).range(-10.0..=10.0).speed(0.1));
                }
            }
        }
        MovementAction::SetGravity { enabled } => {
            ui.checkbox(enabled, "Gravity enabled (restores when status ends)");
        }
        MovementAction::MoveBy { dx, dy } => {
            ui.label("World X / Y");
            ui.add(egui::DragValue::new(dx).range(-50.0..=50.0));
            ui.add(egui::DragValue::new(dy).range(-50.0..=50.0));
        }
        MovementAction::SetFacing(facing) => {
            for (value, label) in [
                (Facing::Left, "Left"),
                (Facing::Right, "Right"),
                (Facing::Turn, "Turn"),
                (Facing::Opponent, "Opponent"),
            ] {
                ui.selectable_value(facing, value, label);
            }
        }
    }
}

/// "Super armor · this status", "Damage taken × 0.5 · 60 frames", "Clear armor".
pub(crate) fn combat_text<E>(
    op: &CombatOp<E>,
    scope: PolicyScope,
    value: impl Fn(&E) -> String,
) -> String {
    match op {
        CombatOp::Clear(channel) => format!("Clear {}", channel.label()),
        _ => match op.value() {
            Some(v) => format!("{} {} · {}", op.label(), value(v), scope.label()),
            None => format!("{} · {}", op.label(), scope.label()),
        },
    }
}

fn action_label(action: super::controller::ControllerActionKind) -> &'static str {
    use super::controller::ControllerActionKind::*;
    match action {
        Set => "Set variable",
        Add => "Add to variable",
        Raise => "Enable flag",
        Clear => "Disable flag",
        RequestFighterStatus => "Change status",
        GenerateArticle => "Spawn article",
        RemoveArticle => "Remove article",
        RemoveNewestArticle => "Remove newest article",
        MoveArticle => "Move projectile",
        SetObjectSpeed => "Set object speed",
        RemoveObject => "Remove object",
        RequestArticleStatus => "Change article status",
        Resource => "Change resource",
        ChooseHeroMenuSpell => "Choose Hero menu spell",
        AddHeldOpponentDamage => "Add held opponent damage %",
        ThrowHeldOpponent => "Throw held opponent now",
        ReleaseHeldOpponent => "Release held opponent",
        AddDamage => "Add to damage %",
        CombatPolicy => "Combat policy (armor, intangibility, multipliers)",
        ChangeKinetic => "Change kinetics",
        ConfigureEnergy => "Configure energy",
        SetSituation => "Set situation",
        SetGroundPolicy => "Ground / cliff policy",
        Reflector => "Reflect projectiles",
        GrabBox => "Grab box",
        Absorber => "Absorb projectiles",
        SetAttackDamage => "Set hitbox damage",
        PlayMotion => "Play motion",
        InitializeStatus => "Initialize status",
        RequestStatus => "Request status",
        TransitionTerm => "Allow transition",
        TransitionGroup => "Allow transition group",
        EnableCancel => "Enable cancel",
        TryCommonTransition => "Try common transition",
        ChangeStatusNow => "Change status now",
        CancelIncomingHit => "Cancel incoming hit",
        SetMotionRate => "Set animation speed",
        AddSpeed => "Add speed",
        SetProjectileValue => "Set projectile value",
        SetNative => "Set game value",
        AddNative => "Add to game value",
        SetNativeFlag => "Set game flag",
        SetSpeed => "Set speed",
        SetGravity => "Set gravity",
        MoveBy => "Move by",
        SetFacing => "Set facing",
        SetMeshVisibility => "Show or hide mesh",
        HoldItem => "Spawn item into hand",
        ThrowItem => "Throw item",
        DropItem => "Drop item",
        RemoveItem => "Remove item",
    }
}

fn param_control(ui: &mut Ui, value: &mut ParamValue) -> bool {
    match value {
        ParamValue::Bool(value) => ui.checkbox(value, "").changed(),
        ParamValue::Float(value) => ui.add(egui::DragValue::new(value).speed(0.01)).changed(),
        ParamValue::I8(value) => ui.add(egui::DragValue::new(value)).changed(),
        ParamValue::U8(value) => ui.add(egui::DragValue::new(value)).changed(),
        ParamValue::I16(value) => ui.add(egui::DragValue::new(value)).changed(),
        ParamValue::U16(value) => ui.add(egui::DragValue::new(value)).changed(),
        ParamValue::I32(value) => ui.add(egui::DragValue::new(value)).changed(),
        ParamValue::U32(value) => ui.add(egui::DragValue::new(value)).changed(),
        ParamValue::Hash(_) => false,
    }
}

fn read_template(path: &Path) -> Result<BehaviorTemplate, String> {
    BehaviorTemplate::decode(&read_text(path)?)
}

fn read_text(path: &Path) -> Result<String, String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    Ok(text)
}

fn write_template(path: &Path, template: &BehaviorTemplate) -> Result<(), String> {
    let text = template.encode()?;
    write_text(path, &text)
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    file.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

fn apply_setup(
    fighter: &str,
    prepared: &super::blueprint::PreparedGimmicks,
    params: &mut BTreeMap<String, ParamMod>,
    behaviors: &mut BTreeMap<String, BehaviorGraph>,
    meters: &mut BTreeMap<String, Vec<super::meter::MeterBinding>>,
) -> Result<(), String> {
    let key = super::params::personal_path_for(fighter).map_err(|error| error.to_string())?;
    if prepared.personal.files.keys().any(|path| path != &key) {
        return Err(
            "Setup contains parameter files outside the selected donor's personal values.".into(),
        );
    }
    if super::registry::fighter_kind_symbol(fighter).is_none() {
        return Err(format!("Unknown engine fighter {fighter:?}."));
    }
    if !prepared.behavior.is_empty() {
        BehaviorTemplate::from_graph("Setup", Some(fighter), prepared.behavior.clone())?
            .instantiate(fighter)?;
    }
    for binding in &prepared.meters {
        if binding.fighter != fighter || !super::meter::validate(binding).is_empty() {
            return Err(
                "Setup contains a meter that does not belong to the selected donor.".into(),
            );
        }
    }
    if let Some(target) = params.get_mut(fighter) {
        target.files.remove(&key);
    }
    if let Some(fields) = prepared
        .personal
        .files
        .get(&key)
        .filter(|fields| !fields.is_empty())
    {
        params
            .entry(fighter.to_string())
            .or_default()
            .files
            .insert(key, fields.clone());
    }
    if prepared.behavior.is_empty() {
        behaviors.remove(fighter);
    } else {
        behaviors.insert(fighter.to_string(), prepared.behavior.clone());
    }
    if prepared.meters.is_empty() {
        meters.remove(fighter);
    } else {
        meters.insert(fighter.to_string(), prepared.meters.clone());
    }
    Ok(())
}

fn status_choice<T: std::fmt::Debug + Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut T,
    choices: &[T],
) {
    let label = |v: &T| {
        let name = format!("{v:?}");
        name.chars()
            .enumerate()
            .flat_map(|(i, c)| {
                if i > 0 && c.is_uppercase() {
                    vec![' ', c.to_ascii_lowercase()]
                } else {
                    vec![c]
                }
            })
            .collect::<String>()
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(label(value))
        .show_ui(ui, |ui| {
            for choice in choices {
                ui.selectable_value(value, *choice, label(choice));
            }
        });
}

fn projectile_inputs(fighter: &str) -> Vec<NativeInput> {
    [NativeKind::Float, NativeKind::Int, NativeKind::Flag]
        .into_iter()
        .flat_map(|kind| NativeInput::catalog_for(fighter, kind))
        .filter(|input| input.projectile_article().is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projectile_picker_defaults_are_typed_and_keep_the_projectile_action() {
        for fighter in ["mario", "samus", "cloud"] {
            let action = default_action(
                fighter,
                &BehaviorGraph::default(),
                ControllerActionKind::SetProjectileValue,
                false,
            );
            assert_eq!(
                action_kind(&action),
                ControllerActionKind::SetProjectileValue
            );
            assert!(action_available(
                fighter,
                &BehaviorGraph::default(),
                &action,
                false
            ));
            match action {
                ControllerAction::SetNative { input, value } => {
                    assert!(input.projectile_article().is_some());
                    assert_eq!(
                        value,
                        super::super::value_edit::zero(native_var_type(&input))
                    );
                }
                ControllerAction::SetNativeFlag { input, .. } => {
                    assert_eq!(input.kind(), Some(NativeKind::Flag))
                }
                _ => panic!("unexpected projectile action"),
            }
        }
    }

    #[test]
    fn call_capture_detection_does_not_hide_authored_or_move_private_names() {
        assert!(is_call_capture("__mv_arg_0123456789abcdef"));
        assert!(is_call_capture("__mv_out_0123456789abcdef"));
        for name in [
            "count",
            "arg_result",
            "__mv_13579bdf_delay",
            "__mv_arg_short",
            "__mv_out_short",
            "__mv_arg_0123456789abcdeg",
            "__ri_0123456789abcdef",
        ] {
            assert!(!is_call_capture(name), "{name}");
        }
    }

    #[test]
    fn live_state_defaults_to_authored_values_and_zero_initial_timers_keep_units() {
        fn text(shape: &egui::epaint::Shape, output: &mut String) {
            match shape {
                egui::epaint::Shape::Text(shape) => {
                    output.push_str(shape.galley.text());
                    output.push('\n');
                }
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text(shape, output);
                    }
                }
                _ => {}
            }
        }
        let graph = BehaviorGraph {
            vars: vec![VarDef {
                keep_across_form_switch: false,
                name: "delay".into(),
                kind: VarType::Timer,
                initial: ConstValue::Int(0),
            }],
            ..Default::default()
        };
        let values = serde_json::json!({
            "delay":{"Int":90}, "reported":{"Int":7},
            "__mv_arg_0123456789abcdef":{"Int":99},
            "__mv_arg_fedcba9876543210":{"Flag":true}
        })
        .as_object()
        .unwrap()
        .clone();
        let context = egui::Context::default();
        let output = context.run_ui(Default::default(), |ui| {
            live_values_ui(ui, &values, Some(&graph), None);
        });
        let mut displayed = String::new();
        for shape in &output.shapes {
            text(&shape.shape, &mut displayed);
        }
        assert!(displayed.contains("90 game ticks"), "{displayed}");
        assert!(displayed.contains("1.50s"), "{displayed}");
        assert!(displayed.contains("reported"), "{displayed}");
        assert!(displayed.contains("Call storage (2)"), "{displayed}");
        assert!(
            !displayed.contains("__mv_arg_"),
            "private IDs should be collapsed: {displayed}"
        );
        assert!(
            !displayed.contains("99"),
            "private values should be collapsed: {displayed}"
        );
    }
}
