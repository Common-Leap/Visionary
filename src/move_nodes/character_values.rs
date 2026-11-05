//! Typed controller reads shared by branch export, import, and validation.

use super::model::{CharacterValue, MoveCondition, NativeComparison};
use crate::gimmicks::behavior::{BehaviorGraph, VarType};
use crate::mod_project::ControllerChart;
use std::collections::BTreeMap;

pub(super) type Choices = Vec<(String, Vec<crate::gimmicks::behavior::VarDef>)>;

pub(super) fn choices(main: &BehaviorGraph, charts: &BTreeMap<String, ControllerChart>) -> Choices {
    std::iter::once(("main".to_owned(), main.vars.clone()))
        .chain(
            charts
                .iter()
                .map(|(id, chart)| (id.clone(), chart.graph.vars.clone())),
        )
        .filter(|(_, vars)| !vars.is_empty())
        .collect()
}

pub(super) fn compatible(kind: VarType, value: CharacterValue) -> bool {
    matches!(
        (kind, value),
        (VarType::Int | VarType::Timer, CharacterValue::Int(_))
            | (VarType::Float, CharacterValue::Float(_))
            | (VarType::Flag, CharacterValue::Flag(_))
    )
}

pub(super) fn validate(
    condition: &MoveCondition,
    main: &BehaviorGraph,
    charts: &BTreeMap<String, ControllerChart>,
) -> Result<(), String> {
    match condition {
        MoveCondition::CharacterValue {
            chart_id,
            var,
            op,
            value,
        } => {
            expression(chart_id, var, *op, *value)?;
            let graph = if chart_id == "main" {
                main
            } else {
                &charts
                    .get(chart_id)
                    .ok_or_else(|| {
                        format!("character value controller {chart_id:?} does not exist")
                    })?
                    .graph
            };
            let definition = graph
                .vars
                .iter()
                .find(|item| item.name == *var)
                .ok_or_else(|| {
                    format!("character value variable {chart_id:?} · {var:?} does not exist")
                })?;
            if !compatible(definition.kind, *value) {
                return Err(format!(
                    "character value {chart_id:?} · {var:?} type mismatch: expected {:?}",
                    definition.kind
                ));
            }
        }
        MoveCondition::Not(inner) | MoveCondition::Group(inner) => validate(inner, main, charts)?,
        MoveCondition::And { left, right } | MoveCondition::Or { left, right } => {
            validate(left, main, charts)?;
            validate(right, main, charts)?;
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn expression(
    chart: &str,
    var: &str,
    op: NativeComparison,
    value: CharacterValue,
) -> Result<String, String> {
    for (text, limit) in [
        (
            chart,
            crate::gimmicks::controller_protocol::MAX_CHART_ID_BYTES,
        ),
        (var, crate::gimmicks::live_model::MAX_NAME_BYTES),
    ] {
        if text.trim().is_empty() || text.len() > limit || text.contains('\0') {
            return Err(
                "character value requires bounded, nonblank controller and variable names".into(),
            );
        }
    }
    let (kind, right) = match value {
        CharacterValue::Int(value) => ("int", value.to_string()),
        CharacterValue::Float(value) if value.is_finite() => ("float", crate::acmd::num(value)),
        CharacterValue::Float(_) => {
            return Err("character value comparison requires a finite float".into())
        }
        CharacterValue::Flag(value) => {
            if !matches!(op, NativeComparison::Eq | NativeComparison::Ne) {
                return Err("character flag comparison requires == or !=".into());
            }
            ("flag", value.to_string())
        }
    };
    Ok(format!("crate::slight::gimmicks::read_controller_{kind}_from_module(agent.module_accessor, {chart:?}, {var:?}) {} {right}", op.token()))
}

pub(super) fn import(header: &str) -> Option<MoveCondition> {
    let source = header.strip_prefix("if ")?.strip_suffix(" {")?;
    let binary = syn::parse_str::<syn::ExprBinary>(source).ok()?;
    let op = match binary.op {
        syn::BinOp::Eq(_) => NativeComparison::Eq,
        syn::BinOp::Ne(_) => NativeComparison::Ne,
        syn::BinOp::Lt(_) => NativeComparison::Lt,
        syn::BinOp::Le(_) => NativeComparison::Lte,
        syn::BinOp::Gt(_) => NativeComparison::Gt,
        syn::BinOp::Ge(_) => NativeComparison::Gte,
        _ => return None,
    };
    let syn::Expr::Call(call) = *binary.left else {
        return None;
    };
    let syn::Expr::Path(path) = *call.func else {
        return None;
    };
    if path.qself.is_some()
        || path.path.leading_colon.is_some()
        || path.path.segments.len() != 4
        || path
            .path
            .segments
            .iter()
            .any(|part| !part.arguments.is_empty())
    {
        return None;
    }
    let segments: Vec<_> = path
        .path
        .segments
        .iter()
        .map(|part| part.ident.to_string())
        .collect();
    if segments[..3] != ["crate", "slight", "gimmicks"] {
        return None;
    }
    let args: Vec<_> = call.args.iter().collect();
    let [syn::Expr::Field(module), syn::Expr::Lit(chart), syn::Expr::Lit(var)] = args.as_slice()
    else {
        return None;
    };
    let syn::Expr::Path(agent) = module.base.as_ref() else {
        return None;
    };
    if !agent.path.is_ident("agent")
        || module.member
            != syn::Member::Named(syn::Ident::new(
                "module_accessor",
                proc_macro2::Span::call_site(),
            ))
    {
        return None;
    }
    let (syn::Lit::Str(chart), syn::Lit::Str(var)) = (&chart.lit, &var.lit) else {
        return None;
    };
    let (negative, literal) = match binary.right.as_ref() {
        syn::Expr::Lit(literal) => (false, &literal.lit),
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Neg(_)) => {
            let syn::Expr::Lit(literal) = unary.expr.as_ref() else {
                return None;
            };
            (true, &literal.lit)
        }
        _ => return None,
    };
    let value = match (segments[3].as_str(), literal) {
        ("read_controller_int_from_module", syn::Lit::Int(value)) if value.suffix().is_empty() => {
            let magnitude = value.base10_parse::<i64>().ok()?;
            CharacterValue::Int(i32::try_from(if negative { -magnitude } else { magnitude }).ok()?)
        }
        ("read_controller_float_from_module", syn::Lit::Float(value))
            if value.suffix().is_empty() =>
        {
            let value = value.base10_parse::<f32>().ok()?;
            CharacterValue::Float(if negative { -value } else { value })
        }
        ("read_controller_flag_from_module", syn::Lit::Bool(value)) if !negative => {
            CharacterValue::Flag(value.value)
        }
        _ => return None,
    };
    let (chart_id, var) = (chart.value(), var.value());
    expression(&chart_id, &var, op, value).ok()?;
    Some(MoveCondition::CharacterValue {
        chart_id,
        var,
        op,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::templates::Preset;
    use crate::move_nodes::model::{MoveGraph, MoveNode, MoveTrack, NodeKind, Track};

    fn condition(chart: &str, var: &str, value: CharacterValue) -> MoveCondition {
        MoveCondition::CharacterValue {
            chart_id: chart.into(),
            var: var.into(),
            op: NativeComparison::Eq,
            value,
        }
    }

    fn graph(track: Track, condition: MoveCondition) -> MoveGraph {
        MoveGraph {
            tracks: vec![MoveTrack {
                track,
                nodes: vec![MoveNode {
                    id: 1,
                    kind: NodeKind::Branch {
                        condition,
                        then_nodes: vec![MoveNode {
                            id: 2,
                            kind: NodeKind::Wait { frames: 1.0 },
                        }],
                        else_nodes: vec![MoveNode {
                            id: 3,
                            kind: NodeKind::Wait { frames: 2.0 },
                        }],
                    },
                }],
                baseline: Vec::new(),
                effect_execute_baseline: Vec::new(),
            }],
            ..MoveGraph::default()
        }
    }

    #[test]
    fn character_value_serde_is_typed_and_strict() {
        for value in [
            CharacterValue::Int(100),
            CharacterValue::Float(0.5),
            CharacterValue::Flag(true),
        ] {
            let expected = condition("main", "value", value);
            let encoded = serde_json::to_value(&expected).unwrap();
            let decoded: MoveCondition = serde_json::from_value(encoded.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
            let mut unknown = encoded;
            unknown["character_value"]["unexpected"] = true.into();
            assert!(serde_json::from_value::<MoveCondition>(unknown).is_err());
        }
        assert!(serde_json::from_str::<CharacterValue>(r#"{"int":1,"extra":true}"#).is_err());
        assert!(serde_json::from_str::<CharacterValue>(r#"{"int":0.5}"#).is_err());
    }

    #[test]
    fn character_value_compile_validates_same_fighter_chart_and_type() {
        let main = Preset::Stance.graph();
        let charts = BTreeMap::from([
            (
                "meter_1".into(),
                ControllerChart {
                    graph: Preset::Meter.graph(),
                    ..Default::default()
                },
            ),
            (
                "ammo_1".into(),
                ControllerChart {
                    graph: Preset::Ammo.graph(),
                    ..Default::default()
                },
            ),
            (
                "timer_1".into(),
                ControllerChart {
                    graph: Preset::Timer.graph(),
                    ..Default::default()
                },
            ),
        ]);
        for (chart, var, value) in [
            ("main", "stance", CharacterValue::Int(1)),
            ("meter_1", "meter", CharacterValue::Float(100.0)),
            ("ammo_1", "ammo", CharacterValue::Int(0)),
            ("timer_1", "ready", CharacterValue::Flag(true)),
            ("timer_1", "remaining", CharacterValue::Int(0)),
        ] {
            let graph = graph(Track::Game, condition(chart, var, value));
            assert!(super::super::project::referenced_controller_charts(&graph)
                .unwrap()
                .contains(chart));
            super::super::project::compile_recipes_with_controllers(
                "mario",
                &main,
                &charts,
                &BTreeMap::from([("attack11".into(), graph)]),
            )
            .unwrap();
        }
        for (chart, var, value, reason) in [
            (
                "missing",
                "meter",
                CharacterValue::Float(100.0),
                "controller",
            ),
            (
                "main",
                "meter",
                CharacterValue::Float(100.0),
                "does not exist",
            ),
            (
                "meter_1",
                "missing",
                CharacterValue::Float(100.0),
                "does not exist",
            ),
            (
                "meter_1",
                "mètre",
                CharacterValue::Float(100.0),
                "does not exist",
            ),
            (
                "meter_1",
                "meter",
                CharacterValue::Int(100),
                "type mismatch",
            ),
            ("timer_1", "ready", CharacterValue::Int(1), "type mismatch"),
        ] {
            let graph = graph(
                Track::Game,
                MoveCondition::Not(Box::new(condition(chart, var, value))),
            );
            let errors = super::super::project::compile_recipes_with_controllers(
                "mario",
                &main,
                &charts,
                &BTreeMap::from([("attack11".into(), graph)]),
            )
            .err()
            .unwrap();
            assert!(
                errors.iter().any(|error| error.reason.contains(reason)),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn character_value_export_import_round_trip_all_tracks_and_operators() {
        for value in [
            CharacterValue::Int(i32::MIN),
            CharacterValue::Float(-0.125),
            CharacterValue::Flag(false),
        ] {
            for op in NativeComparison::ALL {
                if matches!(value, CharacterValue::Flag(_))
                    && !matches!(op, NativeComparison::Eq | NativeComparison::Ne)
                {
                    continue;
                }
                let mut expected = condition("meter_1", "mètre\"quoted", value);
                let MoveCondition::CharacterValue { op: slot, .. } = &mut expected else {
                    unreachable!()
                };
                *slot = op;
                let expected = MoveCondition::Not(Box::new(MoveCondition::Group(Box::new(
                    MoveCondition::And {
                        left: Box::new(expected),
                        right: Box::new(MoveCondition::Grounded),
                    },
                ))));
                for track in [Track::Game, Track::Effect, Track::Sound, Track::Expression] {
                    let compiled = super::super::timeline::compile(
                        "mario",
                        "attack11",
                        &graph(track, expected.clone()),
                    )
                    .unwrap();
                    let script = &compiled[0].1;
                    let source = match track {
                        Track::Game => crate::acmd::preview_game_fn(script, "attack11"),
                        Track::Effect => {
                            crate::acmd::preview_effect_structure_fn(script, "attack11")
                        }
                        Track::Sound => crate::acmd::preview_sound_fn(script, "attack11"),
                        Track::Expression => crate::acmd::preview_expression_fn(script, "attack11"),
                    };
                    let imported = match track {
                        Track::Effect => super::super::effects::import_for_fighter(
                            "mario",
                            &crate::acmd::parse_effect_script(&source),
                        ),
                        _ => {
                            let parsed = match track {
                                Track::Game => crate::acmd::parse_acmd_script(&source),
                                Track::Sound => crate::acmd::parse_sound_script(&source),
                                Track::Expression => crate::acmd::parse_expression_script(&source),
                                _ => unreachable!(),
                            };
                            super::super::timeline::import_for_fighter("mario", track, &parsed)
                        }
                    };
                    let NodeKind::Branch { condition, .. } = &imported.nodes[0].kind else {
                        panic!("{source}");
                    };
                    assert_eq!(
                        serde_json::to_value(condition).unwrap(),
                        serde_json::to_value(&expected).unwrap(),
                        "{source}"
                    );
                }
            }
        }
        for malformed in [
            "if crate::slight::gimmicks::read_controller_int_from_module(boma, \"main\", \"stance\") == 1 {",
            "if crate::slight::gimmicks::read_controller_flag_from_module(agent.module_accessor, \"main\", \"ready\") > true {",
            "if crate::slight::gimmicks::read_controller_float_from_module(agent.module_accessor, \"main\", \"meter\") == f32::NAN {",
        ] {
            assert!(super::super::timeline::import_condition(Some("mario"), malformed).is_none());
        }
    }
}
