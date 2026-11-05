//! Compact, exact-chart receiver selection for timed move events.
use crate::data::ExcuteStmt;
use crate::gimmicks::behavior::BehaviorGraph;
use crate::gimmicks::behavior::{ConstValue, VarType};
use crate::gimmicks::controller_project::MAIN_CHART;
use crate::gimmicks::event_payload::{Argument, FieldDef};
use crate::mod_project::ControllerChart;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Receiver {
    name: String,
    fields: Vec<FieldDef>,
}

impl From<&str> for Receiver {
    fn from(name: &str) -> Self {
        Self {
            name: name.into(),
            fields: Vec::new(),
        }
    }
}

pub(super) type EventChoices = Vec<(String, Vec<Receiver>)>;

pub(super) fn choices(
    fighter: &str,
    main: &BehaviorGraph,
    charts: &BTreeMap<String, ControllerChart>,
) -> EventChoices {
    let Ok(compiled) = crate::gimmicks::controller_project::compile(fighter, main, charts) else {
        return Vec::new();
    };
    compiled
        .into_iter()
        .filter_map(|chart| {
            let main = chart.chart_id == MAIN_CHART;
            let mut names = chart
                .program
                .event_bindings
                .into_iter()
                .map(|binding| Receiver {
                    name: binding.name,
                    fields: binding.fields,
                })
                // The main controller's move sends carry no inputs.
                .filter(|receiver| !main || receiver.fields.is_empty())
                .collect::<Vec<_>>();
            names.sort_by(|left, right| left.name.cmp(&right.name));
            (!names.is_empty()).then_some((chart.chart_id, names))
        })
        .collect()
}

pub(super) fn first(choices: &[(String, Vec<Receiver>)]) -> Option<(&str, &str)> {
    choices.iter().find_map(|(chart, events)| {
        events
            .first()
            .map(|event| (chart.as_str(), event.name.as_str()))
    })
}

fn receiver<'a>(
    choices: &'a [(String, Vec<Receiver>)],
    chart: &str,
    event: &str,
) -> Option<&'a Receiver> {
    choices
        .iter()
        .find(|(id, _)| id == chart)?
        .1
        .iter()
        .find(|item| item.name == event)
}

pub(super) fn command_for(
    choices: &[(String, Vec<Receiver>)],
    chart: &str,
    event: &str,
) -> Option<ExcuteStmt> {
    let selected = receiver(choices, chart, event)?;
    if chart == MAIN_CHART {
        Some(ExcuteStmt::GimmickEvent { name: event.into() })
    } else if selected.fields.is_empty() {
        Some(ExcuteStmt::GimmickChartEvent {
            chart_id: chart.into(),
            name: event.into(),
        })
    } else {
        Some(ExcuteStmt::GimmickChartEventValues {
            chart_id: chart.into(),
            name: event.into(),
            arguments: selected
                .fields
                .iter()
                .map(|field| Argument {
                    field: field.name.clone(),
                    value: match field.kind {
                        VarType::Int => ConstValue::Int(0),
                        VarType::Float => ConstValue::Float(0.0),
                        VarType::Flag => ConstValue::Flag(false),
                        VarType::Timer => unreachable!("compiled event schema excludes timers"),
                    },
                })
                .collect(),
        })
    }
}

pub(super) fn selector(
    ui: &mut egui::Ui,
    command: &mut ExcuteStmt,
    choices: &[(String, Vec<Receiver>)],
) {
    let (mut chart_id, mut name) = match command {
        ExcuteStmt::GimmickChartEvent { chart_id, name }
        | ExcuteStmt::GimmickChartEventValues { chart_id, name, .. } => {
            (chart_id.clone(), name.clone())
        }
        ExcuteStmt::GimmickEvent { name } => (MAIN_CHART.to_owned(), name.clone()),
        _ => return,
    };
    let before = (chart_id.clone(), name.clone());
    ui.horizontal_wrapped(|ui| {
        ui.label("Controller");
        let previous = chart_id.clone();
        egui::ComboBox::from_id_salt("timed_chart_target")
            .selected_text(if chart_id.is_empty() {
                "Choose controller…"
            } else {
                chart_id.as_str()
            })
            .show_ui(ui, |ui| {
                for (chart, _) in choices {
                    ui.selectable_value(&mut chart_id, chart.clone(), chart);
                }
            });
        if previous != chart_id {
            name = choices
                .iter()
                .find(|(chart, _)| chart == &chart_id)
                .and_then(|(_, events)| events.first())
                .map(|event| event.name.clone())
                .unwrap_or_default();
        }
    });
    let events = choices
        .iter()
        .find(|(chart, _)| chart == &chart_id)
        .map(|(_, events)| events.as_slice())
        .unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label("Event");
        egui::ComboBox::from_id_salt("timed_chart_event")
            .selected_text(if name.is_empty() {
                "Choose receiver…"
            } else {
                name.as_str()
            })
            .show_ui(ui, |ui| {
                for event in events {
                    ui.selectable_value(&mut name, event.name.clone(), &event.name);
                }
            });
    });
    if before != (chart_id.clone(), name.clone()) {
        if let Some(next) = command_for(choices, &chart_id, &name) {
            *command = next;
        }
    }
    if let Some(selected) = receiver(choices, &chart_id, &name) {
        match command {
            ExcuteStmt::GimmickChartEventValues { arguments, .. } => {
                if let Err(errors) = crate::gimmicks::event_payload::canonicalize_arguments(
                    "Selected controller",
                    &name,
                    &selected.fields,
                    arguments,
                ) {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        "Event inputs changed. Reset inputs to match the receiver.",
                    )
                    .on_hover_text(
                        errors
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                    if ui.button("Reset event inputs").clicked() {
                        if let Some(next) = command_for(choices, &chart_id, &name) {
                            *command = next;
                        }
                    }
                } else {
                    for field in &selected.fields {
                        let argument = arguments
                            .iter_mut()
                            .find(|argument| argument.field == field.name)
                            .expect("validated named field");
                        ui.horizontal_wrapped(|ui| {
                            ui.label(&field.name);
                            match &mut argument.value {
                                ConstValue::Int(value) => {
                                    ui.add(
                                        egui::DragValue::new(value).range(-1_000_000..=1_000_000),
                                    );
                                }
                                ConstValue::Float(value) => {
                                    ui.add(
                                        egui::DragValue::new(value)
                                            .speed(0.05)
                                            .range(-1_000_000.0..=1_000_000.0),
                                    );
                                }
                                ConstValue::Flag(value) => {
                                    ui.checkbox(value, "Enabled");
                                }
                            }
                        });
                    }
                }
            }
            ExcuteStmt::GimmickChartEvent { .. } if !selected.fields.is_empty() => {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    "This receiver now requires inputs.",
                );
                if ui.button("Add event inputs").clicked() {
                    if let Some(next) = command_for(choices, &chart_id, &name) {
                        *command = next;
                    }
                }
            }
            _ => {}
        }
    } else {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            "Receiver missing. Choose a controller and one of its move events.",
        );
    }
    ui.weak("Only this controller receives it. Needs Export Mod Folder to take effect in game.");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_chart_receiver_inspector_preserves_saved_selection_when_idle() {
        let context = egui::Context::default();
        let mut command = ExcuteStmt::GimmickChartEvent {
            chart_id: "missing".into(),
            name: "original".into(),
        };
        let before = serde_json::to_value(&command).unwrap();
        for width in [280.0, 330.0] {
            let _ = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 320.0),
                    )),
                    ..Default::default()
                },
                |ui| selector(ui, &mut command, &[("other".into(), vec!["ready".into()])]),
            );
            assert_eq!(serde_json::to_value(&command).unwrap(), before);
        }
    }
    #[test]
    fn first_receiver_keeps_exact_chart_and_event_identity() {
        assert_eq!(first(&[]), None);
        let choices = vec![
            ("empty".into(), vec![]),
            ("charge".into(), vec!["ready".into()]),
            ("cooldown".into(), vec!["ready".into()]),
        ];
        assert_eq!(first(&choices), Some(("charge", "ready")));
    }

    #[test]
    fn chart_choices_are_exact_owner_typed_receivers_and_do_not_mutate_graphs() {
        let mut graph = crate::gimmicks::templates::Preset::Timer.graph();
        let index = graph.transitions.len();
        graph.transitions.push(graph.transitions[0].clone());
        graph
            .emission_plan
            .event_bindings
            .push(crate::gimmicks::codegen::EventBinding {
                name: "pulse".into(),
                transitions: vec![index],
                fields: vec![],
            });
        let chart = ControllerChart {
            graph,
            ..Default::default()
        };
        let mut charts = BTreeMap::from([
            ("left".into(), chart.clone()),
            ("right".into(), chart.clone()),
        ]);
        let mut typed = chart;
        typed.graph.emission_plan.event_bindings[0].fields.push(
            crate::gimmicks::event_payload::FieldDef {
                name: "amount".into(),
                kind: crate::gimmicks::behavior::VarType::Int,
            },
        );
        charts.insert("typed".into(), typed);
        let before = serde_json::to_value(&charts).unwrap();
        let found = choices("mario", &BehaviorGraph::default(), &charts);
        assert_eq!(
            found
                .iter()
                .map(|(chart, events)| (
                    chart.as_str(),
                    events
                        .iter()
                        .map(|event| event.name.as_str())
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("left", vec!["pulse"]),
                ("right", vec!["pulse"]),
                ("typed", vec!["pulse"]),
            ]
        );
        assert!(matches!(
            command_for(&found, "left", "pulse"),
            Some(ExcuteStmt::GimmickChartEvent { .. })
        ));
        assert!(
            matches!(command_for(&found, "typed", "pulse"), Some(ExcuteStmt::GimmickChartEventValues { arguments, .. }) if arguments == vec![Argument { field: "amount".into(), value: ConstValue::Int(0) }])
        );
        assert!(choices("unknown_owner", &BehaviorGraph::default(), &charts).is_empty());
        assert_eq!(serde_json::to_value(&charts).unwrap(), before);
        charts.insert("invalid".into(), ControllerChart::default());
        assert!(choices("mario", &BehaviorGraph::default(), &charts).is_empty());
    }

    #[test]
    fn typed_event_inspector_shows_schema_fields_without_mutating_idle_or_stale_values() {
        let receivers = vec![(
            "right".into(),
            vec![Receiver {
                name: "pulse".into(),
                fields: vec![
                    FieldDef {
                        name: "amount".into(),
                        kind: VarType::Int,
                    },
                    FieldDef {
                        name: "gain".into(),
                        kind: VarType::Float,
                    },
                    FieldDef {
                        name: "active".into(),
                        kind: VarType::Flag,
                    },
                ],
            }],
        )];
        let valid = command_for(&receivers, "right", "pulse").unwrap();
        let mut stale = valid.clone();
        let ExcuteStmt::GimmickChartEventValues { arguments, .. } = &mut stale else {
            unreachable!()
        };
        arguments[0].field = "original-field".into();
        for mut command in [
            valid,
            stale,
            ExcuteStmt::GimmickChartEvent {
                chart_id: "right".into(),
                name: "pulse".into(),
            },
        ] {
            let before = serde_json::to_value(&command).unwrap();
            for width in [280.0, 330.0] {
                let context = egui::Context::default();
                let output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 360.0),
                        )),
                        ..Default::default()
                    },
                    |ui| selector(ui, &mut command, &receivers),
                );
                assert!(!output.shapes.is_empty());
                assert_eq!(serde_json::to_value(&command).unwrap(), before);
            }
        }
    }
}
