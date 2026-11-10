//! Compact authoring controls. Drafts do not reach persistence or live preview until applied.
use std::collections::{BTreeMap, VecDeque};

use egui::Ui;

#[path = "workspace.rs"]
pub(super) mod workspace;

use super::model::*;
use crate::data::{AcmdScript, ExcuteStmt};
use crate::gimmicks::behavior::{
    BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, VarDef, VarType,
};
use crate::gimmicks::hooks::HookName;

#[derive(Clone, serde::Serialize)]
struct EditorSnapshot {
    graph: MoveGraph,
    #[serde(skip_serializing_if = "Option::is_none")]
    character: Option<workspace::CharacterDraft>,
}

#[derive(Clone, Copy, Default)]
struct CommandOptions<'a> {
    track: Track,
    joints: &'a [String],
    chart_events: &'a [(String, Vec<super::chart_event_view::Receiver>)],
    character_values: &'a [(String, Vec<crate::gimmicks::behavior::VarDef>)],
}

#[derive(Default)]
pub struct NodeEditor {
    workspace: bool,
    character: Option<workspace::CharacterDraft>,
    saved_character: Option<workspace::CharacterDraft>,
    character_inspector: crate::gimmicks::graph_view::GraphView,
    workspace_track: usize,
    key: String,
    draft: MoveGraph,
    track: usize,
    add_kind: usize,
    palette_query: String,
    joint_options: Vec<String>,
    controller_charts: BTreeMap<String, crate::mod_project::ControllerChart>,
    controller_events: super::chart_event_view::EventChoices,
    controller_values: super::character_values::Choices,
    error: String,
    canvas: super::canvas::CanvasState,
    list_mode: bool,
    undo: VecDeque<EditorSnapshot>,
    /// Undo depth at the last Apply/Revert; any other depth means unapplied edits.
    clean_depth: usize,
    /// Space left in the workspace's top bar for Undo/Redo/Revert/Apply, set each frame.
    toolbar_rect: Option<egui::Rect>,
    redo: VecDeque<EditorSnapshot>,
    gesture: Option<EditorSnapshot>,
    gesture_text_focus: Option<egui::Id>,
    gesture_dirty: bool,
    /// What the game's own scripts for this move contain, and which move and size that was.
    source_blocks: Vec<String>,
    source_blocks_for: (String, usize),
    /// Tell a gimmick was picked before any gimmick could hear it; add it once one can.
    pending_tell: bool,
    /// A finished edit is waiting to reach the running match.
    auto_apply: bool,
    in_expanded_field: bool,
    block_clipboard: Option<super::block_clipboard::BlockClipboard>,
}

impl NodeEditor {
    fn copy_selected_block(&mut self, fighter: &str) {
        let Some(id) = self.canvas.selected_node() else {
            self.error = "Select a move block to copy.".into();
            return;
        };
        match super::block_clipboard::BlockClipboard::capture(fighter, &self.draft, id) {
            Ok(clipboard) => {
                self.block_clipboard = Some(clipboard);
                self.error.clear();
            }
            Err(reason) => self.error = reason,
        }
    }

    fn paste_block(
        &mut self,
        fighter: &str,
        move_name: &str,
        base: &BehaviorGraph,
        siblings: &BTreeMap<String, MoveGraph>,
        after: Option<u32>,
    ) {
        let Some(clipboard) = &self.block_clipboard else {
            self.error = "Copy a move block first.".into();
            return;
        };
        let Some(track) = self.draft.tracks.get(self.track).map(|track| track.track) else {
            self.error = "Import the destination move first.".into();
            return;
        };
        if siblings.len() > super::project::MAX_FIGHTER_MOVES {
            self.error = "Character move table exceeds the block reuse limit.".into();
            return;
        }
        for graph in siblings.values() {
            if let Err(reason) = super::block_clipboard::preflight_graph(graph) {
                self.error = format!("Character recipe is invalid: {reason}");
                return;
            }
        }
        // All receiver and sibling dependencies are checked before replacing the draft.
        // The clipboard never imports a donor's hidden native witness or changes live state.
        let (candidate, id) =
            match clipboard.candidate(fighter, move_name, &self.draft, track, after) {
                Ok(candidate) => candidate,
                Err(reason) => {
                    self.error = reason;
                    return;
                }
            };
        let mut all = siblings.clone();

        all.insert(move_name.to_owned(), candidate.clone());
        match super::project::compile_recipes_with_controllers(
            fighter,
            base,
            &self.controller_charts,
            &all,
        ) {
            Ok(_) => {
                self.draft = candidate;
                self.canvas.selected = Some(format!("node:{id}"));
                self.canvas.center_selected();
                self.error.clear();
                self.canvas.clear_errors();
            }
            Err(errors) => {
                self.error = errors
                    .iter()
                    .take(8)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                if errors.len() > 8 {
                    self.error.push_str(&format!(
                        "\n{} further character errors; inspect the saved recipes.",
                        errors.len() - 8
                    ));
                }
            }
        }
    }

    pub fn set_joint_options(&mut self, options: &[String]) {
        if self.joint_options != options {
            self.joint_options = options.to_vec();
        }
    }

    /// Shows the preview's motion frame as a playhead across the workspace lanes.
    /// Tells the live label what the game's script for this move already does.
    pub fn set_source_script(
        &mut self,
        fighter: &str,
        move_name: &str,
        game: &AcmdScript,
        expression: &AcmdScript,
    ) {
        let key = (
            format!("{fighter}/{move_name}"),
            game.stmts.len() + expression.stmts.len(),
        );
        if self.source_blocks_for != key {
            self.source_blocks_for = key;
            self.source_blocks.clear();
            for (track, script) in [(Track::Game, game), (Track::Expression, expression)] {
                let track = super::timeline::import_for_fighter(fighter, track, script);
                super::live_support::source_fingerprints(&track.nodes, &mut self.source_blocks);
            }
        }
    }

    pub fn set_toolbar_rect(&mut self, rect: egui::Rect) {
        self.toolbar_rect = Some(rect);
    }
    pub fn set_playhead(&mut self, frame: f32, playing: bool) {
        self.canvas.playhead = Some(frame);
        self.canvas.follow_playhead = playing;
    }
    /// A frame the user picked on the workspace's frame ruler.
    pub fn take_scrub(&mut self) -> Option<f32> {
        self.canvas.scrub.take()
    }
    pub fn set_controller_charts(
        &mut self,
        fighter: &str,
        main: &BehaviorGraph,
        charts: &BTreeMap<String, crate::mod_project::ControllerChart>,
    ) {
        if crate::gimmicks::controller_project::preflight(main, charts).is_ok() {
            self.controller_charts.clone_from(charts);
            self.controller_values = super::character_values::choices(main, charts);
            self.controller_events = super::chart_event_view::choices(fighter, main, charts);
        } else {
            self.controller_charts.clear();
            self.controller_events.clear();
            self.controller_values.clear();
        }
    }
    pub fn invalidate(&mut self) {
        *self = Self::default();
    }

    fn snapshot_changed(
        &self,
        previous: &EditorSnapshot,
        _siblings: &BTreeMap<String, MoveGraph>,
    ) -> bool {
        serde_json::to_value(previous).ok()
            != serde_json::to_value(EditorSnapshot {
                graph: self.draft.clone(),
                character: self.character.clone(),
            })
            .ok()
    }

    fn finish_gesture(&mut self, siblings: &BTreeMap<String, MoveGraph>) {
        self.gesture_text_focus = None;
        self.gesture_dirty = false;
        if let Some(previous) = self.gesture.take() {
            if self.snapshot_changed(&previous, siblings) {
                if self.undo.len() == 16 {
                    self.undo.pop_front();
                }
                self.undo.push_back(previous);
                self.redo.clear();
                self.auto_apply = true;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        move_name: &str,
        saved: Option<&MoveGraph>,
        scripts: [&AcmdScript; 3],
        effects: &crate::data::EffectScript,
        base: &BehaviorGraph,
        siblings: &BTreeMap<String, MoveGraph>,
    ) -> Option<MoveGraph> {
        let key = format!("{fighter}/{move_name}");
        if self.key != key {
            self.key = key;

            self.draft = saved.cloned().unwrap_or_default();
            self.error.clear();
            self.track = 0;
            self.canvas.reset();
            self.undo.clear();
            self.redo.clear();
            self.clean_depth = 0;
            self.gesture = None;
            self.gesture_text_focus = None;
            self.gesture_dirty = false;

            if self.workspace && self.draft.tracks.is_empty() {
                self.import_tracks(fighter, scripts, effects);
            }
        }
        let workspace_base = self
            .character
            .as_ref()
            .map(|character| character.main.clone());
        let base = workspace_base.as_ref().unwrap_or(base);
        // The canvas is projected before inspector widgets edit the draft. Schedule one
        // following frame for committed text/checkbox gestures so card labels do not wait
        // for the next unrelated pointer movement. Idle inspectors do not continuously paint.
        if ui.input(|input| {
            input.pointer.any_released()
                || input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Key { pressed: true, .. }
                    )
                })
        }) {
            ui.ctx().request_repaint();
        }
        // Typing a value is one edit, even when its digits arrive on separate
        // frames. End that edit before another pointer operation, so clicking
        // Undo or a different control never absorbs that operation into it.
        if self.gesture_text_focus.is_some()
            && (ui.input(|input| input.pointer.any_pressed())
                || ui.memory(|memory| memory.focused()) != self.gesture_text_focus)
        {
            self.finish_gesture(siblings);
        }
        if self.gesture.is_none()
            && ui.input(|input| {
                input.pointer.any_pressed()
                    || input.events.iter().any(|event| {
                        matches!(
                            event,
                            egui::Event::Text(_)
                                | egui::Event::Paste(_)
                                | egui::Event::Key { pressed: true, .. }
                        )
                    })
            })
        {
            self.gesture = Some(EditorSnapshot {
                graph: self.draft.clone(),
                character: self.character.clone(),
            });
        }
        let shortcuts = !ui.ctx().text_edit_focused();
        let undo_key = self.workspace
            && shortcuts
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Z));
        let redo_key = self.workspace
            && shortcuts
            && ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Y)
                    || i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Z)
            });
        let mut apply = false;
        // In the workspace these controls sit at the end of the app's top bar.
        let mut toolbar_ui = self
            .toolbar_rect
            .take()
            .filter(|_| self.workspace)
            .map(|rect| {
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                // The bar lies outside this panel, so the panel's clip would hide it.
                child.set_clip_rect(rect);
                child
            });
        let mut toolbar = |ui: &mut egui::Ui| {
            if ui
                .add_enabled(
                    !self.undo.is_empty() || self.gesture_dirty,
                    egui::Button::new("Undo"),
                )
                .clicked()
                || undo_key
            {
                if let Some(previous) = self.undo.pop_back() {
                    self.redo.push_back(EditorSnapshot {
                        graph: std::mem::replace(&mut self.draft, previous.graph),
                        character: std::mem::replace(&mut self.character, previous.character),
                    });
                    self.auto_apply = true;

                    self.gesture = None;
                    self.gesture_text_focus = None;
                    self.gesture_dirty = false;

                    self.error.clear();
                    self.canvas.clear_errors();
                }
            }
            if ui
                .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                .clicked()
                || redo_key
            {
                if let Some(next) = self.redo.pop_back() {
                    self.undo.push_back(EditorSnapshot {
                        graph: std::mem::replace(&mut self.draft, next.graph),
                        character: std::mem::replace(&mut self.character, next.character),
                    });
                    self.auto_apply = true;

                    self.gesture = None;
                    self.gesture_text_focus = None;
                    self.gesture_dirty = false;

                    self.error.clear();
                    self.canvas.clear_errors();
                }
            }
            if !self.workspace && ui.button("Import current move").on_hover_text("Replace draft tracks with the current game, effects, sound and expression scripts. Apply to save.").clicked() {
                self.draft.tracks = [Track::Game, Track::Sound, Track::Expression].into_iter()
                    .zip(scripts).map(|(track, script)| super::timeline::import_for_fighter(fighter, track, script)).collect();
                self.draft.tracks.push(super::effects::import_for_fighter(fighter, effects));
                self.track = 0;
                super::canvas::prune_positions(&mut self.draft);
                self.canvas.reset();
            }
            if ui.button("Revert draft").clicked() {
                self.draft = saved.cloned().unwrap_or_default();
                if self.workspace {
                    if self.draft.tracks.is_empty() {
                        self.import_tracks(fighter, scripts, effects);
                    }
                    self.character = self.saved_character.clone();
                }
                self.track = 0;
                self.error.clear();
                self.canvas.reset();

                self.clean_depth = self.undo.len();
                self.auto_apply = true;
            }
            let dirty = self.undo.len() != self.clean_depth || self.gesture_dirty;
            if !self.workspace
                && ui
                    .add(egui::Button::new("Apply nodes").selected(dirty))
                    .on_hover_text("Check the blocks, then use them for this move")
                    .clicked()
            {
                apply = true;
            }
            if !self.workspace {
                ui.selectable_value(&mut self.list_mode, false, "Field");
                ui.selectable_value(&mut self.list_mode, true, "List");
            } else {
                let waiting =
                    super::live_support::not_live_blocks(fighter, &self.draft, &self.source_blocks);
                if waiting.is_empty() {
                    ui.weak("● Updates live").on_hover_text(
                        "Your edits reach the running match as soon as you make them.",
                    );
                } else {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("● {} not live yet", waiting.len()))
                        .on_hover_text(format!("These blocks only work after exporting the mod. Everything else updates live.\n{}", waiting.join("\n")));
                }
            }
        };
        match &mut toolbar_ui {
            Some(child) => toolbar(child),
            None => {
                ui.horizontal_wrapped(|ui| toolbar(ui));
            }
        }
        node_edit_error(ui, &self.error);
        if !self.workspace {
            ui.weak("Tracks replace move scripts.");
            if self.draft.tracks.is_empty() {
                ui.label("Import a move to edit its timed nodes.");
            } else {
                self.track = self.track.min(self.draft.tracks.len() - 1);
                let previous_track = self.track;
                ui.horizontal_wrapped(|ui| {
                    for (index, track) in self.draft.tracks.iter().enumerate() {
                        ui.selectable_value(&mut self.track, index, format!("{:?}", track.track));
                    }
                });
                if self.track != previous_track {
                    self.canvas.reset();
                    ui.ctx().request_repaint();
                }
                let next = next_id(&self.draft);
                let current_track = self.draft.tracks[self.track].track;
                let coverage = super::coverage::track_summary(&self.draft.tracks[self.track]);
                if coverage.preserved_block_count > 0 || coverage.preserved_command_count > 0 {
                    ui.weak(format!("{} editable commands · {} preserved commands · {} preserved blocks", coverage.editable_command_count, coverage.preserved_command_count, coverage.preserved_block_count))
                    .on_hover_text("Preserved commands and blocks retain original native structure. They are not fully interpreted building blocks; mixed groups allow their supported neighbors to be edited without changing the preserved slots.");
                }
                ui.horizontal_wrapped(|ui| {
                let selected = self.canvas.selected_node();
                let can_paste = self.block_clipboard.as_ref().is_some_and(|clipboard| clipboard.track() == current_track);
                ui.menu_button("Blocks", |ui| {
                    if ui.add_enabled(selected.is_some(), egui::Button::new("Copy block"))
                        .on_hover_text("Copy this typed block and its complete nested region. Preserved native source cannot be duplicated. The clipboard stays available when changing moves or characters in this project.").clicked() {
                        self.copy_selected_block(fighter);
                        ui.close();
                    }
                    if ui.add_enabled(can_paste && selected.is_some(), egui::Button::new("Paste after selected"))
                        .on_disabled_hover_text("Copy a block from the same track and select its destination.").clicked() {
                        self.paste_block(fighter, move_name, base, siblings, selected);
                        ui.close();
                    }
                    if ui.add_enabled(can_paste, egui::Button::new("Paste at track end"))
                        .on_disabled_hover_text("Copy a block from the same track first.").clicked() {
                        self.paste_block(fighter, move_name, base, siblings, None);
                        ui.close();
                    }
                    if let Some(clipboard) = &self.block_clipboard {
                        ui.separator();
                        ui.weak(format!("{:?} · {}", clipboard.track(), clipboard.origin_fighter()))
                            .on_hover_text("Native commands must be valid for the destination donor. Copying does not remap articles, work IDs, events or resources.");
                    }
                });
                let trail_template = reusable_trail(&self.draft);
                let mut choices = node_choices_for_fighter(current_track, fighter);
                if current_track == Track::Effect && trail_template.is_some() {
                    choices.push((63, "Reuse sword trail"));
                }
                if !choices.iter().any(|(kind, _)| *kind == self.add_kind) {
                    self.add_kind = 0;
                }
                let palette_was_open = egui::ComboBox::is_open(ui.ctx(), ui.make_persistent_id("node_kind"));
                egui::ComboBox::from_id_salt("node_kind")
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .selected_text(
                        choices
                            .iter()
                            .find(|(kind, _)| *kind == self.add_kind)
                            .map(|(_, label)| *label)
                            .unwrap_or("Frame"),
                    )
                    .show_ui(ui, |ui| {
                        let search_response = ui.add(
                            egui::TextEdit::singleline(&mut self.palette_query)
                                .hint_text("Find a block…")
                                .desired_width(170.0),
                        );
                        if !palette_was_open {
                            search_response.request_focus();
                        }
                        let query = self.palette_query.trim().to_ascii_lowercase();
                        for (kind, label) in &choices {
                            if !palette_matches(*kind, label, &query) {
                                continue;
                            }
                            if ui
                                .selectable_value(&mut self.add_kind, *kind, *label)
                                .clicked()
                            {
                                ui.close();
                            }
                        }
                    });
                let can_add = match self.add_kind {
                    101 => super::chart_event_view::first(&self.controller_events).is_some(),
                    _ => true,
                };
                if ui.add_enabled(can_add, egui::Button::new("Add node"))
                    .on_disabled_hover_text(if self.add_kind == 101 {
                        "Add a Named event to an additional controller in Gimmicks → Behavior first."
                    } else { "Add a named event receiver in a character controller first. The timed block selects a declared event; it cannot dispatch an unknown name." })
                    .clicked() {
                    let Some(kind) = self.make_node(self.add_kind, fighter, next, current_track) else { return; };
                    self.draft.tracks[self.track]
                        .nodes
                        .push(MoveNode { id: next, kind });
                    self.canvas.selected = Some(format!("node:{next}"));
                }
            });
                let mut next = next_id(&self.draft);
                if self.list_mode {
                    let options = CommandOptions {
                        track: current_track,
                        joints: &self.joint_options,
                        chart_events: &self.controller_events,
                        character_values: &self.controller_values,
                    };
                    nodes(
                        ui,
                        fighter,
                        current_track,
                        &mut self.draft.tracks[self.track].nodes,
                        &mut next,
                        options,
                    );
                }
            }
            if !self.list_mode {
                let track = self.draft.tracks.get(self.track).map(|track| track.track);
                if !self.canvas.expanded || self.in_expanded_field {
                    let height = if self.in_expanded_field {
                        (ui.ctx().content_rect().height() * 0.60).clamp(360.0, 600.0)
                            - if self.error.is_empty() { 0.0 } else { 30.0 }
                    } else {
                        300.0
                    };
                    ui.push_id("inline_field", |ui| {
                        self.canvas.ui(ui, &mut self.draft, track, height)
                    });
                } else {
                    // The enlarged workspace is rendered next frame with its own
                    // inspector, so properties never hide behind a separate field.
                    ui.weak("Opening field…");
                }
                if self.canvas.selected_node().is_none() {
                    ui.weak("Drag blocks to arrange. Connect execution, event or matching Value ports. Select a block to edit.");
                }
            }
        }
        let pointer_down = ui.input(|input| input.pointer.any_down());
        let mut inspector_ui;
        let inspector_root = if self.workspace {
            let rect = self.workspace_field(ui, fighter, move_name, base, siblings);
            inspector_ui = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("workspace_inspector")
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            &mut inspector_ui
        } else {
            ui
        };
        let workspace = self.workspace;
        let mut inspector = |ui: &mut egui::Ui| {
            if self.workspace {
                ui.strong(if self.canvas.selected.is_some() {
                    "Block"
                } else {
                    "Move"
                });
                if let Some((chart, reference)) = self.canvas.selected_character() {
                    if let Some(character) = &mut self.character {
                        let graph = if chart == "main" {
                            Some(&mut character.main)
                        } else {
                            character
                                .charts
                                .get_mut(&chart)
                                .map(|chart| &mut chart.graph)
                        };
                        if let Some(graph) = graph {
                            self.character_inspector
                                .field_inspector(ui, fighter, graph, &reference);
                        }
                    }
                } else if self.canvas.selected.is_none() {
                    self.story_ui(ui);
                    ui.separator();
                    ui.weak("Click a block to edit it. Blocks run left to right, and wires show what runs next.");
                    ui.weak("F  Fit · Space + drag  Pan\nShift + click / drag  Select group\nCtrl + D  Duplicate · Delete  Remove");
                }
            }
            ui.separator();
            if !self.list_mode {
                self.block_inspector(ui, fighter);
            }
            if !ui.input(|input| input.pointer.any_down()) {
                let editing_text = ui.ctx().text_edit_focused();
                let committed = ui.input(|input| {
                    input.key_pressed(egui::Key::Enter)
                        || input.key_pressed(egui::Key::Escape)
                        || input.key_pressed(egui::Key::Tab)
                });
                if editing_text && !committed {
                    self.gesture_text_focus = ui.memory(|memory| memory.focused());
                    // Enable Undo before its next pointer press. Egui hit-tests the
                    // preceding frame, so enabling it only after that press would
                    // lose a direct Undo click from an active numeric edit.
                    if ui.input(|input| !input.events.is_empty()) {
                        self.gesture_dirty = self
                            .gesture
                            .as_ref()
                            .is_some_and(|gesture| self.snapshot_changed(gesture, siblings));
                    }
                } else {
                    self.finish_gesture(siblings);
                }
            }
        };
        if workspace {
            egui::ScrollArea::vertical()
                .id_salt("workspace_inspector_scroll")
                .auto_shrink([false, false])
                .show(inspector_root, |ui| {
                    ui.set_max_width(ui.available_width());
                    inspector(ui)
                });
        } else {
            inspector(inspector_root);
        }
        // In the workspace a finished edit applies itself, so the running match follows along.
        if self.workspace && self.auto_apply && self.gesture.is_none() && !pointer_down {
            self.auto_apply = false;
            apply = true;
        }
        if apply {
            super::canvas::prune_positions(&mut self.draft);
            if self.draft.is_empty() {
                self.error = "Import a move before applying nodes.".into();
                return None;
            }
            let mut all = siblings.clone();

            all.insert(move_name.to_string(), self.draft.clone());
            let errors = super::project::compile_recipes_with_controllers(
                fighter,
                self.character
                    .as_ref()
                    .map_or(base, |character| &character.main),
                self.character
                    .as_ref()
                    .map_or(&self.controller_charts, |character| &character.charts),
                &all,
            )
            .err()
            .unwrap_or_default();
            self.error = errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            self.clean_depth = self.undo.len();
            if errors.is_empty() {
                self.saved_character = self.character.clone();
                return Some(self.draft.clone());
            }
        }
        None
    }

    /// Keep command and action properties reachable even when the field fills
    /// the window. One movable inspector follows the current card selection.
    fn block_inspector(&mut self, ui: &mut Ui, fighter: &str) -> bool {
        let Some(id) = self.canvas.selected_node() else {
            return false;
        };
        let floating = !self.workspace;
        let open = focused_properties(ui, floating, |ui| {
            let mut next = next_id(&self.draft);
            if let Some(track) = self
                .draft
                .tracks
                .iter_mut()
                .find(|track| super::canvas::contains(&track.nodes, id))
            {
                selected_node(
                    ui,
                    fighter,
                    track.track,
                    &mut track.nodes,
                    id,
                    &mut next,
                    CommandOptions {
                        track: track.track,
                        joints: &self.joint_options,
                        chart_events: &self.controller_events,
                        character_values: &self.controller_values,
                    },
                );
            }
            if !self.workspace {
                ui.weak("Press Apply nodes to use your edits");
            }
        });
        if !open {
            self.canvas.selected = None;
        }
        true
    }
}

fn focused_properties(ui: &mut Ui, floating: bool, add: impl FnOnce(&mut Ui)) -> bool {
    if !floating {
        ui.group(add);
        return true;
    }
    let context = ui.ctx().clone();
    let mut open = true;
    egui::Window::new("Block properties")
        .id(egui::Id::new("move_block_inspector"))
        .open(&mut open)
        .collapsible(false)
        .default_width(330.0)
        .min_width(280.0)
        .max_width(340.0)
        .default_pos(egui::pos2(
            (context.content_rect().right() - 350.0).max(16.0),
            context.content_rect().top() + 120.0,
        ))
        .show(&context, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("selected_block_properties")
                .max_height((context.content_rect().height() - 200.0).max(120.0))
                .show(ui, add);
        });
    open
}

fn selected_node(
    ui: &mut Ui,
    fighter: &str,
    track: Track,
    scope: &mut Vec<MoveNode>,
    id: u32,
    next: &mut u32,
    options: CommandOptions<'_>,
) -> bool {
    if let Some(index) = scope.iter().position(|node| node.id == id) {
        if let Some(help) = super::canvas::node_help(&scope[index]) {
            ui.label(help);
        }
        let mut selected = vec![scope[index].clone()];
        ui.push_id(("inspector", id), |ui| {
            nodes(ui, fighter, track, &mut selected, next, options)
        });
        scope.splice(index..=index, selected);
        return true;
    }
    for node in scope {
        match &mut node.kind {
            NodeKind::Repeat { body, .. } => {
                if selected_node(ui, fighter, track, body, id, next, options) {
                    return true;
                }
            }
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                if selected_node(ui, fighter, track, then_nodes, id, next, options)
                    || selected_node(ui, fighter, track, else_nodes, id, next, options)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn select_index(ui: &mut Ui, id: &str, index: &mut usize, labels: &[&str]) {
    *index = (*index).min(labels.len() - 1);
    egui::ComboBox::from_id_salt(id)
        .selected_text(labels[*index])
        .show_ui(ui, |ui| {
            for (i, label) in labels.iter().enumerate() {
                ui.selectable_value(index, i, *label);
            }
        });
}

pub(super) fn article_target_label(target: crate::data::ArticleTarget) -> &'static str {
    match target {
        crate::data::ArticleTarget::All => "All",
        crate::data::ArticleTarget::Last => "Last created",
    }
}

pub(super) fn short_symbol(symbol: &str) -> &str {
    for separator in [
        "_GENERATE_ARTICLE_",
        "_WORK_ID_FLAG_",
        "_WORK_ID_INT_",
        "_WORK_ID_FLOAT_",
        "_FLAG_",
        "_STATUS_",
    ] {
        if let Some((_, label)) = symbol.split_once(separator) {
            return label;
        }
    }
    symbol
}

fn joint_picker(ui: &mut Ui, joint: &mut String, joints: &[String]) {
    let current = joint.clone();
    egui::ComboBox::from_id_salt("attack_joint").selected_text(current.as_str()).width(90.0).show_ui(ui, |ui| {
        if current != "top" && !joints.contains(&current) {
            ui.selectable_value(joint, current.clone(), format!("{current} (from source)"));
        }
        ui.selectable_value(joint, "top".to_string(), "top");
        for name in joints.iter().filter(|name| name.as_str() != "top") {
            ui.selectable_value(joint, name.clone(), name);
        }
    }).response.on_hover_text("Joints from the loaded character model. An imported virtual joint is retained until explicitly changed.");
}

pub(super) fn variable_picker(
    ui: &mut Ui,
    id: &str,
    name: &mut String,
    vars: &[VarDef],
    kind: Option<VarType>,
) {
    let available: Vec<_> = vars
        .iter()
        .filter(|v| kind.is_none_or(|kind| kind == v.kind))
        .collect();
    egui::ComboBox::from_id_salt(id)
        .selected_text(if name.is_empty() {
            "Choose variable…"
        } else {
            name.as_str()
        })
        .show_ui(ui, |ui| {
            for var in available {
                ui.selectable_value(name, var.name.clone(), &var.name);
            }
        });
}

pub(crate) fn guard_value(
    ui: &mut Ui,
    fighter: &str,
    guard: &mut Predicate,
    variables: &[VarDef],
    depth: usize,
) {
    if depth >= 8 {
        ui.weak("Nested condition")
            .on_hover_text(format!("{guard:?}"));
        return;
    }
    guard_block_ui(ui, fighter, guard, variables);
    match guard {
        Predicate::Not(inner) => {
            ui.indent("not", |ui| {
                guard_value(ui, fighter, inner, variables, depth + 1)
            });
        }
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            ui.indent("conditions", |ui| {
                ui.push_id("left", |ui| {
                    guard_value(ui, fighter, left, variables, depth + 1)
                });
                ui.separator();
                ui.push_id("right", |ui| {
                    guard_value(ui, fighter, right, variables, depth + 1)
                });
            });
        }
        _ => {}
    }
}

pub(crate) fn event_read_ui(ui: &mut Ui, event: &str, field: &str) {
    ui.label("Event field");
    ui.weak(format!("{} · {}", bounded_event_label(event), bounded_event_label(field)))
        .on_hover_text("This value belongs to the current named-event receiver, not to shared character state. Cross-receiver copies are refused.");
}

fn bounded_event_label(name: &str) -> String {
    let mut label: String = name.chars().take(48).collect();
    if label.len() < name.len() {
        label.push('…');
    }
    label
}

pub(super) fn guard_kind_ui(
    ui: &mut Ui,
    fighter: &str,
    guard: &mut Predicate,
    variables: &[VarDef],
) {
    if let Predicate::EventFlag { event, field } = guard {
        event_read_ui(ui, event, field);
        return;
    }
    if matches!(guard, Predicate::DamageStatusIs(_)) {
        ui.weak("Status at contact")
            .on_hover_text("Historical receiver status requires its owning damage root.");
        return;
    }
    let kind = match guard {
        Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::TermEnabled { .. }
        | Predicate::InStatus { .. }
        | Predicate::InMotion { .. }
        | Predicate::Situation { .. } => {
            ui.weak(crate::gimmicks::graph_view::phase_predicate_text(guard));
            return;
        }
        Predicate::DamageIsProjectile => {
            ui.weak("Hit by a projectile");
            return;
        }
        Predicate::Button { button, edge } => {
            ui.weak(crate::gimmicks::graph_view::button_text(*button, *edge));
            return;
        }
        _ if condition_literal(guard) == Some(true) => 7,
        _ if condition_literal(guard) == Some(false) => 8,
        Predicate::Flag(_) => 0,
        Predicate::Expired(_) => 1,
        Predicate::Compare { .. } => 2,
        Predicate::Not(_) => 3,
        Predicate::And(..) => 4,
        Predicate::Or(..) => 5,
        Predicate::NativeFlag(_) => 6,
        Predicate::DamageStatusIs(_) => unreachable!("contact status was inspected above"),
        Predicate::EventFlag { .. } => unreachable!("receiver fields were inspected above"),
    };
    let mut chosen = kind;
    let choices = [
        "Flag is On",
        "Timer is expired",
        "Compare",
        "Not",
        "All",
        "Any",
        "Read native value",
        "True",
        "False",
    ];
    egui::ComboBox::from_id_salt("predicate_kind")
        .selected_text(choices[kind])
        .show_ui(ui, |ui| {
            for (index, label) in choices.iter().enumerate() {
                if index != 6
                    || kind == 6
                    || !crate::gimmicks::native_inputs::NativeInput::catalog_for(
                        fighter,
                        crate::gimmicks::native_inputs::NativeKind::Flag,
                    )
                    .is_empty()
                {
                    ui.selectable_value(&mut chosen, index, *label);
                }
            }
        });
    if chosen != kind {
        let prior = guard.clone();
        *guard = match chosen {
            0 => Predicate::Flag(
                variables
                    .iter()
                    .find(|var| var.kind == VarType::Flag)
                    .map_or_else(String::new, |var| var.name.clone()),
            ),
            1 => Predicate::Expired(
                variables
                    .iter()
                    .find(|var| var.kind == VarType::Timer)
                    .map_or_else(String::new, |var| var.name.clone()),
            ),
            2 => Predicate::Compare {
                op: CmpOp::Gte,
                left: literal(VarType::Int),
                right: literal(VarType::Int),
            },
            3 => Predicate::Not(Box::new(prior)),
            4 => Predicate::And(Box::new(prior), Box::new(default_comparison())),
            5 => Predicate::Or(Box::new(prior), Box::new(default_comparison())),
            7 => literal_condition(true),
            8 => literal_condition(false),
            _ => Predicate::NativeFlag(crate::gimmicks::native_inputs::NativeInput::Unsupported(
                "unselected_native_flag".into(),
            )),
        };
    }
}

pub(crate) fn damage_status_source_ui(
    ui: &mut Ui,
    fighter: &str,
    source: &mut crate::gimmicks::native_inputs::NativeInput,
) {
    use crate::gimmicks::native_inputs::{NativeInput, NativeKind};
    egui::ComboBox::from_id_salt("damage_status_selector")
        .selected_text(native_source_label(source))
        .show_ui(ui, |ui| {
            for candidate in NativeInput::catalog_for(fighter, NativeKind::Flag).into_iter()
                .filter(|source| matches!(source, NativeInput::FighterStatusIs { .. })) {
                let label = native_source_label(&candidate);
                ui.selectable_value(source, candidate, label);
            }
        }).response.on_hover_text("Receiver status at native contact admission, retained by this damage root. Named and ordinary events cannot supply it.");
    if !matches!(source, NativeInput::FighterStatusIs { .. }) || !source.valid_for(fighter) {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Choose a registered receiver status.",
        );
    }
}

fn guard_block_ui(ui: &mut Ui, fighter: &str, guard: &mut Predicate, variables: &[VarDef]) {
    guard_kind_ui(ui, fighter, guard, variables);
    guard_payload_ui(ui, fighter, guard, variables);
}

pub(super) fn guard_payload_ui(
    ui: &mut Ui,
    fighter: &str,
    guard: &mut Predicate,
    variables: &[VarDef],
) {
    if condition_literal(guard).is_some() {
        return;
    }
    match guard {
        Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::TermEnabled { .. }
        | Predicate::InStatus { .. }
        | Predicate::InMotion { .. }
        | Predicate::Situation { .. } => {
            ui.weak(crate::gimmicks::graph_view::phase_predicate_text(guard));
        }
        Predicate::DamageIsProjectile => {
            ui.weak("Hit by a projectile");
        }
        Predicate::Button { button, edge } => {
            ui.weak(crate::gimmicks::graph_view::button_text(*button, *edge));
        }
        Predicate::Flag(name) => {
            variable_picker(ui, "flag_condition", name, variables, Some(VarType::Flag));
        }
        Predicate::Expired(name) => {
            variable_picker(ui, "timer_condition", name, variables, Some(VarType::Timer));
        }
        Predicate::DamageStatusIs(source) => damage_status_source_ui(ui, fighter, source),
        Predicate::NativeFlag(source) => {
            native_source_ui(
                ui,
                fighter,
                source,
                crate::gimmicks::native_inputs::NativeKind::Flag,
            );
        }
        Predicate::Compare { op, left, right } => {
            let mut kind = expression_kind(left, variables);
            ui.horizontal_wrapped(|ui| {
                let previous = kind;
                egui::ComboBox::from_id_salt("comparison_value_kind")
                    .selected_text(if kind == VarType::Float {
                        "Gauge"
                    } else {
                        "Counter"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut kind, VarType::Int, "Counter");
                        ui.selectable_value(&mut kind, VarType::Float, "Gauge");
                    });
                if previous != kind {
                    *left = literal(kind);
                    *right = literal(kind);
                }
                egui::ComboBox::from_id_salt("guard_comparison")
                    .selected_text(op.to_string())
                    .show_ui(ui, |ui| {
                        for kind in [CmpOp::Eq, CmpOp::Lt, CmpOp::Lte, CmpOp::Gt, CmpOp::Gte] {
                            ui.selectable_value(op, kind, kind.to_string());
                        }
                    });
            });
            ui.push_id("left", |ui| {
                expression_ui(ui, fighter, left, variables, kind, 0)
            });
            ui.push_id("right", |ui| {
                expression_ui(ui, fighter, right, variables, kind, 0)
            });
        }
        Predicate::Not(_)
        | Predicate::And(..)
        | Predicate::Or(..)
        | Predicate::EventFlag { .. } => {}
    }
}

fn default_comparison() -> Predicate {
    Predicate::Compare {
        op: CmpOp::Eq,
        left: literal(VarType::Int),
        right: literal(VarType::Int),
    }
}

fn literal(kind: VarType) -> Expr {
    Expr::Const(if kind == VarType::Float {
        ConstValue::Float(0.0)
    } else {
        ConstValue::Int(0)
    })
}

pub(super) fn literal_condition(value: bool) -> Predicate {
    Predicate::Compare {
        op: if value { CmpOp::Eq } else { CmpOp::Lt },
        left: literal(VarType::Int),
        right: literal(VarType::Int),
    }
}

pub(super) fn condition_literal(value: &Predicate) -> Option<bool> {
    match value {
        Predicate::Compare {
            op,
            left: Expr::Const(ConstValue::Int(0)),
            right: Expr::Const(ConstValue::Int(0)),
        } => match op {
            CmpOp::Eq => Some(true),
            CmpOp::Lt => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn expression_kind(value: &Expr, variables: &[VarDef]) -> VarType {
    match value {
        Expr::Const(ConstValue::Float(_)) => VarType::Float,
        Expr::NativeFloat(_) => VarType::Float,
        Expr::Var(name) => variables
            .iter()
            .find(|var| var.name == *name)
            .map_or(VarType::Int, |var| var.kind),
        Expr::Add(left, _) | Expr::Sub(left, _) | Expr::Mul(left, _) => {
            expression_kind(left, variables)
        }
        _ => VarType::Int,
    }
}

fn expression_ui(
    ui: &mut Ui,
    fighter: &str,
    value: &mut Expr,
    variables: &[VarDef],
    kind: VarType,
    depth: usize,
) {
    if depth >= 8 {
        ui.weak("Nested value").on_hover_text(format!("{value:?}"));
        return;
    }
    expression_block_ui(ui, fighter, value, variables, kind);
    if let Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) = value {
        ui.indent("operands", |ui| {
            ui.push_id("left", |ui| {
                expression_ui(ui, fighter, left, variables, kind, depth + 1)
            });
            ui.push_id("right", |ui| {
                expression_ui(ui, fighter, right, variables, kind, depth + 1)
            });
        });
    }
}

pub(super) fn expression_block_ui(
    ui: &mut Ui,
    fighter: &str,
    value: &mut Expr,
    variables: &[VarDef],
    kind: VarType,
) {
    if matches!(value, Expr::PendingDamage) {
        ui.weak("Pending damage")
            .on_hover_text("Positive native units from this damage root.");
        return;
    }
    if let Expr::EventField { event, field } = value {
        event_read_ui(ui, event, field);
        return;
    }
    let previous = match value {
        Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. } => {
            ui.weak(crate::gimmicks::graph_view::source_text(value));
            return;
        }
        Expr::PendingDamage => unreachable!("pending damage was inspected above"),
        Expr::Const(_) => 0,
        Expr::Var(_) => 1,
        Expr::TimerRemaining(_) => 6,
        Expr::Add(..) => 2,
        Expr::Sub(..) => 3,
        Expr::Mul(..) => 4,
        Expr::NativeFloat(_) | Expr::NativeInt(_) => 5,
        Expr::EventField { .. } => unreachable!("receiver fields were inspected above"),
    };
    let mut chosen = previous;
    ui.horizontal_wrapped(|ui| {
        let choices = [
            "Number",
            "Variable",
            "Add",
            "Subtract",
            "Multiply",
            "Read native value",
            "Timer remaining",
        ];
        egui::ComboBox::from_id_salt("value_kind")
            .selected_text(choices[previous])
            .show_ui(ui, |ui| {
                for (index, label) in choices.iter().enumerate() {
                    if index == 6 && previous != 6 &&
                        (!matches!(kind, VarType::Int | VarType::Timer) || !variables.iter().any(|var| var.kind == VarType::Timer)) {
                        continue;
                    }
                    if index != 5
                        || previous == 5
                        || !crate::gimmicks::native_inputs::NativeInput::catalog_for(
                            fighter,
                            if kind == VarType::Float {
                                crate::gimmicks::native_inputs::NativeKind::Float
                            } else {
                                crate::gimmicks::native_inputs::NativeKind::Int
                            },
                        )
                        .is_empty()
                    {
                        ui.selectable_value(&mut chosen, index, *label);
                    }
                }
            });
        if chosen != previous {
            let children = match value {
                Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
                    (left.clone(), right.clone())
                }
                _ => (Box::new(value.clone()), Box::new(literal(kind))),
            };
            *value = match chosen {
                0 => literal(kind),
                1 => Expr::Var(
                    variables
                        .iter()
                        .find(|var| {
                            var.kind
                                == if kind == VarType::Timer {
                                    VarType::Int
                                } else {
                                    kind
                                }
                        })
                        .map_or_else(String::new, |var| var.name.clone()),
                ),
                2 => Expr::Add(children.0, children.1),
                3 => Expr::Sub(children.0, children.1),
                4 => Expr::Mul(children.0, children.1),
                6 => Expr::TimerRemaining(variables.iter()
                    .find(|var| var.kind == VarType::Timer)
                    .map_or_else(String::new, |var| var.name.clone())),
                _ if kind == VarType::Float => {
                    Expr::NativeFloat(crate::gimmicks::native_inputs::NativeInput::Unsupported(
                        "unselected_native_float".into(),
                    ))
                }
                _ => Expr::NativeInt(crate::gimmicks::native_inputs::NativeInput::Unsupported(
                    "unselected_native_int".into(),
                )),
            };
        }
        match value {
            Expr::Const(ConstValue::Int(value)) => {
                let control = egui::DragValue::new(value);
                ui.add(if kind == VarType::Timer {
                    control
                        .range(0..=crate::gimmicks::live_model::MAX_TIMER_FRAMES)
                        .suffix(" ticks")
                } else {
                    control
                });
            }
            Expr::Const(ConstValue::Float(value)) => {
                ui.add(egui::DragValue::new(value).speed(0.1));
            }
            Expr::Const(ConstValue::Flag(value)) => {
                ui.checkbox(value, "On");
            }
            Expr::Var(name) => variable_picker(
                ui,
                "value_variable",
                name,
                variables,
                Some(if kind == VarType::Timer {
                    VarType::Int
                } else {
                    kind
                }),
            ),
            Expr::TimerRemaining(name) => {
                variable_picker(ui, "remaining_timer", name, variables, Some(VarType::Timer));
                ui.weak("ticks").on_hover_text("Read the timer's current remaining game ticks as an Integer. Reading does not advance, restart or expire it; this is not the timer's identity.");
            }
            _ => {}
        }
    });
    if let Expr::NativeFloat(source) = value {
        native_float_source_ui(ui, fighter, source);
    } else if let Expr::NativeInt(source) = value {
        native_source_ui(
            ui,
            fighter,
            source,
            crate::gimmicks::native_inputs::NativeKind::Int,
        );
    }
}

pub(crate) fn native_float_source_ui(
    ui: &mut Ui,
    fighter: &str,
    source: &mut crate::gimmicks::native_inputs::NativeInput,
) {
    native_source_ui(
        ui,
        fighter,
        source,
        crate::gimmicks::native_inputs::NativeKind::Float,
    );
}

pub(crate) fn native_value_detail(
    source: &crate::gimmicks::native_inputs::NativeInput,
    kind: crate::gimmicks::native_inputs::NativeKind,
) -> String {
    use crate::gimmicks::native_inputs::NativeKind;
    let label = if source.kind() == Some(kind) {
        native_source_label(source)
    } else {
        "Unsupported source".into()
    };
    let type_label = match kind {
        NativeKind::Float => "Float",
        NativeKind::Int => "Integer",
        NativeKind::Flag => "Flag",
    };
    format!("{label} · {type_label}")
}

/// Generic names are the exact constant suffix, spaced for scanning. Only the
/// reviewed named sources have semantic labels; no resource meaning is inferred.
pub(crate) fn native_source_label(source: &crate::gimmicks::native_inputs::NativeInput) -> String {
    use crate::gimmicks::native_inputs::NativeInput;
    match source {
        NativeInput::Work { .. } if source.projectile_article().is_some() => format!(
            "Projectile value · {} · {}",
            crate::gimmicks::editor::article_label(source.projectile_article().unwrap()),
            source.label()
        ),
        NativeInput::Work { .. } => source.label().replace('_', " ").to_ascii_lowercase(),
        NativeInput::ArticleExists { symbol, .. } | NativeInput::ArticleCount { symbol, .. } => {
            format!("{} exists", crate::gimmicks::editor::article_label(symbol))
        }
        _ => source.label().to_owned(),
    }
}

/// Shared searchable, bounded list used by both character and move fields.
pub(crate) fn native_source_options_ui(
    ui: &mut Ui,
    fighter: &str,
    source: &mut crate::gimmicks::native_inputs::NativeInput,
    kind: crate::gimmicks::native_inputs::NativeKind,
) {
    use crate::gimmicks::native_inputs::{FighterValue, NativeInput, NativeKind};
    let type_label = match kind {
        NativeKind::Float => "Float",
        NativeKind::Int => "Integer",
        NativeKind::Flag => "Flag",
    };
    let search_key = ui.id().with(("native_source_search", fighter, type_label));
    let mut query = ui.data_mut(|data| data.get_temp::<String>(search_key).unwrap_or_default());
    ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text("Search sources…")
            .char_limit(128)
            .desired_width(210.0),
    );
    let needle = query.trim().to_ascii_lowercase();
    let choices: Vec<_> = NativeInput::catalog_for(fighter, kind)
        .into_iter()
        .filter(|candidate| {
            format!("{} {}", native_source_label(candidate), candidate.symbol())
                .to_ascii_lowercase()
                .contains(&needle)
        })
        .collect();
    ui.scope(|ui| {
        ui.set_max_width(230.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        egui::ScrollArea::vertical().id_salt("native_source_rows").max_height(220.0)
            .show_rows(ui, ui.spacing().interact_size.y, choices.len(), |ui, rows| {
                for candidate in &choices[rows] {
                    let meaning = if *candidate == NativeInput::CloudLimitClearFrame {
                        "Observed integer, not an authored timer or a promise of remaining duration."
                    } else if candidate.projectile_article().is_some() {
                        "Newest owned live projectile, sampled each frame; zero when absent. Prefer counters, charge and flags read by the weapon; some state is overwritten every frame."
                    } else if matches!(candidate, NativeInput::AttackResultFlag { .. }) {
                        "Fighter flag sample, not a collision callback. No victim, projectile identity, deduplication or reset guarantee."
                    } else if matches!(candidate, NativeInput::ArticleExists { .. }) {
                        "Presence of this registered article kind on the triggering fighter. Not a spawn or hit event, article count, instance identity or removal cause."
                    } else if matches!(candidate, NativeInput::Fighter { value: FighterValue::Damage, .. }) {
                        "Read-only DamageModule::damage on this fighter, native slot zero. Not a damage-dealt/taken event, attacker identity or native damage write."
                    } else {
                        "Raw observation. The name does not establish units, capacity, causes or resource-write authority."
                    };
                    if ui.selectable_value(source, candidate.clone(), native_source_label(candidate))
                        .on_hover_text(format!("{} · {type_label} · {}\nRead once per dispatch. {meaning}", fighter, candidate.reference()))
                        .clicked() { ui.close(); }
                }
            });
    });
    if choices.is_empty() {
        ui.weak("No matching sources");
    }
    ui.data_mut(|data| data.insert_temp(search_key, query));
}

pub(crate) fn native_source_ui(
    ui: &mut Ui,
    fighter: &str,
    source: &mut crate::gimmicks::native_inputs::NativeInput,
    kind: crate::gimmicks::native_inputs::NativeKind,
) {
    use crate::gimmicks::native_inputs::{NativeInput, NativeKind};
    let type_label = match kind {
        NativeKind::Float => "Float",
        NativeKind::Int => "Integer",
        NativeKind::Flag => "Flag",
    };
    let label = match &*source {
        known if known.valid_for(fighter) && known.kind() == Some(kind) => {
            native_source_label(known)
        }
        NativeInput::Unsupported(name)
            if matches!(
                name.as_str(),
                "unselected_native_float" | "unselected_native_int" | "unselected_native_flag"
            ) =>
        {
            "Choose source…".into()
        }
        _ => "Unsupported source".into(),
    };
    egui::ComboBox::from_id_salt("native_source")
        .width(210.0)
        .height(300.0)
        .selected_text(label)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| native_source_options_ui(ui, fighter, source, kind))
        .response.on_hover_text(format!("Read-only native input: {}\nSampled once per dispatch on the game thread. Missing observations stop the step without changing authored state.", source.reference()));
    ui.weak(format!("Read-only · {type_label}"));
    if !source.valid_for(fighter) || source.kind() != Some(kind) {
        ui.weak("Unavailable until a supported source is selected.");
    }
}

fn next_id(graph: &MoveGraph) -> u32 {
    fn max(nodes: &[MoveNode]) -> u32 {
        nodes
            .iter()
            .map(|node| {
                node.id.max(match &node.kind {
                    NodeKind::Repeat { body, .. } => max(body),
                    NodeKind::Branch {
                        then_nodes,
                        else_nodes,
                        ..
                    } => max(then_nodes).max(max(else_nodes)),
                    _ => 0,
                })
            })
            .max()
            .unwrap_or(0)
    }
    graph
        .tracks
        .iter()
        .map(|track| max(&track.nodes))
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn node_has_preserved_source(kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Preserved { .. }
        | NodeKind::MixedExecute { .. }
        | NodeKind::MixedEffectExecute { .. } => true,
        NodeKind::Repeat { body, .. } => body
            .iter()
            .any(|node| node_has_preserved_source(&node.kind)),
        NodeKind::Branch {
            then_nodes,
            else_nodes,
            ..
        } => then_nodes
            .iter()
            .chain(else_nodes)
            .any(|node| node_has_preserved_source(&node.kind)),
        _ => false,
    }
}

fn nodes(
    ui: &mut Ui,
    fighter: &str,
    track: Track,
    nodes: &mut Vec<MoveNode>,
    next: &mut u32,
    options: CommandOptions<'_>,
) {
    let options = CommandOptions { track, ..options };
    let mut remove = None;
    let mut reorder = None;
    let mut duplicate = None;
    let node_count = nodes.len();
    for (index, node) in nodes.iter_mut().enumerate() {
        ui.push_id(node.id, |ui| {
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.scope(|ui| match &mut node.kind {
                        NodeKind::Frame { frame } => { ui.label("Frame"); ui.add(egui::DragValue::new(frame).range(0.0..=1_000_000.0)); },
                        NodeKind::NativeExecuteFrame { frame } => { native_execute_frame_ui(ui, frame); },
                        NodeKind::NativeMotionLoopSync => { native_motion_loop_sync_ui(ui); },
                        NodeKind::NativeWaitLoopClear => { ui.label("Wait for motion loop").on_hover_text("Native wait_loop_clear: waits until the motion loops, then restarts the frame count. Direct root Game or Expression placement; To repeat blocks on every loop, use Repeat with Every motion loop."); },
                        NodeKind::Wait { frames } => { ui.label("Wait"); ui.add(egui::DragValue::new(frames).range(0.0..=1_000_000.0)); },
                        NodeKind::MotionRate { rate } => { ui.label("Motion rate"); ui.add(egui::DragValue::new(rate).range(0.001..=1000.0).speed(0.01)).on_hover_text("ACMD motion-rate argument. Timing uses motion frames, not timer ticks; test the resulting playback in game."); },
                        NodeKind::BareCommand { .. } => { ui.label("Top-level command").on_hover_text("Runs outside the Execute guard, exactly as written in the original source."); },
                        NodeKind::Repeat { count, motion_loop, .. } => {
                            use crate::data::MotionLoop;
                            ui.label("Repeat");
                            if motion_loop.is_no() { ui.add(egui::DragValue::new(count).range(1..=100)); }
                            let before = *motion_loop;
                            egui::ComboBox::from_id_salt("repeat_mode")
                                .selected_text(match *motion_loop {
                                    MotionLoop::No => "times",
                                    MotionLoop::Clear => "every motion loop (loop clear)",
                                    MotionLoop::SyncMotion => "every motion loop (sync)",
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(motion_loop, MotionLoop::No, "times");
                                    if matches!(track, Track::Game | Track::Expression) {
                                        ui.selectable_value(motion_loop, MotionLoop::Clear, "every motion loop (loop clear)")
                                            .on_hover_text("Native wait_loop_clear loop, as rapid jabs use: runs the body, waits for the motion to loop, and starts again until the move ends.");
                                    }
                                    ui.selectable_value(motion_loop, MotionLoop::SyncMotion, "every motion loop (sync)")
                                        .on_hover_text("Native wait_loop_sync_mot loop, as run and walk effects, sounds and expressions use.");
                                })
                                .response
                                .on_hover_text("An every-motion-loop repeat must be the last root block of its track;");
                            if *motion_loop != before && motion_loop.is_loop() {
                                *count = 1;
                            }
                        },
                        NodeKind::Branch { .. } => { ui.label("If"); },
                        // Same titles as the block on the field.
                        NodeKind::Execute { commands } => { ui.label(match commands.as_slice() { [command] => super::plain::block_name(super::canvas::command_caption(command).0).to_string(), _ => format!("Actions ({})", commands.len()) }); },
                        NodeKind::MixedExecute { commands } => { ui.label(format!("Imported actions ({})", commands.len())).on_hover_text("Edit supported properties without changing command order. Preserved commands stay in their original slots."); },
                        NodeKind::EffectExecute { commands } => { ui.label(match commands.as_slice() { [command] => super::canvas::effect_caption(command).0.to_string(), _ => format!("Visual effects ({})", commands.len()) }); },
                        NodeKind::MixedEffectExecute { commands, .. } => { ui.label(format!("Imported effects ({})", commands.len())).on_hover_text("Edit supported effects in place. Native commands keep their original slots and execute boundary;"); },
                        NodeKind::Preserved { .. } => { ui.label("Imported structure (read-only)").on_hover_text("Preserves source branches and commands the node editor does not yet author. No flattening or arbitrary Rust editing."); },
                    }).response.on_hover_text(format!("Internal block ID: {}", node.id));
                    if node_count > 1 {
                        if ui.add_enabled(index > 0, egui::Button::new("Up").small()).on_hover_text("Move earlier in this region").clicked() { reorder = Some((index, index - 1)); }
                        if ui.add_enabled(index + 1 < node_count, egui::Button::new("Down").small()).on_hover_text("Move later in this region").clicked() { reorder = Some((index, index + 1)); }
                    }
                    if !node_has_preserved_source(&node.kind) && ui.small_button("Duplicate").clicked() { duplicate = Some(index); }
                    if !node_has_preserved_source(&node.kind) && ui.small_button("Remove").clicked() { remove = Some(index); }
                });
                match &mut node.kind {
                    NodeKind::BareCommand { command } => match command.as_mut() {
                        ExcuteStmt::Sound(sound) => sound_command_ui(ui, sound),
                        ExcuteStmt::Expression(expression) => expression_command_ui(ui, expression),
                        command => { if !native_command_ui(ui, fighter, command, options.joints) { ui.label("Engine command").on_hover_text(format!("{command:?}")); } },
                    },
                    NodeKind::Execute { commands } => {
                        let mut remove_command = None;
                        // A lone command is removed with its block's Remove.
                        let several = commands.len() > 1;
                        for (index, command) in commands.iter_mut().enumerate() {
                            ui.push_id(index, |ui| {
                            if several && ui.small_button("Remove command").clicked() { remove_command = Some(index); }
                            command_ui(ui, fighter, command, options);
                            });
                        }
                        if let Some(index) = remove_command { commands.remove(index); }
                    }
                    NodeKind::MixedExecute { commands } => {
                        for (index, command) in commands.iter_mut().enumerate() {
                            ui.push_id(index, |ui| match command {
                                MoveCommand::Editable(command) => command_ui(ui, fighter, command, options),
                                MoveCommand::Preserved(command) => {
                                    let payload = format!("{command:?}");
                                    ui.weak("Preserved command").on_hover_text(format!("{payload}\n\nRead-only, in its original command slot. Its supported neighbors remain editable."));
                                }
                            });
                        }
                    }
                    NodeKind::EffectExecute { commands } => { for (index, command) in commands.iter_mut().enumerate() { ui.push_id(index, |ui| effect_command_ui_mode(ui, command, options.joints, true, Some(fighter))); } },
                    NodeKind::MixedEffectExecute { commands, .. } => {
                        for (index, command) in commands.iter_mut().enumerate() {
                            ui.push_id(index, |ui| match command {
                                super::model::EffectCommandSlot::Editable(command) => effect_command_ui_mode(ui, command, options.joints, false, Some(fighter)),
                                super::model::EffectCommandSlot::Preserved(command) => {
                                    ui.weak("Preserved native effect").on_hover_text(format!("{command:?}\n\nRead-only, in its original slot. Supported neighboring effects remain editable."));
                                }
                            });
                        }
                    },
                    NodeKind::Repeat { body, .. } => { nodes_ui_body(ui, fighter, track, body, next, options); },
                    NodeKind::Branch { condition, then_nodes, else_nodes, .. } => {
                        condition_picker(ui, fighter, track, condition, options.character_values);
                        ui.push_id("then", |ui| { ui.weak("If yes"); nodes_ui_body(ui, fighter, track, then_nodes, next, options); });
                        ui.push_id("else", |ui| { ui.weak("If no"); nodes_ui_body(ui, fighter, track, else_nodes, next, options); });
                    },
                    _ => {},
                }
            });
        });
    }
    if let Some((from, to)) = reorder {
        if to < nodes.len() {
            nodes.swap(from, to);
        }
    }
    if let Some(index) = duplicate {
        let mut copy = nodes[index].clone();
        rekey(&mut copy, next);
        nodes.insert(index + 1, copy);
    }
    if let Some(index) = remove {
        nodes.remove(index);
    }
}

fn command_ui(ui: &mut Ui, fighter: &str, command: &mut ExcuteStmt, options: CommandOptions<'_>) {
    let joints = options.joints;
    match command {
        ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::ChangeMotion {
            symbol,
            motion,
        }) => {
            article_motion_ui(ui, fighter, options.track, symbol, motion);
        }
        ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind } => {
            attack_reference_joint_ui(ui, kind, options.track);
        }
        ExcuteStmt::GimmickEvent { .. }
        | ExcuteStmt::GimmickChartEvent { .. }
        | ExcuteStmt::GimmickChartEventValues { .. } => {
            super::chart_event_view::selector(ui, command, options.chart_events);
        }
        ExcuteStmt::Catch(call) => {
            volume_ui(
                ui,
                joints,
                &mut call.id,
                &mut call.bone_name,
                &mut call.size,
                [&mut call.offset_x, &mut call.offset_y, &mut call.offset_z],
                &mut call.capsule_end,
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Situation");
                constant_picker(
                    ui,
                    "grab_situation",
                    &mut call.situation,
                    crate::param_labels::SITUATION_MASK,
                    false,
                );
                ui.weak("Captured status").on_hover_text(&call.status);
            });
        }
        ExcuteStmt::Search(call) => {
            volume_ui(
                ui,
                joints,
                &mut call.id,
                &mut call.bone_name,
                &mut call.size,
                [&mut call.offset_x, &mut call.offset_y, &mut call.offset_z],
                &mut call.capsule_end,
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Detect");
                constant_picker(
                    ui,
                    "search_kind",
                    &mut call.extras.collision_kind,
                    crate::param_labels::COLLISION_KIND_MASK,
                    false,
                );
                ui.label("Hurtboxes");
                constant_picker(
                    ui,
                    "search_status",
                    &mut call.extras.hit_status,
                    crate::param_labels::HIT_STATUS_MASK,
                    false,
                );
            });
            ui.collapsing("Collision filters", |ui| {
                constant_picker(
                    ui,
                    "search_situation",
                    &mut call.situation_mask,
                    crate::param_labels::SITUATION_MASK,
                    false,
                );
                constant_picker(
                    ui,
                    "search_category",
                    &mut call.category_mask,
                    crate::param_labels::CATEGORY_MASK,
                    false,
                );
                constant_picker(
                    ui,
                    "search_part",
                    &mut call.part_mask,
                    crate::param_labels::PART_MASK,
                    false,
                );
            });
        }
        ExcuteStmt::AttackAbs(call) => {
            ui.weak("Sets the throw's damage. Apply throw is what hits the held opponent with it.")
                .on_hover_text(&call.kind);
            ui.horizontal_wrapped(|ui| {
                ui.label("ID");
                ui.add(egui::DragValue::new(&mut call.id).range(0..=255));
                ui.label("Damage");
                ui.add(
                    egui::DragValue::new(&mut call.damage)
                        .range(0.0..=999.0)
                        .speed(0.1),
                );
                ui.label("Angle");
                ui.add(egui::DragValue::new(&mut call.angle));
            });
            ui.horizontal_wrapped(|ui| {
                for (label, value) in [
                    ("Growth", &mut call.kb_scaling),
                    ("Base", &mut call.kb_base),
                    ("Fixed", &mut call.fkb),
                ] {
                    ui.label(label);
                    ui.add(egui::DragValue::new(value));
                }
            });
            ui.collapsing("Hit feedback", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Hitlag");
                    ui.add(
                        egui::DragValue::new(&mut call.hitlag_mult)
                            .range(0.0..=999.0)
                            .speed(0.1),
                    );
                    ui.label("Facing");
                    constant_picker(
                        ui,
                        "absolute_facing",
                        &mut call.lr_check,
                        crate::param_labels::LR_CHECK,
                        false,
                    );
                });
                hit_feedback_ui(
                    ui,
                    &mut call.collision_attr,
                    &mut call.sound_level,
                    &mut call.sound_attr,
                    &mut call.attack_region,
                );
            });
        }
        ExcuteStmt::SetSpeed(call) => {
            velocity_ui(ui, &mut call.speed_x, &mut call.speed_y, "Set velocity")
        }
        ExcuteStmt::AddSpeedNoLimit(call) => velocity_ui(
            ui,
            &mut call.speed_x,
            &mut call.speed_y,
            "Add velocity without limit",
        ),
        ExcuteStmt::SetSpeedEx(call) => {
            velocity_ui(
                ui,
                &mut call.speed_x,
                &mut call.speed_y,
                "Set reserve velocity",
            );
            ui.weak("Kinetic reserve").on_hover_text(&call.kinetic_kind);
        }
        ExcuteStmt::DamageNoReaction(call) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Reaction mode");
                constant_picker(
                    ui,
                    "reaction_mode",
                    &mut call.mode,
                    crate::param_labels::DAMAGE_NO_REACTION_MODE,
                    true,
                );
                if let Ok(mut value) = call.value.trim().trim_end_matches("f32").parse::<f32>() {
                    ui.label("Threshold");
                    if ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .range(0.0..=9999.0)
                                .speed(0.1),
                        )
                        .changed()
                    {
                        call.value = format!("{value:?}");
                    }
                } else {
                    ui.weak("Imported threshold").on_hover_text(&call.value);
                }
            });
        }
        ExcuteStmt::AttackMod { kind, id, value } => {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("attack_modifier")
                    .selected_text(if *kind == crate::data::AttackModKind::Power {
                        "Damage"
                    } else {
                        "Shield setoff"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(kind, crate::data::AttackModKind::Power, "Damage");
                        ui.selectable_value(
                            kind,
                            crate::data::AttackModKind::ShieldSetoffMul,
                            "Shield setoff",
                        );
                    });
                ui.label("Hitbox ID");
                ui.add(egui::DragValue::new(id).range(0..=255));
                ui.label("Value");
                ui.add(egui::DragValue::new(value).range(0.0..=999.0).speed(0.1));
            });
        }
        ExcuteStmt::Clear(id) | ExcuteStmt::EraseWind(id) => {
            ui.horizontal(|ui| {
                ui.label("ID");
                ui.add(egui::DragValue::new(id).range(0..=255));
            });
        }
        ExcuteStmt::Wind(wind) => {
            wind_command_ui(ui, wind);
        }
        ExcuteStmt::Attack(attack) => {
            egui::Grid::new("hitbox_basics")
                .num_columns(4)
                .show(ui, |ui| {
                    ui.label("Hitbox");
                    ui.add(egui::DragValue::new(&mut attack.id).range(0..=255));
                    ui.label("Body part");
                    joint_picker(ui, &mut attack.bone_name, joints);
                    ui.end_row();
                    ui.label("Damage");
                    ui.add(
                        egui::DragValue::new(&mut attack.damage)
                            .range(0.0..=999.0)
                            .speed(0.1)
                            .suffix("%"),
                    );
                    ui.label("Size");
                    ui.add(
                        egui::DragValue::new(&mut attack.size)
                            .range(0.0..=999.0)
                            .speed(0.1),
                    );
                    ui.end_row();
                    ui.label("Angle");
                    ui.add(egui::DragValue::new(&mut attack.angle).suffix("°"));
                    ui.end_row();
                });
            ui.collapsing("Shape and knockback", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Offset");
                    for value in [
                        &mut attack.offset_x,
                        &mut attack.offset_y,
                        &mut attack.offset_z,
                    ] {
                        ui.add(egui::DragValue::new(value).speed(0.1));
                    }
                    let mut capsule = attack.capsule_end.is_some();
                    if ui.checkbox(&mut capsule, "Capsule").changed() {
                        attack.capsule_end =
                            capsule.then_some([attack.offset_x, attack.offset_y, attack.offset_z]);
                    }
                    if let Some(end) = &mut attack.capsule_end {
                        ui.label("End");
                        for value in end {
                            ui.add(egui::DragValue::new(value).speed(0.1));
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for (label, value) in [
                        ("Growth", &mut attack.kb_scaling),
                        ("Base", &mut attack.kb_base),
                        ("Fixed", &mut attack.fkb),
                    ] {
                        ui.label(label);
                        ui.add(egui::DragValue::new(value));
                    }
                });
            });
            attack_details_ui(ui, attack);
        }
        ExcuteStmt::Sound(sound) => sound_command_ui(ui, sound),
        ExcuteStmt::Expression(expression) => expression_command_ui(ui, expression),
        command => {
            if !native_command_ui(ui, fighter, command, joints) {
                match super::canvas::command_caption(command) {
                    ("Engine action", detail) => {
                        ui.label(super::plain::block_name(&detail));
                    }
                    (title, detail) => {
                        ui.label(super::plain::block_name(title));
                        if !detail.is_empty() {
                            ui.weak(detail);
                        }
                    }
                }
            }
        }
    }
}

fn attack_details_ui(ui: &mut Ui, attack: &mut crate::data::AttackCall) {
    ui.collapsing("Hit behavior", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Hitlag");
            ui.add(
                egui::DragValue::new(&mut attack.hitlag_mult)
                    .range(0.0..=999.0)
                    .speed(0.1),
            );
            ui.label("SDI");
            ui.add(
                egui::DragValue::new(&mut attack.sdi_mult)
                    .range(0.0..=999.0)
                    .speed(0.1),
            );
            ui.label("Rehit");
            ui.add(egui::DragValue::new(&mut attack.ground_or_air).range(0..=999));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Setoff");
            constant_picker(
                ui,
                "attack_setoff",
                &mut attack.setoff_kind,
                crate::param_labels::SETOFF_KIND,
                false,
            );
            ui.label("Facing");
            constant_picker(
                ui,
                "attack_facing",
                &mut attack.lr_check,
                crate::param_labels::LR_CHECK,
                false,
            );
            ui.checkbox(&mut attack.is_clang, "Set weight");
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Shield damage");
            ui.add(egui::DragValue::new(&mut attack.is_add_attack));
            ui.label("Trip");
            ui.add(egui::DragValue::new(&mut attack.hitbox_attr).speed(0.01));
            ui.label("Part");
            ui.add(egui::DragValue::new(&mut attack.part));
        });
    });
    ui.collapsing("Contact flags", |ui| {
        // Persisted field names predate the pinned ATTACK layout. Keep that
        // schema unchanged, but label the native parameters they actually emit.
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut attack.is_mtk, "Reflectable");
            ui.checkbox(&mut attack.is_shield_disable, "Absorbable");
            ui.checkbox(&mut attack.is_reflectable, "Flinchless");
            ui.checkbox(&mut attack.is_absorbable, "Disable hitlag");
            ui.checkbox(&mut attack.is_landing_attack, "Direct");
            ui.checkbox(&mut attack.no_finish_camera, "Friendly fire");
        });
    });
    ui.collapsing("Collision filters", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Situation");
            constant_picker(
                ui,
                "attack_situation",
                &mut attack.situation_mask,
                crate::param_labels::SITUATION_MASK,
                false,
            );
            ui.label("Category");
            constant_picker(
                ui,
                "attack_category",
                &mut attack.category_mask,
                crate::param_labels::CATEGORY_MASK,
                false,
            );
            ui.label("Part");
            constant_picker(
                ui,
                "attack_part",
                &mut attack.part_mask,
                crate::param_labels::PART_MASK,
                false,
            );
        });
    });
    ui.collapsing("Hit feedback", |ui| {
        hit_feedback_ui(
            ui,
            &mut attack.collision_attr,
            &mut attack.sound_level,
            &mut attack.sound_attr,
            &mut attack.attack_region,
        );
    });
}

fn hit_feedback_ui(
    ui: &mut Ui,
    attribute: &mut String,
    sound_level: &mut String,
    sound: &mut String,
    region: &mut String,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Attribute");
        egui::ComboBox::from_id_salt("hit_attribute")
            .selected_text(
                attribute
                    .strip_prefix("collision_attr_")
                    .unwrap_or(attribute),
            )
            .show_ui(ui, |ui| {
                for name in crate::param_labels::COLLISION_ATTRS {
                    ui.selectable_value(
                        attribute,
                        (*name).to_owned(),
                        name.strip_prefix("collision_attr_").unwrap_or(name),
                    );
                }
            });
        ui.label("Sound");
        constant_picker(
            ui,
            "hit_sound_level",
            sound_level,
            crate::param_labels::SOUND_LEVEL,
            false,
        );
        constant_picker(
            ui,
            "hit_sound",
            sound,
            crate::param_labels::SOUND_ATTR,
            false,
        );
        ui.label("Region");
        constant_picker(
            ui,
            "hit_region",
            region,
            crate::param_labels::ATTACK_REGION,
            false,
        );
    });
}

fn velocity_ui(ui: &mut Ui, x: &mut f32, y: &mut f32, label: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        ui.label("X");
        ui.add(egui::DragValue::new(x).speed(0.1));
        ui.label("Y");
        ui.add(egui::DragValue::new(y).speed(0.1));
    });
}

fn volume_ui(
    ui: &mut Ui,
    joints: &[String],
    id: &mut u32,
    joint: &mut String,
    radius: &mut f32,
    offset: [&mut f32; 3],
    end: &mut Option<[f32; 3]>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("ID");
        ui.add(egui::DragValue::new(id).range(0..=255));
        joint_picker(ui, joint, joints);
        ui.label("Radius");
        ui.add(egui::DragValue::new(radius).range(0.0..=999.0).speed(0.1));
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Offset");
        let initial = [*offset[0], *offset[1], *offset[2]];
        for value in offset {
            ui.add(egui::DragValue::new(value).speed(0.1));
        }
        let mut capsule = end.is_some();
        if ui.checkbox(&mut capsule, "Capsule").changed() {
            *end = capsule.then_some(initial);
        }
        if let Some(end) = end {
            ui.label("End");
            for value in end {
                ui.add(egui::DragValue::new(value).speed(0.1));
            }
        }
    });
}

fn constant_picker(
    ui: &mut Ui,
    id: &str,
    token: &mut String,
    table: crate::param_labels::ConstTable,
    dereference: bool,
) {
    let current = token.trim().trim_start_matches('*');
    let prefix = table
        .first()
        .and_then(|(name, _)| name.rsplit_once('_').map(|(prefix, _)| prefix))
        .unwrap_or("");
    egui::ComboBox::from_id_salt(id)
        .selected_text(
            current
                .strip_prefix(prefix)
                .unwrap_or(current)
                .trim_start_matches('_')
                .replace('_', " "),
        )
        .show_ui(ui, |ui| {
            for (name, _) in table {
                let label = name
                    .strip_prefix(prefix)
                    .unwrap_or(name)
                    .trim_start_matches('_')
                    .replace('_', " ");
                let stored = if dereference {
                    format!("*{name}")
                } else {
                    (*name).to_owned()
                };
                ui.selectable_value(token, stored, label)
                    .on_hover_text(*name);
            }
        });
}

fn nodes_ui_body(
    ui: &mut Ui,
    fighter: &str,
    track: Track,
    body: &mut Vec<MoveNode>,
    next: &mut u32,
    options: CommandOptions<'_>,
) {
    ui.indent("repeat_body", |ui| {
        nodes(ui, fighter, track, body, next, options);
        if ui.button("Add wait").clicked() {
            body.push(MoveNode {
                id: *next,
                kind: NodeKind::Wait { frames: 1.0 },
            });
            *next = next.saturating_add(1);
        }
        ui.menu_button("Add control", |ui| {
            for (label, kind) in [
                ("Frame", NodeKind::Frame { frame: 1.0 }),
                (
                    "Repeat",
                    NodeKind::Repeat {
                        motion_loop: crate::data::MotionLoop::No,
                        count: 2,
                        body: Vec::new(),
                    },
                ),
                (
                    "Branch",
                    NodeKind::Branch {
                        condition: MoveCondition::Grounded,
                        then_nodes: Vec::new(),
                        else_nodes: Vec::new(),
                    },
                ),
            ] {
                if ui.button(label).clicked() {
                    body.push(MoveNode { id: *next, kind });
                    *next = next.saturating_add(1);
                    ui.close();
                }
            }
        });
        egui::containers::menu::MenuButton::new("Add command")
            .config(
                egui::containers::menu::MenuConfig::new()
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
            )
            .ui(ui, |ui| {
                let search_key = ui
                    .id()
                    .with(("nested_command_search", fighter, track as u8));
                let mut search = ui
                    .data_mut(|data| data.get_temp::<String>(search_key))
                    .unwrap_or_default();
                ui.add(
                    egui::TextEdit::singleline(&mut search)
                        .hint_text("Find a block…")
                        .desired_width(190.0),
                );
                let query = search.trim().to_ascii_lowercase();
                ui.data_mut(|data| data.insert_temp(search_key, search));
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .show(ui, |ui| {
                        if track == Track::Effect {
                            for (kind, label) in node_choices_for_fighter(Track::Effect, fighter)
                                .iter()
                                .filter(|(kind, _)| matches!(*kind, 7..=10 | 54..=62 | 87..=89))
                                .filter(|(kind, label)| palette_matches(*kind, label, &query))
                            {
                                if ui.button(*label).clicked() {
                                    body.push(MoveNode {
                                        id: *next,
                                        kind: NodeKind::EffectExecute {
                                            commands: vec![context_effect(*kind, fighter)],
                                        },
                                    });
                                    *next = next.saturating_add(1);
                                    ui.close();
                                }
                            }
                            return;
                        }
                        if matches!(track, Track::Sound | Track::Expression) {
                            for (kind, label) in node_choices_for_fighter(track, fighter)
                                .iter()
                                .filter(|(kind, _)| {
                                    *kind >= 11
                                        && !matches!(kind, 41 | 48 | 96..=99 | 106 | 107 | 121)
                                })
                                .filter(|(kind, label)| palette_matches(*kind, label, &query))
                            {
                                if ui.button(*label).clicked() {
                                    body.push(MoveNode {
                                        id: *next,
                                        kind: if *kind == 16 {
                                            NodeKind::BareCommand {
                                                command: Box::new(default_command(11)),
                                            }
                                        } else {
                                            NodeKind::Execute {
                                                commands: vec![context_command(*kind, fighter)],
                                            }
                                        },
                                    });
                                    *next = next.saturating_add(1);
                                    ui.close();
                                }
                            }
                            return;
                        }
                        for (kind, label) in node_choices_for_fighter(Track::Game, fighter)
                            .iter()
                            .filter(
                                |(kind, _)| matches!(*kind, 18..=49 | 52 | 53 | 64..=86 | 90..=92),
                            )
                            .filter(|(kind, label)| palette_matches(*kind, label, &query))
                        {
                            if ui.button(*label).clicked() {
                                body.push(MoveNode {
                                    id: *next,
                                    kind: context_node(*kind, fighter, *next),
                                });
                                *next = next.saturating_add(if *kind == 80 { 2 } else { 1 });
                                ui.close();
                            }
                        }
                        if palette_matches(101, "Send event", &query)
                            && ui
                                .add_enabled(
                                    super::chart_event_view::first(options.chart_events).is_some(),
                                    egui::Button::new("Send event"),
                                )
                                .on_disabled_hover_text(
                                    "Add an Event · Move event block to a controller first.",
                                )
                                .clicked()
                        {
                            let (chart_id, name) =
                                super::chart_event_view::first(options.chart_events)
                                    .expect("enabled chart receiver");
                            body.push(MoveNode {
                                id: *next,
                                kind: NodeKind::Execute {
                                    commands: vec![super::chart_event_view::command_for(
                                        options.chart_events,
                                        chart_id,
                                        name,
                                    )
                                    .expect("compiled receiver choice")],
                                },
                            });
                            *next = next.saturating_add(1);
                            ui.close();
                        }
                        for (kind, label, command) in [
                            (
                                5,
                                "Hitbox",
                                ExcuteStmt::Attack(crate::data::Hitbox::default().to_attack_call()),
                            ),
                            (2, "Clear hitboxes", ExcuteStmt::ClearAll),
                            (3, "Reverse facing", ExcuteStmt::ReverseLr),
                            (
                                17,
                                "Hurtbox status",
                                ExcuteStmt::HitStatus {
                                    target: crate::data::HurtTarget::Whole,
                                    status: "*HIT_STATUS_NORMAL".into(),
                                },
                            ),
                        ] {
                            if !palette_matches(kind, label, &query) {
                                continue;
                            }
                            if ui.button(label).clicked() {
                                body.push(MoveNode {
                                    id: *next,
                                    kind: NodeKind::Execute {
                                        commands: vec![command],
                                    },
                                });
                                *next = next.saturating_add(1);
                                ui.close();
                            }
                        }
                    });
            });
    });
}

fn wind_command_ui(ui: &mut Ui, wind: &mut crate::data::WindboxData) {
    const FORMATS: [(&str, &str); 3] = [
        ("AREA_WIND_2ND_RAD", "Radial"),
        ("AREA_WIND_2ND_RAD_arg9", "Radial with lifetime"),
        ("AREA_WIND_2ND_arg10", "Rectangle with lifetime"),
    ];
    let before = wind.command.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("Shape");
        egui::ComboBox::from_id_salt("wind_format")
            .selected_text(
                FORMATS
                    .iter()
                    .find(|(command, _)| *command == wind.command)
                    .map_or(wind.command.as_str(), |(_, label)| *label),
            )
            .show_ui(ui, |ui| {
                for (command, label) in FORMATS {
                    ui.selectable_value(&mut wind.command, command.into(), label)
                        .on_hover_text(command);
                }
            });
    });
    if wind.command != before {
        let old_lifetime = if (before == "AREA_WIND_2ND_RAD_arg9" && wind.args.len() == 9)
            || (before == "AREA_WIND_2ND_arg10" && wind.args.len() == 10)
        {
            wind.args.last().copied().unwrap_or(0.0)
        } else {
            0.0
        };
        let mut args = vec![0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 6.0, 10.0];
        // The existing wind payload places ID, native slots, offset and first
        // extent in the same positions. Preserve those values rather than
        // inventing a radius/width conversion or normalizing on inspection.
        for (value, previous) in args.iter_mut().zip(&wind.args) {
            *value = *previous;
        }
        if wind.command == "AREA_WIND_2ND_arg10" {
            args.push(10.0);
        }
        if wind.command != "AREA_WIND_2ND_RAD" {
            args.push(old_lifetime);
        }
        wind.args = args;
    }
    let radial = wind.is_radial();
    ui.horizontal_wrapped(|ui| {
        for (index, label) in [
            (0, "ID"),
            (5, "X"),
            (6, "Y"),
            (7, if radial { "Radius" } else { "Width" }),
        ] {
            if let Some(value) = wind.args.get_mut(index) {
                number_field_ui(ui, ("wind_property", label), label, value, 0.1, None);
            }
        }
        if !radial {
            if let Some(height) = wind.args.get_mut(8) {
                number_field_ui(ui, "wind_height", "Height", height, 0.1, None);
            }
        }
        if wind.has_lifetime() {
            number_field_ui(
                ui,
                "wind_lifetime",
                "Lifetime",
                wind.args.last_mut().unwrap(),
                1.0,
                Some("Native lifetime argument; not a character timer or wall-clock duration."),
            );
        }
    });
    ui.collapsing("Native arguments", |ui| {
        ui.horizontal_wrapped(|ui| {
            for index in 1..5 {
                if let Some(value) = wind.args.get_mut(index) {
                    number_field_ui(
                        ui,
                        ("wind_native_slot", index),
                        &format!("Slot {index}"),
                        value,
                        0.1,
                        None,
                    );
                }
            }
        });
    });
}

/// Keep each label and its value together when the inspector wraps to another row.
fn number_field_ui(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    label: &str,
    value: &mut f32,
    speed: f64,
    help: Option<&str>,
) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let label_width = ui
        .painter()
        .layout_no_wrap(label.into(), font, ui.visuals().text_color())
        .size()
        .x;
    let value_width = 56.0;
    let size = egui::vec2(
        label_width + ui.spacing().item_spacing.x + value_width,
        ui.spacing().interact_size.y,
    );
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.push_id(id, |ui| {
                let response =
                    ui.add(egui::Label::new(label).wrap_mode(egui::TextWrapMode::Extend));
                if let Some(help) = help {
                    response.on_hover_text(help);
                }
                ui.add_sized(
                    [value_width, ui.spacing().interact_size.y],
                    egui::DragValue::new(value).speed(speed),
                );
            });
        },
    );
}

/// Search aliases describe existing blocks; filtering by track and donor happens first.
fn native_execute_frame_ui(ui: &mut Ui, frame: &mut f32) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Native execute frame").on_hover_text("Exact sv_animcmd::execute on this agent's Lua state. Not an At motion frame wait, Execute guard, or timer. Direct root placement only;");
        ui.add(egui::DragValue::new(frame).range(0.0..=1_000_000.0));
    });
}

fn native_motion_loop_sync_ui(ui: &mut Ui) {
    ui.label("Sync motion loop").on_hover_text("Exact native motion-loop synchronization on this agent's Lua state. Not a Repeat loop, timer, or animation-finished event. Direct root Effect, Sound or Expression placement only; The desktop preview does not simulate this scheduler primitive.");
}

fn palette_matches(kind: usize, label: &str, query: &str) -> bool {
    let aliases = match kind {
        0 | 1 => "timeline timing delay",
        2 | 27 => "attack damage collision clear remove",
        5 => "attack damage collision",
        6 => "condition if otherwise trigger",
        15 => "animation playback speed",
        17 | 20 | 85 => "invulnerability intangible hurtbox normal reset",
        21..=23 => "velocity speed momentum physics",
        35 => "spawn projectile weapon article",
        120 => "charge projectile hitbox lerp blend weak strong",
        36 => "despawn projectile weapon article remove",
        38 => "model appearance presentation sword shield body visibility",
        41 => "projectile weapon animation",
        48 => "native attack reference joint bone direction axis expression source",
        101 => "custom event emit trigger dispatch signal controller chart target",
        72 | 73 => "native whole fighter slow slowdown",
        90 => "native slow slowdown opponent",
        91 => "optional hit feedback effect resource",
        92 => "optional hit feedback sound resource",
        93 => "throw finish camera cinematic",
        94 => "final smash camera cinematic startup",
        95 => "final smash fear face expression native argument",
        96 => "whole model visibility visible hidden show hide rendering",
        97 => "model default appearance group hash presentation",
        98 => "spawn projectile weapon article generate enabled native",
        99 => "projectile weapon article state status hookshot tether sword shoot rewind extend back native",
        100 => "native item shoot bullet firing held gun equipment source",
        102 => "start info flash eye eyes final smash visual presentation source",
        103 => "camera zoom out cinematic final presentation source",
        104 => "start cutin cut in cinematic final presentation source",
        105 => "grab capture cut common default catch absolute damage setup source",
        106 => "native execute frame timing animation source sync",
        107 => "native timing animation synchronize motion loop source",
        121 => "native timing rapid jab loop wait_loop_clear motion source",
        77 => "projectile weapon article visibility",
        78 | 79 => "projectile weapon article shoot launch",
        84 => "attack hitbox radius size",
        86 => "attack hitbox vector target position",
        _ => "",
    };
    let searchable = format!("{} {aliases}", label.to_ascii_lowercase());
    query
        .to_ascii_lowercase()
        .split_ascii_whitespace()
        .all(|word| searchable.contains(word))
}

fn node_choices(track: Track) -> &'static [(usize, &'static str)] {
    match track {
        Track::Game => &[
            (0, "Frame"),
            (106, "Native execute frame"),
            (121, "Wait for motion loop"),
            (1, "Wait"),
            (2, "Clear hitboxes"),
            (3, "Reverse facing"),
            (4, "Repeat"),
            (5, "Hitbox"),
            (6, "Branch"),
            (15, "Motion rate"),
            (17, "Hurtbox status"),
            (18, "Grab volume"),
            (19, "Clear grabs"),
            (20, "Reset hurtboxes"),
            (21, "Set velocity"),
            (22, "Add velocity (no limit)"),
            (23, "Clear velocity"),
            (24, "Set airborne"),
            (25, "Armor / damage reaction"),
            (26, "Modify hitbox power"),
            (52, "Reaction frames"),
            (66, "Reaction frames · revised"),
            (27, "Clear one hitbox"),
            (28, "Erase wind"),
            (29, "Search volume"),
            (30, "Wind area"),
            (31, "Define throw damage"),
            (53, "Apply throw"),
            (32, "Slope alignment"),
            (33, "Native flag"),
            (108, "Change status"),
            (120, "Interpolate attacks"),
            (114, "Shield setoff (three attacks)"),
            (115, "Shield setoff (four attacks)"),
            (109, "Enable safe position"),
            (110, "Disable attack tip"),
            (111, "Keep attack rumble"),
            (112, "Cliff hang profile"),
            (113, "Article part visibility"),
            (67, "Allow dash → run"),
            (74, "Allow transition"),
            (75, "Disable transition"),
            (76, "Allow transition group"),
            (68, "Item pull step"),
            (69, "Item pull offset limit"),
            (70, "Final wave angle"),
            (71, "Inherited open motion"),
            (72, "Set slowdown"),
            (73, "Clear slowdown"),
            (90, "Slow opponents"),
            (100, "Fire held item"),
            (102, "Eye flash"),
            (103, "Zoom camera out"),
            (104, "Start cut-in"),
            (105, "Default catch setup"),
            (101, "Send event"),
            (34, "Held item visibility"),
            (35, "Generate article"),
            (98, "Generate enabled article"),
            (99, "Article state"),
            (36, "Remove article"),
            (77, "Article visibility"),
            (78, "Launch article"),
            (79, "Launch if present"),
            (80, "Open/close sword"),
            (37, "Body collision"),
            (64, "Body collision area"),
            (65, "Body collision area XY"),
            (38, "Model visibility"),
            (96, "Whole model visibility"),
            (97, "Model default"),
            (39, "Attack no-dead mode"),
            (40, "Attack catch-only mode"),
            (41, "Article motion"),
            (42, "Grab rebound"),
            (43, "Attack finish camera"),
            (44, "Force attack reaction"),
            (45, "Attack height"),
            (46, "Final finish cut-in"),
            (47, "Initialize attack position"),
            (48, "Attack reference joint"),
            (49, "Damage fly smoke"),
            (81, "Damage shake scale"),
            (82, "Unique attack effects"),
            (83, "Attack target filter"),
            (84, "Hitbox size"),
            (85, "All hurtboxes normal"),
            (86, "Vector target"),
            (91, "Optional hit effect"),
            (92, "Optional hit sound"),
            (93, "Finish camera"),
            (94, "Final-start camera"),
            (95, "Final fear face"),
        ],
        Track::Effect => &[
            (0, "Frame"),
            (107, "Sync motion loop"),
            (106, "Native execute frame"),
            (1, "Wait"),
            (4, "Repeat"),
            (6, "Branch"),
            (7, "Effect spawn"),
            (8, "Effect rate"),
            (9, "Effect color"),
            (10, "Effect off"),
            (54, "Effect opacity"),
            (55, "Particle color"),
            (56, "Effect camera offset"),
            (57, "Effect scale"),
            (58, "Detach effect"),
            (59, "Disable effect render offset"),
            (60, "Sync effect initial position"),
            (61, "Fighter color"),
            (62, "End sword trail"),
            (116, "Fill screen model color"),
            (117, "Cancel screen fill"),
            (118, "Circle post effect"),
            (119, "Remove line post effect"),
            (87, "Screen effect"),
            (88, "Camera depth influence"),
            (89, "Native effect off"),
        ],
        Track::Sound => &[
            (0, "Frame"),
            (107, "Sync motion loop"),
            (1, "Wait"),
            (4, "Repeat"),
            (6, "Branch"),
            (11, "Play sound"),
            (12, "Stop sound"),
            (16, "Top-level sound"),
        ],
        Track::Expression => &[
            (0, "Frame"),
            (107, "Sync motion loop"),
            (121, "Wait for motion loop"),
            (106, "Native execute frame"),
            (1, "Wait"),
            (102, "Eye flash"),
            (4, "Repeat"),
            (6, "Branch"),
            (13, "Hit rumble"),
            (51, "Rumble pattern"),
            (14, "Camera shake"),
            (17, "Hurtbox status"),
            (20, "Reset hurtboxes"),
            (30, "Wind area"),
            (28, "Erase wind"),
            (32, "Slope alignment"),
            (48, "Attack reference joint"),
            (34, "Held item visibility"),
            (41, "Article motion"),
            (35, "Generate article"),
            (98, "Generate enabled article"),
            (99, "Article state"),
            (36, "Remove article"),
            (77, "Article visibility"),
            (79, "Launch if present"),
            (38, "Model visibility"),
            (96, "Whole model visibility"),
            (97, "Model default"),
        ],
    }
}

fn node_choices_for_fighter(track: Track, fighter: &str) -> Vec<(usize, &'static str)> {
    let has_articles = crate::gimmicks::registry::entries_for(
        fighter,
        crate::gimmicks::registry::IdentifierKind::Article,
    )
    .next()
    .is_some();
    node_choices(track)
        .iter()
        .copied()
        .filter(|(kind, _)| !matches!(kind, 35 | 36 | 77..=79) || has_articles)
        .filter(|(kind, _)| {
            *kind != 38
                || crate::data::body_visibility_catalog::pairs_for(fighter)
                    .next()
                    .is_some()
        })
        .filter(|(kind, _)| {
            *kind != 41
                || measured_motion_articles_on_track(fighter, track)
                    .next()
                    .is_some()
        })
        .filter(|(kind, _)| {
            !matches!(kind, 67..=69 | 72 | 73 | 83..=86 | 90..=92)
                || crate::gimmicks::registry::fighter_kind_symbol(fighter).is_some()
        })
        .filter(|(kind, _)| match kind {
            93 => crate::data::FINISH_CAMERA_PRESETS
                .iter()
                .any(|preset| preset.fighter == fighter),
            94 => crate::data::FINAL_START_CAMERA_PRESETS
                .iter()
                .any(|preset| preset.fighter == fighter),
            95 => crate::data::FINAL_FEAR_FACE_PRESETS
                .iter()
                .any(|preset| preset.fighter == fighter),
            97 => crate::data::PINNED_DEFAULT_INT64_GROUP_COUNTS
                .iter()
                .any(|(owner, _, _)| *owner == fighter),
            98 => crate::data::PINNED_ENABLED_ARTICLE_COUNTS
                .iter()
                .any(|(owner, _, _)| *owner == fighter),
            99 => crate::data::article_status_catalog::presets_for(fighter)
                .next()
                .is_some(),
            112 => crate::gimmicks::registry::fighter_kind_symbol(fighter).is_some(),
            113 => has_articles,
            108 => crate::gimmicks::registry::entries_for(
                fighter,
                crate::gimmicks::registry::IdentifierKind::FighterStatus,
            )
            .next()
            .is_some(),
            70 => fighter == "miiswordsman",
            71 => matches!(fighter, "eflame" | "elight"),
            80 => article_partial_presets(fighter).next().is_some(),
            89 => crate::data::EffectOffKindWorkPreset::all()
                .iter()
                .any(|preset| preset.fighter == fighter),
            74 | 75 => crate::gimmicks::registry::fighter_kind_symbol(fighter).is_some(),
            76 => super::native_work::source_transition_term_descriptors()
                .iter()
                .any(|descriptor| {
                    descriptor.fighter == fighter
                        && descriptor.action == transition_palette_action(*kind)
                }),
            _ => true,
        })
        .collect()
}

pub(super) fn context_command(kind: usize, fighter: &str) -> ExcuteStmt {
    if let Some(line) = match kind {
        120 => Some("attack!(agent, *MA_MSC_CMD_ATTACK_SET_LERP, 0, 1);".to_owned()),
        114 => Some("macros::ATK_SET_SHIELD_SETOFF_MUL_arg4(agent, 0, 1, 2, 1.0);".to_owned()),
        115 => Some("macros::ATK_SET_SHIELD_SETOFF_MUL_arg5(agent, 0, 1, 2, 3, 1.0);".to_owned()),
        116 => Some("macros::FILL_SCREEN_MODEL_COLOR(agent, 0, 0, 1, 1, 1, 0, 0, 0, 1, 1, 0, 200);".to_owned()),
        117 => Some("macros::CANCEL_FILL_SCREEN(agent, 0, 0);".to_owned()),
        118 => Some("EffectModule::request_post_effect_line_circle(agent.module_accessor, Hash40::new(\"sys_hit_normal\"), Hash40::new(\"top\"), smash::phx::simd::Vector2::new(1.0, 1.0), smash::phx::simd::Vector3::new(0.0, 0.0, 0.0), false, 1.0, 0.0);".to_owned()),
        119 => Some("EffectModule::remove_post_effect_line(agent.module_accessor, 0, true);".to_owned()),
        109 => Some("AttackModule::enable_safe_pos(agent.module_accessor);".to_owned()),
        110 => Some("AttackModule::disable_tip(agent.module_accessor);".to_owned()),
        111 => Some("AttackModule::set_attack_keep_rumble(agent.module_accessor, 0, true);".to_owned()),
        112 => Some("GroundModule::select_cliff_hangdata(agent.module_accessor, *FIGHTER_CLIFF_HANG_DATA_DEFAULT as u32);".to_owned()),
        113 => crate::gimmicks::registry::entries_for(fighter, crate::gimmicks::registry::IdentifierKind::Article).next()
            .map(|entry| format!("ArticleModule::set_visibility(agent.module_accessor, *{}, Hash40::new(\"set_main\"), Hash40::new(\"group_main_default\"), ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));", entry.symbol)),
        _ => None,
    } {
        return ExcuteStmt::NativeCall(super::native_calls::parse(&line).expect("typed palette call"));
    }
    if kind == 108 {
        return ExcuteStmt::NativeCall(super::native_calls::NativeCall {
            callee: "StatusModule::change_status_request_from_script".into(),
            arguments: vec!["*FIGHTER_STATUS_KIND_SPECIAL_HI".into(), "true".into()],
            receiver: None,
            binding: None,
            source_line: None,
        });
    }
    if kind == 99 {
        let preset = crate::data::article_status_catalog::presets_for(fighter)
            .next()
            .expect("article-state palette contains only measured donors");
        return ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::ChangeStatus {
            symbol: preset.article.into(),
            status: preset.status.into(),
            if_present: preset.if_present,
        });
    }
    if kind == 98 {
        let (_, symbol, _) = crate::data::PINNED_ENABLED_ARTICLE_COUNTS
            .iter()
            .find(|(owner, _, _)| *owner == fighter)
            .expect("enabled-generation palette contains only measured donors");
        return ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::GenerateEnabled {
            symbol: (*symbol).into(),
        });
    }
    if kind == 97 {
        let (_, group, _) = crate::data::PINNED_DEFAULT_INT64_GROUP_COUNTS
            .iter()
            .find(|(owner, _, _)| *owner == fighter)
            .expect("model-default palette contains only measured donors");
        return ExcuteStmt::VisibilitySetDefaultInt64 {
            group: (*group).into(),
        };
    }
    if kind == 38 {
        let (group, status) = crate::data::body_visibility_catalog::pairs_for(fighter)
            .next()
            .expect("model visibility palette contains only measured donors");
        return ExcuteStmt::VisibilitySetInt64 {
            group: group.into(),
            status: status.into(),
        };
    }
    if kind == 95 {
        let preset = crate::data::FINAL_FEAR_FACE_PRESETS
            .iter()
            .find(|preset| preset.fighter == fighter)
            .expect("final-fear-face palette contains only measured donors");
        return ExcuteStmt::FinalFearFace(crate::data::FinalFearFaceCall {
            argument: preset.argument.into(),
        });
    }
    if kind == 94 {
        let preset = crate::data::FINAL_START_CAMERA_PRESETS
            .iter()
            .find(|preset| preset.fighter == fighter)
            .expect("final-start-camera palette contains only measured donors");
        return ExcuteStmt::CheckFinalStartCamera(crate::data::FinalStartCameraCall {
            args: preset.args.map(str::to_owned),
        });
    }
    if kind == 93 {
        let preset = crate::data::FINISH_CAMERA_PRESETS
            .iter()
            .find(|preset| preset.fighter == fighter)
            .expect("finish-camera palette contains only measured donors");
        return ExcuteStmt::CheckFinishCamera(crate::data::FinishCameraCall {
            arg1: preset.arg1.into(),
            arg2: preset.arg2.into(),
        });
    }
    if kind == 80 {
        return ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::AddMotionPartial {
            preset: article_partial_presets(fighter)
                .next()
                .expect("palette contains only measured sword owners"),
        });
    }
    if matches!(kind, 74..=76) {
        let action = transition_palette_action(kind);
        // A new block starts on a real choice, never on the internal dash-to-run term.
        let term = super::native_work::source_transition_term_choices(fighter)
            .into_iter()
            .find(|descriptor| {
                descriptor.action == action
                    && descriptor.transition_term != super::native_work::DASH_TO_RUN_TERM
            })
            .map_or("", |descriptor| descriptor.transition_term);
        return ExcuteStmt::WorkTransitionTerm(crate::data::WorkTransitionTermCall {
            action,
            receiver: crate::data::WorkTransitionTermReceiver::AgentAccessor,
            transition_term: term.into(),
        });
    }
    if matches!(kind, 70 | 71) {
        let setter_kind = if kind == 70 {
            crate::data::WorkModuleSetKind::Float
        } else {
            crate::data::WorkModuleSetKind::Int64
        };
        let descriptor = super::native_work::source_work_module_set_descriptors()
            .iter()
            .find(|descriptor| descriptor.fighter == fighter && descriptor.kind == setter_kind);
        return ExcuteStmt::WorkModuleSet(crate::data::WorkModuleSetCall {
            kind: setter_kind,
            receiver: "agent.module_accessor".into(),
            value: descriptor
                .map_or("", |descriptor| descriptor.values[0])
                .into(),
            slot: descriptor.map_or("", |descriptor| descriptor.slot).into(),
        });
    }
    if kind == 41 {
        let symbol = measured_motion_articles(fighter)
            .next()
            .map_or_else(String::new, |entry| entry.symbol.to_owned());
        let motion = crate::data::measured_article_motions(&symbol)
            .first()
            .copied()
            .unwrap_or_default()
            .to_owned();
        return ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::ChangeMotion {
            symbol,
            motion,
        });
    }
    if matches!(kind, 35 | 36 | 77..=79) {
        let symbol = super::plain::default_object(fighter);
        use crate::data::{ArticleLifecycleCall, ArticleTarget};
        return ExcuteStmt::ArticleLifecycle(match kind {
            35 => ArticleLifecycleCall::Generate { symbol },
            36 => ArticleLifecycleCall::RemoveAll { symbol },
            77 => ArticleLifecycleCall::SetVisibilityWhole {
                symbol,
                visible: true,
            },
            78 => ArticleLifecycleCall::Shoot {
                symbol,
                target: ArticleTarget::All,
            },
            79 => ArticleLifecycleCall::ShootExist {
                symbol,
                target: ArticleTarget::All,
            },
            _ => unreachable!("closed article palette"),
        });
    }
    default_command(kind)
}

fn article_partial_presets(
    fighter: &str,
) -> impl Iterator<Item = crate::data::ArticlePartialMotionPreset> + '_ {
    crate::data::ArticlePartialMotionPreset::ALL
        .into_iter()
        .filter(move |preset| preset.fighter() == fighter)
}

/// Build the command's native execution shape, retaining its required source guards.
fn context_node_on_track(kind: usize, fighter: &str, id: u32, track: Track) -> NodeKind {
    if kind == 41 && track == Track::Expression {
        let symbol = measured_motion_articles_on_track(fighter, track)
            .next()
            .map_or_else(String::new, |entry| entry.symbol.to_owned());
        let motion = article_motions_on_track(track, &symbol)
            .first()
            .copied()
            .unwrap_or_default()
            .to_owned();
        return NodeKind::Execute {
            commands: vec![ExcuteStmt::ArticleLifecycle(
                crate::data::ArticleLifecycleCall::ChangeMotion { symbol, motion },
            )],
        };
    }
    context_node(kind, fighter, id)
}

fn context_node(kind: usize, fighter: &str, id: u32) -> NodeKind {
    if kind == 107 {
        return NodeKind::NativeMotionLoopSync;
    }
    if kind == 121 {
        return NodeKind::NativeWaitLoopClear;
    }
    if kind == 106 {
        return NodeKind::NativeExecuteFrame { frame: 1.0 };
    }
    if kind == 105 {
        return NodeKind::BareCommand {
            command: Box::new(ExcuteStmt::CaptureCutCommon),
        };
    }
    let command = context_command(kind, fighter);
    if let ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::AddMotionPartial {
        preset,
    }) = &command
    {
        NodeKind::Branch {
            condition: MoveCondition::ArticleExists {
                symbol: preset.article_symbol().into(),
            },
            then_nodes: vec![MoveNode {
                id: id.saturating_add(1),
                kind: NodeKind::Execute {
                    commands: vec![command],
                },
            }],
            else_nodes: Vec::new(),
        }
    } else {
        NodeKind::Execute {
            commands: vec![command],
        }
    }
}

fn measured_motion_articles(
    fighter: &str,
) -> impl Iterator<Item = &'static crate::gimmicks::registry::RegistryEntry> + '_ {
    measured_motion_articles_on_track(fighter, Track::Game)
}

fn article_motions_on_track(track: Track, symbol: &str) -> &'static [&'static str] {
    match track {
        Track::Game => crate::data::measured_article_motions(symbol),
        Track::Expression => crate::data::measured_expression_article_motions(symbol),
        Track::Sound | Track::Effect => &[],
    }
}

fn measured_motion_articles_on_track(
    fighter: &str,
    track: Track,
) -> impl Iterator<Item = &'static crate::gimmicks::registry::RegistryEntry> + '_ {
    crate::gimmicks::registry::entries_for(
        fighter,
        crate::gimmicks::registry::IdentifierKind::Article,
    )
    .filter(move |entry| !article_motions_on_track(track, entry.symbol).is_empty())
}

pub(super) fn default_command(kind: usize) -> ExcuteStmt {
    use crate::data::{ExpressionCall, SoundCall};
    match kind {
        5 => ExcuteStmt::Attack(crate::data::Hitbox::default().to_attack_call()),
        17 => ExcuteStmt::HitStatus {
            target: crate::data::HurtTarget::Whole,
            status: "*HIT_STATUS_NORMAL".into(),
        },
        11 | 12 => ExcuteStmt::Sound(SoundCall {
            func: if kind == 11 { "PLAY_SE" } else { "STOP_SE" }.into(),
            sounds: vec!["se_common_swing_01".into()],
            tail: None,
        }),
        13 => ExcuteStmt::Expression(ExpressionCall::RumbleHit {
            kind: "Hash40::new(\"rbkind_attackm\")".into(),
            unk: "0".into(),
        }),
        14 => ExcuteStmt::Expression(ExpressionCall::Quake {
            kind: "*CAMERA_QUAKE_KIND_M".into(),
        }),
        51 => ExcuteStmt::Expression(ExpressionCall::ControlModuleSetRumble {
            receiver: "agent.module_accessor".into(),
            kind: "Hash40::new(\"rbkind_nohitm\")".into(),
            duration: "0".into(),
            looped: "false".into(),
            target: "*BATTLE_OBJECT_ID_INVALID as u32".into(),
        }),
        18 => ExcuteStmt::Catch(crate::data::CatchCall {
            id: 0,
            bone_name: "top".into(),
            size: 3.0,
            offset_x: 0.0,
            offset_y: 6.0,
            offset_z: 5.0,
            capsule_end: None,
            status: "FIGHTER_STATUS_KIND_CAPTURE_PULLED".into(),
            situation: "COLLISION_SITUATION_MASK_GA".into(),
        }),
        19 => ExcuteStmt::GrabClearAll,
        20 => ExcuteStmt::HitResetAll,
        21 => ExcuteStmt::SetSpeed(crate::data::SetSpeedCall {
            speed_x: 0.0,
            speed_y: 0.0,
        }),
        22 => ExcuteStmt::AddSpeedNoLimit(crate::data::AddSpeedNoLimitCall {
            speed_x: 0.0,
            speed_y: 0.0,
        }),
        23 => ExcuteStmt::KineticClearSpeedAll,
        24 => ExcuteStmt::SetAir,
        25 => ExcuteStmt::DamageNoReaction(crate::data::DamageNoReactionCall {
            command: "*MA_MSC_DAMAGE_DAMAGE_NO_REACTION".into(),
            mode: "*DAMAGE_NO_REACTION_MODE_NORMAL".into(),
            value: "0.0".into(),
        }),
        26 => ExcuteStmt::AttackMod {
            kind: crate::data::AttackModKind::Power,
            id: 0,
            value: 10.0,
        },
        27 => ExcuteStmt::Clear(0),
        28 => ExcuteStmt::EraseWind(0),
        29 => ExcuteStmt::Search(crate::data::SearchCall {
            id: 0,
            part: 0,
            bone_name: "top".into(),
            size: 3.0,
            offset_x: 0.0,
            offset_y: 6.0,
            offset_z: 5.0,
            capsule_end: None,
            situation_mask: "COLLISION_SITUATION_MASK_GA".into(),
            category_mask: "COLLISION_CATEGORY_MASK_FIGHTER".into(),
            part_mask: "COLLISION_PART_MASK_ALL".into(),
            extras: crate::data::SearchExtras {
                collision_kind: "COLLISION_KIND_MASK_HIT".into(),
                hit_status: "HIT_STATUS_MASK_ALL".into(),
                unk: 0,
                unk2: false,
            },
        }),
        30 => ExcuteStmt::Wind(crate::data::WindboxData {
            command: "AREA_WIND_2ND_RAD".into(),
            args: vec![0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 6.0, 10.0],
        }),
        31 => ExcuteStmt::AttackAbs(crate::data::AttackAbsCall {
            kind: "FIGHTER_ATTACK_ABSOLUTE_KIND_THROW".into(),
            id: 0,
            damage: 5.0,
            angle: 45,
            kb_scaling: 100,
            fkb: 0,
            kb_base: 50,
            hitlag_mult: 1.0,
            lr_check: "ATTACK_LR_CHECK_F".into(),
            collision_attr: "collision_attr_normal".into(),
            sound_level: "ATTACK_SOUND_LEVEL_S".into(),
            sound_attr: "COLLISION_SOUND_ATTR_NONE".into(),
            attack_region: "ATTACK_REGION_THROW".into(),
            unknowns: (0.0, 0.0, true),
        }),
        32 => ExcuteStmt::Slope(crate::data::SlopeCall::Set {
            status: crate::data::SlopeStatus::Lr,
        }),
        33 => ExcuteStmt::WorkFlag(crate::data::WorkFlagCall {
            action: crate::data::WorkFlagAction::On,
            flag: "*FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO".into(),
        }),
        34 => ExcuteStmt::ItemVisibility { visible: true },
        100 => ExcuteStmt::ShootItemBullet,
        102 => ExcuteStmt::StartInfoFlashEye,
        103 => ExcuteStmt::CameraZoomOut,
        104 => ExcuteStmt::StartCutIn,
        105 => ExcuteStmt::CaptureCutCommon,
        37 => ExcuteStmt::JostleSetStatus { enabled: true },
        64 => ExcuteStmt::FixedJostleArea(crate::data::FixedJostleAreaCall::EnableFix {
            arg2: 5.0,
            arg3: 6.5,
        }),
        65 => ExcuteStmt::FixedJostleArea(crate::data::FixedJostleAreaCall::EnableFixXy {
            arg2: 2.0,
            arg3: 3.0,
            arg4: 3.0,
            arg5: 5.0,
        }),
        42 => ExcuteStmt::GrabRebound { enabled: true },
        45 => ExcuteStmt::AttackModuleSetHeightAll {
            height: crate::data::AttackHeightAllKind::Low,
            arg3: false,
        },
        46 => ExcuteStmt::AttackModuleSetFinalFinishCutIn {
            id: 0,
            arg3: true,
            arg4: false,
            arg5: -1.0,
            arg6: false,
        },
        47 => ExcuteStmt::AttackModuleInitAttackPos { id: 0 },
        48 => ExcuteStmt::AttackModuleSetAttackReferenceJointId {
            kind: crate::data::AttackReferenceJointKind::TopYZX,
        },
        49 => ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll {
            arg2: true,
            arg3: false,
        },
        81 => ExcuteStmt::AttackModuleSetDamageShakeScale(
            crate::data::AttackModuleDamageShakeScale::PointFive,
        ),
        82 => ExcuteStmt::AttackModuleSetNoUniqEffectAll(
            crate::data::AttackModuleNoUniqEffectAllPair::TrueFalse,
        ),
        83 => ExcuteStmt::AttackModuleSetTargetCategory(
            crate::data::AttackModuleTargetCategoryId::Zero,
        ),
        84 => ExcuteStmt::AttackModuleSetSize(crate::data::AttackModuleSizePreset::Id0Size0_1),
        85 => ExcuteStmt::HitModuleSetStatusAllNormal,
        86 => ExcuteStmt::AttackModuleSetVecTargetPos(
            crate::data::AttackModuleVecTargetPosPreset::ALL[0],
        ),
        52 => ExcuteStmt::AttackModuleAddReactionFrame { id: 0, frames: 3.0 },
        66 => ExcuteStmt::AttackModuleAddReactionFrameRevised { id: 0, frames: 3.0 },
        67 => ExcuteStmt::WorkTransitionTerm(crate::data::WorkTransitionTermCall {
            receiver: Default::default(),
            action: crate::data::WorkTransitionTermAction::Enable,
            transition_term: super::native_work::DASH_TO_RUN_TERM.into(),
        }),
        68 | 69 => ExcuteStmt::WorkModuleSet(crate::data::WorkModuleSetCall {
            kind: crate::data::WorkModuleSetKind::Int,
            receiver: "agent.module_accessor".into(),
            value: if kind == 68 {
                super::native_work::ITEM_GRASS_PULL_STEP_VALUES[0]
            } else {
                super::native_work::THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_VALUE
            }
            .into(),
            slot: if kind == 68 {
                super::native_work::ITEM_GRASS_PULL_NEXT_STEP_SLOT
            } else {
                super::native_work::THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT
            }
            .into(),
        }),
        70 => context_command(kind, "miiswordsman"),
        71 => context_command(kind, "eflame"),
        72 => {
            let pair = crate::data::SLOW_WHOLE_SET_PAIRS[0];
            ExcuteStmt::SlowWhole(crate::data::SlowWholeCall::Set {
                multiplier: pair.multiplier,
                frames: pair.frames,
            })
        }
        73 => ExcuteStmt::SlowWhole(crate::data::SlowWholeCall::Clear),
        90 => ExcuteStmt::SlowOpponent(crate::data::SlowOpponentCall {
            slow_mul: 13.0,
            slow_time: 60.0,
        }),
        91 | 92 => {
            let kind = if kind == 91 {
                crate::data::OptionalHitFeedbackKind::Effect
            } else {
                crate::data::OptionalHitFeedbackKind::Sound
            };
            ExcuteStmt::OptionalHitFeedback(crate::data::OptionalHitFeedbackCall {
                kind,
                arg2: 0,
                hash: crate::data::Hash40Literal::Name(optional_feedback_names(kind)[0].into()),
            })
        }
        53 => ExcuteStmt::AttackAbsApplyThrow,
        43 | 44 => ExcuteStmt::AttackModuleSetIndexed {
            kind: if kind == 43 {
                crate::data::AttackModuleSetIndexedKind::NoFinishCamera
            } else {
                crate::data::AttackModuleSetIndexedKind::ForceReaction
            },
            id: 0,
            arg3: true,
            arg4: false,
        },
        38 => ExcuteStmt::VisibilitySetInt64 {
            group: "body".into(),
            status: "body_normal".into(),
        },
        96 => ExcuteStmt::VisibilitySetWhole { visible: true },
        97 => ExcuteStmt::VisibilitySetDefaultInt64 {
            group: "panel".into(),
        },
        98 => ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::GenerateEnabled {
            symbol: "FIGHTER_KOOPAJR_GENERATE_ARTICLE_HAMMER".into(),
        }),
        99 => ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::ChangeStatus {
            symbol: "FIGHTER_MASTER_GENERATE_ARTICLE_SWORD".into(),
            status: "WEAPON_MASTER_SWORD_STATUS_KIND_EXTEND".into(),
            if_present: false,
        }),
        39 | 40 => ExcuteStmt::AttackModuleSetAll {
            kind: if kind == 39 {
                crate::data::AttackModuleSetAllKind::NoDeadAll
            } else {
                crate::data::AttackModuleSetAllKind::CatchOnlyAll
            },
            arg2: true,
            arg3: false,
        },
        _ => unreachable!("not a sound/expression node choice"),
    }
}

fn sound_command_ui(ui: &mut Ui, sound: &mut crate::data::SoundCall) {
    const FUNCTIONS: &[&str] = &[
        "PLAY_SE",
        "PLAY_SE_NO_3D",
        "PLAY_SE_REMAIN",
        "STOP_SE",
        "PLAY_STEP",
        "PLAY_STEP_FLIPPABLE",
        "PLAY_SEQUENCE",
        "PLAY_STATUS",
        "PLAY_LANDING_SE",
        "PLAY_DOWN_SE",
        "PLAY_FLY_VOICE",
        "SET_PLAY_INHIVIT",
    ];
    ui.horizontal_wrapped(|ui| {
        let previous = sound.func.clone();
        egui::ComboBox::from_id_salt("sound_function").selected_text(&sound.func).show_ui(ui, |ui| {
            for function in FUNCTIONS {
                ui.selectable_value(&mut sound.func, (*function).into(), *function);
            }
        });
        if sound.func != previous {
            normalize_sound_shape(sound);
        }
        for (index, name) in sound.sounds.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.label(if index == 0 { "Sound" } else { "Alternate" });
                ui.add(egui::TextEdit::singleline(name).desired_width(150.0))
                    .on_hover_text("Asset name from the selected character or common sound bank. The node compiler checks syntax, not whether the asset exists.");
            });
        }
        if let Some(tail) = &mut sound.tail {
            let mut frames = tail.parse::<f32>().ok().filter(|value| value.is_finite());
            if let Some(frames) = &mut frames {
                ui.label("Suppress frames");
                if ui.add(egui::DragValue::new(frames).range(0.0..=10000.0)).changed() {
                    *tail = frames.to_string();
                }
            } else { ui.label("Imported duration").on_hover_text(tail.as_str()); }
        }
    });
}

fn normalize_sound_shape(sound: &mut crate::data::SoundCall) {
    let count = if matches!(
        sound.func.as_str(),
        "PLAY_STEP_FLIPPABLE" | "PLAY_FLY_VOICE"
    ) {
        2
    } else {
        1
    };
    let fallback = sound
        .sounds
        .first()
        .cloned()
        .unwrap_or_else(|| "se_common_swing_01".into());
    sound.sounds.resize(count, fallback);
    if sound.func == "SET_PLAY_INHIVIT" {
        if sound.tail.is_none() {
            sound.tail = Some("5".into());
        }
    } else {
        sound.tail = None;
    }
}

fn native_work_picker(
    ui: &mut Ui,
    fighter: &str,
    token: &mut String,
    kind: crate::gimmicks::registry::IdentifierKind,
) {
    work_symbol_picker(ui, "native_work", fighter, token, kind, true);
}

fn work_symbol_picker(
    ui: &mut Ui,
    id: &str,
    fighter: &str,
    token: &mut String,
    kind: crate::gimmicks::registry::IdentifierKind,
    dereference: bool,
) {
    work_symbol_choices_picker(
        ui,
        id,
        token,
        kind,
        dereference,
        &native_work_choices(fighter, kind),
    );
}

fn work_symbol_choices_picker(
    ui: &mut Ui,
    id: &str,
    token: &mut String,
    kind: crate::gimmicks::registry::IdentifierKind,
    dereference: bool,
    choices: &[&str],
) {
    let key = ui.id().with((id, kind, "search"));
    let mut search = ui
        .data_mut(|data| data.get_temp::<String>(key))
        .unwrap_or_default();
    let current = token.trim().trim_start_matches('*');
    egui::ComboBox::from_id_salt((id, kind))
        .selected_text(if current.is_empty() {
            if kind == crate::gimmicks::registry::IdentifierKind::Article {
                "Choose article…"
            } else {
                "Choose native value…"
            }
        } else if kind == crate::gimmicks::registry::IdentifierKind::Article {
            short_symbol(current)
        } else {
            common_flag_label(current)
        })
        .width(200.0)
        .truncate()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut search).hint_text("Find native value…"));
            let query = search.trim().to_ascii_lowercase();
            for &symbol in choices {
                let label = (if kind == crate::gimmicks::registry::IdentifierKind::Article {
                    short_symbol(symbol)
                } else {
                    common_flag_label(symbol)
                })
                .replace('_', " ")
                .to_lowercase();
                if !format!("{label} {symbol}")
                    .to_ascii_lowercase()
                    .contains(&query)
                {
                    continue;
                }
                let value = if dereference {
                    format!("*{symbol}")
                } else {
                    symbol.to_owned()
                };
                if ui
                    .selectable_value(token, value, label)
                    .on_hover_text(symbol)
                    .clicked()
                {
                    ui.close();
                }
            }
        });
    ui.data_mut(|data| data.insert_temp(key, search));
}

fn native_work_choices(
    fighter: &str,
    kind: crate::gimmicks::registry::IdentifierKind,
) -> Vec<&'static str> {
    use crate::gimmicks::registry;
    let mut symbols = Vec::new();
    if kind == registry::IdentifierKind::WorkFlag
        && registry::fighter_kind_symbol(fighter).is_some()
    {
        for symbol in super::native_work::common_flags() {
            symbols.push(*symbol);
        }
        symbols.extend(super::native_status_flags::choices(fighter));
    }
    for entry in registry::entries_for(fighter, kind) {
        if !symbols.contains(&entry.symbol) {
            symbols.push(entry.symbol);
        }
    }
    symbols
}

fn native_condition_int_sources(fighter: &str) -> Vec<&'static str> {
    let mut sources =
        native_work_choices(fighter, crate::gimmicks::registry::IdentifierKind::WorkInt);
    if crate::gimmicks::registry::fighter_kind_symbol(fighter).is_some() {
        sources.splice(
            0..0,
            super::native_conditions::common_int_reads().iter().copied(),
        );
        sources.extend(super::native_conditions::status_int_reads(fighter));
    }
    sources
}

fn native_condition_flag_sources(fighter: &str) -> Vec<&'static str> {
    let mut sources =
        native_work_choices(fighter, crate::gimmicks::registry::IdentifierKind::WorkFlag);
    sources.extend(super::native_work::read_only_condition_flag_choices(
        fighter,
    ));
    sources
}

fn common_flag_label(symbol: &str) -> &str {
    match symbol {
        "FIGHTER_INSTANCE_WORK_ID_INT_KIND" => "Native fighter kind",
        "FIGHTER_INSTANCE_WORK_ID_INT_COLOR" => "Native color ID",
        "FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA" => "Final-start camera disabled",
        "FIGHTER_STATUS_GENESIS_SHOOT_FLAG_TURN" => "Genesis turn flag",
        "FIGHTER_STATUS_JUMP_FLAG_GIMMICK_SPRING_JUMP_FROM_RING" => "Spring jump from ring flag",
        "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO" => "Attack combo",
        "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_RESTART" => "Attack restart",
        "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_NO_HIT_COMBO" => "Attack combo without hit",
        "FIGHTER_STATUS_ATTACK_AIR_FLAG_ENABLE_LANDING" => "Air attack landing",
        "FIGHTER_STATUS_CATCH_FLAG_CATCH_WAIT" => "Grab wait",
        "FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD" => "Attack smash-hold flag",
        "FIGHTER_STATUS_ATTACK_FLAG_SMASH_SMASH_HOLD_TO_ATTACK" => "Smash hold to attack",
        "FIGHTER_IKE_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL"
        | "FIGHTER_ROY_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL" => "Neutral special ready level",
        "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_CHANGE_STATUS_DLAY_MOTION" => {
            "Status-change delay-motion flag"
        }
        "FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECK" => "Rapid-jab continue-check flag",
        "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_FFLOWER_SHOOT" => "Fire Flower shoot flag",
        "FIGHTER_STATUS_ITEM_SWING_WORK_FLAG_START_SMASH_HOLD" => "Item smash-hold flag",
        "FIGHTER_STATUS_JUMP_FLY_NEXT" => "Next flying-jump flag",
        "FIGHTER_STATUS_RUN_BRAKE_FLAG_STOP" => "Run brake stop",
        "FIGHTER_STATUS_GENESIS_SHOOT_FLAG_REQUEST_RADIATING" => "Genesis radiating request",
        "FIGHTER_STATUS_RUN_BRAKE_FLAG_TURN_RUN" => "Run brake turn-run",
        "FIGHTER_STATUS_DASH_FLAG_TURN_DASH" => "Turn dash",
        "FIGHTER_STATUS_ESCAPE_AIR_FLAG_SLIDE_ENABLE_CONTROL" => "Sliding air dodge control",
        "FIGHTER_STATUS_ITEM_GRASS_PULL_FLAG_STTUS_END_IF_PICKUP_FAILED" => {
            "Grass pull end if pickup failed"
        }
        "FIGHTER_STATUS_ITEM_SPECIALFLAG_HOIST_WORK_FLAG_HOIST_START" => "Flag hoist start",
        "FIGHTER_STATUS_ESCAPE_AIR_FLAG_SLIDE_ENABLE_GRAVITY" => "Sliding air dodge gravity",
        "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_100" => "Rapid jab enable",
        "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_SHOOT" => "Super Scope shoot flag",
        "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_SOUND" => {
            "Super Scope aerial sound request"
        }
        "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_EFFECT" => {
            "Super Scope aerial effect request"
        }
        "FIGHTER_STATUS_SQUAT_FLAG_REQUEST_SQUAT_SE" => "Crouch sound request",
        "FIGHTER_STATUS_VISUAL_SCENE_FLAG_END_EXIT" => "Visual scene end-exit flag",
        "FIGHTER_STATUS_SUPER_JUMP_PUNCH_FLAG_MOVE_TRANS" => "Super jump-punch transition flag",
        "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_GRAVITY_STABLE_UNABLE" => {
            "Gravity-stability reserve flag"
        }
        "FIGHTER_STATUS_ITEM_LIFT_WORK_FLAG_STOP" => "Item lift stop flag",
        "FIGHTER_STATUS_AIR_LASSO_FLAG_CHECK" => "Air lasso check flag",
        "FIGHTER_STATUS_SUPER_JUMP_PUNCH_FLAG_REVERSE_LR" => "Super jump-punch facing flag",
        "FIGHTER_STATUS_VISUAL_SCENE_FLAG_ABS_SET" => "Visual scene absolute-set flag",
        "FIGHTER_STATUS_AIR_LASSO_FLAG_LANDING" => "Air lasso landing flag",
        "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_LANDING_ATTACK_AIR_SQUAT" => {
            "Air-attack landing crouch reserve"
        }
        "FIGHTER_STATUS_STOP_CEIL_FLAG_FALL" => "Ceiling stop fall flag",
        "FIGHTER_INSTANCE_WORK_ID_FLAG_BACKSHIELD_INVISIBLE" => "Back shield visibility flag",
        "FIGHTER_INSTANCE_WORK_ID_FLAG_NO_SPEED_OPERATION_CHK" => "Native speed-check flag",
        "FIGHTER_INSTANCE_WORK_ID_FLAG_REVERSE_LR_FINISH_CAMERA_THROW_ORBIT" => {
            "Throw finish-camera orbit flag"
        }
        "FIGHTER_INSTANCE_WORK_ID_FLAG_FORCE_LOUPE" => "Force loupe flag",
        "FIGHTER_STATUS_ATTACK_AIR_FLAG_LANDING_CLEAR_SPEED" => "Air attack landing clear speed",
        "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_ATTACK_DISABLE_MINI_JUMP_ATTACK" => {
            "Reserve disable short-hop attack"
        }
        "FIGHTER_STATUS_THROW_FLAG_START_AIR" => "Throw start in air",
        "FIGHTER_STATUS_ATTACH_WALL_FLAG_MOT_STOP" => "Wall attachment motion stop",
        "FIGHTER_STATUS_TURN_RUN_FLAG_STOP" => "Turn-run stop",
        "FIGHTER_STATUS_THROW_FLAG_STOP" => "Throw stop flag",
        "FIGHTER_STATUS_ATTACK_AIR_FLAG_KEEP_AIR" => "Air attack keep airborne",
        "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO_INPUT" => "Attack combo input",
        "FIGHTER_STATUS_AIR_LASSO_FLAG_HANG_IMMIDIATE" => "Air lasso immediate hang",
        "FIGHTER_STATUS_TURN_RUN_FLAG_TO_RUN" => "Turn-run to run",
        _ => short_symbol(symbol),
    }
}

fn fixed_jostle_area_ui(ui: &mut Ui, call: &mut crate::data::FixedJostleAreaCall) {
    use crate::data::FixedJostleAreaCall;
    let (label, values): (&str, Vec<&mut f32>) = match call {
        FixedJostleAreaCall::EnableFix { arg2, arg3 } => ("Body collision area", vec![arg2, arg3]),
        FixedJostleAreaCall::EnableFixXy {
            arg2,
            arg3,
            arg4,
            arg5,
        } => ("Body collision area XY", vec![arg2, arg3, arg4, arg5]),
    };
    ui.label(label).on_hover_text(
        "Fixed native fighter body-collision area, not an attack hitbox. The two native layouts stay distinct; their argument meanings are not inferred from the method name.",
    );
    ui.horizontal_wrapped(|ui| {
        for (index, value) in values.into_iter().enumerate() {
            number_field_ui(
                ui,
                ("fixed_jostle_area", index),
                &format!("Value {}", index + 1),
                value,
                0.1,
                Some("Ordered native f32 argument after the fighter accessor. Finite values within ±10000;"),
            );
        }
    });
    ui.weak("?").on_hover_text(
        "Reconstructs enable_fix_jostle_area or enable_fix_jostle_area_xy. This does not enable jostling, create an attack volume, or publish a flat live override.",
    );
}

/// Native command properties remain typed; no arbitrary source input is exposed.
fn transition_palette_action(kind: usize) -> crate::data::WorkTransitionTermAction {
    use crate::data::WorkTransitionTermAction;
    match kind {
        75 => WorkTransitionTermAction::Unable,
        76 => WorkTransitionTermAction::EnableGroup,
        _ => WorkTransitionTermAction::Enable,
    }
}

fn transition_permission_ui(
    ui: &mut Ui,
    fighter: &str,
    call: &mut crate::data::WorkTransitionTermCall,
) {
    use crate::data::WorkTransitionTermAction;
    let operation = match call.action {
        WorkTransitionTermAction::Enable => "Allow transition",
        WorkTransitionTermAction::Unable => "Disable transition",
        WorkTransitionTermAction::EnableGroup => "Allow transition group",
        WorkTransitionTermAction::UnableGroupEx => "Disable transition group",
    };
    let accepted = super::native_work::validate_source_transition_term(fighter, call).is_ok();
    let choices: Vec<_> = super::native_work::source_transition_term_choices(fighter)
        .into_iter()
        .filter(|descriptor| descriptor.action == call.action)
        .collect();
    let label = choices
        .iter()
        .find(|descriptor| descriptor.transition_term == call.transition_term)
        .map_or("Not available for this fighter", |descriptor| {
            descriptor.label
        });
    ui.label(super::plain::block_name(operation));
    if accepted {
        egui::ComboBox::from_id_salt("transition_permission")
            .width(220.0)
            .selected_text(label)
            .show_ui(ui, |ui| {
                for descriptor in choices {
                    ui.selectable_value(
                        &mut call.transition_term,
                        descriptor.transition_term.into(),
                        descriptor.label,
                    );
                }
            });
    } else {
        ui.weak("Kept as the game wrote it");
    }
    ui.weak("?").on_hover_text(
        "Changes native transition permission at this point in the Game track. The operation and fighter accessor stay fixed; choices are source-proven for this fighter. Permission does not force a status change or emit an event. No flat live native writes are sent.",
    );
}

fn vector_target_label(preset: crate::data::AttackModuleVecTargetPosPreset) -> String {
    let (id, x, y, arg5) = preset.source_parts();
    format!("ID {id} · ({x}, {y}) · {arg5}")
}

fn vector_target_options_ui(ui: &mut Ui, preset: &mut crate::data::AttackModuleVecTargetPosPreset) {
    use crate::data::AttackModuleVecTargetPosPreset;
    let search_id = ui.id().with("vector_target_search");
    let mut query = ui.data_mut(|data| data.get_temp::<String>(search_id).unwrap_or_default());
    ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text("Search presets…")
            .char_limit(128)
            .desired_width(225.0),
    );
    let needle = query.trim().to_ascii_lowercase();
    let choices: Vec<_> = AttackModuleVecTargetPosPreset::ALL
        .into_iter()
        .filter(|choice| {
            vector_target_label(*choice)
                .to_ascii_lowercase()
                .contains(&needle)
        })
        .collect();
    ui.scope(|ui| {
        ui.set_max_width(245.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        egui::ScrollArea::vertical()
            .id_salt("vector_target_rows")
            .max_height(240.0)
            .show_rows(
                ui,
                ui.spacing().interact_size.y,
                choices.len(),
                |ui, rows| {
                    for choice in &choices[rows] {
                        if ui
                            .selectable_value(preset, *choice, vector_target_label(*choice))
                            .on_hover_text(choice.source_line())
                            .clicked()
                        {
                            ui.close();
                        }
                    }
                },
            );
    });
    if choices.is_empty() {
        ui.weak("No matching presets");
    }
    ui.data_mut(|data| data.insert_temp(search_id, query));
}

fn attack_reference_joint_choices(
    track: Track,
) -> &'static [crate::data::AttackReferenceJointKind] {
    use crate::data::AttackReferenceJointKind;
    match track {
        Track::Game => &AttackReferenceJointKind::GAME_CHOICES,
        Track::Expression => &AttackReferenceJointKind::EXPRESSION_CHOICES,
        Track::Sound | Track::Effect => &[],
    }
}

pub(super) fn attack_reference_joint_label(kind: crate::data::AttackReferenceJointKind) -> String {
    let (joint, a, b, c) = kind.native_parts();
    let axis = |token: &str| {
        token
            .strip_prefix("ATTACK_DIRECTION_")
            .unwrap_or(token)
            .replace("_MINUS", "−")
            .replace("NONE", "None")
    };
    format!("{joint} · {} / {} / {}", axis(a), axis(b), axis(c))
}

fn attack_reference_joint_ui(
    ui: &mut Ui,
    kind: &mut crate::data::AttackReferenceJointKind,
    track: Track,
) {
    let choices = attack_reference_joint_choices(track);
    let search_id = ui.id().with("attack_reference_joint_search");
    let mut search = ui
        .data_mut(|data| data.get_temp::<String>(search_id))
        .unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label("Attack reference joint");
        ui.add_enabled_ui(!choices.is_empty(), |ui| {
            egui::ComboBox::from_id_salt("native_attack_reference_joint")
                .selected_text(attack_reference_joint_label(*kind))
                .width(230.0)
                .height(220.0)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| {
                    if choices.len() > 8 {
                        ui.add(
                            egui::TextEdit::singleline(&mut search)
                                .hint_text("Search joints or axes…")
                                .char_limit(128)
                                .desired_width(215.0),
                        );
                    }
                    let query = search.to_ascii_lowercase();
                    for choice in choices {
                        let label = attack_reference_joint_label(*choice);
                        if choices.len() > 8
                            && !query
                                .split_ascii_whitespace()
                                .all(|word| label.to_ascii_lowercase().contains(word))
                        {
                            continue;
                        }
                        if ui
                            .selectable_value(kind, *choice, label)
                            .on_hover_text(format!("{:?}", choice.native_parts()))
                            .clicked()
                        {
                            ui.close();
                        }
                    }
                });
        });
    });
    ui.data_mut(|data| data.insert_temp(search_id, search));
    ui.weak("Measured joint / axis tuple")
        .on_hover_text("Retains the whole native tuple, not independent axis settings. The destination model must contain the joint. Expression calls stay in a direct root Execute block. This is not flat live replay or a projectile direction control.");
}

fn article_motion_ui(
    ui: &mut Ui,
    fighter: &str,
    track: Track,
    symbol: &mut String,
    motion: &mut String,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Object");
        egui::ComboBox::from_id_salt("timeline_motion_article")
            .selected_text(super::plain::object_name(symbol))
            .show_ui(ui, |ui| {
                for entry in measured_motion_articles_on_track(fighter, track) {
                    if ui
                        .selectable_value(
                            symbol,
                            entry.symbol.to_owned(),
                            super::plain::object_name(entry.symbol),
                        )
                        .on_hover_text(entry.symbol)
                        .changed()
                    {
                        *motion = article_motions_on_track(track, symbol)[0].to_owned();
                    }
                }
            });
        ui.label("Motion");
        egui::ComboBox::from_id_salt("timeline_article_motion")
            .selected_text(motion.as_str())
            .width(180.0)
            .height(220.0)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show_ui(ui, |ui| {
                let search_id =
                    ui.id()
                        .with(("article_motion_search", symbol.as_str(), track as u8));
                let mut search = ui
                    .data_mut(|data| data.get_temp::<String>(search_id))
                    .unwrap_or_default();
                let choices = article_motions_on_track(track, symbol);
                if choices.len() > 8 {
                    ui.add(
                        egui::TextEdit::singleline(&mut search)
                            .hint_text("Find a motion…")
                            .char_limit(128)
                            .desired_width(175.0),
                    );
                }
                let query = search.trim().to_ascii_lowercase();
                for name in choices {
                    if choices.len() > 8 && !name.to_ascii_lowercase().contains(&query) {
                        continue;
                    }
                    if ui
                        .selectable_value(motion, (*name).to_owned(), *name)
                        .clicked()
                    {
                        ui.close();
                    }
                }
                ui.data_mut(|data| data.insert_temp(search_id, search));
            });
    });
    ui.weak("Timed article motion").on_hover_text("Only the engine donor's measured article/motion pairs for this track are offered. The native false/-1.0 tail stays fixed. Expression blocks require direct root Execute placement. This does not generate an article or change a live controller;");
}

fn native_command_ui(
    ui: &mut Ui,
    fighter: &str,
    command: &mut ExcuteStmt,
    joints: &[String],
) -> bool {
    use crate::data::{HurtTarget, WorkFlagAction, WorkModuleSetKind};
    use crate::gimmicks::registry::IdentifierKind;
    match command {
        ExcuteStmt::WorkTransitionTerm(call) => {
            transition_permission_ui(ui, fighter, call);
        }
        ExcuteStmt::AttackAbsApplyThrow => {
            ui.weak("Uses the native captured throw target")
                .on_hover_text("Hits the held opponent with the throw damage set above. Only works during a throw, after something has grabbed them.");
        }
        ExcuteStmt::AttackModuleAddReactionFrame { id, frames }
        | ExcuteStmt::AttackModuleAddReactionFrameRevised { id, frames } => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Hitbox ID");
                egui::ComboBox::from_id_salt("reaction_frame_hitbox")
                    .selected_text(id.to_string())
                    .show_ui(ui, |ui| {
                        for value in 0..=8 {
                            ui.selectable_value(id, value, value.to_string());
                        }
                    });
                number_field_ui(ui, "reaction_frame_amount", "Frames", frames, 0.1, Some(
                    "Native reaction-frame adjustment. Negative values are supported; this is not a timer or a reaction event.",
                ));
            });
            ui.weak("?").on_hover_text(
                "Retains the selected native API and its fixed false tail. Original and revised calls are separate blocks; neither is a timer or hit event.",
            );
        }
        ExcuteStmt::AttackModuleInitAttackPos { id } => {
            ui.label(format!("Initialize attack position · ID {id}"))
                .on_hover_text("Only native ID 0 is source-measured. This is not a projectile position or live handle.");
        }
        ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind } => {
            attack_reference_joint_ui(ui, kind, Track::Game);
        }
        ExcuteStmt::AttackModuleSetTargetCategory(id) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Hitbox ID");
                egui::ComboBox::from_id_salt("native_attack_target_id")
                    .selected_text(id.attack_id().to_string())
                    .show_ui(ui, |ui| {
                        for choice in crate::data::AttackModuleTargetCategoryId::ALL {
                            ui.selectable_value(id, choice, choice.attack_id().to_string());
                        }
                    });
                ui.weak("NO_IF").on_hover_text("Exact measured COLLISION_CATEGORY_MASK_NO_IF as u32 argument. This changes the exported AttackModule call for an existing hitbox ID; it does not create a hitbox, choose a live target, or enable a collision callback.");
            });
        }
        ExcuteStmt::AttackModuleSetSize(preset) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Hitbox size");
                egui::ComboBox::from_id_salt("native_attack_size")
                    .height(360.0)
                    .selected_text(format!("ID {} · {}", preset.attack_id(), preset.size_token()))
                    .show_ui(ui, |ui| {
                        for choice in crate::data::AttackModuleSizePreset::ALL {
                            ui.selectable_value(preset, choice, format!("ID {} · {}", choice.attack_id(), choice.size_token()));
                        }
                    }).response.on_hover_text("Exact measured hitbox ID / native size tuple. This changes an existing attack through AttackModule::set_size; it does not create an attack.");
            });
        }
        ExcuteStmt::HitModuleSetStatusAllNormal => {
            ui.label("Hurtboxes back to normal")
                .on_hover_text("Makes every hurtbox on the fighter hittable again.");
        }
        ExcuteStmt::AttackModuleSetVecTargetPos(preset) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Vector target");
                egui::ComboBox::from_id_salt("native_vector_target")
                    .width(225.0).height(310.0).selected_text(vector_target_label(*preset))
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show_ui(ui, |ui| vector_target_options_ui(ui, preset))
                    .response.on_hover_text("Pulls whoever this hitbox hits toward a point near the fighter. The numbers are the hitbox, then the point's sideways and up offset.");
            });
        }
        ExcuteStmt::AttackModuleSetDamageShakeScale(scale) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Damage shake scale");
                egui::ComboBox::from_id_salt("native_damage_shake_scale")
                    .selected_text(scale.source_token())
                    .show_ui(ui, |ui| {
                        for choice in crate::data::AttackModuleDamageShakeScale::ALL {
                            ui.selectable_value(scale, choice, choice.source_token());
                        }
                    }).response.on_hover_text("Measured native numeric values only. This changes the exported AttackModule call, not hitbox damage or a live HUD meter.");
            });
        }
        ExcuteStmt::AttackModuleSetNoUniqEffectAll(pair) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Unique attack effects");
                let (arg2, arg3) = pair.source_args();
                egui::ComboBox::from_id_salt("native_unique_effect_pair")
                    .selected_text(format!("{arg2}, {arg3}"))
                    .show_ui(ui, |ui| {
                        for choice in crate::data::AttackModuleNoUniqEffectAllPair::ALL {
                            let (arg2, arg3) = choice.source_args();
                            ui.selectable_value(pair, choice, format!("{arg2}, {arg3}"));
                        }
                    }).response.on_hover_text("Exact measured arguments to AttackModule::set_no_uniq_effect_all. Their separate meanings are not inferred. this is not a live effect toggle.");
            });
        }
        ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll { arg2, arg3 } => {
            ui.label("Damage fly smoke · native setting");
            ui.weak(format!("Args: {arg2}, {arg3}"))
                .on_hover_text("Exact measured AttackModule call. No inferred smoke timing or live override is exposed.");
        }
        ExcuteStmt::AttackModuleSetHeightAll { height, arg3 } => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Attack height");
                egui::ComboBox::from_id_salt("native_attack_height")
                    .selected_text(match height {
                        crate::data::AttackHeightAllKind::High => "High",
                        crate::data::AttackHeightAllKind::Low => "Low",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(height, crate::data::AttackHeightAllKind::High, "High");
                        ui.selectable_value(height, crate::data::AttackHeightAllKind::Low, "Low");
                    });
                ui.weak(format!("Arg: {arg3}")).on_hover_text(
                    "Exact measured native tail. it is not a live collision override.",
                );
            });
        }
        ExcuteStmt::AttackModuleSetFinalFinishCutIn {
            id,
            arg3,
            arg4,
            arg5,
            arg6,
        } => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Final finish cut-in");
                ui.label("Hitbox ID");
                egui::ComboBox::from_id_salt("native_final_finish_id")
                    .selected_text(id.to_string())
                    .show_ui(ui, |ui| {
                        for value in 0..=3 {
                            ui.selectable_value(id, value, value.to_string());
                        }
                    });
                ui.weak(format!("Args: {arg3}, {arg4}, {arg5}, {arg6}"))
                    .on_hover_text(
                        "Exact observed native arguments, not inferred camera settings.",
                    );
            });
        }
        ExcuteStmt::GrabRebound { enabled } => {
            ui.checkbox(enabled, "Grab rebound")
                .on_hover_text("Native GrabModule rebound setting;");
        }
        ExcuteStmt::AttackModuleSetIndexed {
            kind,
            id,
            arg3,
            arg4,
        } => {
            ui.horizontal_wrapped(|ui| {
                ui.label(match kind { crate::data::AttackModuleSetIndexedKind::NoFinishCamera => "No finish camera", crate::data::AttackModuleSetIndexedKind::ForceReaction => "Force reaction" });
                ui.label("Hitbox ID");
                egui::ComboBox::from_id_salt("native_attack_indexed_id").selected_text(id.to_string()).show_ui(ui, |ui| {
                    for value in 0..=3 { ui.selectable_value(id, value, value.to_string()); }
                });
                ui.weak(format!("Args: {arg3}, {arg4}")).on_hover_text("The measured native boolean tail is true, false. Its arguments are retained exactly; their independent engine semantics are not inferred.");
            });
        }
        ExcuteStmt::AttackModuleSetAll { kind, arg2, arg3 } => {
            ui.horizontal_wrapped(|ui| {
                ui.label(match kind {
                    crate::data::AttackModuleSetAllKind::NoDeadAll => "No-dead attack mode",
                    crate::data::AttackModuleSetAllKind::CatchOnlyAll => "Catch-only attack mode",
                }).on_hover_text("Native AttackModule operation. Only observed argument combinations are offered;");
                egui::ComboBox::from_id_salt("native_attack_all_arguments")
                    .selected_text(format!("Args: {arg2}, {arg3}"))
                    .show_ui(ui, |ui| {
                        for candidate in [false, true] {
                            if candidate && *kind != crate::data::AttackModuleSetAllKind::NoDeadAll { continue; }
                            if ui.selectable_label(*arg2 && *arg3 == candidate, format!("true, {candidate}")).clicked() {
                                *arg2 = true;
                                *arg3 = candidate;
                            }
                        }
                    }).response.on_hover_text("The two native boolean arguments are preserved exactly; their independent engine semantics are not inferred.");
            });
        }
        ExcuteStmt::JostleSetStatus { enabled } => {
            ui.checkbox(enabled, "Body collision")
                .on_hover_text("Controls native fighter jostling, not attack hitboxes.");
        }
        ExcuteStmt::FixedJostleArea(call) => fixed_jostle_area_ui(ui, call),
        ExcuteStmt::VisibilitySetInt64 { group, status } => {
            body_visibility_ui(ui, fighter, group, status);
        }
        ExcuteStmt::VisibilitySetDefaultInt64 { group } => {
            model_default_visibility_ui(ui, fighter, group);
        }
        ExcuteStmt::VisibilitySetWhole { visible } => {
            ui.checkbox(visible, "Show whole model").on_hover_text(
                "Changes this fighter model's rendering, not collision or owned articles. this is not a flat live command.",
            );
        }
        ExcuteStmt::ArticleLifecycle(call) => {
            if let crate::data::ArticleLifecycleCall::ChangeStatus {
                symbol,
                status,
                if_present,
            } = call
            {
                article_state_ui(ui, fighter, symbol, status, if_present);
                return true;
            }
            if let crate::data::ArticleLifecycleCall::GenerateEnabled { symbol } = call {
                enabled_article_ui(ui, fighter, symbol);
                return true;
            }
            if let crate::data::ArticleLifecycleCall::AddMotionPartial { preset } = call {
                egui::ComboBox::from_id_salt("timeline_sword_partial")
                    .selected_text(preset.label())
                    .width(205.0)
                    .truncate()
                    .show_ui(ui, |ui| {
                        for choice in article_partial_presets(fighter) {
                            ui.selectable_value(preset, choice, choice.label())
                                .on_hover_text(choice.source_line());
                        }
                    });
                ui.weak("E-Sword").on_hover_text(
                    "Uses only the donor's measured open/close clip and paired native timing arguments. New blocks include an Article exists guard. This does not generate the sword, change donors, or create a live resource write. The part-set and native flags remain fixed.",
                );
                return true;
            }
            if let crate::data::ArticleLifecycleCall::ChangeMotion { symbol, motion } = call {
                article_motion_ui(ui, fighter, Track::Game, symbol, motion);
                return true;
            }
            let (label, symbol) = match call {
                crate::data::ArticleLifecycleCall::Generate { symbol } => ("Generate", symbol),
                crate::data::ArticleLifecycleCall::RemoveAll { symbol } => ("Object", symbol),
                crate::data::ArticleLifecycleCall::SetVisibilityWhole { symbol, .. } => {
                    ("Object", symbol)
                }
                crate::data::ArticleLifecycleCall::Shoot { symbol, .. } => ("Launch", symbol),
                crate::data::ArticleLifecycleCall::ShootExist { symbol, .. } => {
                    ("Launch if present", symbol)
                }
                crate::data::ArticleLifecycleCall::ChangeMotion { .. }
                | crate::data::ArticleLifecycleCall::GenerateEnabled { .. }
                | crate::data::ArticleLifecycleCall::ChangeStatus { .. }
                | crate::data::ArticleLifecycleCall::AddMotionPartial { .. } => {
                    unreachable!("measured motion controls handled above")
                }
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(label);
                work_symbol_picker(
                    ui,
                    "timeline_article",
                    fighter,
                    symbol,
                    IdentifierKind::Article,
                    false,
                );
            });
            match call {
                crate::data::ArticleLifecycleCall::SetVisibilityWhole { visible, .. } => {
                    ui.checkbox(visible, "Visible").on_hover_text("Changes visibility of all instances of the selected article kind. It does not generate, launch or remove them.");
                }
                crate::data::ArticleLifecycleCall::Shoot { target, .. }
                | crate::data::ArticleLifecycleCall::ShootExist { target, .. } => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Which");
                        egui::ComboBox::from_id_salt("timeline_article_target")
                            .selected_text(article_target_label(*target))
                            .show_ui(ui, |ui| {
                                for option in [
                                    crate::data::ArticleTarget::All,
                                    crate::data::ArticleTarget::Last,
                                ] {
                                    ui.selectable_value(
                                        target,
                                        option,
                                        article_target_label(option),
                                    )
                                    .on_hover_text(option.source_token());
                                }
                            });
                    });
                    ui.weak("Only objects already out").on_hover_text("Generate an article separately, then launch it at the required frame. Launch and Launch if present retain distinct native APIs and the fixed false tail. Launching does not establish projectile-hit or ownership-transfer events.");
                }
                _ => {}
            }
        }
        ExcuteStmt::ItemVisibility { visible } => {
            ui.checkbox(visible, "Held item visible").on_hover_text("Controls held item slot 0 on this fighter. This is not article generation, equipment ownership, or a collision event.");
        }
        ExcuteStmt::NativeCall(call) => {
            super::native_calls::inspector_for_fighter(ui, call, fighter)
        }
        ExcuteStmt::ShootItemBullet => {
            ui.label("Fire held item");
            ui.weak("?").on_hover_text("Runs the native SHOOT_ITEM_BULLET command in a root Game Execute block, not inside Branch or Repeat. Its effect depends on the current item; it does not give the fighter an item, generate an article, or create a projectile-hit trigger. flat live preview cannot send this command.");
        }
        ExcuteStmt::StartInfoFlashEye => {
            ui.weak("?").on_hover_text("Runs START_INFO_FLASH_EYE at this motion frame in a root Game or Expression Execute block. It has no editable payload. Keep imported category and placement; it is not a fighter-color setting, article spawn, collision trigger or live replay. the result depends on native fighter state.");
        }
        ExcuteStmt::CameraZoomOut => {
            ui.weak("?").on_hover_text("Runs CAM_ZOOM_OUT at this motion frame in a root Game Execute block. There are no editable camera arguments. This is not CAM_ZOOM_OUT_FINAL, a throw-finish zoom-rate setting or a live camera slider. native camera context still controls the result.");
        }
        ExcuteStmt::StartCutIn => {
            ui.weak("?").on_hover_text("Runs FT_START_CUTIN at this motion frame in a root Game Execute block. It has no payload and depends on native cinematic context. It does not choose a camera, request a status or create a confirmed-hit event.");
        }
        ExcuteStmt::CaptureCutCommon => {
            ui.weak("Top-level helper").on_hover_text("Runs game_CaptureCutCommon exactly once here. The pinned helper owns its Execute guard and defines the native default absolute Catch attack; it does not create a grab volume or apply throw damage. Keep it outside Execute, Branch and Repeat. Use separate blocks for authored collision volumes and throw application.");
        }
        ExcuteStmt::Slope(call) => {
            use crate::data::{SlopeCall, SlopeStatus};
            ui.horizontal_wrapped(|ui| {
                ui.label("Slope");
                let mut interpolate = matches!(call, SlopeCall::Interpolate { .. });
                if ui.checkbox(&mut interpolate, "Interpolate").changed() {
                    let status = match call {
                        SlopeCall::Set { status } | SlopeCall::Interpolate { status, .. } => {
                            *status
                        }
                    };
                    *call = if interpolate {
                        SlopeCall::Interpolate {
                            status,
                            frames: 3,
                            trailing_flag: None,
                        }
                    } else {
                        SlopeCall::Set { status }
                    };
                }
                let status = match call {
                    SlopeCall::Set { status } | SlopeCall::Interpolate { status, .. } => status,
                };
                egui::ComboBox::from_id_salt("slope_status")
                    .selected_text(slope_label(*status))
                    .show_ui(ui, |ui| {
                        for option in [
                            SlopeStatus::Lr,
                            SlopeStatus::Top,
                            SlopeStatus::None,
                            SlopeStatus::Left,
                            SlopeStatus::Right,
                        ] {
                            ui.selectable_value(status, option, slope_label(option))
                                .on_hover_text(option.symbol());
                        }
                    });
                if let SlopeCall::Interpolate { frames, .. } = call {
                    ui.label("Frames");
                    ui.add(
                        egui::DragValue::new(frames)
                            .range(0..=crate::data::MAX_SLOPE_INTERPOLATION_FRAMES),
                    );
                }
            });
            if let SlopeCall::Interpolate { trailing_flag, .. } = call {
                ui.collapsing("Native option", |ui| {
                    let mut present = trailing_flag.is_some();
                    if ui.checkbox(&mut present, "Include optional flag").changed() { *trailing_flag = present.then_some(false); }
                    if let Some(value) = trailing_flag { ui.checkbox(value, "On"); }
                    ui.weak("The optional native argument is preserved separately; its gameplay meaning is not inferred.");
                });
            }
            ui.weak("Animation alignment");
        }
        ExcuteStmt::HitStatus { target, status } => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Hurtbox");
                let mut kind = match target {
                    HurtTarget::Whole => 0,
                    HurtTarget::Bone(_) => 1,
                    HurtTarget::Group(_) => 2,
                };
                let previous = kind;
                select_index(
                    ui,
                    "hurt_target",
                    &mut kind,
                    &["Whole fighter", "Joint", "Group"],
                );
                if kind != previous {
                    *target = match kind {
                        1 => HurtTarget::Bone("\"top\"".into()),
                        2 => HurtTarget::Group(0),
                        _ => HurtTarget::Whole,
                    };
                }
                match target {
                    HurtTarget::Bone(joint) => joint_picker(ui, joint, joints),
                    HurtTarget::Group(group) => {
                        ui.add(egui::DragValue::new(group).range(0..=255));
                    }
                    HurtTarget::Whole => {}
                }
                let number = status.trim().parse::<i64>().ok().or_else(|| {
                    crate::param_labels::const_value(
                        crate::param_labels::HIT_STATUS,
                        status.trim().trim_start_matches('*'),
                    )
                });
                let options = [
                    ("Normal", "HIT_STATUS_NORMAL", 0),
                    ("Invincible", "HIT_STATUS_INVINCIBLE", 1),
                    ("Intangible", "HIT_STATUS_XLU", 2),
                    ("Off", "HIT_STATUS_OFF", 3),
                ];
                egui::ComboBox::from_id_salt("hurt_status")
                    .selected_text(
                        options
                            .iter()
                            .find(|(_, _, value)| Some(*value) == number)
                            .map_or("Imported state", |(label, _, _)| *label),
                    )
                    .show_ui(ui, |ui| {
                        for (label, symbol, value) in options {
                            if ui
                                .selectable_label(number == Some(value), label)
                                .on_hover_text(symbol)
                                .clicked()
                            {
                                *status = format!("*{symbol}");
                            }
                        }
                    });
            });
        }
        ExcuteStmt::WorkFlag(call) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Native flag");
                native_work_picker(ui, fighter, &mut call.flag, IdentifierKind::WorkFlag);
                egui::ComboBox::from_id_salt("work_flag_action")
                    .selected_text(if call.action == WorkFlagAction::On {
                        "On"
                    } else {
                        "Off"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut call.action, WorkFlagAction::On, "On");
                        ui.selectable_value(&mut call.action, WorkFlagAction::Off, "Off");
                    });
            });
        }
        ExcuteStmt::WorkModuleIncInt(call) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Native counter +1");
                native_work_picker(ui, fighter, &mut call.slot, IdentifierKind::WorkInt);
            });
        }
        ExcuteStmt::WorkModuleSet(call)
            if super::native_work::is_source_only_work_module_set(call)
                && !super::native_work::is_numeric_source_work_module_set(call) =>
        {
            source_work_module_set_ui(ui, fighter, call);
        }
        ExcuteStmt::SlowWhole(call) => slow_whole_command_ui(ui, fighter, call),
        ExcuteStmt::SlowOpponent(call) => slow_opponent_command_ui(ui, fighter, call),
        ExcuteStmt::CheckFinishCamera(call) => finish_camera_ui(ui, fighter, call),
        ExcuteStmt::CheckFinalStartCamera(call) => final_start_camera_ui(ui, fighter, call),
        ExcuteStmt::FinalFearFace(call) => final_fear_face_ui(ui, fighter, call),
        ExcuteStmt::OptionalHitFeedback(call) => optional_hit_feedback_ui(ui, fighter, call),
        ExcuteStmt::WorkModuleSet(call) if call.kind != WorkModuleSetKind::Int64 => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Native value");
                native_work_picker(
                    ui,
                    fighter,
                    &mut call.slot,
                    if call.kind == WorkModuleSetKind::Int {
                        IdentifierKind::WorkInt
                    } else {
                        IdentifierKind::WorkFloat
                    },
                );
                ui.label("Value");
                if call.kind == WorkModuleSetKind::Int {
                    if let Ok(mut value) = call.value.trim().parse::<i32>() {
                        if ui.add(egui::DragValue::new(&mut value)).changed() {
                            call.value = value.to_string();
                        }
                    } else {
                        ui.weak("Imported value").on_hover_text(&call.value);
                    }
                } else if let Ok(mut value) =
                    call.value.trim().trim_end_matches("f32").parse::<f32>()
                {
                    if ui
                        .add(egui::DragValue::new(&mut value).speed(0.1))
                        .changed()
                    {
                        call.value = format!("{value:?}");
                    }
                } else {
                    ui.weak("Imported value").on_hover_text(&call.value);
                }
            });
        }
        _ => return false,
    }
    true
}

fn body_visibility_label(group: &str, status: &str) -> String {
    let suffix = status
        .strip_prefix(group)
        .and_then(|rest| rest.strip_prefix('_'))
        .unwrap_or(status);
    format!("{} / {}", group.replace('_', " "), suffix.replace('_', " "))
}

pub(super) fn article_state_label(symbol: &str, status: &str, if_present: bool) -> String {
    let state = status
        .split_once("_STATUS_KIND_")
        .map_or(status, |(_, state)| state);
    format!(
        "{} / {} · {}",
        short_symbol(symbol),
        state,
        if if_present { "If present" } else { "All" }
    )
}

fn article_state_ui(
    ui: &mut Ui,
    fighter: &str,
    symbol: &mut String,
    status: &mut String,
    if_present: &mut bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("State");
        egui::ComboBox::from_id_salt("article_state")
            .selected_text(article_state_label(symbol, status, *if_present))
            .width(220.0).truncate()
            .show_ui(ui, |ui| {
                for preset in crate::data::article_status_catalog::presets_for(fighter) {
                    let selected = symbol == preset.article && status == preset.status && *if_present == preset.if_present;
                    if ui.selectable_label(selected, article_state_label(preset.article, preset.status, preset.if_present))
                        .on_hover_text(format!("{}\n{}\n{}\n{} pinned source calls", preset.article, preset.status, if preset.if_present { "change_status_exist" } else { "change_status · ARTICLE_OPE_TARGET_ALL" }, preset.source_count))
                        .clicked() {
                        *symbol = preset.article.into();
                        *status = preset.status.into();
                        *if_present = preset.if_present;
                    }
                }
            }).response.on_hover_text(
                "Selects a measured article, status and native API together. All uses change_status for all instances; If present retains change_status_exist. Generation and launching remain separate.",
            );
    });
}

fn enabled_article_ui(ui: &mut Ui, fighter: &str, symbol: &mut String) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Object");
        egui::ComboBox::from_id_salt("enabled_article")
            .selected_text(super::plain::object_name(symbol))
            .show_ui(ui, |ui| {
                for &(owner, choice, _) in crate::data::PINNED_ENABLED_ARTICLE_COUNTS {
                    if owner == fighter {
                        ui.selectable_value(symbol, choice.to_owned(), super::plain::object_name(choice))
                            .on_hover_text(choice);
                    }
                }
            })
            .response
            .on_hover_text(
                "Uses generate_article_enable, distinct from ordinary generation, with the measured false / -1 arguments. It does not launch the article, provide a live handle or prove the custom model's resource binding.",
            );
    });
}

fn model_default_visibility_ui(ui: &mut Ui, fighter: &str, group: &mut String) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Group");
        egui::ComboBox::from_id_salt("model_default_group")
            .selected_text(group.as_str())
            .show_ui(ui, |ui| {
                for &(owner, choice, _) in crate::data::PINNED_DEFAULT_INT64_GROUP_COUNTS {
                    if owner == fighter {
                        ui.selectable_value(group, choice.to_owned(), choice);
                    }
                }
            })
            .response
            .on_hover_text(
                "Authors the native default-group hash setter, not a visible / hidden state. Choices follow the source donor. existing Hash40-form dump calls remain preserved. This does not create model resources or infer the result for a custom model.",
            );
    });
}

fn body_visibility_ui(ui: &mut Ui, fighter: &str, group: &mut String, status: &mut String) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Model state");
        egui::ComboBox::from_id_salt("native_model_visibility")
            .selected_text(body_visibility_label(group, status))
            .width(180.0).truncate()
            .show_ui(ui, |ui| {
                for (candidate_group, candidate_status) in crate::data::body_visibility_catalog::pairs_for(fighter) {
                    if ui.selectable_label(group == candidate_group && status == candidate_status,
                        body_visibility_label(candidate_group, candidate_status))
                        .on_hover_text(format!("{candidate_group} / {candidate_status}"))
                        .clicked()
                    {
                        *group = candidate_group.into();
                        *status = candidate_status.into();
                    }
                }
            }).response.on_hover_text("Complete model-group/state pairs measured for this character's source donor. Labels shorten the native names; they do not infer hidden/visible semantics. New pairs belong in a root Execute block, not a Branch or Repeat. This is not article visibility or a live model command.");
    });
}

fn finish_camera_ui(ui: &mut Ui, fighter: &str, call: &mut crate::data::FinishCameraCall) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Finish camera");
        egui::ComboBox::from_id_salt("finish_camera_preset")
            .selected_text(format!("({}, {})", call.arg1, call.arg2))
            .show_ui(ui, |ui| {
                for preset in crate::data::FINISH_CAMERA_PRESETS.iter().filter(|preset| preset.fighter == fighter) {
                    ui.selectable_value(call, crate::data::FinishCameraCall {
                        arg1: preset.arg1.into(), arg2: preset.arg2.into(),
                    }, format!("({}, {})", preset.arg1, preset.arg2));
                }
            }).response.on_hover_text("Exact CHECK_FINISH_CAMERA value pairs measured for this donor. The native argument units and axes are not established. this does not expose a flat live camera command.");
    });
    if !call.is_measured_for(fighter) {
        ui.weak("Unmeasured donor pair · Preserved until repaired");
    }
}

fn final_start_camera_ui(ui: &mut Ui, fighter: &str, call: &mut crate::data::FinalStartCameraCall) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Final-start camera");
        egui::ComboBox::from_id_salt("final_start_camera_preset")
            .selected_text(format!("({})", call.args.join(", ")))
            .show_ui(ui, |ui| {
                for preset in crate::data::FINAL_START_CAMERA_PRESETS
                    .iter()
                    .filter(|preset| preset.fighter == fighter)
                {
                    ui.selectable_value(
                        call,
                        crate::data::FinalStartCameraCall {
                            args: preset.args.map(str::to_owned),
                        },
                        format!("({})", preset.args.join(", ")),
                    );
                }
            })
            .response
            .on_hover_text("Exact CHECK_VALID_FINAL_START_CAMERA tuples measured for this donor. The six native arguments have no established labels, axes or units; they stay together. This is not a live camera control or an event trigger.");
    });
    if !call.is_measured_for(fighter) {
        ui.weak("Unmeasured donor tuple · Preserved until repaired");
    }
}

fn final_fear_face_ui(ui: &mut Ui, fighter: &str, call: &mut crate::data::FinalFearFaceCall) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Native argument");
        egui::ComboBox::from_id_salt("final_fear_face_preset")
            .selected_text(&call.argument)
            .show_ui(ui, |ui| {
                for preset in crate::data::FINAL_FEAR_FACE_PRESETS
                    .iter()
                    .filter(|preset| preset.fighter == fighter)
                {
                    ui.selectable_value(&mut call.argument, preset.argument.into(), preset.argument);
                }
            })
            .response
            .on_hover_text("Exact FT_SET_FINAL_FEAR_FACE integer tokens measured for this donor. The pinned wrapper calls this argument unk; its meaning and units are not established. Existing conditions remain separate Branch blocks; this is not a timer, event trigger or live command.");
    });
    if !call.is_measured_for(fighter) {
        ui.weak("Unmeasured donor argument · Preserved until repaired");
    }
}

fn item_pull_step_label(value: &str) -> &'static str {
    match value {
        "*FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP" => "Pick up",
        "*FIGHTER_ITEM_GRASS_PULL_STEP_PULL" => "Pull",
        "*FIGHTER_ITEM_GRASS_PULL_STEP_HOIST" => "Hoist",
        "*FIGHTER_ITEM_GRASS_PULL_STEP_END" => "End",
        _ => "Unsupported step",
    }
}

pub(super) fn source_work_value_label(value: &str) -> &str {
    match value {
        "hash40(\"final_air_attack\") as i64" => "Final Smash · air attack",
        "hash40(\"final_attack\") as i64" => "Final Smash · ground attack",
        "hash40(\"special_air_n4\") as i64" => "Neutral special · air variant 4",
        "hash40(\"special_air_n_hold\") as i64" => "Neutral special · air hold",
        "hash40(\"special_air_n_start\") as i64" => "Neutral special · air start",
        "hash40(\"special_n\") as i64" => "Neutral special",
        "hash40(\"special_n1\") as i64" => "Neutral special · variant 1",
        "hash40(\"special_n4\") as i64" => "Neutral special · variant 4",
        "hash40(\"special_n_hold\") as i64" => "Neutral special · ground hold",
        "hash40(\"special_s\") as i64" => "Side special",
        "hash40(\"special_air_hi_jump\") as i64" => "Up special · air jump",
        "hash40(\"attack_hi4_hold\") as i64" => "Up smash · hold",
        "hash40(\"attack_lw4_hold\") as i64" => "Down smash · hold",
        "hash40(\"attack_s4_hold\") as i64" => "Side smash · hold",
        "hash40(\"visual_scene_01\") as i64" => "Visual scene 1",
        "hash40(\"visual_scene_02\") as i64" => "Visual scene 2",
        "hash40(\"visual_scene_03\") as i64" => "Visual scene 3",
        "hash40(\"visual_scene_04\") as i64" => "Visual scene 4",
        _ => value,
    }
}

fn optional_feedback_names(kind: crate::data::OptionalHitFeedbackKind) -> &'static [&'static str] {
    match kind {
        crate::data::OptionalHitFeedbackKind::Effect => &[
            "diddy_final_finish_hit",
            "eflame_attack_s4s_explosion",
            "ken_final_shippu_hit_rush",
        ],
        crate::data::OptionalHitFeedbackKind::Sound => &[
            "se_chrom_criticalhit",
            "se_ike_criticalhit",
            "se_lucina_criticalhit",
            "se_marth_criticalhit",
            "se_roy_criticalhit",
        ],
    }
}

fn optional_hit_feedback_ui(
    ui: &mut Ui,
    fighter: &str,
    call: &mut crate::data::OptionalHitFeedbackCall,
) {
    use crate::data::{Hash40Literal, OptionalHitFeedbackCall, OptionalHitFeedbackKind};
    if crate::gimmicks::registry::fighter_kind_symbol(fighter).is_none() {
        ui.weak("Unsupported owner").on_hover_text(
            "Choose a concrete base fighter for your custom character. Imported arguments are retained.",
        );
        return;
    }
    let names = optional_feedback_names(call.kind);
    let resource_label = match &call.hash {
        Hash40Literal::Name(name) if names.contains(&name.as_str()) => name.replace('_', " "),
        Hash40Literal::Name(_) => "Custom name".into(),
        Hash40Literal::Raw(raw) => format!("Raw 0x{raw:x}"),
    };
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("feedback_arg")
            .selected_text(format!("Native argument {}", call.arg2))
            .show_ui(ui, |ui| {
                for value in OptionalHitFeedbackCall::MIN_ARG2..=OptionalHitFeedbackCall::MAX_ARG2 {
                    ui.selectable_value(&mut call.arg2, value, format!("Native argument {value}"));
                }
            })
            .response.on_hover_text(
                "The SDK names this integer arg2. The editor accepts the observed range 0–4; it is not labeled as a hitbox ID, duration or engine limit.",
            );
        egui::ComboBox::from_id_salt("feedback_resource")
            .selected_text(resource_label)
            .show_ui(ui, |ui| {
                for name in names {
                    ui.selectable_value(&mut call.hash, Hash40Literal::Name((*name).into()), name.replace('_', " "))
                        .on_hover_text(*name);
                }
                if call.kind == OptionalHitFeedbackKind::Effect {
                    for raw in [0x148397b09f, 0x1484fa7486] {
                        ui.selectable_value(&mut call.hash, Hash40Literal::Raw(raw), format!("Raw 0x{raw:x}"))
                            .on_hover_text("Unresolved raw value retained from native source; no resource name is inferred.");
                    }
                }
                let custom = matches!(&call.hash, Hash40Literal::Name(name) if !names.contains(&name.as_str()));
                if ui.selectable_label(custom, "Custom name…").clicked() && !custom {
                    call.hash = Hash40Literal::Name(String::new());
                }
            })
            .response.on_hover_text(
                "These are source-observed names, not a verified asset inventory for this fighter. Choose an installed resource or enter the name from your custom assets. No resource is spawned by this editor control.",
            );
    });
    match &mut call.hash {
        Hash40Literal::Name(name) if !names.contains(&name.as_str()) => {
            ui.add(egui::TextEdit::singleline(name).hint_text("Resource name…").desired_width(250.0))
                .on_hover_text("A name value, not Rust code. Must be non-empty, at most 255 UTF-8 bytes and contain no NUL. Resource availability is unverified.");
        }
        Hash40Literal::Raw(raw) => {
            ui.monospace(format!("0x{raw:x}"))
                .on_hover_text("Unresolved raw hash. All 64 bits are preserved; the editor does not guess a name. Choose a named resource explicitly to replace it.");
        }
        _ => {}
    }
    ui.weak("?").on_hover_text(
        "Retains the native optional-hit-feedback command. This is not a live effect/sound spawn, an On-hit event or proof that the selected resource exists.",
    );
}

fn slow_opponent_command_ui(ui: &mut Ui, fighter: &str, call: &mut crate::data::SlowOpponentCall) {
    if crate::gimmicks::registry::fighter_kind_symbol(fighter).is_none() {
        ui.weak("Unsupported owner").on_hover_text(
            "Choose a known concrete fighter or the base fighter of your custom character. Imported arguments are retained.",
        );
        return;
    }
    ui.horizontal_wrapped(|ui| {
        number_field_ui(
            ui,
            "slow_mul",
            "Times slower",
            &mut call.slow_mul,
            0.1,
            None,
        );
        number_field_ui(ui, "slow_time", "Frames", &mut call.slow_time, 1.0, None);
    });
}

fn slow_whole_pair_label(multiplier: u8, frames: i32) -> String {
    if frames == 0 {
        format!("{multiplier}x slower until cleared")
    } else {
        format!("{multiplier}x slower for {frames} frames")
    }
}

fn slow_whole_command_ui(ui: &mut Ui, fighter: &str, call: &mut crate::data::SlowWholeCall) {
    use crate::data::SlowWholeCall;
    let pairs = crate::data::measured_slow_whole_set_pairs();
    let known_owner = crate::gimmicks::registry::fighter_kind_symbol(fighter).is_some();
    match call {
        SlowWholeCall::Set { multiplier, frames } => {
            let measured = pairs
                .iter()
                .any(|pair| pair.multiplier == *multiplier && pair.frames == *frames);
            if !known_owner || !measured {
                ui.weak("Unsupported slowdown").on_hover_text(format!(
                    "SlowModule::set_whole(agent.module_accessor, {multiplier}, {frames});\nOwner or argument pair is unreviewed. Imported arguments are retained."
                ));
                return;
            }
            ui.label("Set slowdown")
                .on_hover_text("SlowModule::set_whole(agent.module_accessor, multiplier, frames)");
            let mut selected = (*multiplier, *frames);
            egui::ComboBox::from_id_salt("slow_whole_pair")
                .selected_text(slow_whole_pair_label(selected.0, selected.1))
                .show_ui(ui, |ui| {
                    for pair in pairs {
                        ui.selectable_value(
                            &mut selected,
                            (pair.multiplier, pair.frames),
                            slow_whole_pair_label(pair.multiplier, pair.frames),
                        );
                    }
                });
            (*multiplier, *frames) = selected;
        }
        SlowWholeCall::Clear if known_owner => {
            ui.label("Clear slowdown")
                .on_hover_text("SlowModule::clear_whole(agent.module_accessor)");
        }
        SlowWholeCall::Clear => {
            ui.weak("Unsupported slowdown").on_hover_text(
                "A known concrete fighter owner is required. Imported arguments are retained.",
            );
            return;
        }
    }
    ui.weak("?").on_hover_text(
        "Retains the measured native whole-slow operation on this fighter's module accessor. Frame and Wait connections determine when it runs. The two setter arguments are a reviewed pair; zero remains the exact native frame argument, not an authored timer. Clear slowdown is a separate block. No global-game or live slowdown write is published.",
    );
}

fn source_work_module_set_ui(
    ui: &mut Ui,
    fighter: &str,
    call: &mut crate::data::WorkModuleSetCall,
) {
    use super::native_work as native;
    if call.slot == native::ITEM_GRASS_PULL_NEXT_STEP_SLOT
        || call.slot == native::THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT
    {
        item_pull_command_ui(ui, fighter, call);
        return;
    }
    let provenance = format!(
        "{}({}, {}, {})",
        call.kind.func(),
        call.receiver,
        call.value,
        call.slot
    );
    if let Err(error) = native::validate_source_only_work_module_set(fighter, call) {
        ui.weak("Unsupported native setter").on_hover_text(format!(
            "{provenance}\n{error}\nImported arguments are retained."
        ));
        return;
    }
    let Some(descriptor) =
        native::source_work_module_set_descriptor(fighter, call.kind, &call.slot)
    else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(descriptor.label).on_hover_text(&provenance);
        egui::ComboBox::from_id_salt("source_work_value")
            .selected_text(source_work_value_label(&call.value))
            .height(260.0)
            .show_ui(ui, |ui| {
                for value in descriptor.values {
                    ui.selectable_value(
                        &mut call.value,
                        (*value).into(),
                        source_work_value_label(value),
                    )
                    .on_hover_text(*value);
                }
            });
    });
    ui.weak("?").on_hover_text(if descriptor.kind.is_float() {
        "Retains the measured Final wave-angle setter for Mii Swordfighter. Choose a reviewed value, and connect Frame or Wait for timing. Units are not inferred. This does not spawn a wave, force a status, or publish a live native write."
    } else {
        "Records the reviewed sword open-motion name for this donor. Pyra and Mythra have separate measured choices; the shared native slot spelling does not make them interchangeable. This does not play a motion, verify an asset, or publish a live native write."
    });
}

fn item_pull_command_ui(ui: &mut Ui, fighter: &str, call: &mut crate::data::WorkModuleSetCall) {
    use super::native_work as native;
    let provenance = format!(
        "WorkModule::set_int({}, {}, {})",
        call.receiver, call.value, call.slot
    );
    if let Err(error) = native::validate_source_only_work_module_set(fighter, call) {
        ui.weak("Unsupported native setter").on_hover_text(format!(
            "{provenance}\n{error}\nImported arguments are retained."
        ));
        return;
    }
    if call.slot == native::ITEM_GRASS_PULL_NEXT_STEP_SLOT {
        ui.horizontal_wrapped(|ui| {
            ui.label("Item pull step").on_hover_text(&provenance);
            egui::ComboBox::from_id_salt("item_pull_step")
                .selected_text(item_pull_step_label(&call.value))
                .show_ui(ui, |ui| {
                    for value in native::ITEM_GRASS_PULL_STEP_VALUES {
                        ui.selectable_value(
                            &mut call.value,
                            (*value).into(),
                            item_pull_step_label(value),
                        )
                        .on_hover_text(*value);
                    }
                });
        });
    } else {
        ui.label("Item pull offset limit · 4")
            .on_hover_text(provenance);
    }
    ui.weak("?").on_hover_text(
        "Reconstructs the exact shared setter measured in ItemGrassPull. Connect after Frame or Wait to choose when it runs. The offset limit retains the measured value 4; its units are not inferred. These blocks do not create an item, force a fighter status, or publish a live native write.",
    );
}

fn slope_label(status: crate::data::SlopeStatus) -> &'static str {
    use crate::data::SlopeStatus;
    match status {
        SlopeStatus::Lr => "Left / right",
        SlopeStatus::Top => "Top",
        SlopeStatus::None => "None",
        SlopeStatus::Left => "Left",
        SlopeStatus::Right => "Right",
    }
}

fn expression_command_ui(ui: &mut Ui, expression: &mut crate::data::ExpressionCall) {
    use crate::data::ExpressionCall;
    ui.horizontal_wrapped(|ui| match expression {
        ExpressionCall::Quake { kind } => {
            ui.label("Camera shake");
            egui::ComboBox::from_id_salt("quake_kind")
                .selected_text(kind.trim_start_matches("*CAMERA_QUAKE_KIND_"))
                .show_ui(ui, |ui| {
                    for label in ["S", "M", "L"] {
                        ui.selectable_value(kind, format!("*CAMERA_QUAKE_KIND_{label}"), label);
                    }
                });
        }
        ExpressionCall::ControlModuleSetRumble {
            kind,
            duration,
            looped,
            target,
            ..
        } => {
            rumble_picker(ui, kind);
            if let Ok(mut value) = duration.trim().parse::<i32>() {
                ui.label("Duration").on_hover_text(
                    "Native signed integer argument; units and zero behavior are not inferred.",
                );
                if ui
                    .add(egui::DragValue::new(&mut value).range(0..=i32::MAX))
                    .changed()
                {
                    *duration = value.to_string();
                }
            } else {
                ui.weak("Imported duration")
                    .on_hover_text(duration.as_str());
            }
            if let Ok(mut value) = looped.trim().parse::<bool>() {
                if ui.checkbox(&mut value, "Loop").changed() {
                    *looped = value.to_string();
                }
            } else {
                ui.weak("Imported loop option")
                    .on_hover_text(looped.as_str());
            }
            ui.weak("Native target").on_hover_text(format!(
                "{target}\nThe measured target is retained; no live object ID is authored here."
            ));
        }
        ExpressionCall::RumbleHit { kind, unk } => {
            rumble_picker(ui, kind);
            if let Ok(mut value) = unk.parse::<i32>() {
                ui.label("Variant");
                if ui.add(egui::DragValue::new(&mut value)).changed() {
                    *unk = value.to_string();
                }
            }
        }
        expression => {
            ui.label("Imported expression")
                .on_hover_text(format!("{expression:?}"));
        }
    });
}

// Literal patterns measured in the pinned Expression corpus. Selecting one
// replaces only the asset token; viewing an unknown/imported token retains it.
const RUMBLE_PATTERNS: &[&str] = &[
    "rbkind_aerial",
    "rbkind_attackl",
    "rbkind_attackm",
    "rbkind_attacks",
    "rbkind_beams",
    "rbkind_dash",
    "rbkind_down",
    "rbkind_drgn_start",
    "rbkind_elecattack",
    "rbkind_erase",
    "rbkind_erase_sp",
    "rbkind_escape",
    "rbkind_explosion",
    "rbkind_explosionl",
    "rbkind_explosionm",
    "rbkind_furafura",
    "rbkind_gene_blast",
    "rbkind_gene_charge",
    "rbkind_genesis",
    "rbkind_getbonus",
    "rbkind_getbonusl",
    "rbkind_grapple",
    "rbkind_impact",
    "rbkind_jump",
    "rbkind_landl_hv",
    "rbkind_lands",
    "rbkind_nohit_explosion",
    "rbkind_nohit_explosionm",
    "rbkind_nohitl",
    "rbkind_nohitl_l",
    "rbkind_nohitll",
    "rbkind_nohitll_l",
    "rbkind_nohitm",
    "rbkind_nohitm_l",
    "rbkind_nohits",
    "rbkind_nohits_l",
    "rbkind_run",
    "rbkind_sleep",
    "rbkind_smashhold1",
    "rbkind_smashhold2",
    "rbkind_sp_flag",
    "rbkind_swim",
    "rbkind_walk",
    "rbkind_walk_hv",
    "rbkind_warpstar",
];

fn rumble_picker(ui: &mut Ui, kind: &mut String) {
    let literal = syn::parse_str::<syn::ExprCall>(kind).ok().and_then(|call| {
        if call.args.len() != 1 {
            return None;
        }
        let syn::Expr::Path(path) = *call.func else {
            return None;
        };
        if path
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>()
            != ["Hash40", "new"]
        {
            return None;
        }
        match call.args.first()? {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(name),
                ..
            }) => Some(name.value()),
            _ => None,
        }
    });
    let label = |name: &str| name.trim_start_matches("rbkind_").replace('_', " ");
    let search_id = ui.make_persistent_id("rumble_pattern_search");
    let mut search = ui
        .data_mut(|data| data.get_temp::<String>(search_id))
        .unwrap_or_default();
    ui.label("Pattern");
    egui::ComboBox::from_id_salt("rumble_pattern")
        .selected_text(
            literal
                .as_deref()
                .map_or_else(|| "Imported pattern".into(), label),
        )
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut search).hint_text("Find pattern…"));
            let query = search.trim().to_lowercase();
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for name in RUMBLE_PATTERNS
                        .iter()
                        .filter(|name| name.contains(&query) || label(name).contains(&query))
                    {
                        if ui
                            .selectable_label(literal.as_deref() == Some(*name), label(name))
                            .on_hover_text(*name)
                            .clicked()
                        {
                            *kind = format!("Hash40::new({name:?})");
                            ui.close();
                        }
                    }
                });
        })
        .response
        .on_hover_text(kind.as_str());
    ui.data_mut(|data| data.insert_temp(search_id, search));
}

fn context_effect(kind: usize, fighter: &str) -> crate::data::EffectMacro {
    if kind == 89 {
        return crate::data::EffectMacro::Control(crate::data::EffectControl::OffKindWork {
            preset_id: crate::data::EffectOffKindWorkPreset::all()
                .iter()
                .find(|preset| preset.fighter == fighter)
                .map_or_else(String::new, |preset| preset.id.to_owned()),
        });
    }
    default_effect(kind)
}

pub(super) fn default_effect(kind: usize) -> crate::data::EffectMacro {
    use crate::data::EffectMacro;
    match kind {
        8 => EffectMacro::LastEffectSetRate { rate: 1.0 },
        9 => EffectMacro::LastEffectSetColor { rgb: [1.0; 3] },
        10 => EffectMacro::EffectOffKind {
            effect_name: "sys_smash_flash".into(),
            fade: false,
            detach: true,
        },
        54 => EffectMacro::LastEffectSetAlpha { alpha: 1.0 },
        55 => EffectMacro::LastParticleSetColor { rgb: [1.0; 3] },
        56 => EffectMacro::LastEffectSetOffsetToCameraFlat { offset: 0.0 },
        57 => EffectMacro::LastEffectSetScaleW {
            values: vec![1.0; 3],
        },
        58 => EffectMacro::Control(crate::data::EffectControl::DetachKind {
            effect_name: "sys_smash_flash".into(),
            unk: -1,
        }),
        59 => EffectMacro::Control(crate::data::EffectControl::SetDisableRenderOffsetLast),
        60 => EffectMacro::Control(crate::data::EffectControl::EnableSyncInitPosLast),
        61 => EffectMacro::Color {
            command: "FLASH".into(),
            color: crate::data::ColorCall {
                transition: None,
                rgba: Some([1.0; 4]),
            },
        },
        62 => EffectMacro::AfterImageOff {
            arg: crate::data::TRAIL_OFF_DEFAULT,
        },
        87 => EffectMacro::Control(crate::data::EffectControl::ReqScreen {
            preset_id: "bg_mario_final".into(),
        }),
        88 => EffectMacro::Control(crate::data::EffectControl::SetDisableCameraDepthInfluence {
            value: false,
        }),
        _ => EffectMacro::Effect {
            effect_name: "sys_smash_flash".into(),
            effect_name_alt: None,
            spawn_func: "EFFECT_FOLLOW".into(),
            bone_name: "top".into(),
            offset: [0.0; 3],
            rotation: [0.0; 3],
            scale: 1.0,
            follows_bone: true,
            extra_args: Vec::new(),
            flip_axis: None,
        },
    }
}

pub(super) fn set_effect_spawn_variant(command: &mut crate::data::EffectMacro, name: &str) -> bool {
    let Some(layout) = crate::acmd::effect_spawn_layout(name) else {
        return false;
    };
    let crate::data::EffectMacro::Effect {
        effect_name,
        effect_name_alt,
        spawn_func,
        follows_bone,
        extra_args,
        flip_axis,
        ..
    } = command
    else {
        return false;
    };
    *spawn_func = layout.name.into();
    *follows_bone = layout.follows_bone;
    if layout.flip {
        effect_name_alt.get_or_insert_with(|| effect_name.clone());
        *flip_axis = Some("0".into());
    } else {
        *effect_name_alt = None;
        *flip_axis = None;
    }
    *extra_args = layout
        .tail
        .iter()
        .map(|(default, _)| (*default).into())
        .collect();
    true
}

fn effect_spawn_mode_label(name: &str) -> &str {
    match name {
        "EFFECT" => "Free effect",
        "EFFECT_ALPHA" => "Free with opacity",
        "EFFECT_ATTR" => "Free with attributes",
        "EFFECT_FLIP" => "Free, flipped",
        "EFFECT_FLIP_ALPHA" => "Free, flipped with opacity",
        "EFFECT_FOLLOW" => "Follow joint",
        "EFFECT_FOLLOW_ALPHA" => "Follow with opacity",
        "EFFECT_FOLLOW_COLOR" => "Follow with color",
        "EFFECT_FOLLOW_FLIP" => "Follow, flipped",
        "EFFECT_FOLLOW_FLIP_ALPHA" => "Follow, flipped with opacity",
        "EFFECT_FOLLOW_FLIP_COLOR" => "Follow, flipped with color",
        "EFFECT_FOLLOW_FLIP_RND" => "Follow, randomized flip",
        "EFFECT_FOLLOW_NO_SCALE" => "Follow, no scale",
        "EFFECT_FOLLOW_NO_STOP" => "Follow, no stop",
        "EFFECT_FOLLOW_NO_STOP_FLIP" => "Follow, no stop, flipped",
        "EFFECT_FLW_POS" => "Follow position",
        "EFFECT_FLW_POS_NO_STOP" => "Follow position, no stop",
        "EFFECT_FLW_POS_UNSYNC_VIS" => "Follow position, unsynced visibility",
        "EFFECT_FLW_UNSYNC_VIS" => "Follow, unsynced visibility",
        "LANDING_EFFECT" => "Landing",
        "LANDING_EFFECT_FLIP" => "Landing, flipped",
        "FOOT_EFFECT" => "Foot",
        "FOOT_EFFECT_FLIP" => "Foot, flipped",
        "DOWN_EFFECT" => "Down",
        _ => name,
    }
}

fn effect_spawn_variant_ui(ui: &mut Ui, command: &mut crate::data::EffectMacro) {
    effect_spawn_variant_ui_mode(ui, command, true);
}

fn effect_spawn_variant_ui_mode(
    ui: &mut Ui,
    command: &mut crate::data::EffectMacro,
    mode_editable: bool,
) {
    let crate::data::EffectMacro::Effect { spawn_func, .. } = command else {
        return;
    };
    let before = spawn_func.clone();
    let mut selected = before.clone();
    let search_id = ui.make_persistent_id("effect_spawn_mode_search");
    let mut search = ui
        .data_mut(|data| data.get_temp::<String>(search_id))
        .unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label("Spawn mode");
        ui.add_enabled_ui(mode_editable, |ui| {
        egui::ComboBox::from_id_salt("effect_spawn_mode")
            .width(180.0)
            .truncate()
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .selected_text(effect_spawn_mode_label(&selected))
            .show_ui(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut search).hint_text("Find spawn mode…").desired_width(220.0));
                let query = search.trim().to_ascii_lowercase();
                for layout in crate::acmd::EFFECT_SPAWN_LAYOUTS.iter().filter(|layout| {
                    layout.name.to_ascii_lowercase().contains(&query)
                        || effect_spawn_mode_label(layout.name).to_ascii_lowercase().contains(&query)
                }) {
                    if ui.selectable_value(&mut selected, layout.name.into(), effect_spawn_mode_label(layout.name))
                        .on_hover_text(layout.name).clicked() {
                        ui.close();
                    }
                }
            }).response.on_hover_text(format!("{before}\n\nChanging mode preserves graphic, joint and transform, and resets only the mode-specific native tail to its published defaults. Native execution needs separate validation."));
        }).response.on_disabled_hover_text("Imported mixed groups keep their original command family. Graphic and transform properties remain editable.");
    });
    ui.data_mut(|data| data.insert_temp(search_id, search));
    if selected != before {
        set_effect_spawn_variant(command, &selected);
    }
    let crate::data::EffectMacro::Effect {
        spawn_func,
        extra_args,
        flip_axis,
        ..
    } = command
    else {
        return;
    };
    let Some(layout) = crate::acmd::effect_spawn_layout(spawn_func) else {
        return;
    };
    egui::CollapsingHeader::new("Native arguments")
        .id_salt("effect_spawn_native_arguments")
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, (default, kind)) in layout.tail.iter().enumerate() {
                    let mut token = extra_args
                        .get(index)
                        .map_or_else(|| (*default).into(), Clone::clone);
                    if *kind == crate::acmd::EffectSpawnTailKind::Integer && layout.flip {
                        if let Some(axis) = flip_axis.as_ref() {
                            token = axis.clone();
                        }
                    }
                    let before = token.clone();
                    match kind {
                        crate::acmd::EffectSpawnTailKind::Number => {
                            if let Ok(mut value) = token.parse::<f32>() {
                                number_field_ui(
                                    ui,
                                    ("tail_number", index),
                                    &format!("Arg {}", index + 1),
                                    &mut value,
                                    0.1,
                                    None,
                                );
                                // Opening the inspector must not normalize an imported token.
                                if token.parse::<f32>().ok() != Some(value) {
                                    token = value.to_string();
                                }
                            } else {
                                ui.weak(&token);
                            }
                        }
                        crate::acmd::EffectSpawnTailKind::Bool => {
                            let mut value = token == "true";
                            if ui
                                .checkbox(&mut value, format!("Arg {}", index + 1))
                                .changed()
                            {
                                token = value.to_string();
                            }
                        }
                        crate::acmd::EffectSpawnTailKind::Integer => {
                            let label = |value: &str| match value {
                                "0" => "Default",
                                "*EF_FLIP_NONE" => "None",
                                "*EF_FLIP_XY" | "*EF_FLIP_AXIS_XY" => "XY",
                                "*EF_FLIP_YZ" | "*EF_FLIP_AXIS_YZ" => "YZ",
                                "*EF_FLIP_ZX" | "*EF_FLIP_AXIS_ZX" => "ZX",
                                "*EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE" => "No joint scale",
                                _ => "Imported control",
                            };
                            let caption = if layout.flip { "Flip" } else { "Attribute" };
                            let box_width = if layout.flip { 55.0 } else { 110.0 };
                            let label_width = ui
                                .painter()
                                .layout_no_wrap(
                                    caption.into(),
                                    egui::TextStyle::Body.resolve(ui.style()),
                                    ui.visuals().text_color(),
                                )
                                .size()
                                .x;
                            ui.allocate_ui_with_layout(
                                egui::vec2(
                                    label_width + ui.spacing().item_spacing.x + box_width,
                                    ui.spacing().interact_size.y,
                                ),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label(caption);
                                    egui::ComboBox::from_id_salt(("integer_tail", index))
                                        .width(box_width)
                                        .truncate()
                                        .selected_text(label(&token))
                                        .show_ui(ui, |ui| {
                                            for value in
                                                crate::acmd::effect_spawn_integer_tail_tokens(
                                                    layout.name,
                                                )
                                            {
                                                // Alias constants retain their source spelling without duplicate choices.
                                                if value.contains("_AXIS_") {
                                                    continue;
                                                }
                                                ui.selectable_value(
                                                    &mut token,
                                                    (*value).into(),
                                                    label(value),
                                                )
                                                .on_hover_text(*value);
                                            }
                                        })
                                        .response
                                        .on_hover_text(&token);
                                },
                            );
                        }
                    }
                    if token != before {
                        if extra_args.is_empty() {
                            *extra_args = layout
                                .tail
                                .iter()
                                .map(|(default, _)| (*default).into())
                                .collect();
                        }
                        if let Some(value) = extra_args.get_mut(index) {
                            *value = token.clone();
                        }
                        if layout.flip && *kind == crate::acmd::EffectSpawnTailKind::Integer {
                            *flip_axis = Some(token);
                        }
                    }
                }
            });
        });
}

fn screen_effect_label(preset: &crate::data::EffectReqScreenPreset) -> String {
    preset
        .effect_name
        .strip_prefix("bg_")
        .unwrap_or(preset.effect_name)
        .replace('_', " ")
}

pub(crate) fn screen_effect_preset_ui(ui: &mut Ui, preset_id: &mut String) -> bool {
    use crate::data::EffectReqScreenPreset;
    let before = preset_id.clone();
    let selected = EffectReqScreenPreset::by_id(preset_id)
        .map(screen_effect_label)
        .unwrap_or_else(|| format!("Unsupported · {preset_id}"));
    egui::ComboBox::from_id_salt("screen_effect_preset")
        .selected_text(selected)
        .width(225.0)
        .truncate()
        .height(310.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            let search_id = ui.id().with("screen_effect_search");
            let mut search = ui.data_mut(|data| data.get_temp::<String>(search_id).unwrap_or_default());
            ui.add(egui::TextEdit::singleline(&mut search)
                .hint_text("Search screens…").char_limit(128).desired_width(225.0));
            let query = search.trim().to_ascii_lowercase();
            let choices = EffectReqScreenPreset::all().iter().filter(|preset| {
                preset.effect_name.contains(&query)
                    || screen_effect_label(preset).contains(&query)
            }).collect::<Vec<_>>();
            ui.scope(|ui| {
                ui.set_max_width(245.0);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                egui::ScrollArea::vertical().id_salt("screen_effect_rows")
                    .max_height(240.0).show_rows(ui, ui.spacing().interact_size.y, choices.len(), |ui, rows| {
                        for preset in &choices[rows] {
                            if ui.selectable_value(preset_id, preset.id.to_owned(), screen_effect_label(preset))
                                .on_hover_text(format!("{}\nFlags: {}, {}, {}\nThe graphic must be available in the mod's resources.", preset.effect_name, preset.flags[0], preset.flags[1], preset.flags[2]))
                                .clicked() {
                                ui.close();
                            }
                        }
                    });
            });
            if choices.is_empty() { ui.weak("No matching screens"); }
            ui.data_mut(|data| data.insert_temp(search_id, search));
        }).response.on_hover_text("One of 110 measured screen-effect calls. Selecting a graphic retains its complete native flag tuple. This does not create a returned effect handle or enable live preview.");
    *preset_id != before
}

pub(crate) fn camera_depth_influence_ui(ui: &mut Ui, disabled: &mut bool) -> bool {
    let before = *disabled;
    egui::ComboBox::from_id_salt("camera_depth_influence")
        .selected_text(if *disabled { "Disabled" } else { "Enabled" })
        .show_ui(ui, |ui| {
            ui.selectable_value(disabled, false, "Enabled");
            ui.selectable_value(disabled, true, "Disabled");
        }).response.on_hover_text("Native ColorBlendModule camera-depth control. Enabled passes false; Disabled passes true.");
    *disabled != before
}

/// A short label for an exact native slot, not a guessed resource or returned handle.
pub(crate) fn native_effect_slot_label(work_symbol: &str) -> String {
    work_symbol
        .split_once("_INT_EFFECT_KIND_")
        .or_else(|| work_symbol.split_once("_INT_EFT_ID_"))
        .map_or(work_symbol, |(_, name)| name)
        .to_ascii_lowercase()
        .replace('_', " ")
}

pub(crate) fn native_effect_off_label(preset: &crate::data::EffectOffKindWorkPreset) -> String {
    format!(
        "{} · {} / {}",
        native_effect_slot_label(preset.work_symbol),
        preset.flags[0],
        preset.flags[1]
    )
}

pub(crate) fn native_effect_off_preset_ui(
    ui: &mut Ui,
    fighter: Option<&str>,
    preset_id: &mut String,
) -> bool {
    use crate::data::EffectOffKindWorkPreset;
    let before = preset_id.clone();
    let selected =
        EffectOffKindWorkPreset::by_id(preset_id).filter(|preset| Some(preset.fighter) == fighter);
    let label = selected
        .map(native_effect_off_label)
        .unwrap_or_else(|| "Unsupported saved preset".into());
    egui::ComboBox::from_id_salt("native_effect_off_preset")
        .selected_text(label).width(225.0).truncate().height(210.0)
        .show_ui(ui, |ui| {
            let mut count = 0;
            for preset in EffectOffKindWorkPreset::all().iter()
                .filter(|preset| Some(preset.fighter) == fighter)
            {
                count += 1;
                ui.selectable_value(preset_id, preset.id.to_owned(), native_effect_off_label(preset))
                    .on_hover_text(format!("{}\nOwner: {}\nFlags: {}, {}\nReads the effect kind from this registered slot; no Work value is written.", preset.work_symbol, preset.fighter, preset.flags[0], preset.flags[1]));
            }
            if count == 0 { ui.weak("No measured presets for this donor"); }
        }).response.on_hover_text("Closed native effect-off presets for this character's donor. The two literal flags stay together. This is not an arbitrary Work-slot or effect-handle editor, and does not enable flat live preview.");
    *preset_id != before
}

#[cfg(test)]
fn effect_command_ui(ui: &mut Ui, command: &mut crate::data::EffectMacro, joints: &[String]) {
    effect_command_ui_mode(ui, command, joints, true, None);
}

fn effect_command_ui_mode(
    ui: &mut Ui,
    command: &mut crate::data::EffectMacro,
    joints: &[String],
    mode_editable: bool,
    fighter: Option<&str>,
) {
    use crate::data::{EffectControl, EffectMacro};
    if matches!(command, EffectMacro::AfterImage { .. }) {
        trail_command_ui(ui, command, joints);
        return;
    }
    if let EffectMacro::EffectFollowWork {
        work_symbol,
        bone_name,
        offset,
        rotation,
        scale,
        unk8,
    } = command
    {
        let compatible = crate::data::EffectFollowWorkPreset::by_work_symbol(work_symbol)
            .is_some_and(|preset| Some(preset.fighter) == fighter);
        ui.horizontal_wrapped(|ui| {
            ui.label("Follow native effect");
            ui.weak(if compatible {
                format!("{} · {bone_name}", native_effect_slot_label(work_symbol))
            } else {
                "Unsupported native source".into()
            }).on_hover_text(format!(
                "Slot: {work_symbol}\nJoint: {bone_name}\nNative unk8: {unk8}\nImported source identity stays fixed. This reads the donor's native effect kind; it is not a graphic resource or returned handle."
            ));
            number_field_ui(ui, "native_effect_scale", "Scale", scale, 0.05, None);
        });
        ui.collapsing("Transform", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Offset");
                for (axis, value) in ["X", "Y", "Z"].into_iter().zip(offset) {
                    number_field_ui(ui, ("native_effect_offset", axis), axis, value, 0.1, None);
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Rotation");
                // Effect rotation storage is z/y/x; the inspector presents native x/y/z.
                for (axis, value) in ["Z", "Y", "X"].into_iter().zip(rotation).rev() {
                    number_field_ui(ui, ("native_effect_rotation", axis), axis, value, 1.0, None);
                }
            });
        }).header_response.on_hover_text(
            "Position and rotation use the native X/Y/Z order. Finite values within ±10000; Slot, joint and unk8 remain anchored to the imported source."
        );
        return;
    }
    if mode_editable {
        effect_spawn_variant_ui(ui, command);
    } else {
        effect_spawn_variant_ui_mode(ui, command, false);
    }
    ui.horizontal_wrapped(|ui| match command {
        EffectMacro::Effect {
            effect_name,
            effect_name_alt,
            bone_name,
            offset,
            rotation,
            scale,
            ..
        } => {
            ui.label("Graphic");
            ui.add(egui::TextEdit::singleline(effect_name).desired_width(130.0));
            ui.label("Body part");
            ui.add(egui::TextEdit::singleline(bone_name).desired_width(70.0));
            number_field_ui(ui, "effect_scale", "Scale", scale, 0.1, None);
            if let Some(alternate) = effect_name_alt {
                ui.label("Alternate graphic");
                ui.add(egui::TextEdit::singleline(alternate).desired_width(130.0));
            }
            ui.collapsing("Transform", |ui| {
                for (label, values) in [("Offset", offset), ("Rotation", rotation)] {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(label);
                        for value in values {
                            ui.add(egui::DragValue::new(value).speed(0.1));
                        }
                    });
                }
            });
        }
        EffectMacro::LastEffectSetRate { rate } => {
            number_field_ui(ui, "effect_rate", "Rate", rate, 0.1, None);
        }
        EffectMacro::LastEffectSetColor { rgb } | EffectMacro::LastParticleSetColor { rgb } => {
            effect_rgb_ui(ui, rgb);
        }
        EffectMacro::LastEffectSetAlpha { alpha } => {
            number_field_ui(ui, "effect_alpha", "Opacity", alpha, 0.01, None);
        }
        EffectMacro::LastEffectSetOffsetToCameraFlat { offset } => {
            number_field_ui(ui, "effect_camera_offset", "Offset", offset, 0.1,
                Some("Native camera-flat offset argument for the last spawned effect."));
        }
        EffectMacro::LastEffectSetScaleW { values } => {
            ui.label("Scale components");
            egui::ComboBox::from_id_salt("effect_scale_component_count")
                .selected_text(values.len().to_string())
                .show_ui(ui, |ui| {
                    for count in 1..=3 {
                        if ui.selectable_label(values.len() == count, count.to_string()).clicked() {
                            values.resize(count, 1.0);
                        }
                    }
                }).response.on_hover_text("Preserves the native one-, two- or three-value scale form. Changing the count retains existing components and initializes new ones to 1.");
            for (index, value) in values.iter_mut().enumerate() {
                number_field_ui(ui, ("effect_scale_component", index), ["X", "Y", "Z"][index.min(2)], value, 0.1, None);
            }
        }
        EffectMacro::EffectOffKind {
            effect_name,
            fade,
            detach,
        } => {
            ui.label("Stop graphic");
            ui.add(egui::TextEdit::singleline(effect_name).desired_width(130.0));
            ui.checkbox(fade, "Fade");
            ui.checkbox(detach, "Detach");
        }
        EffectMacro::AfterImageOff { arg } => {
            number_field_ui(ui, "trail_end_control", "Control", arg, 1.0,
                Some("Native AFTER_IMAGE_OFF argument, preserved from source. This is not an inferred duration or object ID."));
        }
        EffectMacro::Control(EffectControl::DetachKind { effect_name, unk }) => {
            ui.label("Graphic");
            ui.add(egui::TextEdit::singleline(effect_name).desired_width(130.0));
            ui.label("Native argument");
            ui.add(egui::DragValue::new(unk).range(-1024..=1024))
                .on_hover_text("The native argument is retained without assigning an unverified gameplay meaning.");
        }
        EffectMacro::Control(EffectControl::SetDisableRenderOffsetLast
            | EffectControl::EnableSyncInitPosLast) => {
            ui.weak("Last spawned effect")
                .on_hover_text("Ordered native control with no arguments. Does not create an effect or support flat live injection.");
        }
        EffectMacro::Control(EffectControl::ReqScreen { preset_id }) => {
            ui.label("Screen graphic");
            screen_effect_preset_ui(ui, preset_id);
            ui.weak("?").on_hover_text(
                "Requests the selected native screen graphic. Its three flags stay together; their gameplay meanings are not inferred. Check the mod's graphic resources before game testing.",
            );
        }
        EffectMacro::Control(EffectControl::SetDisableCameraDepthInfluence { value }) => {
            ui.label("Camera depth influence");
            camera_depth_influence_ui(ui, value);
        }
        EffectMacro::Control(EffectControl::OffKindWork { preset_id }) => {
            ui.label("Native effect off");
            native_effect_off_preset_ui(ui, fighter, preset_id);
        }
        EffectMacro::NativeCall(call) => super::native_calls::inspector(ui, call),
        EffectMacro::Color { command, color } => {
            const LABELS: [&str; 7] = ["Flash", "Flash transition", "Burn color", "Burn transition", "Reset burn color", "Flash eyes", "Reset color"];
            let before = command.clone();
            ui.add_enabled_ui(mode_editable, |ui| {
            egui::ComboBox::from_id_salt("fighter_color_command")
                .selected_text(crate::data::COLOR_COMMANDS.iter().position(|(name, _, _)| *name == command).map_or(command.as_str(), |index| LABELS[index]))
                .show_ui(ui, |ui| {
                    for ((name, _, _), label) in crate::data::COLOR_COMMANDS.iter().zip(LABELS) {
                        ui.selectable_value(command, (*name).into(), label).on_hover_text(*name);
                    }
                });
            }).response.on_disabled_hover_text("Imported mixed groups keep their original color command. Color and transition properties remain editable.");
            if *command != before {
                if let Some((transition, rgba)) = crate::data::color_command_layout(command) {
                    color.transition = transition.then_some(color.transition.unwrap_or(1.0));
                    color.rgba = rgba.then_some(color.rgba.unwrap_or([1.0; 4]));
                }
            }
            if let Some(rgba) = &mut color.rgba {
                let mut rgb = [rgba[0], rgba[1], rgba[2]];
                effect_rgb_ui(ui, &mut rgb);
                rgba[..3].copy_from_slice(&rgb);
                number_field_ui(ui, "fighter_color_blend", "Blend", &mut rgba[3], 0.01, None);
            }
            if let Some(frames) = &mut color.transition {
                number_field_ui(ui, "fighter_color_transition", "Frames", frames, 0.1, None);
            }
        }
        command => {
            ui.label("Imported effect command")
                .on_hover_text(format!("{command:?}"));
        }
    });
}

/// Reuse only a checked native trail already present in this move, including nested regions.
/// Never manufacture another fighter's textures or native arguments as a palette default.
fn reusable_trail(graph: &MoveGraph) -> Option<crate::data::EffectMacro> {
    fn find(nodes: &[MoveNode]) -> Option<crate::data::EffectMacro> {
        nodes.iter().find_map(|node| match &node.kind {
            NodeKind::EffectExecute { commands } => commands
                .iter()
                .find(|command| super::trails::settings(command).is_ok())
                .cloned(),
            NodeKind::MixedEffectExecute { commands, .. } => commands.iter().find_map(|slot| {
                let super::model::EffectCommandSlot::Editable(command) = slot else {
                    return None;
                };
                super::trails::settings(command)
                    .is_ok()
                    .then(|| command.clone())
            }),
            NodeKind::Repeat { body, .. } => find(body),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => find(then_nodes).or_else(|| find(else_nodes)),
            _ => None,
        })
    }
    graph
        .tracks
        .iter()
        .filter(|track| track.track == Track::Effect)
        .find_map(|track| find(&track.nodes))
}

fn trail_command_ui(ui: &mut Ui, command: &mut crate::data::EffectMacro, joints: &[String]) {
    let Ok(mut value) = super::trails::settings(command) else {
        ui.weak("Preserved native trail");
        return;
    };
    let before = value.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("Length").on_hover_text(
            "Native unsigned trail-length argument; it is not a timer in motion frames.",
        );
        ui.add(egui::DragValue::new(&mut value.length).range(0..=10_000));
        ui.label("Axis");
        egui::ComboBox::from_id_salt("trail_axis")
            .width(50.0)
            .selected_text(value.axis.trim_start_matches("*EFFECT_AXIS_"))
            .show_ui(ui, |ui| {
                for axis in ["X", "Y"] {
                    ui.selectable_value(&mut value.axis, format!("*EFFECT_AXIS_{axis}"), axis);
                }
            });
    });
    for (index, joint, offset) in [
        (0, &mut value.bone1, &mut value.offset1),
        (1, &mut value.bone2, &mut value.offset2),
    ] {
        ui.push_id(("trail_edge", index), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Edge {}", index + 1));
                joint_picker(ui, joint, joints);
            });
            ui.horizontal_wrapped(|ui| {
                for (axis, component) in ["X", "Y", "Z"].into_iter().zip(offset) {
                    number_field_ui(ui, axis, axis, component, 0.1, None);
                }
            });
        });
    }
    ui.collapsing("Textures and flare", |ui| {
        // Resource names are a small explicit subform, not Rust input. Retain unfinished
        // typing separately so clearing a field does not reset it or alter the valid block.
        let current = [value.texture1.clone(), value.texture2.clone(), value.flare.clone()];
        let id = ui.make_persistent_id("trail_resource_draft");
        let mut draft = ui.data_mut(|data| data.get_temp::<([String; 3], [String; 3])>(id))
            .filter(|(origin, _)| origin == &current)
            .map_or_else(|| current.clone(), |(_, draft)| draft);
        for (index, (label, name)) in ["Texture 1", "Texture 2", "Flare"].into_iter().zip(&mut draft).enumerate() {
            ui.horizontal(|ui| {
                ui.label(label);
                ui.add(egui::TextEdit::singleline(name).id_salt(("trail_resource", index))
                    .desired_width(170.0).char_limit(128));
            });
        }
        let mut proposed = value.clone();
        proposed.texture1 = draft[0].clone();
        proposed.texture2 = draft[1].clone();
        proposed.flare = draft[2].clone();
        let validation = super::trails::update(&mut command.clone(), &proposed);
        if ui.add_enabled(draft != current && validation.is_ok(), egui::Button::new("Update resources"))
            .on_hover_text("Update this draft block, then use Apply nodes to save it. Textures must exist in the character's installed assets; names are not proof of availability.")
            .on_disabled_hover_text(validation.err().unwrap_or_else(|| "No resource changes.".into()))
            .clicked() {
            value = proposed;
        }
        ui.data_mut(|data| data.insert_temp(id, (current, draft)));
        ui.horizontal_wrapped(|ui| {
            ui.label("Flare joint");
            ui.push_id("trail_flare_joint", |ui| joint_picker(ui, &mut value.flare_bone, joints));
            number_field_ui(ui, "trail_flare_scale", "Scale", &mut value.flare_scale, 0.1, None);
        });
        for (label, vector) in [("Offset", &mut value.flare_offset), ("Rotation", &mut value.flare_rotation)] {
            ui.push_id(("trail_flare", label), |ui| {
                ui.label(label);
                ui.horizontal_wrapped(|ui| {
                    for (axis, component) in ["X", "Y", "Z"].into_iter().zip(vector) {
                        number_field_ui(ui, axis, axis, component, 0.1, None);
                    }
                });
            });
        }
    });
    ui.collapsing("Native controls", |ui| {
        ui.checkbox(&mut value.native_bool, "Native option")
            .on_hover_text(
                "Literal boolean from the pinned wrapper. Its gameplay meaning is not inferred.",
            );
        ui.horizontal_wrapped(|ui| {
            for (index, number) in value.native_values.iter_mut().enumerate() {
                number_field_ui(
                    ui,
                    index,
                    &format!("Arg {}", index + 1),
                    number,
                    0.05,
                    Some(
                        "Native f32 argument, retained without assigning an undocumented meaning.",
                    ),
                );
            }
        });
    });
    if value != before {
        if let Err(error) = super::trails::update(command, &value) {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }
}

fn effect_rgb_ui(ui: &mut Ui, rgb: &mut [f32; 3]) {
    let mut picked = rgb.map(|value| value.clamp(0.0, 1.0));
    if ui.color_edit_button_rgb(&mut picked).changed() {
        *rgb = picked;
    }
    for (index, value) in rgb.iter_mut().enumerate() {
        number_field_ui(
            ui,
            ("effect_color", index),
            ["R", "G", "B"][index],
            value,
            0.01,
            None,
        );
    }
}

fn rekey(node: &mut MoveNode, next: &mut u32) {
    node.id = *next;
    *next = next.saturating_add(1);
    match &mut node.kind {
        NodeKind::Repeat { body, .. } => {
            for child in body {
                rekey(child, next);
            }
        }
        NodeKind::Branch {
            then_nodes,
            else_nodes,
            ..
        } => {
            for child in then_nodes.iter_mut().chain(else_nodes.iter_mut()) {
                rekey(child, next);
            }
        }
        _ => {}
    }
}

fn condition_picker(
    ui: &mut Ui,
    fighter: &str,
    track: Track,
    condition: &mut MoveCondition,
    values: &[(String, Vec<crate::gimmicks::behavior::VarDef>)],
) {
    if !super::compound_conditions::within_limits(condition) {
        ui.weak("Condition exceeds editing limits").on_hover_text(
            "The saved condition is unchanged. Replace it explicitly, or reduce its depth and size before Apply."
        );
        if ui.button("Replace with Grounded").clicked() {
            *condition = MoveCondition::Grounded;
        }
        return;
    }
    ui.vertical(|ui| condition_controls(ui, fighter, track, condition, values));
}

pub(super) fn node_edit_error(ui: &mut Ui, error: &str) {
    if error.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        // A short message is shown as it is; only a long one folds away.
        let first: String = error
            .lines()
            .next()
            .unwrap_or(error)
            .chars()
            .take(70)
            .collect();
        ui.colored_label(egui::Color32::LIGHT_RED, first)
            .on_hover_text(error);
        ui.menu_button("Details", |ui| {
            ui.set_max_width(360.0);
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.add(egui::Label::new(error).wrap());
                    });
                });
        });
    });
}

fn condition_part(
    ui: &mut Ui,
    fighter: &str,
    track: Track,
    label: &str,
    condition: &mut MoveCondition,
    values: &[(String, Vec<crate::gimmicks::behavior::VarDef>)],
) {
    ui.push_id(label, |ui| {
        let summary = super::canvas::condition_caption(condition);
        let mut preview: String = summary.chars().take(18).collect();
        if preview.len() < summary.len() {
            preview.push('…');
        }
        egui::CollapsingHeader::new(format!("{label} · {preview}"))
            .id_salt("native_condition_part")
            .default_open(false)
            .show_unindented(ui, |ui| {
                condition_controls(ui, fighter, track, condition, values)
            })
            .header_response
            .on_hover_text(summary);
    });
}

fn hit_check_ui(
    ui: &mut Ui,
    query: &mut super::model::InflictionQuery,
    mask: &mut super::model::InflictionMask,
) {
    use super::model::{InflictionMask, InflictionQuery};
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("hit_check_mask")
            .selected_text(match mask { InflictionMask::Hit => "Hit", InflictionMask::Shield => "Shield" })
            .show_ui(ui, |ui| {
                ui.selectable_value(mask, InflictionMask::Hit, "Hit");
                ui.selectable_value(mask, InflictionMask::Shield, "Shield");
            });
        egui::ComboBox::from_id_salt("hit_check_window")
            .selected_text(match query { InflictionQuery::CurrentHitlag => "During hitlag", InflictionQuery::CurrentStatus => "Current status" })
            .show_ui(ui, |ui| {
                ui.selectable_value(query, InflictionQuery::CurrentHitlag, "During hitlag");
                ui.selectable_value(query, InflictionQuery::CurrentStatus, "Current status");
            }).response.on_hover_text("During hitlag samples AttackModule::is_infliction; Current status samples is_infliction_status. The pinned SDK documents their hitlag/status windows. Hit and Shield use distinct native masks. This runs only where the source branch executes, not on every collision. It has no victim, projectile, damage, hitbox ID, or duplicate-hit tracking; it is not an On attack hit event. Use Not to test absence.");
    });
}

fn condition_controls(
    ui: &mut Ui,
    fighter: &str,
    track: Track,
    condition: &mut MoveCondition,
    values: &[(String, Vec<crate::gimmicks::behavior::VarDef>)],
) {
    let mut kind = match condition {
        MoveCondition::Stick { .. } => 15,
        MoveCondition::IsExecute => {
            ui.weak("Runs whenever the move is running");
            return;
        }
        MoveCondition::Source { header } => {
            ui.weak("The game's own check, kept as written")
                .on_hover_text(header.trim());
            return;
        }
        MoveCondition::ItemHeld => {
            ui.weak("Holding an item");
            return;
        }
        MoveCondition::Grounded => 0,
        MoveCondition::Airborne => 1,
        MoveCondition::WorkFlag { .. } => 2,
        MoveCondition::ButtonHeld { .. } => 3,
        MoveCondition::WorkInt { .. } => 4,
        MoveCondition::WorkFloat { .. } => 5,
        MoveCondition::And { .. } => 6,
        MoveCondition::Or { .. } => 7,
        MoveCondition::Not(_) => 8,
        MoveCondition::Group(_) => 9,
        MoveCondition::ArticleExists { .. } => 10,
        MoveCondition::MotionChanging => 11,
        MoveCondition::NativeLrNegative => 12,
        MoveCondition::AttackInfliction { .. } => 13,
        MoveCondition::CharacterValue { .. } => 14,
        MoveCondition::InStatus { .. } => 16,
        MoveCondition::HoldingItemKind { .. } => 17,
    };
    let old = kind;
    let float_sources = native_work_choices(
        fighter,
        crate::gimmicks::registry::IdentifierKind::WorkFloat,
    );
    let article_sources: Vec<_> = crate::gimmicks::registry::article_entries()
        .filter(|entry| entry.fighter == fighter)
        .collect();
    let labels = [
        "On the ground",
        "In the air",
        "A game flag",
        "A button is held",
        "A game number",
        "A game decimal",
        "All of these",
        "Any of these",
        "The opposite of",
        "A group of checks",
        "An object is out",
        "The animation just changed",
        "Facing left",
        "The attack connected",
        "A gimmick value",
        "Stick direction",
        "During an action",
        "Holding a certain item",
    ];
    egui::ComboBox::from_id_salt("condition_kind")
        .selected_text(labels[kind])
        .width(140.0)
        .height(300.0)
        .truncate()
        .show_ui(ui, |ui| {
            for (index, label) in labels.into_iter().enumerate() {
                let available = (index != 5 || !float_sources.is_empty())
                    && (index != 10 || !article_sources.is_empty())
                    && (index != 11 || track == Track::Game)
                    && (index != 12 || track != Track::Sound)
                    && (index != 13 || track == Track::Game)
                    && (index != 14 || !values.is_empty());
                ui.add_enabled_ui(available, |ui| {
                    ui.selectable_value(&mut kind, index, label)
                        .on_disabled_hover_text(if index == 14 {
                            "This character has no controller variables."
                        } else if index == 13 {
                            "Advanced"
                        } else if index == 12 {
                            "Native facing tests are supported on Game, Effect and Expression tracks."
                        } else if index == 11 {
                            "Motion changing is supported on Game tracks only."
                        } else if index == 10 {
                            "This donor has no registered articles."
                        } else {
                            "This fighter has no registered native float sources."
                        });
                });
            }
        });
    if kind != old {
        *condition = match kind {
            0 => MoveCondition::Grounded,
            1 => MoveCondition::Airborne,
            2 => MoveCondition::WorkFlag {
                symbol: native_condition_flag_sources(fighter)
                    .first()
                    .map(|symbol| (*symbol).to_owned())
                    .unwrap_or_default(),
                expected: true,
            },
            3 => MoveCondition::ButtonHeld {
                button: InputButton::Special,
            },
            4 => MoveCondition::WorkInt {
                symbol: super::native_conditions::common_int_reads()[1].into(),
                op: super::model::NativeComparison::Eq,
                value: super::model::NativeIntOperand::Literal(0),
            },
            5 => MoveCondition::WorkFloat {
                symbol: float_sources
                    .first()
                    .map(|symbol| (*symbol).to_owned())
                    .unwrap_or_default(),
                op: super::model::NativeComparison::Gte,
                value: 0.0,
            },
            6 => MoveCondition::And {
                left: Box::new(MoveCondition::Grounded),
                right: Box::new(MoveCondition::ButtonHeld {
                    button: InputButton::Special,
                }),
            },
            7 => MoveCondition::Or {
                left: Box::new(MoveCondition::ButtonHeld {
                    button: InputButton::Attack,
                }),
                right: Box::new(MoveCondition::ButtonHeld {
                    button: InputButton::Special,
                }),
            },
            8 => MoveCondition::Not(Box::new(MoveCondition::Grounded)),
            10 => MoveCondition::ArticleExists {
                symbol: article_sources
                    .first()
                    .map(|entry| entry.symbol.to_owned())
                    .unwrap_or_default(),
            },
            11 => MoveCondition::MotionChanging,
            15 => MoveCondition::Stick {
                axis: super::model::StickAxis::X,
                op: super::model::NativeComparison::Gte,
                value: 0.5,
                relative_to_facing: true,
            },
            12 => MoveCondition::NativeLrNegative,
            16 => MoveCondition::InStatus {
                symbol: crate::gimmicks::registry::entries_for(
                    fighter,
                    crate::gimmicks::registry::IdentifierKind::FighterStatus,
                )
                .next()
                .map(|entry| entry.symbol.to_owned())
                .unwrap_or_default(),
            },
            17 => MoveCondition::HoldingItemKind {
                item: "ITEM_KIND_BEAMSWORD".into(),
            },
            13 => MoveCondition::AttackInfliction {
                query: super::model::InflictionQuery::CurrentStatus,
                mask: super::model::InflictionMask::Hit,
            },
            14 => {
                let (chart, vars) = &values[0];
                MoveCondition::CharacterValue {
                    chart_id: chart.clone(),
                    var: vars[0].name.clone(),
                    op: super::model::NativeComparison::Eq,
                    value: character_constant(vars[0].kind),
                }
            }
            _ => MoveCondition::Group(Box::new(MoveCondition::Grounded)),
        };
    }
    match condition {
        MoveCondition::IsExecute => {}
        MoveCondition::ItemHeld => {}
        MoveCondition::InStatus { symbol } => {
            crate::gimmicks::registry::fighter_status_picker(ui, fighter, symbol)
        }
        MoveCondition::HoldingItemKind { item } => crate::gimmicks::registry::item_picker(ui, item),
        MoveCondition::CharacterValue {
            chart_id,
            var,
            op,
            value,
        } => {
            character_value_picker(ui, values, chart_id, var, op, value);
        }
        MoveCondition::AttackInfliction { query, mask } => hit_check_ui(ui, query, mask),
        MoveCondition::NativeLrNegative => {
            ui.weak("Native facing comparison").on_hover_text(
                "Tests get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 at this point in the move. The exact native sign test is retained without guessing Left or Right. Use Then/Else or Not to compose it. this is not a live event or a writable value. Sound tracks are unsupported.",
            );
        }
        MoveCondition::MotionChanging => {
            ui.weak("Native motion-change state").on_hover_text("Exact MotionModule::is_changing read at this point in a Game track. It is a condition, not a motion-change event or a frame clock. other tracks remain unsupported.");
        }
        MoveCondition::ArticleExists { symbol } => {
            work_symbol_picker(
                ui,
                "condition_article",
                fighter,
                symbol,
                crate::gimmicks::registry::IdentifierKind::Article,
                false,
            );
            ui.weak("Checks presence at this point in the move").on_hover_text(
                "Evaluates the donor-owned article in the native ACMD branch. It is not an article-appearance event, live object selector, instance count, or confirmed projectile hit. Use Not to require absence.",
            );
        }
        MoveCondition::WorkFlag { symbol, expected } => {
            work_symbol_choices_picker(
                ui,
                "condition_flag",
                symbol,
                crate::gimmicks::registry::IdentifierKind::WorkFlag,
                false,
                &native_condition_flag_sources(fighter),
            );
            ui.checkbox(expected, "Is on");
            if super::native_work::read_only_condition_flags().contains(&symbol.as_str()) {
                ui.weak("Read-only").on_hover_text("Reads this exact native flag at this point in the move. Set tests true; unchecked tests false.");
            }
        }
        MoveCondition::Stick {
            axis,
            op,
            value,
            relative_to_facing,
        } => {
            ui.horizontal_wrapped(|ui| {
                let mut choice = match (*axis, *relative_to_facing) {
                    (super::model::StickAxis::X, true) => 0,
                    (super::model::StickAxis::X, false) => 1,
                    (super::model::StickAxis::Y, _) => 2,
                };
                let labels = ["Stick forward", "Stick right", "Stick up"];
                egui::ComboBox::from_id_salt("condition_stick_axis")
                    .selected_text(labels[choice])
                    .show_ui(ui, |ui| {
                        for (index, label) in labels.into_iter().enumerate() {
                            ui.selectable_value(&mut choice, index, label);
                        }
                    });
                (*axis, *relative_to_facing) = match choice {
                    0 => (super::model::StickAxis::X, true),
                    1 => (super::model::StickAxis::X, false),
                    _ => (super::model::StickAxis::Y, false),
                };
                native_comparison_picker(ui, op);
                ui.add(egui::DragValue::new(value).speed(0.05).range(-1.0..=1.0));
            });
        }
        MoveCondition::ButtonHeld { button } => {
            egui::ComboBox::from_id_salt("condition_button")
                .selected_text(format!("{button:?}"))
                .show_ui(ui, |ui| {
                    for value in [
                        InputButton::Attack,
                        InputButton::Special,
                        InputButton::Jump,
                        InputButton::Guard,
                        InputButton::Grab,
                    ] {
                        ui.selectable_value(button, value, format!("{value:?}"));
                    }
                });
        }
        MoveCondition::WorkInt { symbol, op, value } => {
            use super::model::NativeIntOperand;
            let kind = crate::gimmicks::registry::IdentifierKind::WorkInt;
            let choices = native_condition_int_sources(fighter);
            work_symbol_choices_picker(ui, "condition_int", symbol, kind, false, &choices);
            let enum_choices = super::native_conditions::integer_choices(fighter, symbol);
            let labels = ["Number", "Fighter kind", "Native choice"];
            let mut operand = match value {
                NativeIntOperand::Literal(_) => 0,
                NativeIntOperand::FighterKind(_) => 1,
                NativeIntOperand::Choice(_) => 2,
            };
            let before = operand;
            ui.horizontal_wrapped(|ui| {
                native_comparison_picker(ui, op);
                egui::ComboBox::from_id_salt("condition_int_operand")
                    .width(100.0)
                    .truncate()
                    .selected_text(labels[operand])
                    .show_ui(ui, |ui| {
                        for (index, label) in labels.into_iter().enumerate() {
                            ui.add_enabled_ui(index != 2 || !enum_choices.is_empty(), |ui| {
                                ui.selectable_value(&mut operand, index, label)
                                    .on_disabled_hover_text(
                                        "This source has no reviewed native choices.",
                                    );
                            });
                        }
                    });
            });
            if operand != before {
                *value = match operand {
                    1 => NativeIntOperand::FighterKind("kirby".into()),
                    2 => NativeIntOperand::Choice(enum_choices[0].symbol.into()),
                    _ => NativeIntOperand::Literal(0),
                };
            }
            match value {
                NativeIntOperand::Literal(value) => {
                    ui.add(egui::DragValue::new(value));
                }
                NativeIntOperand::FighterKind(owner) => {
                    egui::ComboBox::from_id_salt("condition_fighter_kind")
                        .selected_text(owner.as_str())
                        .width(140.0)
                        .truncate()
                        .show_ui(ui, |ui| {
                            for entry in crate::gimmicks::registry::FIGHTER_KINDS {
                                ui.selectable_value(owner, entry.fighter.into(), entry.fighter)
                                    .on_hover_text(entry.symbol);
                            }
                        });
                }
                NativeIntOperand::Choice(choice) => {
                    egui::ComboBox::from_id_salt("condition_native_choice")
                        .selected_text(
                            enum_choices
                                .iter()
                                .find(|entry| entry.symbol == choice)
                                .map_or("Saved choice", |entry| entry.label),
                        )
                        .width(140.0)
                        .truncate()
                        .show_ui(ui, |ui| {
                            for entry in enum_choices {
                                ui.selectable_value(choice, entry.symbol.into(), entry.label)
                                    .on_hover_text(entry.symbol);
                            }
                        })
                        .response
                        .on_hover_text(choice.as_str());
                    if !enum_choices.iter().any(|entry| entry.symbol == choice) {
                        ui.weak("Choose a value for this source").on_hover_text(
                            "The saved native choice is preserved, but does not match this source. Select a reviewed choice or switch to Number before Apply."
                        );
                    }
                }
            }
            native_branch_hint(ui);
        }
        MoveCondition::WorkFloat { symbol, op, value } => {
            work_symbol_picker(
                ui,
                "condition_float",
                fighter,
                symbol,
                crate::gimmicks::registry::IdentifierKind::WorkFloat,
                false,
            );
            ui.horizontal_wrapped(|ui| {
                native_comparison_picker(ui, op);
                ui.add(egui::DragValue::new(value).speed(0.1));
            });
            native_branch_hint(ui);
        }
        MoveCondition::And { left, right } | MoveCondition::Or { left, right } => {
            condition_part(ui, fighter, track, "First", left, values);
            condition_part(ui, fighter, track, "Second", right, values);
        }
        MoveCondition::Not(inner) | MoveCondition::Group(inner) => {
            condition_part(ui, fighter, track, "Condition", inner, values);
        }
        _ => {}
    }
}

fn character_constant(kind: VarType) -> super::model::CharacterValue {
    use super::model::CharacterValue;
    match kind {
        VarType::Int | VarType::Timer => CharacterValue::Int(0),
        VarType::Float => CharacterValue::Float(0.0),
        VarType::Flag => CharacterValue::Flag(true),
    }
}

fn character_value_picker(
    ui: &mut Ui,
    choices: &[(String, Vec<crate::gimmicks::behavior::VarDef>)],
    chart: &mut String,
    var: &mut String,
    op: &mut super::model::NativeComparison,
    value: &mut super::model::CharacterValue,
) {
    use super::model::{CharacterValue, NativeComparison};
    ui.horizontal_wrapped(|ui| {
        let previous_chart = chart.clone();
        egui::ComboBox::from_id_salt("character_controller")
            .width(100.0)
            .selected_text(chart.as_str())
            .show_ui(ui, |ui| {
                for (id, _) in choices {
                    ui.selectable_value(chart, id.clone(), id);
                }
            });
        let Some((_, vars)) = choices.iter().find(|(id, _)| id == chart) else {
            ui.weak("Missing controller");
            return;
        };
        if *chart != previous_chart {
            if let Some(first) = vars.first() {
                *var = first.name.clone();
                *value = character_constant(first.kind);
                *op = NativeComparison::Eq;
            }
        }
        let previous_var = var.clone();
        egui::ComboBox::from_id_salt("character_variable")
            .width(100.0)
            .selected_text(var.as_str())
            .show_ui(ui, |ui| {
                for definition in vars {
                    ui.selectable_value(var, definition.name.clone(), &definition.name);
                }
            });
        if *var != previous_var {
            if let Some(definition) = vars.iter().find(|item| item.name == *var) {
                *value = character_constant(definition.kind);
                *op = NativeComparison::Eq;
            }
        }
        if !vars
            .iter()
            .any(|item| item.name == *var && super::character_values::compatible(item.kind, *value))
        {
            ui.weak("Missing variable or changed type");
        }
        egui::ComboBox::from_id_salt("character_comparison")
            .width(50.0)
            .selected_text(op.token())
            .show_ui(ui, |ui| {
                for choice in NativeComparison::ALL {
                    if !matches!(value, CharacterValue::Flag(_))
                        || matches!(choice, NativeComparison::Eq | NativeComparison::Ne)
                    {
                        ui.selectable_value(op, choice, choice.token());
                    }
                }
            });
        match value {
            CharacterValue::Int(value) => {
                ui.add(egui::DragValue::new(value));
            }
            CharacterValue::Float(value) => {
                ui.add(egui::DragValue::new(value).speed(0.1));
            }
            CharacterValue::Flag(value) => {
                ui.checkbox(value, "Set");
            }
        }
    });
}

fn native_comparison_picker(ui: &mut Ui, op: &mut super::model::NativeComparison) {
    egui::ComboBox::from_id_salt("native_comparison")
        .selected_text(super::plain::compare(*op))
        .width(100.0)
        .show_ui(ui, |ui| {
            for choice in super::model::NativeComparison::ALL {
                ui.selectable_value(op, choice, super::plain::compare(choice));
            }
        });
}

fn native_branch_hint(ui: &mut Ui) {
    ui.weak("?").on_hover_text(
        "Reads native state at this move's branch. It does not write a resource or infer a hit, spend or refill event. Status-dependent fields require their original valid status context; this is not a live character-controller input."
    );
}

#[cfg(test)]
mod tests {
    use crate::move_nodes::test_support as fixture;
    use crate::move_nodes::test_support::assert_json_eq;

    use super::*;

    fn native_expression_frame(
        context: &egui::Context,
        fighter: &str,
        value: &mut Expr,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(282.0, 400.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let kind = expression_kind(value, &[]);
                    expression_block_ui(ui, fighter, value, &[], kind);
                });
            },
        )
    }

    fn click_native_expression(
        context: &egui::Context,
        fighter: &str,
        value: &mut Expr,
        label: &str,
    ) {
        native_expression_frame(context, fighter, value, Vec::new());
        let output = native_expression_frame(context, fighter, value, Vec::new());
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| text_position(&shape.shape, label))
            .unwrap_or_else(|| panic!("missing native-expression control {label:?}"));
        for pressed in [true, false] {
            native_expression_frame(
                context,
                fighter,
                value,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    #[test]
    fn native_input_selector_requires_explicit_source_and_retains_donor_mismatches() {
        use crate::gimmicks::native_inputs::NativeInput;
        let context = egui::Context::default();
        let mut value = Expr::Const(ConstValue::Float(7.0));
        click_native_expression(&context, "cloud", &mut value, "Number");
        click_native_expression(&context, "cloud", &mut value, "Read native value");
        assert!(matches!(
            &value,
            Expr::NativeFloat(NativeInput::Unsupported(_))
        ));
        click_native_expression(&context, "cloud", &mut value, "Choose source…");
        click_native_expression(&context, "cloud", &mut value, "Search sources…");
        let searched = native_expression_frame(
            &context,
            "cloud",
            &mut value,
            vec![egui::Event::Text("LIMIT_GAUGE".into())],
        );
        assert!(
            searched
                .shapes
                .iter()
                .any(|shape| { text_position(&shape.shape, "Limit gauge").is_some() }),
            "clicking and typing in search must keep the source popup open"
        );
        assert!(matches!(
            &value,
            Expr::NativeFloat(NativeInput::Unsupported(_))
        ));
        click_native_expression(&context, "cloud", &mut value, "Limit gauge");
        assert_eq!(value, Expr::NativeFloat(NativeInput::CloudLimitGauge));
        let original = serde_json::to_value(&value).unwrap();
        for _ in 0..3 {
            let output = native_expression_frame(&context, "mario", &mut value, Vec::new());
            assert!(output.shapes.iter().any(|shape| text_position(
                &shape.shape,
                "Unsupported source"
            )
            .is_some()));
            assert_eq!(serde_json::to_value(&value).unwrap(), original);
        }
    }

    #[test]
    fn integer_native_selector_is_explicit_and_excludes_float_and_flag_sources() {
        use crate::gimmicks::native_inputs::NativeInput;
        let context = egui::Context::default();
        let mut value = Expr::Const(ConstValue::Int(7));
        click_native_expression(&context, "cloud", &mut value, "Number");
        click_native_expression(&context, "cloud", &mut value, "Read native value");
        assert!(matches!(
            &value,
            Expr::NativeInt(NativeInput::Unsupported(_))
        ));
        click_native_expression(&context, "cloud", &mut value, "Choose source…");
        let output = native_expression_frame(&context, "cloud", &mut value, Vec::new());
        for excluded in ["Limit gauge", "Limit active"] {
            assert!(!output
                .shapes
                .iter()
                .any(|shape| text_position(&shape.shape, excluded).is_some()));
        }
        click_native_expression(&context, "cloud", &mut value, "Limit clear-frame value");
        assert_eq!(value, Expr::NativeInt(NativeInput::CloudLimitClearFrame));
        let original = serde_json::to_value(&value).unwrap();
        let output = native_expression_frame(&context, "mario", &mut value, Vec::new());
        assert!(output.shapes.iter().any(|shape| text_position(
            &shape.shape,
            "Unsupported source"
        )
        .is_some()));
        assert_eq!(serde_json::to_value(&value).unwrap(), original);
        assert_eq!(serde_json::from_value::<Expr>(original).unwrap(), value);
    }

    fn native_guard_frame(
        context: &egui::Context,
        fighter: &str,
        guard: &mut Predicate,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(282.0, 500.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .show_inside(ui, |ui| guard_block_ui(ui, fighter, guard, &[]));
            },
        )
    }

    fn click_native_guard(
        context: &egui::Context,
        fighter: &str,
        guard: &mut Predicate,
        label: &str,
    ) {
        native_guard_frame(context, fighter, guard, Vec::new());
        let output = native_guard_frame(context, fighter, guard, Vec::new());
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| text_position(&shape.shape, label))
            .unwrap_or_else(|| panic!("missing native-guard control {label:?}"));
        for pressed in [true, false] {
            native_guard_frame(
                context,
                fighter,
                guard,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    #[test]
    fn native_flag_selector_outputs_a_condition_and_preserves_foreign_or_wrong_kind_sources() {
        use crate::gimmicks::native_inputs::NativeInput;
        let context = egui::Context::default();
        let mut guard = default_comparison();
        click_native_guard(&context, "cloud", &mut guard, "True");
        click_native_guard(&context, "cloud", &mut guard, "Read native value");
        assert!(matches!(
            &guard,
            Predicate::NativeFlag(NativeInput::Unsupported(_))
        ));
        click_native_guard(&context, "cloud", &mut guard, "Choose source…");
        let output = native_guard_frame(&context, "cloud", &mut guard, Vec::new());
        for excluded in ["Limit gauge", "Limit clear-frame value"] {
            assert!(!output
                .shapes
                .iter()
                .any(|shape| text_position(&shape.shape, excluded).is_some()));
        }
        click_native_guard(&context, "cloud", &mut guard, "Limit active");
        assert_eq!(guard, Predicate::NativeFlag(NativeInput::CloudLimitBreak));
        for (fighter, source) in [
            ("mario", NativeInput::CloudLimitBreak),
            ("cloud", NativeInput::CloudLimitGauge),
            (
                "cloud",
                NativeInput::Unsupported("future_native_flag".into()),
            ),
        ] {
            guard = Predicate::NativeFlag(source);
            let before = serde_json::to_value(&guard).unwrap();
            let output = native_guard_frame(&context, fighter, &mut guard, Vec::new());
            assert_eq!(serde_json::to_value(&guard).unwrap(), before);
            assert!(output.shapes.iter().any(|shape| text_position(
                &shape.shape,
                "Unsupported source"
            )
            .is_some()));
            assert!(output.shapes.iter().any(|shape| text_position(
                &shape.shape,
                "Read-only · Flag"
            )
            .is_some()));
        }
        guard = Predicate::NativeFlag(NativeInput::CloudLimitBreak);
        click_native_guard(&context, "cloud", &mut guard, "Read native value");
        click_native_guard(&context, "cloud", &mut guard, "Not");
        assert!(
            matches!(guard, Predicate::Not(inner) if *inner == Predicate::NativeFlag(NativeInput::CloudLimitBreak))
        );
    }

    fn native_condition_test_cases() -> Vec<(&'static str, MoveCondition)> {
        let costume_source = "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND";
        let mut cases = vec![
            (
                "cloud",
                MoveCondition::WorkInt {
                    symbol: "FIGHTER_INSTANCE_WORK_ID_INT_COLOR".into(),
                    op: NativeComparison::Ne,
                    value: NativeIntOperand::Literal(-17),
                },
            ),
            (
                "cloud",
                MoveCondition::WorkInt {
                    symbol: "FIGHTER_INSTANCE_WORK_ID_INT_KIND".into(),
                    op: NativeComparison::Eq,
                    value: NativeIntOperand::FighterKind("kirby".into()),
                },
            ),
            (
                "cloud",
                MoveCondition::WorkFloat {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                    op: NativeComparison::Gte,
                    value: -12.75,
                },
            ),
            (
                "cloud",
                MoveCondition::WorkFloat {
                    symbol: "UNKNOWN_IMPORTED_SOURCE_WITH_A_VERY_LONG_NAME".into(),
                    op: NativeComparison::Eq,
                    value: 87.5,
                },
            ),
            (
                "bayonetta",
                MoveCondition::WorkInt {
                    symbol: costume_source.into(),
                    op: NativeComparison::Ne,
                    value: NativeIntOperand::Choice(
                        "UNKNOWN_IMPORTED_CHOICE_WITH_A_VERY_LONG_NAME".into(),
                    ),
                },
            ),
            (
                "cloud",
                MoveCondition::WorkInt {
                    symbol: "FIGHTER_INSTANCE_WORK_ID_INT_COLOR".into(),
                    op: NativeComparison::Eq,
                    value: NativeIntOperand::Choice(
                        "FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT".into(),
                    ),
                },
            ),
        ];
        cases.extend(
            super::super::native_conditions::integer_choices("bayonetta", costume_source)
                .iter()
                .map(|choice| {
                    (
                        "bayonetta",
                        MoveCondition::WorkInt {
                            symbol: costume_source.into(),
                            op: NativeComparison::Ne,
                            value: NativeIntOperand::Choice(choice.symbol.into()),
                        },
                    )
                }),
        );
        for fighter in ["ike", "roy"] {
            cases.extend(
                super::super::native_conditions::status_int_reads(fighter)
                    .iter()
                    .map(|source| {
                        (
                            fighter,
                            MoveCondition::WorkInt {
                                symbol: (*source).into(),
                                op: NativeComparison::Eq,
                                value: NativeIntOperand::Literal(3),
                            },
                        )
                    }),
            );
        }
        cases.extend(
            [
                ("robot", "FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO"),
                ("mario", "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL"),
                ("eflame", "FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD"),
                ("cloud", "FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO"),
                ("unknown", "FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO"),
                ("robot", ""),
            ]
            .into_iter()
            .map(|(fighter, symbol)| {
                (
                    fighter,
                    MoveCondition::ArticleExists {
                        symbol: symbol.into(),
                    },
                )
            }),
        );
        let leaves = cases.clone();
        for (fighter, leaf) in leaves {
            cases.push((
                fighter,
                MoveCondition::And {
                    left: Box::new(MoveCondition::Grounded),
                    right: Box::new(leaf.clone()),
                },
            ));
            cases.push((
                fighter,
                MoveCondition::Or {
                    left: Box::new(leaf.clone()),
                    right: Box::new(MoveCondition::Not(Box::new(MoveCondition::Airborne))),
                },
            ));
            cases.push((fighter, MoveCondition::Not(Box::new(leaf.clone()))));
            cases.push((fighter, MoveCondition::Group(Box::new(leaf))));
        }
        let too_deep = (0..=super::super::compound_conditions::MAX_DEPTH)
            .fold(MoveCondition::Grounded, |inner, _| {
                MoveCondition::Group(Box::new(inner))
            });
        cases.push(("cloud", too_deep));
        cases
    }

    #[test]
    fn native_condition_inspectors_do_not_normalize_imported_values() {
        for width in [282.0, 354.0] {
            for (fighter, mut condition) in native_condition_test_cases() {
                let original = serde_json::to_value(&condition).unwrap();
                let context = egui::Context::default();
                for _ in 0..3 {
                    let _ = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 260.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            egui::CentralPanel::default().show_inside(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    condition_picker(ui, fighter, Track::Game, &mut condition, &[])
                                });
                            });
                        },
                    );
                    assert_eq!(serde_json::to_value(&condition).unwrap(), original);
                }
            }
        }
        assert!(native_work_choices(
            "mario",
            crate::gimmicks::registry::IdentifierKind::WorkFloat
        )
        .is_empty());
        assert!(!native_work_choices(
            "cloud",
            crate::gimmicks::registry::IdentifierKind::WorkFloat
        )
        .is_empty());
        for read in super::super::native_conditions::common_int_reads() {
            assert!(
                !native_work_choices("cloud", crate::gimmicks::registry::IdentifierKind::WorkInt)
                    .contains(read),
                "source-only read leaked into write picker"
            );
        }
        for fighter in ["ike", "roy"] {
            for source in super::super::native_conditions::status_int_reads(fighter) {
                assert!(native_condition_int_sources(fighter).contains(source));
                assert!(!native_work_choices(
                    fighter,
                    crate::gimmicks::registry::IdentifierKind::WorkInt
                )
                .contains(source));
                let foreign = if fighter == "ike" { "roy" } else { "ike" };
                assert!(!native_condition_int_sources(foreign).contains(source));
            }
        }
        assert!(native_condition_int_sources("unknown").is_empty());
    }

    #[test]
    fn compound_child_disclosure_stays_open_when_its_caption_changes() {
        let context = egui::Context::default();
        let mut condition = MoveCondition::Grounded;
        let render = |condition: &mut MoveCondition, events| {
            context.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    condition_part(ui, "mario", Track::Game, "First", condition, &[]);
                },
            )
        };
        render(&mut condition, Vec::new());
        let output = render(&mut condition, Vec::new());
        let position = output
            .shapes
            .iter()
            .find_map(|shape| text_position(&shape.shape, "First · on the ground"))
            .expect("child disclosure is visible");
        for pressed in [true, false] {
            render(
                &mut condition,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        context.global_style_mut(|style| style.animation_time = 0.0);
        condition = MoveCondition::ButtonHeld {
            button: InputButton::Special,
        };
        for _ in 0..3 {
            render(&mut condition, Vec::new());
        }
        let output = render(&mut condition, Vec::new());
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| { text_position(&shape.shape, "A button is held").is_some() }),
            "changing the summary closed the child controls"
        );
        assert!(matches!(
            condition,
            MoveCondition::ButtonHeld {
                button: InputButton::Special
            }
        ));
    }

    #[test]
    fn rejected_edit_details_are_wrapped_without_justified_symbol_spacing() {
        let context = egui::Context::default();
        let error = "bayonetta.tracks[0].nodes[1]#2: native choice FIGHTER_BAYONETTA_COSTUME_KIND_JEANNE does not match FIGHTER_INSTANCE_WORK_ID_INT_COLOR";
        let render = |events| {
            context.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    node_edit_error(ui, error);
                },
            )
        };
        render(Vec::new());
        let output = render(Vec::new());
        let position = output
            .shapes
            .iter()
            .find_map(|shape| text_position(&shape.shape, "Details"))
            .expect("error details are reachable");
        for pressed in [true, false] {
            render(vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ]);
        }
        for _ in 0..3 {
            render(Vec::new());
        }
        let output = render(Vec::new());
        let details = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text == error => Some(text),
                _ => None,
            })
            .expect("details show the full diagnostic");
        assert!(!details.galley.job.justify);
        assert!(
            details.galley.size().x <= 360.0,
            "{:?}",
            details.galley.size()
        );
    }

    #[test]
    fn receiver_field_inspection_preserves_scoped_reads_without_global_variable_aliases() {
        let context = egui::Context::default();
        let mut expression = Expr::EventField {
            event: "ready".into(),
            field: "amount".into(),
        };
        let mut guard = Predicate::EventFlag {
            event: "ready".into(),
            field: "enabled".into(),
        };
        let before = (
            serde_json::to_value(&expression).unwrap(),
            serde_json::to_value(&guard).unwrap(),
        );
        let _ = context.run_ui(Default::default(), |ui| {
            expression_block_ui(ui, "mario", &mut expression, &[], VarType::Float);
            guard_block_ui(ui, "mario", &mut guard, &[]);
        });
        assert_eq!(serde_json::to_value(&expression).unwrap(), before.0);
        assert_eq!(serde_json::to_value(&guard).unwrap(), before.1);
        let oversized = "x".repeat(1_048_576);
        assert!(bounded_event_label(&oversized).len() < 64);
        let _ = context.run_ui(Default::default(), |ui| {
            event_read_ui(ui, &oversized, &oversized)
        });
    }

    #[test]
    fn timed_event_inspector_preserves_missing_names_and_apply_rejects_them() {
        let mut command = ExcuteStmt::GimmickEvent {
            name: "unbound".into(),
        };
        let context = egui::Context::default();
        let before = serde_json::to_value(&command).unwrap();
        for _ in 0..2 {
            let _ = context.run_ui(Default::default(), |ui| {
                command_ui(ui, "mario", &mut command, CommandOptions::default())
            });
        }
        assert_eq!(serde_json::to_value(&command).unwrap(), before);
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        view.draft.tracks[0]
            .nodes
            .push(fixture::execute(90, vec![command]));
        assert!(click(&mut view, &context, "Apply nodes").is_none());
        assert!(
            view.error.contains("unbound")
                && view.error.contains("no matching compiled event binding"),
            "{}",
            view.error
        );
    }

    fn text_position(shape: &egui::epaint::Shape, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.size() / 2.0)
            }
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().find_map(|shape| text_position(shape, label))
            }
            _ => None,
        }
    }

    fn click(view: &mut NodeEditor, context: &egui::Context, label: &str) -> Option<MoveGraph> {
        click_with_recipes(view, context, label, &BTreeMap::new())
    }

    fn click_with_recipes(
        view: &mut NodeEditor,
        context: &egui::Context,
        label: &str,
        siblings: &BTreeMap<String, MoveGraph>,
    ) -> Option<MoveGraph> {
        // Existing property-form tests intentionally exercise the optional list view.
        view.list_mode = true;
        let script = AcmdScript {
            stmts: vec![
                crate::data::AcmdStmt::Frame(3.0),
                crate::data::AcmdStmt::Excute(vec![ExcuteStmt::Attack(
                    crate::data::Hitbox::default().to_attack_call(),
                )]),
            ],
        };
        let base = BehaviorGraph::default();
        let mut position = None;
        for _ in 0..2 {
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(460.0, 1800.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    view.ui(
                        ui,
                        "mario",
                        "attack11",
                        None,
                        [&script, &AcmdScript::default(), &AcmdScript::default()],
                        &crate::data::EffectScript::default(),
                        &base,
                        siblings,
                    );
                },
            );
            position = output
                .shapes
                .iter()
                .find_map(|shape| text_position(&shape.shape, label));
        }
        let position = position.unwrap_or_else(|| panic!("button {label:?} not rendered"));
        let mut result = None;
        for pressed in [true, false] {
            let _ = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(460.0, 1800.0),
                    )),
                    events: vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    if let Some(graph) = view.ui(
                        ui,
                        "mario",
                        "attack11",
                        None,
                        [&script, &AcmdScript::default(), &AcmdScript::default()],
                        &crate::data::EffectScript::default(),
                        &base,
                        siblings,
                    ) {
                        result = Some(graph);
                    }
                },
            );
        }
        result
    }

    #[test]
    fn pointer_undo_redo_restores_the_entire_imported_draft() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        assert!(!view.list_mode, "the field is the default workspace");
        click(&mut view, &context, "Import current move");
        let imported = serde_json::to_value(&view.draft).unwrap();
        click(&mut view, &context, "Undo");
        assert!(view.draft.is_empty());
        click(&mut view, &context, "Redo");
        assert_eq!(serde_json::to_value(&view.draft).unwrap(), imported);
    }

    #[test]
    fn numeric_typing_is_one_undo_step_on_enter_or_direct_undo() {
        fn frame(
            context: &egui::Context,
            view: &mut NodeEditor,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(460.0, 1800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    view.ui(
                        ui,
                        "mario",
                        "attack11",
                        None,
                        [&AcmdScript::default(); 3],
                        &crate::data::EffectScript::default(),
                        &BehaviorGraph::default(),
                        &BTreeMap::new(),
                    );
                },
            )
        }
        fn press(context: &egui::Context, view: &mut NodeEditor, label: &str) {
            frame(context, view, vec![]);
            let output = frame(context, view, vec![]);
            let point = output
                .shapes
                .iter()
                .find_map(|shape| text_position(&shape.shape, label))
                .unwrap_or_else(|| panic!("missing control {label:?}"));
            for pressed in [true, false] {
                frame(
                    context,
                    view,
                    vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
        }
        fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }
        }
        for (opponent_slow, commit_with_enter) in
            [(false, true), (false, false), (true, true), (true, false)]
        {
            let context = egui::Context::default();
            let mut view = NodeEditor {
                key: "mario/attack11".into(),
                list_mode: true,
                draft: MoveGraph {
                    tracks: vec![fixture::track(
                        Track::Game,
                        vec![fixture::node(
                            1,
                            if opponent_slow {
                                NodeKind::Execute {
                                    commands: vec![default_command(90)],
                                }
                            } else {
                                NodeKind::Frame { frame: 987.0 }
                            },
                        )],
                    )],
                    ..Default::default()
                },
                ..Default::default()
            };
            let before = serde_json::to_value(&view.draft).unwrap();
            press(
                &context,
                &mut view,
                if opponent_slow { "13.0" } else { "987" },
            );
            assert!(context.text_edit_focused());
            frame(
                &context,
                &mut view,
                vec![key(egui::Key::A, egui::Modifiers::COMMAND)],
            );
            frame(&context, &mut view, vec![egui::Event::Text("1".into())]);
            frame(&context, &mut view, vec![]);
            frame(&context, &mut view, vec![egui::Event::Text("2".into())]);
            let expected = if opponent_slow {
                NodeKind::Execute {
                    commands: vec![ExcuteStmt::SlowOpponent(crate::data::SlowOpponentCall {
                        slow_mul: 12.0,
                        slow_time: 60.0,
                    })],
                }
            } else {
                NodeKind::Frame { frame: 12.0 }
            };
            assert_json_eq!(&view.draft.tracks[0].nodes[0].kind, &expected);
            assert!(
                view.undo.is_empty(),
                "an active field edit is not split by frames"
            );
            if commit_with_enter {
                frame(
                    &context,
                    &mut view,
                    vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
                );
                assert_eq!(view.undo.len(), 1);
            }
            press(&context, &mut view, "Undo");
            assert_eq!(
                serde_json::to_value(&view.draft).unwrap(),
                before,
                "commit_with_enter={commit_with_enter}"
            );
            press(&context, &mut view, "Redo");
            assert_json_eq!(&view.draft.tracks[0].nodes[0].kind, &expected);
            assert!(super::super::timeline::compile("mario", "attack11", &view.draft).is_ok());
        }
    }

    #[test]
    fn pointer_import_add_hitbox_and_apply_uses_valid_typed_graph() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        assert!(click(&mut view, &context, "Import current move").is_none());
        assert_eq!(view.draft.tracks.len(), 4);
        view.add_kind = 5;
        assert!(click(&mut view, &context, "Add node").is_none());
        let graph = click(&mut view, &context, "Apply nodes").expect("valid draft applies");
        assert!(view.error.is_empty(), "{}", view.error);
        let scripts = super::super::timeline::compile("mario", "attack11", &graph).unwrap();
        assert_eq!(scripts[0].1.to_hitboxes().len(), 2);
    }

    #[test]
    fn effect_palette_adds_editable_blocks_and_source_only_tracks() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        view.track = view
            .draft
            .tracks
            .iter()
            .position(|track| track.track == Track::Effect)
            .unwrap();
        let kinds = (7..=10).chain(54..=62).chain(87..=88).collect::<Vec<_>>();
        for kind in &kinds {
            assert!(node_choices(Track::Effect)
                .iter()
                .any(|(candidate, _)| candidate == kind));
            assert!(super::super::effects::command_lines(&default_effect(*kind)).is_ok());
            view.add_kind = *kind;
            click(&mut view, &context, "Add node");
        }
        let graph = click(&mut view, &context, "Apply nodes")
            .unwrap_or_else(|| panic!("effect palette rejected: {}", view.error));
        let track = graph
            .tracks
            .iter()
            .find(|track| track.track == Track::Effect)
            .unwrap();
        assert!(
            track
                .nodes
                .iter()
                .filter(|node| matches!(&node.kind, NodeKind::EffectExecute { .. }))
                .count()
                >= kinds.len()
        );
        assert!(super::super::live_support::source_only_tracks(&graph)
            .any(|track| track == Track::Effect));
        let compiled = super::super::timeline::compile("mario", "attack11", &graph).unwrap();
        let script = &compiled
            .iter()
            .find(|(track, _)| *track == Track::Effect)
            .unwrap()
            .1;
        let source = crate::acmd::preview_effect_structure_fn(script, "attack11");
        syn::parse_file(&source).unwrap();
        let parsed = crate::acmd::parse_effect_script(&source);
        let imported = super::super::effects::import(&parsed);
        let graph = MoveGraph {
            tracks: vec![imported],
            ..MoveGraph::default()
        };
        let recompiled = super::super::timeline::compile("mario", "attack11", &graph).unwrap();
        let reparsed = crate::acmd::parse_effect_script(&crate::acmd::preview_effect_structure_fn(
            &recompiled[0].1,
            "attack11",
        ));
        assert_json_eq!(parsed, reparsed);
    }

    #[test]
    fn trail_reuse_palette_requires_a_measured_move_template_and_keeps_its_payload() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        assert!(reusable_trail(&view.draft).is_none());
        assert!(!node_choices(Track::Effect)
            .iter()
            .any(|(kind, _)| *kind == 63));
        let source = format!(
            "unsafe extern \"C\" fn effect_attack11(agent: &mut L2CAgentBase) {{\nif macros::is_excute(agent) {{\n{}\n}}\n}}",
            super::super::trails::TRAIL4_SOURCE_FIXTURE
        );
        let measured = super::super::effects::import(&crate::acmd::parse_effect_script(&source));
        view.track = view
            .draft
            .tracks
            .iter()
            .position(|track| track.track == Track::Effect)
            .unwrap();
        view.draft.tracks[view.track] = measured;
        let original = reusable_trail(&view.draft).unwrap();
        view.add_kind = 63;
        click(&mut view, &context, "Add node");
        let graph =
            click(&mut view, &context, "Apply nodes").expect("measured trail clone applies");
        let commands: Vec<_> = graph.tracks[view.track]
            .nodes
            .iter()
            .flat_map(|node| {
                let NodeKind::EffectExecute { commands } = &node.kind else {
                    panic!("expected trail command group")
                };
                commands
            })
            .collect();
        assert_eq!(commands.len(), 2);
        for command in commands {
            assert_json_eq!(command, &original);
        }
        let mut command = original;
        let before = serde_json::to_value(&command).unwrap();
        for _ in 0..3 {
            let _ = context.run_ui(Default::default(), |ui| {
                effect_command_ui(ui, &mut command, &["top".into(), "haver".into()]);
            });
        }
        assert_eq!(serde_json::to_value(command).unwrap(), before);
    }

    #[test]
    fn switching_tracks_does_not_close_the_expanded_field() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        view.canvas.expanded = true;
        click(&mut view, &context, "Effect");
        assert!(view.canvas.expanded);
        assert_eq!(view.draft.tracks[view.track].track, Track::Effect);
        click(&mut view, &context, "Game");
        assert!(view.canvas.expanded);
        assert_eq!(view.draft.tracks[view.track].track, Track::Game);
    }

    #[test]
    fn effect_spawn_modes_preserve_transforms_and_reset_only_native_tail() {
        let mut command = default_effect(7);
        if let crate::data::EffectMacro::Effect {
            effect_name,
            bone_name,
            offset,
            rotation,
            scale,
            ..
        } = &mut command
        {
            *effect_name = "custom_spark".into();
            *bone_name = "handl".into();
            *offset = [-7.0, 2.5, 4.0];
            *rotation = [45.0, 0.0, -12.0];
            *scale = 2.0;
        }
        for layout in crate::acmd::EFFECT_SPAWN_LAYOUTS {
            assert!(set_effect_spawn_variant(&mut command, layout.name));
            let crate::data::EffectMacro::Effect {
                effect_name,
                effect_name_alt,
                spawn_func,
                bone_name,
                offset,
                rotation,
                scale,
                follows_bone,
                extra_args,
                flip_axis,
            } = &command
            else {
                panic!();
            };
            assert_eq!(effect_name, "custom_spark");
            assert_eq!(bone_name, "handl");
            assert_eq!(*offset, [-7.0, 2.5, 4.0]);
            assert_eq!(*rotation, [45.0, 0.0, -12.0]);
            assert_eq!(*scale, 2.0);
            assert_eq!(spawn_func, layout.name);
            assert_eq!(*follows_bone, layout.follows_bone);
            assert_eq!(effect_name_alt.is_some(), layout.flip);
            assert_eq!(flip_axis.is_some(), layout.flip);
            assert_eq!(
                extra_args,
                &layout
                    .tail
                    .iter()
                    .map(|(default, _)| (*default).to_string())
                    .collect::<Vec<_>>()
            );
            super::super::effects::command_lines(&command).unwrap();
        }
        let before = serde_json::to_value(&command).unwrap();
        assert!(!set_effect_spawn_variant(&mut command, "unverified_spawn"));
        assert_eq!(serde_json::to_value(&command).unwrap(), before);
    }

    fn render_effect_inspector(
        context: &egui::Context,
        command: &mut crate::data::EffectMacro,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, egui::Rect) {
        let mut bounds = egui::Rect::NOTHING;
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1000.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                bounds = egui::Window::new("Effect properties")
                    .default_pos(egui::pos2(1050.0, 120.0))
                    .default_width(280.0)
                    .min_width(280.0)
                    .max_width(340.0)
                    .show(ui.ctx(), |ui| effect_command_ui(ui, command, &[]))
                    .unwrap()
                    .response
                    .rect;
            },
        );
        (output, bounds)
    }

    fn open_effect_native_arguments(
        context: &egui::Context,
        command: &mut crate::data::EffectMacro,
    ) -> (egui::FullOutput, egui::Rect) {
        render_effect_inspector(context, command, Vec::new());
        let (output, _) = render_effect_inspector(context, command, Vec::new());
        let position = output
            .shapes
            .iter()
            .find_map(|shape| text_position(&shape.shape, "Native arguments"))
            .expect("native header must be visible");
        for pressed in [true, false] {
            render_effect_inspector(
                context,
                command,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..4 {
            render_effect_inspector(context, command, Vec::new());
        }
        render_effect_inspector(context, command, Vec::new())
    }

    #[test]
    fn viewing_effect_inspectors_does_not_normalize_native_values() {
        let mut commands = (7..=10)
            .chain(54..=61)
            .map(default_effect)
            .collect::<Vec<_>>();
        commands.push(crate::data::EffectMacro::LastEffectSetColor {
            rgb: [2.0, -0.25, 0.75],
        });
        commands.push(crate::data::EffectMacro::Color {
            command: "FLASH_FRM".into(),
            color: crate::data::ColorCall {
                transition: Some(12.0),
                rgba: Some([2.0, 0.5, 0.75, 0.667]),
            },
        });
        let mut flipped = default_effect(7);
        set_effect_spawn_variant(&mut flipped, "EFFECT_FOLLOW_FLIP");
        if let crate::data::EffectMacro::Effect { flip_axis, .. } = &mut flipped {
            *flip_axis = Some("*EF_FLIP_YZ".into());
        }
        commands.push(flipped);
        let before = serde_json::to_value(&commands).unwrap();
        for command in &mut commands {
            let context = egui::Context::default();
            if matches!(command, crate::data::EffectMacro::Effect { .. }) {
                let (output, _) = open_effect_native_arguments(&context, command);
                assert!(output
                    .shapes
                    .iter()
                    .any(|shape| text_position(&shape.shape, "Arg 1").is_some()));
            } else {
                for _ in 0..3 {
                    render_effect_inspector(&context, command, Vec::new());
                }
            }
        }
        assert_eq!(serde_json::to_value(commands).unwrap(), before);
    }

    #[test]
    fn effect_native_tails_do_not_expand_the_properties_window_behind_the_field() {
        for layout in crate::acmd::EFFECT_SPAWN_LAYOUTS {
            let context = egui::Context::default();
            let mut command = default_effect(7);
            set_effect_spawn_variant(&mut command, layout.name);
            let original = serde_json::to_value(&command).unwrap();
            let (output, bounds) = open_effect_native_arguments(&context, &mut command);
            let native_argument_visible = output
                .shapes
                .iter()
                .any(|shape| text_position(&shape.shape, "Arg 1").is_some());
            assert!(
                native_argument_visible,
                "{} native arguments were not actually open",
                layout.name
            );
            assert!(
                bounds.left() >= 1040.0 && bounds.right() <= 1400.0 && bounds.width() <= 360.0,
                "{} expanded behind the field: {bounds:?}",
                layout.name
            );
            assert_eq!(serde_json::to_value(&command).unwrap(), original);
            assert_ne!(
                effect_spawn_mode_label(layout.name),
                layout.name,
                "missing readable label"
            );
        }
        assert_eq!(effect_spawn_mode_label("unmeasured"), "unmeasured");
    }

    #[test]
    fn selecting_native_constants_keeps_each_commands_token_convention() {
        let choose = |token: &mut String, table, dereference, initial: &str, chosen: &str| {
            let context = egui::Context::default();
            let render = |token: &mut String, events| {
                context.run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        constant_picker(ui, "test_native_constant", token, table, dereference);
                    },
                )
            };
            render(token, Vec::new());
            let output = render(token, Vec::new());
            let click = |token: &mut String, output: &egui::FullOutput, text: &str| {
                let pos = output
                    .shapes
                    .iter()
                    .find_map(|shape| text_position(&shape.shape, text))
                    .unwrap_or_else(|| panic!("choice {text} missing"));
                for pressed in [true, false] {
                    render(
                        token,
                        vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: Default::default(),
                            },
                        ],
                    );
                }
                render(token, Vec::new())
            };
            let output = click(token, &output, initial);
            click(token, &output, chosen);
        };
        let mut grab = default_command(18);
        let ExcuteStmt::Catch(call) = &mut grab else {
            panic!()
        };
        choose(
            &mut call.situation,
            crate::param_labels::SITUATION_MASK,
            false,
            "GA",
            "A",
        );
        assert_eq!(call.situation, "COLLISION_SITUATION_MASK_A");
        let mut armor = default_command(25);
        let ExcuteStmt::DamageNoReaction(call) = &mut armor else {
            panic!()
        };
        choose(
            &mut call.mode,
            crate::param_labels::DAMAGE_NO_REACTION_MODE,
            true,
            "NORMAL",
            "ALWAYS",
        );
        assert_eq!(call.mode, "*DAMAGE_NO_REACTION_MODE_ALWAYS");
        let script = AcmdScript {
            stmts: vec![crate::data::AcmdStmt::Excute(vec![grab, armor])],
        };
        let source = crate::acmd::preview_game_fn(&script, "attack11");
        assert!(
            !source.contains("**"),
            "selector introduced double-dereferenced constants: {source}"
        );
        assert_json_eq!(&script, crate::acmd::parse_acmd_script(&source));
    }

    #[test]
    fn attack_contact_controls_change_the_native_property_not_its_legacy_namesake() {
        let context = egui::Context::default();
        context.global_style_mut(|style| style.animation_time = 0.0);
        let mut call = crate::data::Hitbox::default().to_attack_call();
        call.is_mtk = false;
        call.is_shield_disable = false;
        call.is_reflectable = false;
        call.is_absorbable = false;
        call.is_landing_attack = false;
        call.no_finish_camera = false;
        let before = call.clone();
        let render = |call: &mut crate::data::AttackCall, events| {
            context.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    attack_details_ui(ui, call);
                },
            )
        };
        render(&mut call, Vec::new());
        let mut output = render(&mut call, Vec::new());
        for label in ["Contact flags", "Reflectable"] {
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| text_position(&shape.shape, label))
                .unwrap_or_else(|| panic!("control {label} missing"));
            for pressed in [true, false] {
                render(
                    &mut call,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
            output = render(&mut call, Vec::new());
        }
        let mut expected = before;
        expected.is_mtk = true;
        assert_json_eq!(
            &call,
            &expected,
            "Reflectable must change only the native reflectable slot"
        );
        let script = AcmdScript {
            stmts: vec![crate::data::AcmdStmt::Excute(vec![ExcuteStmt::Attack(
                call,
            )])],
        };
        let source = crate::acmd::preview_game_fn(&script, "attack11");
        assert_json_eq!(&script, crate::acmd::parse_acmd_script(&source));
    }

    #[test]
    fn nested_command_search_exposes_recent_blocks_and_preserves_guard_ids() {
        fn frame(
            ctx: &egui::Context,
            body: &mut Vec<MoveNode>,
            next: &mut u32,
            track: Track,
            fighter: &str,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(354.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| nodes_ui_body(ui, fighter, track, body, next, CommandOptions::default()),
            )
        }
        fn press(
            ctx: &egui::Context,
            body: &mut Vec<MoveNode>,
            next: &mut u32,
            track: Track,
            fighter: &str,
            label: &str,
        ) {
            frame(ctx, body, next, track, fighter, Vec::new());
            let output = frame(ctx, body, next, track, fighter, Vec::new());
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| text_position(&shape.shape, label))
                .unwrap_or_else(|| panic!("missing nested command control {label}"));
            for pressed in [true, false] {
                frame(
                    ctx,
                    body,
                    next,
                    track,
                    fighter,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
        }
        for (track, fighter, query, label, expected_next) in [
            (Track::Game, "eflame", "sword", "Open/close sword", 13),
            (Track::Game, "robot", "launch", "Launch if present", 12),
            (Track::Effect, "mario", "trail", "End sword trail", 12),
            (Track::Game, "mario", "hitbox radius", "Hitbox size", 12),
            (
                Track::Game,
                "mario",
                "invulnerability normal",
                "All hurtboxes normal",
                12,
            ),
            (Track::Game, "mario", "vector position", "Vector target", 12),
            (Track::Game, "mario", "slow opponent", "Slow opponents", 12),
            (
                Track::Game,
                "mario",
                "feedback effect",
                "Optional hit effect",
                12,
            ),
            (
                Track::Game,
                "mario",
                "feedback sound",
                "Optional hit sound",
                12,
            ),
            (
                Track::Game,
                "mario",
                "spawn projectile",
                "Generate article",
                12,
            ),
        ] {
            let ctx = egui::Context::default();
            let mut body = Vec::new();
            let mut next = 11;
            press(&ctx, &mut body, &mut next, track, fighter, "Add command");
            press(&ctx, &mut body, &mut next, track, fighter, "Find a block…");
            frame(
                &ctx,
                &mut body,
                &mut next,
                track,
                fighter,
                vec![egui::Event::Text(query.into())],
            );
            let output = frame(&ctx, &mut body, &mut next, track, fighter, Vec::new());
            assert!(output
                .shapes
                .iter()
                .all(|shape| text_position(&shape.shape, "Native flag").is_none()));
            press(&ctx, &mut body, &mut next, track, fighter, label);
            assert_eq!(body.len(), 1);
            assert_eq!(body[0].id, 11);
            assert_eq!(next, expected_next);
            assert!(matches!(&body[0].kind, NodeKind::Branch { .. }) == (fighter == "eflame"));
            let graph = MoveGraph {
                tracks: vec![fixture::track(track, body)],
                ..Default::default()
            };
            assert!(super::super::timeline::compile(fighter, "attack11", &graph).is_ok());
        }
    }

    fn closed_property_frame(
        context: &egui::Context,
        events: Vec<egui::Event>,
        render: &mut impl FnMut(&mut Ui),
    ) -> egui::FullOutput {
        context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(354.0, 540.0),
                )),
                events,
                ..Default::default()
            },
            render,
        )
    }

    fn click_closed_property(
        context: &egui::Context,
        label: &str,
        render: &mut impl FnMut(&mut Ui),
    ) {
        let output = closed_property_frame(context, Vec::new(), render);
        let position = output
            .shapes
            .iter()
            .rev()
            .find_map(|shape| text_position(&shape.shape, label))
            .unwrap_or_else(|| panic!("missing closed selector {label:?}"));
        for pressed in [true, false] {
            closed_property_frame(
                context,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                render,
            );
        }
    }

    #[test]
    fn native_effect_follow_transform_typing_edits_only_the_native_axis() {
        use crate::data::{EffectMacro, EffectStmt};
        let source = "unsafe extern \"C\" fn effect_native(agent: &mut L2CAgentBase) {\nif macros::is_excute(agent) {\nmacros::EFFECT_FOLLOW_WORK(agent, *FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE, Hash40::new(\"haver\"), -1, 2.5, 3, 10, 20, 30, 1.2, true);\n}\n}";
        let parsed = crate::acmd::parse_effect_script(source);
        let EffectStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!()
        };
        let mut command = commands[0].clone();
        let context = egui::Context::default();
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        {
            let mut render =
                |ui: &mut Ui| effect_command_ui_mode(ui, &mut command, &[], false, Some("cloud"));
            closed_property_frame(&context, Vec::new(), &mut render);
            closed_property_frame(&context, Vec::new(), &mut render);
            click_closed_property(&context, "Transform", &mut render);
            // Allow the collapsing region animation to settle before hit-testing its fields.
            for _ in 0..30 {
                closed_property_frame(&context, Vec::new(), &mut render);
            }
            click_closed_property(&context, "10", &mut render);
            click_closed_property(&context, "10", &mut render);
            closed_property_frame(
                &context,
                vec![key(egui::Key::A, egui::Modifiers::COMMAND)],
                &mut render,
            );
            closed_property_frame(&context, vec![egui::Event::Text("45".into())], &mut render);
            closed_property_frame(
                &context,
                vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
                &mut render,
            );
        }
        let mut expected = commands[0].clone();
        let EffectMacro::EffectFollowWork { rotation, .. } = &mut expected else {
            panic!()
        };
        rotation[2] = 45.0;
        assert_json_eq!(&command, &expected);
        let mut track = super::super::effects::import_for_fighter("cloud", &parsed);
        let NodeKind::MixedEffectExecute { commands, .. } = &mut track.nodes[0].kind else {
            panic!()
        };
        commands[0] = super::super::model::EffectCommandSlot::Editable(command);
        let graph = MoveGraph {
            tracks: vec![track],
            ..Default::default()
        };
        let output = super::super::timeline::compile("cloud", "attack11", &graph).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&output[0].1, "attack11");
        assert!(emitted.contains("-1, 2.5, 3, 45, 20, 30, 1.2, true"));
    }

    #[test]
    fn mixed_inspector_keeps_opaque_slots_read_only_and_applies_known_edits() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        let source = AcmdScript {
            stmts: vec![crate::data::AcmdStmt::Excute(vec![
                ExcuteStmt::Attack(crate::data::Hitbox::default().to_attack_call()),
                ExcuteStmt::Raw("retained_native_call(agent);".into()),
                ExcuteStmt::ClearAll,
            ])],
        };
        view.draft.tracks = vec![super::super::timeline::import_for_fighter(
            "mario",
            Track::Game,
            &source,
        )];
        let before = serde_json::to_value(&view.draft).unwrap();
        for _ in 0..2 {
            let output = context.run_ui(Default::default(), |ui| {
                let mut next = 10;
                nodes(
                    ui,
                    "mario",
                    Track::Game,
                    &mut view.draft.tracks[0].nodes,
                    &mut next,
                    CommandOptions {
                        joints: &["top".into()],
                        ..Default::default()
                    },
                );
            });
            for forbidden in ["Remove command", "Duplicate", "Remove", "Up", "Down"] {
                assert!(
                    !output
                        .shapes
                        .iter()
                        .any(|shape| text_position(&shape.shape, forbidden).is_some()),
                    "mixed block offered {forbidden}"
                );
            }
            assert!(output.shapes.iter().any(|shape| text_position(
                &shape.shape,
                "Preserved command"
            )
            .is_some()));
            assert!(output
                .shapes
                .iter()
                .any(|shape| text_position(&shape.shape, "Damage").is_some()));
        }
        assert_eq!(serde_json::to_value(&view.draft).unwrap(), before);
        let NodeKind::MixedExecute { commands } = &mut view.draft.tracks[0].nodes[0].kind else {
            panic!()
        };
        let MoveCommand::Editable(ExcuteStmt::Attack(attack)) = &mut commands[0] else {
            panic!()
        };
        attack.damage = 12.5;
        let graph =
            click(&mut view, &context, "Apply nodes").expect("supported mixed edit applies");
        let saved: MoveGraph =
            serde_json::from_value(serde_json::to_value(graph).unwrap()).unwrap();
        let compiled = super::super::timeline::compile("mario", "attack11", &saved).unwrap();
        let crate::data::AcmdStmt::Excute(commands) = &compiled[0].1.stmts[0] else {
            panic!()
        };
        assert!(matches!(&commands[0], ExcuteStmt::Attack(attack) if attack.damage == 12.5));
        assert!(
            matches!(&commands[1], ExcuteStmt::Raw(text) if text == "retained_native_call(agent);")
        );
        assert!(matches!(&commands[2], ExcuteStmt::ClearAll));
        assert!(super::super::live_support::source_only_tracks(&saved)
            .any(|track| track == Track::Game));
    }

    #[test]
    fn baseline_bound_regions_do_not_offer_invalid_duplication_or_removal() {
        let native = fixture::node(
            1,
            NodeKind::Preserved {
                statement: crate::data::AcmdStmt::Raw("native(agent);".into()),
            },
        );
        let repeat = NodeKind::Repeat {
            motion_loop: crate::data::MotionLoop::No,
            count: 2,
            body: vec![native.clone()],
        };
        assert!(node_has_preserved_source(&repeat));
        for (then_nodes, else_nodes) in [
            (vec![native.clone()], Vec::new()),
            (Vec::new(), vec![native]),
        ] {
            assert!(node_has_preserved_source(&NodeKind::Branch {
                condition: MoveCondition::Grounded,
                then_nodes,
                else_nodes
            }));
        }
        assert!(!node_has_preserved_source(&NodeKind::Repeat {
            motion_loop: crate::data::MotionLoop::No,
            count: 2,
            body: vec![MoveNode {
                id: 2,
                kind: NodeKind::Wait { frames: 1.0 }
            }]
        }));
        assert!(!node_has_preserved_source(&NodeKind::EffectExecute {
            commands: vec![crate::data::EffectMacro::LastEffectSetRate { rate: 0.5 }]
        }));
    }

    #[test]
    fn mixed_effect_inspector_keeps_native_slots_and_applies_neighbor_edits() {
        use super::super::model::EffectCommandSlot;
        use crate::data::{EffectControl, EffectMacro, EffectScript, EffectStmt};
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        let source = EffectScript {
            stmts: vec![EffectStmt::Excute(vec![
                EffectMacro::LastEffectSetRate { rate: 0.5 },
                EffectMacro::Raw("retained_native_effect(agent);".into()),
                EffectMacro::Control(EffectControl::SetDisableCameraDepthInfluence {
                    value: false,
                }),
            ])],
        };
        view.draft.tracks = vec![super::super::effects::import_for_fighter("mario", &source)];
        let before = serde_json::to_value(&view.draft).unwrap();
        for _ in 0..2 {
            let output = context.run_ui(Default::default(), |ui| {
                let mut next = 10;
                nodes(
                    ui,
                    "mario",
                    Track::Effect,
                    &mut view.draft.tracks[0].nodes,
                    &mut next,
                    CommandOptions::default(),
                );
            });
            for forbidden in ["Remove command", "Duplicate", "Remove", "Up", "Down"] {
                assert!(
                    !output
                        .shapes
                        .iter()
                        .any(|shape| text_position(&shape.shape, forbidden).is_some()),
                    "mixed effects offered {forbidden}"
                );
            }
            for visible in ["Preserved native effect", "Rate", "Camera depth influence"] {
                assert!(
                    output
                        .shapes
                        .iter()
                        .any(|shape| text_position(&shape.shape, visible).is_some()),
                    "missing mixed effect property: {visible}"
                );
            }
        }
        assert_eq!(serde_json::to_value(&view.draft).unwrap(), before);
        let NodeKind::MixedEffectExecute { commands, .. } = &mut view.draft.tracks[0].nodes[0].kind
        else {
            panic!("mixed importer did not expose neighbors")
        };
        let EffectCommandSlot::Editable(EffectMacro::LastEffectSetRate { rate }) = &mut commands[0]
        else {
            panic!("known rate was preserved")
        };
        *rate = 1.75;
        let EffectCommandSlot::Editable(EffectMacro::Control(
            EffectControl::SetDisableCameraDepthInfluence { value },
        )) = &mut commands[2]
        else {
            panic!("known camera control was preserved")
        };
        *value = true;
        let graph = click(&mut view, &context, "Apply nodes").expect("known effect edits apply");
        let restored: MoveGraph =
            serde_json::from_value(serde_json::to_value(graph).unwrap()).unwrap();
        let compiled = super::super::timeline::compile("mario", "attack11", &restored).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "attack11");
        let reparsed = crate::acmd::parse_effect_script(&emitted);
        let EffectStmt::Excute(commands) = &reparsed.stmts[0] else {
            panic!("execute boundary changed")
        };
        assert_eq!(commands.len(), 3);
        assert!(matches!(&commands[0], EffectMacro::LastEffectSetRate { rate } if *rate == 1.75));
        assert!(
            matches!(&commands[1], EffectMacro::Raw(line) if line == "retained_native_effect(agent);")
        );
        assert!(matches!(
            &commands[2],
            EffectMacro::Control(EffectControl::SetDisableCameraDepthInfluence { value: true })
        ));
        assert!(super::super::live_support::source_only_tracks(&restored)
            .any(|track| track == Track::Effect));
    }

    #[test]
    fn removing_one_command_keeps_the_execute_blocks_other_commands() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        let NodeKind::Execute { commands } = &mut view.draft.tracks[0].nodes[1].kind else {
            panic!()
        };
        commands.push(ExcuteStmt::ClearAll);
        click(&mut view, &context, "Remove command");
        let graph = click(&mut view, &context, "Apply nodes").expect("remaining command applies");
        let NodeKind::Execute { commands } = &graph.tracks[0].nodes[1].kind else {
            panic!()
        };
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0], ExcuteStmt::ClearAll));
    }

    #[test]
    fn node_choices_never_offer_a_command_for_the_wrong_category() {
        for track in [Track::Game, Track::Sound, Track::Expression, Track::Effect] {
            for (kind, _) in node_choices(track) {
                if matches!(kind, 2 | 3 | 5) {
                    assert_eq!(track, Track::Game);
                }
                if matches!(kind, 7..=10) {
                    assert_eq!(track, Track::Effect);
                }
                if matches!(kind, 11 | 12) {
                    assert_eq!(track, Track::Sound);
                }
                if matches!(kind, 13 | 14 | 51) {
                    assert_eq!(track, Track::Expression);
                }
            }
        }
    }

    #[test]
    fn palette_alias_search_is_word_order_independent_without_bypassing_track_or_owner() {
        for (kind, label, query) in [
            (35, "Generate article", "SPAWN projectile"),
            (36, "Remove article", "projectile despawn"),
            (17, "Hurtbox status", "invulnerability"),
            (15, "Motion rate", "animation speed"),
            (21, "Set velocity", "momentum set"),
            (101, "Send event", "custom trigger"),
            (90, "Slow opponents", "opponent slow"),
        ] {
            assert!(palette_matches(kind, label, query));
            assert!(palette_matches(kind, label, " \t "));
            assert!(!palette_matches(kind, label, "not_a_block"));
        }
        for track in [Track::Effect, Track::Sound, Track::Expression] {
            assert!(!node_choices_for_fighter(track, "mario")
                .iter()
                .any(|(kind, label)| palette_matches(*kind, label, "opponent slow")));
        }
        for query in ["spawn projectile", "opponent slow"] {
            assert!(!node_choices_for_fighter(Track::Game, "unknown")
                .iter()
                .any(|(kind, label)| palette_matches(*kind, label, query)));
        }
        assert!(
            !palette_matches(35, "Generate article", "projectile hit"),
            "search must not invent a causal hit receiver"
        );
        assert!(!palette_matches(72, "Set slowdown", "opponent"));
        assert!(!palette_matches(73, "Clear slowdown", "opponent"));
    }

    #[test]
    fn palette_open_focuses_search_without_an_extra_click_or_source_edit() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        let before = serde_json::to_value(&view.draft).unwrap();
        click(&mut view, &context, "Frame");
        let script = AcmdScript {
            stmts: vec![
                crate::data::AcmdStmt::Frame(3.0),
                crate::data::AcmdStmt::Excute(vec![ExcuteStmt::Attack(
                    crate::data::Hitbox::default().to_attack_call(),
                )]),
            ],
        };
        let _ = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(460.0, 1800.0),
                )),
                events: vec![egui::Event::Text("slow opponents".into())],
                ..Default::default()
            },
            |ui| {
                view.ui(
                    ui,
                    "mario",
                    "attack11",
                    None,
                    [&script, &AcmdScript::default(), &AcmdScript::default()],
                    &crate::data::EffectScript::default(),
                    &BehaviorGraph::default(),
                    &BTreeMap::new(),
                );
            },
        );
        assert_eq!(view.palette_query, "slow opponents");
        assert_eq!(serde_json::to_value(&view.draft).unwrap(), before);
    }

    #[test]
    fn timer_remaining_selector_is_explicit_timer_only_and_nonmutating_on_inspection() {
        let context = egui::Context::default();
        let variables = [
            fixture::var("cooldown".into(), VarType::Timer, ConstValue::Int(3)),
            fixture::var("counter".into(), VarType::Int, ConstValue::Int(0)),
        ];
        let mut value = Expr::Const(ConstValue::Int(0));
        let mut render =
            |ui: &mut Ui| expression_block_ui(ui, "mario", &mut value, &variables, VarType::Int);
        click_closed_property(&context, "Number", &mut render);
        click_closed_property(&context, "Timer remaining", &mut render);
        assert!(matches!(&value, Expr::TimerRemaining(name) if name == "cooldown"));
        let mut value = Expr::TimerRemaining("missing_timer".into());
        let before = serde_json::to_value(&value).unwrap();
        for _ in 0..2 {
            closed_property_frame(&context, vec![], &mut |ui| {
                expression_block_ui(ui, "mario", &mut value, &variables, VarType::Int)
            });
        }
        assert_eq!(
            serde_json::to_value(value).unwrap(),
            before,
            "viewing an unavailable timer must not substitute another source"
        );
    }

    #[test]
    fn block_clipboard_menu_is_nonmutating_and_paste_is_one_undoable_edit() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        view.canvas.selected = Some("node:2".into());
        let before = serde_json::to_value(&view.draft).unwrap();
        let undo_count = view.undo.len();
        click(&mut view, &context, "Blocks");
        click(&mut view, &context, "Copy block");
        assert!(view.block_clipboard.is_some());
        assert_eq!(serde_json::to_value(&view.draft).unwrap(), before);
        assert_eq!(view.undo.len(), undo_count);
        click(&mut view, &context, "Blocks");
        click(&mut view, &context, "Paste after selected");
        assert!(view.error.is_empty(), "{}", view.error);
        let pasted = serde_json::to_value(&view.draft).unwrap();
        assert_ne!(pasted, before);
        assert_eq!(view.draft.tracks[0].nodes.len(), 3);
        assert_eq!(view.undo.len(), undo_count + 1);
        assert!(view.canvas.selected_node().unwrap() > 2);
        click(&mut view, &context, "Undo");
        assert_eq!(serde_json::to_value(&view.draft).unwrap(), before);
        assert!(
            view.block_clipboard.is_some(),
            "Undo changes the draft, not the clipboard"
        );
        click(&mut view, &context, "Redo");
        assert_eq!(serde_json::to_value(&view.draft).unwrap(), pasted);
        let applied = click(&mut view, &context, "Apply nodes").unwrap();
        let restored: MoveGraph =
            serde_json::from_value(serde_json::to_value(applied).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), pasted);
    }

    #[test]
    fn block_clipboard_failure_preserves_previous_copy_and_survives_move_switch() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        view.canvas.selected = Some("node:1".into());
        view.copy_selected_block("mario");
        view.canvas.selected = None;
        view.copy_selected_block("mario");
        assert!(view.block_clipboard.is_some());
        let _ = context.run_ui(egui::RawInput::default(), |ui| {
            view.ui(
                ui,
                "cloud",
                "attack12",
                None,
                [
                    &AcmdScript::default(),
                    &AcmdScript::default(),
                    &AcmdScript::default(),
                ],
                &crate::data::EffectScript::default(),
                &BehaviorGraph::default(),
                &BTreeMap::new(),
            );
        });
        assert_eq!(
            view.block_clipboard.as_ref().unwrap().origin_fighter(),
            "mario"
        );
        assert_eq!(view.key, "cloud/attack12");
        assert!(view.draft.is_empty());
    }

    #[test]
    fn sound_and_expression_nodes_apply_through_pointer_controls() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        click(&mut view, &context, "Import current move");
        for (track, kind) in [
            (0, 52),
            (1, 11),
            (1, 12),
            (2, 13),
            (2, 14),
            (2, 51),
            (2, 17),
            (2, 20),
            (2, 30),
            (2, 28),
            (2, 35),
            (2, 38),
        ] {
            view.track = track;
            view.add_kind = kind;
            click(&mut view, &context, "Add node");
        }
        let graph =
            click(&mut view, &context, "Apply nodes").expect("sound/expression draft applies");
        let scripts = super::super::timeline::compile("mario", "attack11", &graph).unwrap();
        assert_eq!(
            scripts
                .iter()
                .find(|(track, _)| *track == Track::Sound)
                .unwrap()
                .1
                .to_sound_events()
                .len(),
            2
        );
        assert!(super::super::live_support::source_only_tracks(&graph)
            .any(|track| track == Track::Expression));
        assert_eq!(
            scripts
                .iter()
                .find(|(track, _)| *track == Track::Expression)
                .unwrap()
                .1
                .to_expression_events()
                .len(),
            3
        );
    }

    #[test]
    fn branch_duplicate_rekeys_both_arms() {
        let mut node = fixture::node(
            1,
            NodeKind::Branch {
                condition: MoveCondition::Grounded,
                then_nodes: vec![fixture::wait(2, 1.0)],
                else_nodes: vec![fixture::wait(3, 2.0)],
            },
        );
        let mut next = 9;
        rekey(&mut node, &mut next);
        let NodeKind::Branch {
            then_nodes,
            else_nodes,
            ..
        } = &node.kind
        else {
            panic!()
        };
        assert_eq!([node.id, then_nodes[0].id, else_nodes[0].id], [9, 10, 11]);
        assert_eq!(next, 12);
    }

    #[test]
    fn empty_draft_cannot_apply() {
        let context = egui::Context::default();
        let mut view = NodeEditor::default();
        assert!(click(&mut view, &context, "Apply nodes").is_none());
        assert!(view.error.contains("Import"));
    }
    #[test]
    fn duplicate_repeat_rekeys_every_descendant() {
        let mut node = fixture::node(
            1,
            NodeKind::Repeat {
                motion_loop: crate::data::MotionLoop::No,
                count: 2,
                body: vec![fixture::wait(2, 1.0)],
            },
        );
        let mut next = 8;
        rekey(&mut node, &mut next);
        assert_eq!(node.id, 8);
        let NodeKind::Repeat { body, .. } = node.kind else {
            panic!()
        };
        assert_eq!(body[0].id, 9);
        assert_eq!(next, 10);
    }

    #[test]
    fn sound_selector_normalizes_native_arities_and_tail() {
        let ExcuteStmt::Sound(mut sound) = default_command(11) else {
            panic!()
        };
        sound.func = "PLAY_STEP_FLIPPABLE".into();
        normalize_sound_shape(&mut sound);
        assert_eq!(sound.sounds.len(), 2);
        assert!(sound.tail.is_none());
        sound.func = "SET_PLAY_INHIVIT".into();
        normalize_sound_shape(&mut sound);
        assert_eq!(sound.sounds.len(), 1);
        assert_eq!(sound.tail.as_deref(), Some("5"));
        sound.func = "STOP_SE".into();
        normalize_sound_shape(&mut sound);
        assert!(sound.tail.is_none());
    }

    #[test]
    fn native_command_inspection_preserves_payloads_across_owner_and_track_contexts() {
        for (kind, owner) in [
            (38, "kirby"),
            (41, "kirby"),
            (70, "miiswordsman"),
            (71, "eflame"),
            (90, "mario"),
            (91, "mario"),
            (92, "mario"),
            (93, "robot"),
            (94, "eflame"),
            (95, crate::data::FINAL_FEAR_FACE_PRESETS[0].fighter),
            (96, "mario"),
            (97, "gamewatch"),
            (98, "koopajr"),
            (99, "samus"),
            (102, "buddy"),
            (103, "gaogaen"),
            (104, "gaogaen"),
            (105, "bayonetta"),
            (120, "samus"),
            (114, "mario"),
            (115, "mario"),
            (116, "mario"),
            (117, "mario"),
            (118, "mario"),
            (119, "mario"),
            (109, "bayonetta"),
            (110, "brave"),
            (111, "luigi"),
            (112, "diddy"),
            (113, "diddy"),
        ] {
            let mut command = context_command(kind, owner);
            let before = serde_json::to_value(&command).unwrap();
            for fighter in [owner, "unknown"] {
                for track in [Track::Game, Track::Expression] {
                    let context = egui::Context::default();
                    for _ in 0..2 {
                        let _ = context.run_ui(Default::default(), |ui| {
                            command_ui(
                                ui,
                                fighter,
                                &mut command,
                                CommandOptions {
                                    track,
                                    ..Default::default()
                                },
                            );
                        });
                    }
                    assert_eq!(
                        serde_json::to_value(&command).unwrap(),
                        before,
                        "inspecting palette {kind} as {fighter} on {track:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn block_clipboard_rechecks_character_receivers_before_changing_draft() {
        let source = MoveGraph {
            tracks: vec![fixture::track(
                Track::Game,
                vec![fixture::execute(
                    1,
                    vec![ExcuteStmt::GimmickEvent {
                        name: "begin_charge".into(),
                    }],
                )],
            )],
            ..MoveGraph::default()
        };
        let mut view = NodeEditor {
            block_clipboard: Some(
                super::super::block_clipboard::BlockClipboard::capture("mario", &source, 1)
                    .unwrap(),
            ),
            draft: MoveGraph {
                tracks: vec![fixture::track(Track::Game, vec![])],
                ..MoveGraph::default()
            },
            ..NodeEditor::default()
        };
        let before = serde_json::to_value(&view.draft).unwrap();
        view.paste_block(
            "mario",
            "attack11",
            &BehaviorGraph::default(),
            &BTreeMap::new(),
            None,
        );
        assert!(view.error.contains("begin_charge"), "{}", view.error);
        assert_eq!(serde_json::to_value(&view.draft).unwrap(), before);
        assert!(view.canvas.selected.is_none());
        let receiver = fixture::controller_event("begin_charge", vec![]);
        view.paste_block("mario", "attack11", &receiver, &BTreeMap::new(), None);
        assert!(view.error.is_empty(), "{}", view.error);
        assert_eq!(view.draft.tracks[0].nodes.len(), 1);
    }
}

#[cfg(test)]
mod palette_live_tests {
    /// Every block a user can add, for every fighter, reaches the running match.
    #[test]
    fn every_palette_block_updates_live() {
        use super::super::model::*;
        std::panic::set_hook(Box::new(|_| {}));
        let mut missing = Vec::new();
        for fighter in crate::gimmicks::registry::FIGHTER_KINDS
            .iter()
            .map(|kind| kind.fighter)
        {
            for track in [Track::Game, Track::Sound, Track::Expression, Track::Effect] {
                for (kind, label) in super::node_choices_for_fighter(track, fighter) {
                    // Some kinds are not offered on this track and refuse to build.
                    let Ok(node) =
                        std::panic::catch_unwind(|| super::context_node(kind, fighter, 1))
                    else {
                        continue;
                    };
                    let graph = MoveGraph {
                        tracks: vec![MoveTrack {
                            track,
                            nodes: vec![MoveNode { id: 1, kind: node }],
                            baseline: Vec::new(),
                            effect_execute_baseline: Vec::new(),
                        }],
                        ..Default::default()
                    };
                    if !super::super::live_support::not_live_blocks(fighter, &graph, &[]).is_empty()
                    {
                        missing.push(format!("{fighter}: {label}"));
                    }
                }
            }
        }
        let _ = std::panic::take_hook();
        assert!(missing.is_empty(), "not live: {missing:?}");
    }
}
