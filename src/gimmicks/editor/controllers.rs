//! Independent controller selection and draft ownership in the character workspace.

use super::*;

const MAIN: &str = crate::gimmicks::controller_project::MAIN_CHART;

#[derive(Default)]
pub(super) struct ControllerSession {
    action_drafts: HashMap<String, ControllerAction>,
    action_test_boid: Option<u32>,
    preview_frames: u32,
    preview_cursor: usize,
    preview: Vec<crate::gimmicks::controller::ControllerFrame>,
    preview_graph: Option<BehaviorGraph>,
    pending: Option<(String, BehaviorGraph)>,
}

pub(super) fn effective_main(
    fighter: &str,
    graphs: &BTreeMap<String, BehaviorGraph>,
    recipes: Option<&BTreeMap<String, crate::move_nodes::model::MoveGraph>>,
    charts: Option<&BTreeMap<String, crate::mod_project::ControllerChart>>,
) -> Result<BehaviorGraph, String> {
    let empty = BehaviorGraph::default();
    let main = graphs.get(fighter).unwrap_or(&empty);
    crate::gimmicks::behavior::preflight_bounds(main)?;
    match recipes.filter(|recipes| !recipes.is_empty()) {
        Some(recipes) => crate::move_nodes::project::compile_recipes_with_controllers(
            fighter,
            main,
            charts.unwrap_or(&BTreeMap::new()),
            recipes,
        )
        .map(|compiled| compiled.behavior)
        .map_err(|errors| {
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        }),
        None => Ok(main.clone()),
    }
}

enum ChartCommand {
    Add(Preset),
    AddDefense(crate::gimmicks::status_recipes::Defense),
    Duplicate,
    Remove(String),
    Import(
        Box<crate::gimmicks::controller_asset::ControllerAsset>,
        String,
    ),
}

pub(super) fn can_remove_chart(
    chart_id: &str,
    recipes: Option<&BTreeMap<String, crate::move_nodes::model::MoveGraph>>,
) -> Result<(), String> {
    let Some(recipes) = recipes else {
        return Ok(());
    };
    if recipes.len() > crate::move_nodes::project::MAX_FIGHTER_MOVES {
        return Err("Move table exceeds the controller-reference scan limit".into());
    }
    for (move_name, graph) in recipes {
        if crate::move_nodes::project::referenced_controller_charts(graph)?.contains(chart_id) {
            return Err(format!("Controller {chart_id:?} is targeted by move {move_name:?}. Change or remove its controller reference first."));
        }
    }
    Ok(())
}

impl GimmicksEditor {
    fn take_controller_session(&mut self) -> ControllerSession {
        ControllerSession {
            action_drafts: std::mem::take(&mut self.action_drafts),
            action_test_boid: self.action_test_boid.take(),
            preview_frames: std::mem::take(&mut self.preview_frames),
            preview_cursor: std::mem::take(&mut self.preview_cursor),
            preview: std::mem::take(&mut self.preview),
            preview_graph: self.preview_graph.take(),
            pending: self.pending.take(),
        }
    }

    fn install_controller_session(&mut self, session: ControllerSession) {
        self.action_drafts = session.action_drafts;
        self.action_test_boid = session.action_test_boid;
        self.preview_frames = session.preview_frames;
        self.preview_cursor = session.preview_cursor;
        self.preview = session.preview;
        self.preview_graph = session.preview_graph;
        self.pending = session.pending;
    }

    pub(super) fn clear_controller_sessions(&mut self) {
        self.install_controller_session(ControllerSession::default());
        self.chart = MAIN.into();
        self.chart_sessions.clear();
        self.pending_chart_delete = None;
        self.controller_library.clear_pending();
    }

    pub(super) fn selected_chart(&self) -> &str {
        if self.chart.is_empty() {
            MAIN
        } else {
            &self.chart
        }
    }

    fn select_chart(&mut self, id: &str) {
        if self.selected_chart() == id {
            return;
        }
        let previous = self.selected_chart().to_owned();
        let active = self.take_controller_session();
        self.chart_sessions.insert(previous, active);
        let next = self.chart_sessions.remove(id).unwrap_or_default();
        self.install_controller_session(next);
        self.chart = id.into();
        self.pending_chart_delete = None;
    }

    pub(super) fn controller_field(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        data: &mut EditorData<'_>,
    ) {
        let charts = data.controller_charts.get(fighter);
        let contains =
            |id: &str| id == MAIN || charts.is_some_and(|charts| charts.contains_key(id));
        if !contains(self.selected_chart()) {
            self.select_chart(MAIN);
        }
        self.chart_sessions.retain(|id, _| contains(id));
        let mut selected = self.selected_chart().to_owned();
        let mut command = None;
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt(("controller_chart", fighter))
                .selected_text(if selected == MAIN { "Main" } else { &selected })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut selected, MAIN.into(), "Main");
                    for id in charts.into_iter().flat_map(|charts| charts.keys()) {
                        ui.selectable_value(&mut selected, id.clone(), id);
                    }
                }).response.on_hover_text("Each controller has independent state, variables, timers and an editable field. Switching keeps unapplied drafts.");
            ui.menu_button("New", |ui| {
                for preset in Preset::ALL {
                    if ui.button(preset.label()).clicked() {
                        command = Some(ChartCommand::Add(preset));
                        ui.close();
                    }
                }
                ui.separator();
                for defense in crate::gimmicks::status_recipes::Defense::ALL {
                    if ui.button(defense.label()).on_hover_text(defense.hover()).clicked() {
                        command = Some(ChartCommand::AddDefense(defense));
                        ui.close();
                    }
                }
            });
            let extra = selected != MAIN;
            if ui.add_enabled(extra, egui::Button::new("Duplicate").small())
                .on_hover_text("Copy the selected additional controller's applied graph and layout. Unapplied drafts are not copied.")
                .clicked() {
                command = Some(ChartCommand::Duplicate);
            }
            if ui.add_enabled(extra, egui::Button::new("Delete").small()).clicked() {
                self.pending_chart_delete = Some(selected.clone());
            }
            self.controller_library.ui(ui, fighter, &selected, charts);
        });
        if let Some((asset, id)) = self.controller_library.preview(ui) {
            command = Some(ChartCommand::Import(Box::new(asset), id));
        }
        if selected != self.selected_chart() {
            self.select_chart(&selected);
        }
        if let Some(id) = self.pending_chart_delete.clone() {
            ui.group(|ui| {
                ui.label(format!("Delete controller {id}?"));
                ui.weak("Removes its applied graph, layout and unapplied draft. Other controllers stay unchanged.");
                ui.horizontal(|ui| {
                    if ui.button("Delete controller").clicked() {
                        command = Some(ChartCommand::Remove(id));
                    }
                    if ui.button("Cancel").clicked() { self.pending_chart_delete = None; }
                });
            });
        }
        if let Some(command) = command {
            let result = match command {
                ChartCommand::Remove(id) => {
                    let result = can_remove_chart(&id, data.recipes).and_then(|()| {
                        data.controller_charts
                            .get_mut(fighter)
                            .ok_or_else(|| "No additional controllers to delete".into())
                            .and_then(|charts| {
                                crate::gimmicks::controller_edit::remove(charts, &id)
                            })
                    });
                    result.map(|_| {
                        self.select_chart(MAIN);
                        self.chart_sessions.remove(&id);
                        self.pending_chart_delete = None;
                        MAIN.to_owned()
                    })
                }
                command => effective_main(
                    fighter,
                    data.behaviors,
                    data.recipes,
                    data.controller_charts.get(fighter),
                )
                .and_then(|main| {
                    let charts = data.controller_charts.entry(fighter.into()).or_default();
                    match command {
                        ChartCommand::Add(preset) => {
                            let stem = preset.stem();
                            crate::gimmicks::controller_edit::add(
                                fighter,
                                &main,
                                charts,
                                stem,
                                preset.graph(),
                            )
                        }
                        ChartCommand::AddDefense(defense) => {
                            defense.graph(fighter).and_then(|graph| {
                                crate::gimmicks::controller_edit::add(
                                    fighter,
                                    &main,
                                    charts,
                                    defense.stem(),
                                    graph,
                                )
                            })
                        }
                        ChartCommand::Duplicate => crate::gimmicks::controller_edit::duplicate(
                            fighter,
                            &main,
                            charts,
                            self.selected_chart(),
                        ),
                        ChartCommand::Import(asset, id) => asset
                            .candidate(fighter, &main, charts, &id)
                            .map(|candidate| {
                                *charts = candidate;
                                self.controller_library.applied();
                                id
                            }),
                        ChartCommand::Remove(_) => unreachable!(),
                    }
                }),
            };
            if data
                .controller_charts
                .get(fighter)
                .is_some_and(BTreeMap::is_empty)
            {
                data.controller_charts.remove(fighter);
            }
            match result {
                Ok(id) => {
                    self.select_chart(&id);
                    self.status =
                        "Controllers saved; compatible live preview updates automatically.".into();
                }
                Err(error) => self.status = error,
            }
        }
        let chart_id = self.selected_chart().to_owned();
        ui.push_id(("chart_workspace", &chart_id), |ui| {
            if chart_id == MAIN {
                self.behaviors(ui, fighter, data.behaviors, data.link, data.recipes);
            } else if let Some(chart) = data
                .controller_charts
                .get_mut(fighter)
                .and_then(|charts| charts.get_mut(&chart_id))
            {
                if let Err(error) = crate::gimmicks::behavior::preflight_bounds(&chart.graph) {
                    ui.weak(error);
                    return;
                }
                // A local map keeps template edits from swapping the global main graph or
                // leaking temporary state into autosave or live sync.
                let mut graphs = BTreeMap::from([(fighter.to_owned(), chart.graph.clone())]);
                self.behaviors(ui, fighter, &mut graphs, data.link, None);
                chart.graph = graphs.remove(fighter).unwrap_or_default();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_delete_refuses_exact_move_target_without_guessing_donor() {
        use crate::move_nodes::model::{MoveGraph, MoveNode, MoveTrack, NodeKind, Track};
        let graph = MoveGraph {
            tracks: vec![MoveTrack {
                track: Track::Game,
                baseline: vec![],
                effect_execute_baseline: vec![],
                nodes: vec![MoveNode {
                    id: 1,
                    kind: NodeKind::Execute {
                        commands: vec![
                            crate::data::ExcuteStmt::ArticleLifecycle(
                                crate::data::ArticleLifecycleCall::Generate {
                                    symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
                                },
                            ),
                            crate::data::ExcuteStmt::GimmickChartEvent {
                                chart_id: "charge".into(),
                                name: "pulse".into(),
                            },
                        ],
                    },
                }],
            }],
            ..Default::default()
        };
        let recipes = BTreeMap::from([("special_n".into(), graph)]);
        let before = serde_json::to_value(&recipes).unwrap();
        assert!(can_remove_chart("charge", Some(&recipes))
            .unwrap_err()
            .contains("special_n"));
        can_remove_chart("unreferenced_bad_chart", Some(&recipes)).unwrap();
        can_remove_chart("charge", None).unwrap();
        assert_eq!(serde_json::to_value(&recipes).unwrap(), before);
    }

    #[test]
    fn switching_preserves_each_pending_draft_preview_and_action_target() {
        let mut editor = GimmicksEditor {
            pending: Some(("main draft".into(), Preset::KoTally.graph())),
            preview_frames: 17,
            action_test_boid: Some(123),
            ..Default::default()
        };
        editor.select_chart("timer_1");
        assert!(editor.pending.is_none());
        assert_eq!(editor.preview_frames, 0);
        editor.pending = Some(("timer draft".into(), Preset::Timer.graph()));
        editor.preview_frames = 99;
        editor.action_test_boid = Some(456);
        editor.select_chart(MAIN);
        assert_eq!(editor.pending.as_ref().unwrap().0, "main draft");
        assert_eq!(editor.preview_frames, 17);
        assert_eq!(editor.action_test_boid, Some(123));
        editor.select_chart("timer_1");
        assert_eq!(editor.pending.as_ref().unwrap().0, "timer draft");
        assert_eq!(editor.preview_frames, 99);
        assert_eq!(editor.action_test_boid, Some(456));
        editor.clear_controller_sessions();
        assert_eq!(editor.selected_chart(), MAIN);
        assert!(editor.pending.is_none());
        assert!(editor.chart_sessions.is_empty());
    }

    #[test]
    fn effective_main_rejects_oversized_drafts_before_clone() {
        let mut graph = Preset::Timer.graph();
        graph.vars.resize(65, graph.vars[0].clone());
        assert!(effective_main(
            "mario",
            &BTreeMap::from([("mario".into(), graph)]),
            None,
            None
        )
        .is_err());
    }

    fn chart(graph: BehaviorGraph) -> crate::mod_project::ControllerChart {
        crate::mod_project::ControllerChart {
            graph,
            layout: Default::default(),
        }
    }

    #[test]
    fn selecting_each_chart_renders_without_mutating_other_graphs_or_layouts() {
        let ctx = egui::Context::default();
        let mut editor = GimmicksEditor {
            fighter: "mario".into(),
            profile: crate::gimmicks::catalog::profile("mario"),
            section: Section::Behavior,
            ..Default::default()
        };
        let mut params = BTreeMap::new();
        let mut graphs = BTreeMap::from([("mario".into(), Preset::KoTally.graph())]);
        let original_graphs = graphs.clone();
        let mut layouts = BTreeMap::new();
        let mut charts = BTreeMap::from([(
            "mario".into(),
            BTreeMap::from([
                ("timer_1".into(), chart(Preset::Timer.graph())),
                ("charge_1".into(), chart(Preset::Charge.graph())),
            ]),
        )]);
        charts
            .get_mut("mario")
            .unwrap()
            .get_mut("timer_1")
            .unwrap()
            .layout
            .positions
            .insert("state/idle".into(), [17.0, 35.0]);
        let original_charts = charts.clone();
        let mut meters = BTreeMap::new();
        let labels = HashMap::new();
        let link = GameLink::offline_for_test();
        for id in [MAIN, "timer_1", "charge_1", MAIN, "timer_1"] {
            editor.select_chart(id);
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(420.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    editor.ui(
                        ui,
                        &CharacterContext::new("mario", "Mario", Vec::new()),
                        EditorData {
                            roots: &[],
                            labels: &labels,
                            params: &mut params,
                            behaviors: &mut graphs,
                            behavior_layouts: &mut layouts,
                            controller_charts: &mut charts,
                            meters: &mut meters,
                            link: &link,
                            recipes: None,
                        },
                    );
                },
            );
            assert_eq!(editor.selected_chart(), id);
            assert_eq!(graphs, original_graphs);
            assert_eq!(charts, original_charts);
        }
        assert!(layouts.is_empty());
        assert!(params.is_empty());
        assert!(meters.is_empty());
    }

    #[test]
    fn invalid_chart_change_rolls_back_complete_owner_not_other_fighters() {
        let mut params = BTreeMap::new();
        let mut graphs = BTreeMap::from([("mario".into(), Preset::Timer.graph())]);
        let mut layouts = BTreeMap::new();
        let mut charts = BTreeMap::from([
            (
                "mario".into(),
                BTreeMap::from([("timer".into(), chart(Preset::Timer.graph()))]),
            ),
            (
                "cloud".into(),
                BTreeMap::from([("charge".into(), chart(Preset::Charge.graph()))]),
            ),
        ]);
        let before = charts.clone();
        let mut meters = BTreeMap::new();
        let labels = HashMap::new();
        let link = GameLink::offline_for_test();
        let mut data = EditorData {
            roots: &[],
            labels: &labels,
            params: &mut params,
            behaviors: &mut graphs,
            behavior_layouts: &mut layouts,
            controller_charts: &mut charts,
            meters: &mut meters,
            link: &link,
            recipes: None,
        };
        let saved = SavedGimmicks::capture("mario", &data);
        data.controller_charts
            .get_mut("mario")
            .unwrap()
            .get_mut("timer")
            .unwrap()
            .graph
            .vars
            .clear();
        assert!(saved
            .validate_change("mario", &mut data)
            .unwrap_err()
            .contains("Controller not applied"));
        assert_eq!(*data.controller_charts, before);
    }

    #[test]
    fn deleting_invalid_chart_and_moving_layout_can_repair_invalid_saved_set() {
        let mut invalid = Preset::Timer.graph();
        invalid.vars.clear();
        let mut params = BTreeMap::new();
        let mut graphs = BTreeMap::new();
        let mut layouts = BTreeMap::new();
        let mut charts = BTreeMap::from([(
            "mario".into(),
            BTreeMap::from([
                ("bad".into(), chart(invalid.clone())),
                ("repair".into(), chart(invalid)),
            ]),
        )]);
        let mut meters = BTreeMap::new();
        let labels = HashMap::new();
        let link = GameLink::offline_for_test();
        let mut data = EditorData {
            roots: &[],
            labels: &labels,
            params: &mut params,
            behaviors: &mut graphs,
            behavior_layouts: &mut layouts,
            controller_charts: &mut charts,
            meters: &mut meters,
            link: &link,
            recipes: None,
        };
        let saved = SavedGimmicks::capture("mario", &data);
        let owner = data.controller_charts.get_mut("mario").unwrap();
        crate::gimmicks::controller_edit::remove(owner, "bad").unwrap();
        owner
            .get_mut("repair")
            .unwrap()
            .layout
            .positions
            .insert("state/idle".into(), [1.0, 2.0]);
        saved.validate_change("mario", &mut data).unwrap();
        assert_eq!(data.controller_charts["mario"].len(), 1);
        assert_eq!(
            data.controller_charts["mario"]["repair"].layout.positions["state/idle"],
            [1.0, 2.0]
        );
    }
}
