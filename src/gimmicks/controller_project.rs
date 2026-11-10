//! Project-side validation and lowering of independent controller charts.
//!
//! This produces the reviewed bundle DTO only. It does not install controllers,
//! advertise a plugin capability, or broaden any native event permissions.

use std::collections::BTreeMap;

use crate::mod_project::ControllerChart;

use super::{behavior, controller_protocol, controller_runtime, field_projection, registry};

pub const MAIN_CHART: &str = "main";

/// Bound all additional authored charts before a project or compiler clones them.
/// Semantic errors remain repairable authored data; compilation checks them below.
pub fn preflight(
    main: &behavior::BehaviorGraph,
    charts: &BTreeMap<String, ControllerChart>,
) -> Result<(), String> {
    let count = charts.len().saturating_add(usize::from(!main.is_empty()));
    if count > controller_runtime::MAX_CHARTS {
        return Err(format!(
            "controller count {count} exceeds {} charts per fighter, including main",
            controller_runtime::MAX_CHARTS
        ));
    }
    for (id, chart) in charts {
        if id == MAIN_CHART {
            return Err("additional controller ID main is reserved for the legacy behavior".into());
        }
        if id.trim().is_empty()
            || id.len() > controller_protocol::MAX_CHART_ID_BYTES
            || id.as_bytes().contains(&0)
        {
            return Err(format!(
                "controller ID must be nonblank, at most {} UTF-8 bytes, and contain no NUL",
                controller_protocol::MAX_CHART_ID_BYTES
            ));
        }
        behavior::preflight_bounds(&chart.graph)
            .map_err(|reason| format!("controller {id:?}: {reason}"))?;
        if chart.layout.positions.len() > 8_192 {
            return Err(format!("controller {id:?}: layout exceeds 8192 positions"));
        }
        for (key, position) in &chart.layout.positions {
            // Layout deserialization enforces these same bounds. Recheck in-memory
            // drafts without serializing or cloning their recursive graph data.
            if key.len() > 4_096 {
                return Err(format!("controller {id:?}: layout key exceeds 4096 bytes"));
            }
            field_projection::FieldRef::from_key(key)
                .map_err(|reason| format!("controller {id:?}: layout: {reason}"))?;
            if position
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 100_000.0)
            {
                return Err(format!(
                    "controller {id:?}: layout coordinate is out of bounds"
                ));
            }
        }
    }
    Ok(())
}

/// Lower a character's effective main graph and its independent additional charts.
/// Move recipes must already have been merged into main by the caller. No chart
/// can satisfy another chart's variable references or named-event receivers.
pub fn compile(
    fighter: &str,
    main: &behavior::BehaviorGraph,
    charts: &BTreeMap<String, ControllerChart>,
) -> Result<Vec<controller_protocol::ControllerProgram>, String> {
    preflight(main, charts)?;
    behavior::preflight_bounds(main)?;
    let kind = registry::FIGHTER_KINDS
        .iter()
        .find(|entry| entry.fighter == fighter)
        .ok_or_else(|| format!("fighter {fighter:?} has no exact pinned fighter-kind mapping"))?;
    let mut records = Vec::new();
    for (id, graph) in std::iter::once((MAIN_CHART, main))
        .filter(|(_, graph)| !graph.is_empty())
        .chain(charts.iter().map(|(id, chart)| (id.as_str(), &chart.graph)))
    {
        if graph.is_empty() {
            return Err(format!(
                "fighter {fighter} controller {id:?}: graph is empty"
            ));
        }
        let lowered =
            crate::gimmicks::controller_lower::compile(fighter, graph).map_err(|errors| {
                format!(
                    "fighter {fighter} controller {id:?}: {}",
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            })?;
        super::graph_edit::validate_graph(&lowered, fighter)
            .map_err(|reason| format!("fighter {fighter} controller {id:?}: {reason}"))?;
        // Preserve legacy IDs exactly. The pinned kind gives additional IDs a
        // short, unambiguous namespace even for 48-byte UTF-8 chart identities.
        let program_id = if id == MAIN_CHART {
            format!("live:{fighter}")
        } else {
            format!("chart:{}:{id}", kind.value)
        };
        let mut program =
            super::live_protocol::from_graph(program_id, fighter, &lowered, &lowered.emission_plan)
                .map_err(|errors| {
                    format!("fighter {fighter} controller {id:?}: {}", errors.join("; "))
                })?;
        // The costume choice is the fighter's, so every chart of it follows the main one.
        program.costumes = main.costumes.clone();
        records.push(controller_protocol::ControllerProgram {
            chart_id: id.into(),
            program,
        });
    }
    let request = controller_protocol::ControllerBundleRequest {
        version: controller_protocol::REQUEST_VERSION,
        revision: 0,
        disabled_fighters: Vec::new(),
        charts: records,
    };
    controller_protocol::validate_request(&request)?;
    // Size validation is distinct from table limits and does not install a runtime.
    controller_protocol::encode_request(&request)?;
    Ok(request.charts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::{field_canvas::FieldLayout, templates::Preset};

    fn chart() -> ControllerChart {
        ControllerChart {
            graph: Preset::Timer.graph(),
            layout: FieldLayout::default(),
        }
    }

    #[test]
    fn compile_keeps_main_identity_and_independent_same_name_timers() {
        for fighter in ["mario", "cloud", "pikachu"] {
            let main = Preset::Timer.graph();
            let charts = BTreeMap::from([("second".into(), chart())]);
            let original = serde_json::to_string(&charts).unwrap();
            let records = compile(fighter, &main, &charts).unwrap();
            assert_eq!(records[0].chart_id, "main");
            assert_eq!(records[0].program.id, format!("live:{fighter}"));
            assert_ne!(records[0].program.id, records[1].program.id);
            assert_eq!(records[0].program.vars, records[1].program.vars);
            assert_eq!(serde_json::to_string(&charts).unwrap(), original);
            let request = controller_protocol::ControllerBundleRequest {
                version: controller_protocol::REQUEST_VERSION,
                revision: 1,
                disabled_fighters: Vec::new(),
                charts: records,
            };
            let coordinators = request.coordinators().unwrap();
            assert_eq!(
                coordinators[fighter].chart_ids().collect::<Vec<_>>(),
                ["main", "second"]
            );
        }
    }

    #[test]
    fn legacy_only_and_chart_only_are_valid() {
        assert_eq!(
            compile("mario", &Preset::Timer.graph(), &BTreeMap::new())
                .unwrap()
                .len(),
            1
        );
        let charts = BTreeMap::from([("only".into(), chart())]);
        let records = compile("mario", &Default::default(), &charts).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].chart_id, "only");
    }

    #[test]
    fn chart_ids_are_exact_reserved_and_byte_bounded() {
        for id in ["main".into(), " ".into(), "bad\0id".into(), "é".repeat(25)] {
            let charts = BTreeMap::from([(id, chart())]);
            assert!(preflight(&Default::default(), &charts).is_err());
        }
        let records = compile(
            "miiswordsman",
            &Default::default(),
            &BTreeMap::from([("é".repeat(24), chart())]),
        )
        .unwrap();
        assert!(records[0].program.id.len() <= super::super::live_model::MAX_PROGRAM_ID_BYTES);
    }

    #[test]
    fn main_counts_toward_eight_chart_limit() {
        let charts = (0..8).map(|i| (format!("c{i}"), chart())).collect();
        assert!(preflight(&Default::default(), &charts).is_ok());
        assert!(preflight(&Preset::Timer.graph(), &charts)
            .unwrap_err()
            .contains("including main"));
    }

    #[test]
    fn unknown_owner_and_empty_chart_refuse_without_partial_result() {
        let charts = BTreeMap::from([("broken".into(), ControllerChart::default())]);
        assert!(compile("Cloud", &Default::default(), &charts)
            .unwrap_err()
            .contains("pinned"));
        assert!(compile("cloud", &Preset::Timer.graph(), &charts)
            .unwrap_err()
            .contains("graph is empty"));
    }

    #[test]
    fn additional_chart_cannot_borrow_main_variables() {
        let main = Preset::Timer.graph();
        let mut invalid = chart();
        invalid.graph.vars.clear();
        let charts = BTreeMap::from([("broken".into(), invalid)]);
        let error = compile("mario", &main, &charts).unwrap_err();
        assert!(error.contains("broken"), "{error}");
    }

    #[test]
    fn in_memory_layout_bounds_match_deserialization_checks() {
        for (key, position) in [
            ("not-a-field", [0.0, 0.0]),
            ("state/idle", [f32::NAN, 0.0]),
            ("state/idle", [100_001.0, 0.0]),
        ] {
            let mut value = chart();
            value.layout.positions.insert(key.into(), position);
            assert!(preflight(
                &Default::default(),
                &BTreeMap::from([("chart".into(), value)])
            )
            .is_err());
        }
    }

    #[test]
    fn compiled_bundle_enforces_aggregate_variable_budget() {
        let mut graph = Preset::Timer.graph();
        graph.vars.extend(
            (0..48 - graph.vars.len())
                .map(|i| behavior::VarDef {
                    keep_across_form_switch: false,
                    name: format!("value{i}"),
                    kind: behavior::VarType::Int,
                    initial: behavior::ConstValue::Int(0),
                })
                .collect::<Vec<_>>(),
        );
        assert!(compile("mario", &graph, &BTreeMap::new()).is_ok());
        let charts = (0..3)
            .map(|i| {
                (
                    format!("chart{i}"),
                    ControllerChart {
                        graph: graph.clone(),
                        layout: Default::default(),
                    },
                )
            })
            .collect();
        let error = compile("mario", &Default::default(), &charts).unwrap_err();
        assert!(
            error.contains("variables") && error.contains("128"),
            "{error}"
        );
    }
}
