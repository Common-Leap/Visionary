use super::super::plain::{self, Starter};
use super::*;
use crate::gimmicks::{field_projection::FieldRef, templates::Preset};
use egui::{Rect, Sense, Vec2};

#[derive(Clone, serde::Serialize)]
pub(super) struct CharacterDraft {
    pub main: BehaviorGraph,
    pub charts: BTreeMap<String, crate::mod_project::ControllerChart>,
    pub layout: crate::gimmicks::field_canvas::FieldLayout,
}

#[derive(Clone)]
pub(in crate::move_nodes) enum PaletteBlock {
    Node(usize),
    Preset(Preset),
    NewController,
    Character(CharacterBlock),
    /// Test or set one named native gimmick value from the fighter catalog.
    Native(
        &'static crate::gimmicks::native_catalog::NativeGimmick,
        NativeUse,
    ),
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::move_nodes) enum NativeUse {
    Test,
    Set,
}

/// Building blocks for a character controller, added to the selected controller (or the main one).
#[derive(Clone, Copy, PartialEq)]
pub(in crate::move_nodes) enum CharacterBlock {
    State,
    Variable(VarType),
    Transition(HookName),
    /// A transition run when a move sends this controller a named event.
    EventTransition,
}

#[derive(Clone)]
struct Entry {
    label: String,
    group: &'static str,
    block: PaletteBlock,
}

const GROUPS: &[&str] = &[
    "Flow",
    "Hitboxes & grabs",
    "Hurtboxes & body",
    "Fighter state",
    "Native gimmick",
    "Objects & projectiles",
    "Movement",
    "Animation",
    "Visuals",
    "Audio",
    "Character",
];
const TRACKS: [Track; 4] = [Track::Game, Track::Effect, Track::Sound, Track::Expression];

/// A ready-made block for one catalog value: a Branch testing it (at the top of its known
/// range) or a command setting it. Users then adjust the comparison or value in the inspector.
fn native_node(
    entry: &crate::gimmicks::native_catalog::NativeGimmick,
    use_: NativeUse,
) -> NodeKind {
    use crate::data::{
        ExcuteStmt, WorkFlagAction, WorkFlagCall, WorkModuleSetCall, WorkModuleSetKind,
    };
    use crate::gimmicks::native_inputs::NativeKind;
    let symbol = entry.symbol.to_owned();
    let high = entry.range.map_or(1.0, |(_, max)| max);
    match use_ {
        NativeUse::Test => NodeKind::Branch {
            condition: match entry.kind {
                NativeKind::Flag => MoveCondition::WorkFlag {
                    symbol,
                    expected: true,
                },
                NativeKind::Int => MoveCondition::WorkInt {
                    symbol,
                    op: NativeComparison::Gte,
                    value: NativeIntOperand::Literal(high as i32),
                },
                NativeKind::Float => MoveCondition::WorkFloat {
                    symbol,
                    op: NativeComparison::Gte,
                    value: high as f32,
                },
            },
            then_nodes: Vec::new(),
            else_nodes: Vec::new(),
        },
        NativeUse::Set => NodeKind::Execute {
            commands: vec![match entry.kind {
                NativeKind::Flag => ExcuteStmt::WorkFlag(WorkFlagCall {
                    action: WorkFlagAction::On,
                    flag: format!("*{symbol}"),
                }),
                kind => ExcuteStmt::WorkModuleSet(WorkModuleSetCall {
                    kind: if kind == NativeKind::Int {
                        WorkModuleSetKind::Int
                    } else {
                        WorkModuleSetKind::Float
                    },
                    receiver: "agent.module_accessor".into(),
                    value: if kind == NativeKind::Int {
                        "0".into()
                    } else {
                        "0.0".into()
                    },
                    slot: format!("*{symbol}"),
                }),
            }],
        },
    }
}

fn track_label(track: Track) -> &'static str {
    match track {
        Track::Game => plain::LANES[0],
        Track::Effect => plain::LANES[1],
        Track::Sound => plain::LANES[2],
        Track::Expression => plain::LANES[3],
    }
}

fn group_for(label: &str, track: Track) -> &'static str {
    let label = label.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| label.contains(word));
    if has(&["article", "item", "sword", "launch if present"]) {
        "Objects & projectiles"
    } else if label.contains("event") {
        "Character"
    } else if label.contains("frame") && !label.contains("reaction")
        || matches!(label.as_str(), "wait" | "branch" | "repeat")
    {
        "Flow"
    } else if label.contains("motion") {
        "Animation"
    } else if has(&["velocity", "airborne", "facing", "kinetic"]) {
        "Movement"
    } else if track == Track::Sound || has(&["sound", "rumble"]) && !label.contains("hit sound") {
        "Audio"
    } else if track == Track::Effect
        || track == Track::Expression
        || has(&[
            "camera",
            "visibility",
            "model",
            "cut-in",
            "eye flash",
            "fear face",
        ])
    {
        "Visuals"
    } else if has(&["hurtbox", "armor", "body collision", "slope"]) {
        "Hurtboxes & body"
    } else if has(&[
        "change status",
        "flag",
        "transition",
        "dash",
        "slowdown",
        "slow opponents",
        "wave angle",
    ]) {
        "Fighter state"
    } else {
        "Hitboxes & grabs"
    }
}

fn catalog(fighter: &str, track: Track, draft: &MoveGraph) -> Vec<Entry> {
    let mut result: Vec<_> = node_choices_for_fighter(track, fighter)
        .into_iter()
        .map(|(kind, label)| Entry {
            label: plain::block_name(label).into(),
            group: group_for(label, track),
            block: PaletteBlock::Node(kind),
        })
        .collect();
    if track == Track::Effect && reusable_trail(draft).is_some() {
        result.push(Entry {
            label: "Reuse sword trail".into(),
            group: "Visuals",
            block: PaletteBlock::Node(63),
        });
    }
    result.push(Entry {
        label: plain::block_name("New controller").into(),
        group: "Character",
        block: PaletteBlock::NewController,
    });
    result.push(Entry {
        label: "State".into(),
        group: "Character",
        block: PaletteBlock::Character(CharacterBlock::State),
    });
    for (kind, label) in [
        (VarType::Int, "Counter"),
        (VarType::Float, "Gauge"),
        (VarType::Flag, "Flag"),
        (VarType::Timer, "Timer"),
    ] {
        result.push(Entry {
            label: plain::block_name(label).into(),
            group: "Character",
            block: PaletteBlock::Character(CharacterBlock::Variable(kind)),
        });
    }
    let has_status = crate::gimmicks::registry::entries_for(
        fighter,
        crate::gimmicks::registry::IdentifierKind::FighterStatus,
    )
    .next()
    .is_some();
    // Status edges start on the fighter's first status; their source block picks the status.
    let status_hooks = [HookName::OnStatusEnter, HookName::OnStatusExit]
        .into_iter()
        .filter(|_| has_status);
    for hook in crate::gimmicks::graph_edit::direct_hooks()
        .into_iter()
        .filter(|hook| !matches!(hook, HookName::OnStatusEnter | HookName::OnStatusExit))
        .filter(|hook| {
            *hook != HookName::OnSwitch
                || matches!(
                    fighter,
                    "eflame" | "elight" | "pzenigame" | "pfushigisou" | "plizardon"
                )
        })
        .chain(status_hooks)
    {
        result.push(Entry {
            label: format!(
                "When: {}",
                crate::gimmicks::graph_view::hook_label(hook).to_lowercase()
            ),
            group: "Character",
            block: PaletteBlock::Character(CharacterBlock::Transition(hook)),
        });
    }
    result.push(Entry {
        label: "When: a move tells it".into(),
        group: "Character",
        block: PaletteBlock::Character(CharacterBlock::EventTransition),
    });
    if track == Track::Game {
        for entry in crate::gimmicks::native_catalog::all_for_fighter(fighter) {
            for (verb, use_) in [("If", NativeUse::Test), ("Set", NativeUse::Set)] {
                // Values the runtime refuses to write (Kirby's copy ability) are test-only here too.
                let writable = crate::gimmicks::native_inputs::NativeInput::from_registry(
                    entry.fighter,
                    entry.symbol,
                    entry.kind,
                )
                .and_then(|input| input.validate_write(fighter))
                .is_ok();
                if use_ == NativeUse::Set && !writable {
                    continue;
                }
                result.push(Entry {
                    label: format!("{verb} {}", entry.label.to_lowercase()),
                    group: "Native gimmick",
                    block: PaletteBlock::Native(entry, use_),
                });
            }
        }
    }
    result.extend(Preset::ALL.into_iter().map(|preset| Entry {
        label: preset.label().into(),
        group: "Character",
        block: PaletteBlock::Preset(preset),
    }));
    result
}

/// Script blocks most moves use; the rest of a group folds under "More" until the user searches.
fn is_common(block: &PaletteBlock) -> bool {
    const COMMON: &[usize] = &[
        0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15, 17, 18, 19, 20, 21, 22, 23, 24, 25, 31,
        35, 36, 37, 38, 53, 57, 78, 101, 108,
    ];
    match block {
        PaletteBlock::Node(kind) => COMMON.contains(kind),
        PaletteBlock::Character(CharacterBlock::Transition(hook)) => matches!(
            hook,
            HookName::OnChargeTick
                | HookName::OnTimeout
                | HookName::OnDealDamage
                | HookName::OnTakeDamage
                | HookName::OnStatusEnter
        ),
        // The long list of values from the game stays folded until the user searches.
        PaletteBlock::Native(entry, _) => {
            entry.note != crate::gimmicks::native_catalog::LISTED_FROM_GAME
        }
        // Whole-controller presets are also under Gimmicks > Start from; keep the list short.
        PaletteBlock::Preset(_) => false,
        _ => true,
    }
}

fn matches(entry: &Entry, query: &str) -> bool {
    if let PaletteBlock::Node(kind) = entry.block {
        palette_matches(kind, &format!("{} {}", entry.group, entry.label), query)
    } else {
        let text = format!("{} {}", entry.group, entry.label).to_ascii_lowercase();
        query
            .to_ascii_lowercase()
            .split_whitespace()
            .all(|word| text.contains(word))
    }
}

impl NodeEditor {
    #[allow(clippy::too_many_arguments)]
    pub fn workspace_ui(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        move_name: &str,
        saved: Option<&MoveGraph>,
        scripts: [&AcmdScript; 3],
        effects: &crate::data::EffectScript,
        base: &BehaviorGraph,
        siblings: &BTreeMap<String, MoveGraph>,
        layout: &crate::gimmicks::field_canvas::FieldLayout,
    ) -> Option<MoveGraph> {
        self.workspace = true;
        self.list_mode = false;
        if self.key != format!("{fighter}/{move_name}") || self.character.is_none() {
            self.character = Some(CharacterDraft {
                main: base.clone(),
                charts: self.controller_charts.clone(),
                layout: layout.clone(),
            });
            // A refreshed controller is not an edit; keep it out of a pending undo step.
            if let Some(gesture) = &mut self.gesture {
                gesture.character = self.character.clone();
            }
        }
        if self.key != format!("{fighter}/{move_name}") || self.saved_character.is_none() {
            self.saved_character = self.character.clone();
        }
        // Moves may target controllers that are not applied yet; Apply checks both together.
        if let Some(character) = self.character.clone() {
            self.set_controller_charts(fighter, &character.main, &character.charts);
        }
        self.canvas.workspace = true;
        self.canvas.fighter = fighter.into();
        self.ui(
            ui, fighter, move_name, saved, scripts, effects, base, siblings,
        )
    }

    /// Picks up controller changes made outside the workspace (Gimmicks presets, imports)
    /// unless the workspace holds character edits of its own.
    pub fn refresh_character(&mut self) {
        if serde_json::to_value(&self.character).ok()
            == serde_json::to_value(&self.saved_character).ok()
        {
            self.character = None;
            self.saved_character = None;
        }
    }

    pub fn applied_character(
        &self,
    ) -> Option<(
        BehaviorGraph,
        BTreeMap<String, crate::mod_project::ControllerChart>,
        crate::gimmicks::field_canvas::FieldLayout,
    )> {
        self.character.as_ref().map(|character| {
            (
                character.main.clone(),
                character.charts.clone(),
                character.layout.clone(),
            )
        })
    }

    pub(super) fn import_tracks(
        &mut self,
        fighter: &str,
        scripts: [&AcmdScript; 3],
        effects: &crate::data::EffectScript,
    ) {
        self.draft.tracks = [Track::Game, Track::Sound, Track::Expression]
            .into_iter()
            .zip(scripts)
            .map(|(track, script)| {
                super::super::timeline::import_for_fighter(fighter, track, script)
            })
            .collect();
        self.draft
            .tracks
            .push(super::super::effects::import_for_fighter(fighter, effects));
    }

    pub(super) fn workspace_field(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        move_name: &str,
        base: &BehaviorGraph,
        siblings: &BTreeMap<String, MoveGraph>,
    ) -> Rect {
        let bounds = ui.available_rect_before_wrap();
        let palette_width = 210.0_f32.min(bounds.width() * 0.23);
        let inspector_width = 330.0_f32.min(bounds.width() * 0.32);
        let palette_rect = Rect::from_min_max(
            bounds.min,
            egui::pos2(bounds.left() + palette_width, bounds.bottom()),
        );
        let inspector_rect = Rect::from_min_max(
            egui::pos2(bounds.right() - inspector_width, bounds.top()),
            bounds.max,
        );
        let field_rect = Rect::from_min_max(
            egui::pos2(palette_rect.right() + 8.0, bounds.top()),
            egui::pos2(inspector_rect.left() - 8.0, bounds.bottom()),
        );
        let mut palette = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("workspace_palette")
                .max_rect(palette_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        palette.strong("Blocks");
        palette.add(
            egui::TextEdit::singleline(&mut self.palette_query)
                .hint_text("Search blocks…")
                .desired_width(palette_width - 8.0),
        );
        palette.horizontal_wrapped(|ui| {
            for (index, label) in plain::LANES.iter().enumerate() {
                ui.selectable_value(&mut self.workspace_track, index, *label);
            }
        });
        let track = TRACKS[self.workspace_track.min(3)];
        let entries = catalog(fighter, track, &self.draft);
        let mut clicked = None;
        let mut chosen_starter = None;
        egui::ScrollArea::vertical().id_salt("workspace_catalog").max_height(palette.available_height()).show(&mut palette, |ui| {
            // The tab's own blocks come first and open; events and character blocks are shared
            // by every tab, so they follow, closed.
            let primary = match track {
                Track::Game => "Hitboxes & grabs",
                Track::Effect | Track::Expression => "Visuals",
                Track::Sound => "Audio",
            };
            egui::CollapsingHeader::new("Start a new move").id_salt("workspace_starters").default_open(true).show(ui, |ui| {
                for starter in Starter::ALL {
                    if ui.add(egui::Label::new(format!("+ {}", starter.label())).selectable(false).sense(Sense::click())).on_hover_text(starter.help()).clicked() {
                        chosen_starter = Some(starter);
                    }
                }
            });
            let shared = |group: &str| group == "Character";
            let mut groups = GROUPS.to_vec();
            groups.sort_by_key(|group| (*group != primary, shared(group)));
            for group in groups {
                let entries: Vec<_> = entries.iter().filter(|entry| entry.group == group && matches(entry, &self.palette_query)).collect();
                if entries.is_empty() { continue; }
                let searching = !self.palette_query.is_empty();
                let (common, rare): (Vec<_>, Vec<_>) = entries.into_iter().partition(|entry| searching || is_common(&entry.block));
                egui::CollapsingHeader::new(plain::group_name(group)).id_salt((group, track_label(track))).default_open(group == primary || group == "Flow").open(searching.then_some(true)).show(ui, |ui| {
                    let mut item = |ui: &mut Ui, entry: &Entry| {
                        let response = ui.add(egui::Label::new(format!("+ {}", entry.label)).selectable(false).sense(Sense::click_and_drag())).on_hover_text(format!("{}{}Click to add it after the selected block, or drag it onto the field.", plain::block_help(&entry.label).unwrap_or(""), if plain::block_help(&entry.label).is_some() { "\n" } else { "" }));
                        response.dnd_set_drag_payload(entry.block.clone());
                        if response.clicked() { clicked = Some(entry.block.clone()); }
                    };
                    for entry in common { item(ui, entry); }
                    if !rare.is_empty() {
                        egui::CollapsingHeader::new(format!("More ({})", rare.len())).id_salt((group, track_label(track), "more")).default_open(false).show(ui, |ui| {
                            for entry in rare { item(ui, entry); }
                        });
                    }
                });
            }
        });
        let mut field = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("workspace_field")
                .max_rect(field_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        self.canvas.set_character(
            self.character
                .as_ref()
                .map(|character| (&character.main, &character.charts, &character.layout)),
        );
        self.canvas.ui(
            &mut field,
            &mut self.draft,
            None,
            field_rect.height() - 30.0,
        );
        if let Some(character) = &mut self.character {
            self.canvas.update_character_layout(
                &mut character.main,
                &mut character.layout,
                &mut character.charts,
            );
        }
        if let Some(id) = self.canvas.selected_node() {
            if let Some(index) = self
                .draft
                .tracks
                .iter()
                .position(|track| super::super::canvas::contains(&track.nodes, id))
            {
                if self.track != index {
                    self.track = index;
                    let selected = self.draft.tracks[index].track;
                    self.workspace_track = TRACKS
                        .iter()
                        .position(|track| *track == selected)
                        .unwrap_or(0);
                }
            }
        }
        if let Some((payload, position, after)) = self.canvas.take_palette_drop() {
            // Palette entries are built for one script; a script block belongs in its own lane.
            match self.canvas.lane_track_at(position[1]) {
                Some(lane) if lane != track && matches!(payload, PaletteBlock::Node(_)) => {
                    self.error = format!(
                        "These are {} blocks. Drop them in the {} lane or switch the tab above the list.",
                        track_label(track),
                        track_label(track),
                    );
                }
                _ => self.add_workspace_block(
                    payload, fighter, move_name, base, siblings, track, position, after,
                ),
            }
        }
        if let Some(starter) = chosen_starter {
            self.apply_starter(starter, fighter);
        }
        if let Some(block) = clicked {
            let position = self.canvas.insertion_position();
            self.add_workspace_block(
                block,
                fighter,
                move_name,
                base,
                siblings,
                track,
                position,
                // Blocks chain within a lane, so a block from another tab starts its own lane.
                self.canvas
                    .selected_node()
                    .filter(|id| {
                        self.draft.tracks.iter().any(|source| {
                            source.track == track
                                && super::super::canvas::contains(&source.nodes, *id)
                        })
                    })
                    .map(|id| format!("node:{id}")),
            );
        }
        self.workspace_edit_commands(ui, fighter, move_name, base, siblings);
        if let Some(track) = self.canvas.take_lane_clear() {
            for source in self
                .draft
                .tracks
                .iter_mut()
                .filter(|source| source.track == track)
            {
                source.nodes.clear();
            }
            super::super::canvas::prune_positions(&mut self.draft);
            self.canvas.selected = None;
        }
        if let Some(position) = self.canvas.quick_add_position() {
            let mut chosen = None;
            // A wire continues its own script, whatever palette tab is open.
            let track = self
                .canvas
                .quick_add_source()
                .and_then(|source| source.strip_prefix("node:")?.parse().ok())
                .and_then(|id| {
                    self.draft
                        .tracks
                        .iter()
                        .find(|source| super::super::canvas::contains(&source.nodes, id))
                })
                .map_or(track, |source| source.track);
            let choices = catalog(fighter, track, &self.draft);
            egui::Area::new(egui::Id::new("workspace_quick_add"))
                .fixed_pos(position)
                .constrain(true)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(240.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.palette_query)
                                .hint_text("Connect to…"),
                        )
                        .request_focus();
                        let accepted: Vec<_> = choices
                            .iter()
                            .filter(|entry| {
                                matches(entry, &self.palette_query)
                                    && self.canvas.quick_add_accepts(&entry.block)
                            })
                            .collect();
                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            chosen = accepted.first().map(|entry| entry.block.clone());
                        }
                        egui::ScrollArea::vertical()
                            .max_height(280.0)
                            .show(ui, |ui| {
                                for entry in &accepted {
                                    if ui.button(&entry.label).clicked() {
                                        chosen = Some(entry.block.clone());
                                    }
                                }
                            });
                        if ui.small_button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            self.canvas.cancel_quick_add();
                            self.palette_query.clear();
                        }
                    });
                });
            if let Some(block) = chosen {
                self.palette_query.clear();
                let (world, after) = self.canvas.finish_quick_add();
                self.add_workspace_block(
                    block, fighter, move_name, base, siblings, track, world, after,
                );
            }
        }
        ui.painter().vline(
            palette_rect.right() + 4.0,
            bounds.y_range(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        ui.painter().vline(
            inspector_rect.left() - 4.0,
            bounds.y_range(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        inspector_rect.shrink2(Vec2::new(6.0, 0.0))
    }

    /// A move with no scripts yet gets all four lanes, empty.
    fn import_tracks_empty(&mut self) {
        self.draft.tracks = TRACKS
            .iter()
            .map(|track| MoveTrack {
                track: *track,
                nodes: Vec::new(),
                baseline: Vec::new(),
                effect_execute_baseline: Vec::new(),
            })
            .collect();
    }

    /// Replaces the move with a working starter. The undo step comes from the click's gesture.
    fn apply_starter(&mut self, starter: Starter, fighter: &str) {
        let first = next_id(&self.draft);
        let speed = Some(NodeKind::Execute {
            commands: vec![ExcuteStmt::SetSpeed(crate::data::SetSpeedCall {
                speed_x: 2.5,
                speed_y: 0.0,
            })],
        });
        let shot = self.make_node(35, fighter, first, Track::Game);
        let Some(speed) = speed else { return };
        if self
            .draft
            .tracks
            .iter()
            .all(|source| source.track != Track::Game)
        {
            self.import_tracks_empty();
        }
        for source in &mut self.draft.tracks {
            source.nodes = if source.track == Track::Game {
                plain::starter_nodes(starter, first, speed.clone(), shot.clone())
            } else {
                Vec::new()
            };
            // Nothing of the game's own script is left to preserve.
            source.baseline.clear();
            source.effect_execute_baseline.clear();
        }
        self.draft.positions.clear();
        self.workspace_track = 0;
        self.canvas.selected = None;
        self.canvas.fit_to_view();
        self.error.clear();
    }

    /// The whole move, read as sentences. Shown when nothing is selected.
    pub(super) fn story_ui(&mut self, ui: &mut Ui) {
        ui.strong("What this move does");
        let mut focus = None;
        let mut any = false;
        for (index, source) in self.draft.tracks.iter().enumerate() {
            let mut lines = Vec::new();
            plain::story(
                &source.nodes,
                0,
                &mut 0.0,
                &super::super::canvas::story_caption,
                &mut lines,
            );
            if lines.is_empty() {
                continue;
            }
            any = true;
            let lane = track_label(source.track);
            egui::CollapsingHeader::new(lane).id_salt(("story", index)).default_open(source.track == Track::Game).show(ui, |ui| {
                let mut last = None;
                for line in &lines {
                    if line.depth == 0 && line.frame.is_some() && line.frame != last {
                        last = line.frame;
                        ui.weak(format!("Frame {}", line.frame.unwrap_or_default()));
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(8.0 + line.depth as f32 * 12.0);
                        let label = egui::Label::new(&line.text).sense(Sense::click()).selectable(false);
                        if ui.add(label).on_hover_text("Click to find this block").clicked() {
                            focus = line.node;
                        }
                    });
                }
                if source.track == Track::Game && plain::leaves_hitbox_on(&source.nodes) {
                    ui.colored_label(ui.visuals().warn_fg_color, "A hitbox stays on until the move ends. Add End all hitboxes to stop it sooner.");
                }
            });
        }
        if !any {
            ui.weak("Nothing yet. Pick a starter on the left, or add a block.");
        }
        if let Some(id) = focus {
            self.canvas.focus_node(id);
        }
    }

    /// Adds a state, variable or transition to the selected controller, or to the main one.
    fn add_character_block(&mut self, fighter: &str, block: CharacterBlock) {
        use crate::gimmicks::behavior::{CmpOp, ConstValue, Expr, Predicate, Transition, VarDef};
        use crate::gimmicks::graph_edit;
        let chart = self
            .canvas
            .selected_character()
            .map_or_else(|| "main".to_owned(), |(chart, _)| chart);
        let Some(character) = &mut self.character else {
            return;
        };
        let graph = if chart == "main" {
            &mut character.main
        } else if let Some(chart) = character.charts.get_mut(&chart) {
            &mut chart.graph
        } else {
            return;
        };
        let unique = |prefix: &str, taken: &mut dyn Iterator<Item = &str>| {
            let taken: Vec<&str> = taken.collect();
            (1..)
                .map(|index| format!("{prefix}_{index}"))
                .find(|name| !taken.contains(&name.as_str()))
                .expect("unbounded names")
        };
        let (result, reference) = match block {
            CharacterBlock::State => {
                let name = unique(
                    "state",
                    &mut graph.states.iter().map(|state| state.name.as_str()),
                );
                (
                    graph_edit::add_state(graph, fighter, &name),
                    FieldRef::State(name),
                )
            }
            CharacterBlock::Variable(kind) => {
                let (prefix, initial) = match kind {
                    VarType::Int => ("counter", ConstValue::Int(0)),
                    VarType::Float => ("gauge", ConstValue::Float(0.0)),
                    VarType::Flag => ("flag", ConstValue::Flag(false)),
                    VarType::Timer => ("timer", ConstValue::Int(0)),
                };
                let name = unique(prefix, &mut graph.vars.iter().map(|var| var.name.as_str()));
                (
                    graph_edit::add_variable(
                        graph,
                        fighter,
                        VarDef {
                            keep_across_form_switch: false,
                            name: name.clone(),
                            kind,
                            initial,
                        },
                    ),
                    FieldRef::Variable(name),
                )
            }
            CharacterBlock::EventTransition => {
                if graph.states.is_empty() {
                    self.error =
                        "Add a character state first; transitions move between states.".into();
                    return;
                }
                let name = unique(
                    "event",
                    &mut graph
                        .emission_plan
                        .event_bindings
                        .iter()
                        .map(|binding| binding.name.as_str()),
                );
                let transition = Transition {
                    from: graph.initial.clone(),
                    to: graph.initial.clone(),
                    on_frame: true,
                    guard: crate::gimmicks::graph_view::always(),
                    actions: Vec::new(),
                };
                let index = graph.transitions.len();
                (
                    graph_edit::add_event_transition(graph, fighter, transition, &name),
                    FieldRef::Transition(index),
                )
            }
            CharacterBlock::Transition(hook) => {
                if graph.states.is_empty() {
                    self.error =
                        "Add a character state first; transitions move between states.".into();
                    return;
                }
                let transition = Transition {
                    from: graph.initial.clone(),
                    to: graph.initial.clone(),
                    on_frame: true,
                    guard: Predicate::Compare {
                        op: CmpOp::Eq,
                        left: Expr::Const(ConstValue::Int(0)),
                        right: Expr::Const(ConstValue::Int(0)),
                    },
                    actions: Vec::new(),
                };
                let index = graph.transitions.len();
                let result = if matches!(hook, HookName::OnStatusEnter | HookName::OnStatusExit) {
                    match crate::gimmicks::registry::entries_for(
                        fighter,
                        crate::gimmicks::registry::IdentifierKind::FighterStatus,
                    )
                    .next()
                    {
                        Some(status) => graph_edit::add_status_transition(
                            graph,
                            fighter,
                            transition,
                            hook,
                            status.symbol,
                        ),
                        None => Err("This fighter has no known statuses.".into()),
                    }
                } else {
                    graph_edit::add_transition(graph, fighter, transition, hook)
                };
                (result, FieldRef::Transition(index))
            }
        };
        match result {
            Ok(updated) => {
                *graph = updated;
                let key = format!("character:{chart}:{}", reference.key());
                self.canvas.reveal = Some(key.clone());
                self.canvas.select(key);
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_workspace_block(
        &mut self,
        block: PaletteBlock,
        fighter: &str,
        _move_name: &str,
        base: &BehaviorGraph,
        _siblings: &BTreeMap<String, MoveGraph>,
        track: Track,
        position: [f32; 2],
        after: Option<String>,
    ) {
        let previous = self.draft.clone();
        let key = match block {
            PaletteBlock::Node(kind) => {
                let id = next_id(&self.draft);
                let Some(kind) = self.make_node(kind, fighter, id, track) else {
                    self.error = "Add a \"When: a move tells it\" block to a gimmick first.".into();
                    return;
                };
                let Some(index) = self
                    .draft
                    .tracks
                    .iter()
                    .position(|source| source.track == track)
                else {
                    return;
                };
                self.track = index;
                self.draft.tracks[index].nodes.push(MoveNode { id, kind });
                format!("node:{id}")
            }
            PaletteBlock::Character(block) => {
                self.add_character_block(fighter, block);
                return;
            }
            PaletteBlock::Native(entry, use_) => {
                let Some(index) = self
                    .draft
                    .tracks
                    .iter()
                    .position(|source| source.track == Track::Game)
                else {
                    return;
                };
                let id = next_id(&self.draft);
                self.track = index;
                self.draft.tracks[index].nodes.push(MoveNode {
                    id,
                    kind: native_node(entry, use_),
                });
                format!("node:{id}")
            }
            PaletteBlock::Preset(_) | PaletteBlock::NewController => {
                let Some(character) = &mut self.character else {
                    return;
                };
                let (stem, graph) = match block {
                    PaletteBlock::Preset(preset) => (preset.stem(), preset.graph()),
                    // A controller must be reachable, so it starts with one move-event receiver.
                    _ => (
                        "controller",
                        BehaviorGraph {
                            initial: "active".into(),
                            states: vec![crate::gimmicks::behavior::StateDef {
                                status: None,
                                name: "active".into(),
                            }],
                            transitions: vec![crate::gimmicks::behavior::Transition {
                                from: "active".into(),
                                to: "active".into(),
                                guard: crate::gimmicks::graph_view::always(),
                                on_frame: true,
                                actions: Vec::new(),
                            }],
                            emission_plan: crate::gimmicks::codegen::EmissionPlan {
                                event_bindings: vec![crate::gimmicks::codegen::EventBinding {
                                    name: "event_1".into(),
                                    transitions: vec![0],
                                    fields: Vec::new(),
                                }],
                                ..Default::default()
                            },
                            ..BehaviorGraph::default()
                        },
                    ),
                };
                let result = crate::gimmicks::controller_edit::add(
                    fighter,
                    base,
                    &mut character.charts,
                    stem,
                    graph,
                );
                match result {
                    Ok(id) => {
                        self.error.clear();
                        let key = format!("character:{id}:{}", FieldRef::Graph.key());
                        // A new controller gets its own band below the move; no drop point applies.
                        self.canvas.reveal = Some(key.clone());
                        self.canvas.select(key);
                    }
                    Err(error) => self.error = error,
                }
                return;
            }
        };
        // Script blocks take their lane slot from execution order, so the field shows the real
        // sequence; only free-standing events, variables and controllers keep the drop point.
        if !key.starts_with("node:") {
            self.draft.positions.insert(key.clone(), position);
        }
        if let Some(after) = after {
            if let Err(error) = self.canvas.connect_inserted(&mut self.draft, &after, &key) {
                self.draft = previous;
                self.error = error;
                return;
            }
        }
        if let Some(id) = key.strip_prefix("node:").and_then(|id| id.parse().ok()) {
            if let Some(track) = self.draft.tracks.get_mut(self.track) {
                frame_after_insert_point(&mut track.nodes, id);
                if matches!(block, PaletteBlock::Node(5)) {
                    hitbox_after_insert_point(&mut track.nodes, id);
                }
            }
        }
        self.canvas.reveal = Some(key.clone());
        self.canvas.select(key);
        self.error.clear();
    }

    fn workspace_edit_commands(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        move_name: &str,
        base: &BehaviorGraph,
        siblings: &BTreeMap<String, MoveGraph>,
    ) {
        let command = self.canvas.take_edit_command();
        let keyboard = !ui.ctx().text_edit_focused();
        let key = |key| keyboard && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, key));
        let copy = command == Some(super::super::canvas::EditCommand::Copy) || key(egui::Key::C);
        let duplicate =
            command == Some(super::super::canvas::EditCommand::Duplicate) || key(egui::Key::D);
        if copy || duplicate {
            self.copy_selected_block(fighter);
        }
        if duplicate || key(egui::Key::V) {
            self.paste_block(
                fighter,
                move_name,
                base,
                siblings,
                self.canvas.selected_node(),
            );
        }
        if command == Some(super::super::canvas::EditCommand::Delete)
            || keyboard && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
        {
            for selected in self.canvas.selection() {
                if let Some(id) = selected
                    .strip_prefix("node:")
                    .and_then(|id| id.parse().ok())
                {
                    for track in &mut self.draft.tracks {
                        remove_workspace_node(&mut track.nodes, id);
                    }
                }
            }
            super::super::canvas::prune_positions(&mut self.draft);
        }
    }
    pub(super) fn make_node(
        &self,
        kind: usize,
        fighter: &str,
        next: u32,
        track: Track,
    ) -> Option<NodeKind> {
        if kind == 101 && super::super::chart_event_view::first(&self.controller_events).is_none() {
            return None;
        }
        Some(match kind {
            0 => NodeKind::Frame { frame: 1.0 },
            106 => NodeKind::NativeExecuteFrame { frame: 1.0 },
            107 => NodeKind::NativeMotionLoopSync,
            121 => NodeKind::NativeWaitLoopClear,
            1 => NodeKind::Wait { frames: 1.0 },
            2 => NodeKind::Execute {
                commands: vec![ExcuteStmt::ClearAll],
            },
            3 => NodeKind::Execute {
                commands: vec![ExcuteStmt::ReverseLr],
            },
            4 => NodeKind::Repeat {
                motion_loop: crate::data::MotionLoop::No,
                count: 2,
                body: vec![MoveNode {
                    id: next.saturating_add(1),
                    kind: NodeKind::Wait { frames: 1.0 },
                }],
            },
            5 => NodeKind::Execute {
                commands: vec![ExcuteStmt::Attack(
                    crate::data::Hitbox::default().to_attack_call(),
                )],
            },
            6 => NodeKind::Branch {
                condition: MoveCondition::Grounded,
                then_nodes: Vec::new(),
                else_nodes: Vec::new(),
            },
            7..=10 | 54..=62 | 87..=89 => NodeKind::EffectExecute {
                commands: vec![context_effect(kind, fighter)],
            },
            63 => NodeKind::EffectExecute {
                commands: vec![reusable_trail(&self.draft)
                    .expect("palette contains only measured trail templates")],
            },
            11..=14 | 51 => NodeKind::Execute {
                commands: vec![default_command(kind)],
            },
            15 => NodeKind::MotionRate { rate: 1.0 },
            16 => NodeKind::BareCommand {
                command: Box::new(default_command(11)),
            },
            17 => NodeKind::Execute {
                commands: vec![ExcuteStmt::HitStatus {
                    target: crate::data::HurtTarget::Whole,
                    status: "*HIT_STATUS_NORMAL".into(),
                }],
            },
            18..=34 => NodeKind::Execute {
                commands: vec![default_command(kind)],
            },
            101 => {
                let (chart_id, name) =
                    super::super::chart_event_view::first(&self.controller_events)
                        .expect("enabled chart event choice");
                NodeKind::Execute {
                    commands: vec![super::super::chart_event_view::command_for(
                        &self.controller_events,
                        chart_id,
                        name,
                    )
                    .expect("compiled receiver choice")],
                }
            }
            _ => context_node_on_track(kind, fighter, next, track),
        })
    }
}

fn find_node(nodes: &mut [MoveNode], id: u32) -> Option<&mut MoveNode> {
    nodes.iter_mut().find_map(|node| {
        if node.id == id {
            return Some(node);
        }
        match &mut node.kind {
            NodeKind::Repeat { body, .. } => find_node(body, id),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => find_node(then_nodes, id).or_else(|| find_node(else_nodes, id)),
            _ => None,
        }
    })
}

/// A new Frame block waits until one frame after the point it was inserted; the old default of
/// frame 1 did nothing anywhere past the start of the move.
fn frame_after_insert_point(nodes: &mut [MoveNode], id: u32) {
    let Some(MoveNode {
        kind: NodeKind::Frame { frame },
        ..
    }) = find_node(nodes, id)
    else {
        return;
    };
    *frame = 0.0;
    let mut starts = BTreeMap::new();
    crate::move_nodes::canvas::start_frames(nodes, &mut 0.0, &mut starts);
    let start = starts.get(&id).copied().unwrap_or_default();
    if let Some(MoveNode {
        kind: NodeKind::Frame { frame },
        ..
    }) = find_node(nodes, id)
    {
        *frame = (start.floor() + 1.0).max(1.0);
    }
}

fn hitbox_after_insert_point(nodes: &mut [MoveNode], id: u32) {
    use std::collections::BTreeSet;
    fn apply(command: &ExcuteStmt, active: &mut BTreeSet<u32>) {
        match command {
            ExcuteStmt::Attack(call) => {
                active.insert(call.id);
            }
            ExcuteStmt::Clear(id) => {
                active.remove(id);
            }
            ExcuteStmt::ClearAll => active.clear(),
            _ => {}
        }
    }
    fn scan(nodes: &[MoveNode], id: u32, active: &mut BTreeSet<u32>) -> bool {
        for node in nodes {
            if node.id == id {
                return true;
            }
            match &node.kind {
                NodeKind::Execute { commands } => {
                    for command in commands {
                        apply(command, active);
                    }
                }
                NodeKind::BareCommand { command } => apply(command, active),
                NodeKind::Branch {
                    then_nodes,
                    else_nodes,
                    ..
                } => {
                    let mut other = active.clone();
                    if scan(then_nodes, id, active) {
                        return true;
                    }
                    if scan(else_nodes, id, &mut other) {
                        *active = other;
                        return true;
                    }
                    // Reserve IDs active on either possible path.
                    active.extend(other);
                }
                NodeKind::Repeat { count, body, .. } if *count > 0 => {
                    if scan(body, id, active) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }
    let mut active = BTreeSet::new();
    scan(nodes, id, &mut active);
    let free = (0..).find(|candidate| !active.contains(candidate)).unwrap();
    if let Some(MoveNode {
        kind: NodeKind::Execute { commands },
        ..
    }) = find_node(nodes, id)
    {
        if let [ExcuteStmt::Attack(call)] = commands.as_mut_slice() {
            call.id = free;
        }
    }
}

fn remove_workspace_node(nodes: &mut Vec<MoveNode>, id: u32) {
    nodes.retain(|node| node.id != id);
    for node in nodes {
        match &mut node.kind {
            NodeKind::Repeat { body, .. } => remove_workspace_node(body, id),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                remove_workspace_node(then_nodes, id);
                remove_workspace_node(else_nodes, id);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_hitbox_uses_lowest_free_id_after_clears() {
        let attack = |id| {
            let mut call = crate::data::Hitbox::default().to_attack_call();
            call.id = id;
            ExcuteStmt::Attack(call)
        };
        for (clear, expected) in [
            (None, 2),
            (Some(ExcuteStmt::Clear(0)), 0),
            (Some(ExcuteStmt::Clear(1)), 1),
            (Some(ExcuteStmt::ClearAll), 0),
        ] {
            let mut commands = vec![attack(0), attack(1)];
            commands.extend(clear);
            let mut nodes = vec![
                MoveNode {
                    id: 1,
                    kind: NodeKind::Execute { commands },
                },
                MoveNode {
                    id: 2,
                    kind: NodeKind::Execute {
                        commands: vec![attack(0)],
                    },
                },
                MoveNode {
                    id: 3,
                    kind: NodeKind::Execute {
                        commands: vec![ExcuteStmt::ClearAll],
                    },
                },
            ];
            hitbox_after_insert_point(&mut nodes, 2);
            assert!(
                matches!(&nodes[1].kind, NodeKind::Execute { commands } if matches!(&commands[0], ExcuteStmt::Attack(call) if call.id == expected))
            );
        }
    }
    #[test]
    fn refresh_drops_only_an_unedited_controller_draft() {
        let draft = |initial: &str| CharacterDraft {
            main: BehaviorGraph {
                initial: initial.into(),
                ..BehaviorGraph::default()
            },
            charts: BTreeMap::new(),
            layout: Default::default(),
        };
        let mut editor = NodeEditor {
            character: Some(draft("a")),
            saved_character: Some(draft("a")),
            ..Default::default()
        };
        editor.refresh_character();
        assert!(
            editor.character.is_none(),
            "an untouched draft must pick up Gimmicks-tab changes"
        );
        let mut editor = NodeEditor {
            character: Some(draft("b")),
            saved_character: Some(draft("a")),
            ..Default::default()
        };
        editor.refresh_character();
        assert_eq!(
            editor.character.map(|c| c.main.initial).as_deref(),
            Some("b"),
            "workspace edits survive"
        );
    }
    #[test]
    fn a_new_frame_block_lands_after_its_insert_point() {
        let node = |id, kind| MoveNode { id, kind };
        let mut nodes = vec![
            node(1, NodeKind::Frame { frame: 15.0 }),
            node(2, NodeKind::Wait { frames: 3.0 }),
            node(3, NodeKind::Frame { frame: 1.0 }),
        ];
        frame_after_insert_point(&mut nodes, 3);
        assert!(matches!(nodes[2].kind, NodeKind::Frame { frame } if frame == 19.0));
        // At the start of a move the default stays frame 1, and other blocks are untouched.
        let mut nodes = vec![
            node(1, NodeKind::Frame { frame: 1.0 }),
            node(2, NodeKind::Wait { frames: 4.0 }),
        ];
        frame_after_insert_point(&mut nodes, 1);
        frame_after_insert_point(&mut nodes, 2);
        assert!(matches!(nodes[0].kind, NodeKind::Frame { frame } if frame == 1.0));
        assert!(matches!(nodes[1].kind, NodeKind::Wait { frames } if frames == 4.0));
    }
    #[test]
    fn read_only_natives_offer_a_test_but_no_set_block() {
        let graph = MoveGraph::default();
        let count = |fighter: &str, wanted: NativeUse| {
            catalog(fighter, Track::Game, &graph)
                .iter()
                .filter(
                    |entry| matches!(entry.block, PaletteBlock::Native(_, use_) if use_ == wanted),
                )
                .count()
        };
        // Kirby's copy flag and kind are read-only; Limit goes through its helpers.
        assert!(count("kirby", NativeUse::Set) < count("kirby", NativeUse::Test));
        assert!(count("cloud", NativeUse::Test) > 0);
        assert!(catalog("cloud", Track::Game, &graph).iter().all(|entry| !matches!(
            entry.block,
            PaletteBlock::Native(native, NativeUse::Set) if native.symbol.contains("LIMIT_GAUGE")
        )));
        assert!(count("edge", NativeUse::Test) > 0);
        assert_eq!(
            count("edge", NativeUse::Set),
            count("edge", NativeUse::Test)
        );
    }
    #[test]
    fn catalog_uses_track_owner_filters_and_gameplay_aliases() {
        let graph = MoveGraph::default();
        let game = catalog("mario", Track::Game, &graph);
        assert!(game.iter().any(|entry| matches(entry, "projectile spawn")));
        assert!(game.iter().any(|entry| matches(entry, "armor")));
        assert!(game.iter().any(|entry| matches(entry, "charge")));
        assert!(game
            .iter()
            .any(|entry| matches!(entry.block, PaletteBlock::Node(101))));
        assert!(
            game.iter().all(|entry| GROUPS.contains(&entry.group)),
            "every palette entry must have a visible group"
        );
        assert!(!catalog("mario", Track::Sound, &graph)
            .iter()
            .any(|entry| matches!(entry.block, PaletteBlock::Node(5))));
    }
}
