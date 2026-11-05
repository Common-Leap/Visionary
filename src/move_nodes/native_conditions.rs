//! Closed, source-only numeric native branches.
//!
//! Reads happen at their original ACMD structural location, not through meter
//! telemetry or character-controller state. Registry membership proves a
//! getter's exact identifier/type, not resource meaning or valid status use.
//! Only two independently observed common integer reads have a shared fallback;
//! they do not authorize native writes or enter the live gimmick registry.

use crate::gimmicks::registry::{self, IdentifierKind};

use super::model::{MoveCondition, NativeComparison, NativeIntOperand};

const COMMON_INT_READS: &[&str] = &[
    "FIGHTER_INSTANCE_WORK_ID_INT_KIND",
    "FIGHTER_INSTANCE_WORK_ID_INT_COLOR",
];

const BAYONETTA_COSTUME_KIND: &str = "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeIntChoice {
    pub symbol: &'static str,
    pub label: &'static str,
}

const BAYONETTA_COSTUME_CHOICES: &[NativeIntChoice] = &[
    NativeIntChoice {
        symbol: "FIGHTER_BAYONETTA_COSTUME_KIND_BAYONETTA_1",
        label: "Bayonetta 1",
    },
    NativeIntChoice {
        symbol: "FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT",
        label: "Default",
    },
    NativeIntChoice {
        symbol: "FIGHTER_BAYONETTA_COSTUME_KIND_JEANNE",
        label: "Jeanne",
    },
];

const PIKMIN_HAVE_PIKMIN: &str = "FIGHTER_PIKMIN_STATUS_ATTACK_AIR_WORK_INT_HAVE_PIKMIN";
const PIKMIN_STATUS_INT_READS: &[&str] = &[PIKMIN_HAVE_PIKMIN];

const IKE_READY_LEVEL: &str = "FIGHTER_IKE_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL";
const ROY_READY_LEVEL: &str = "FIGHTER_ROY_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL";
const IKE_STATUS_INT_READS: &[&str] = &[IKE_READY_LEVEL];
const ROY_STATUS_INT_READS: &[&str] = &[ROY_READY_LEVEL];

pub fn common_int_reads() -> &'static [&'static str] {
    COMMON_INT_READS
}

/// Exact native enum constants supported for a particular fighter and
/// WorkInt source. An empty slice means no enum choices are authorized.
pub fn integer_choices(fighter: &str, source: &str) -> &'static [NativeIntChoice] {
    if fighter == "bayonetta" && source == BAYONETTA_COSTUME_KIND {
        BAYONETTA_COSTUME_CHOICES
    } else {
        &[]
    }
}

/// Exact fighter-owned integer reads supported outside the shared registry, including Chrom's
/// native use of Roy's charge-level slot.
pub fn status_int_reads(fighter: &str) -> &'static [&'static str] {
    match fighter {
        "ike" => IKE_STATUS_INT_READS,
        "pikmin" => PIKMIN_STATUS_INT_READS,
        "roy" | "chrom" => ROY_STATUS_INT_READS,
        _ => &[],
    }
}

pub fn validate_source(fighter: &str, symbol: &str, kind: IdentifierKind) -> Result<(), String> {
    if !matches!(kind, IdentifierKind::WorkInt | IdentifierKind::WorkFloat) {
        return Err("numeric branches require a native integer or float source".into());
    }
    if kind == IdentifierKind::WorkInt && COMMON_INT_READS.contains(&symbol) {
        if registry::fighter_kind_symbol(fighter).is_none() {
            return Err(format!(
                "common native read {symbol:?} requires an exact fighter kind for {fighter:?}"
            ));
        }
        return Ok(());
    }
    if kind == IdentifierKind::WorkInt && status_int_reads(fighter).contains(&symbol) {
        if registry::fighter_kind_symbol(fighter).is_none() {
            return Err(format!(
                "fighter status integer read {symbol:?} requires a known exact fighter for {fighter:?}"
            ));
        }
        return Ok(());
    }
    registry::resolve(fighter, symbol, kind)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Emit a validated branch header. No project-controlled receiver, method,
/// raw numeric Work ID or arbitrary right-hand expression reaches source.
pub fn header(fighter: &str, condition: &MoveCondition) -> Result<String, String> {
    let (method, symbol, op, right) = match condition {
        MoveCondition::WorkInt { symbol, op, value } => {
            validate_source(fighter, symbol, IdentifierKind::WorkInt)?;
            let right = match value {
                NativeIntOperand::Literal(value) => value.to_string(),
                NativeIntOperand::FighterKind(owner) => {
                    let symbol = registry::fighter_kind_symbol(owner).ok_or_else(|| {
                        format!("comparison fighter {owner:?} has no exact pinned kind")
                    })?;
                    format!("*{symbol}")
                }
                NativeIntOperand::Choice(choice) => {
                    if !integer_choices(fighter, symbol)
                        .iter()
                        .any(|allowed| allowed.symbol == choice)
                    {
                        return Err(format!(
                            "native integer choice {choice:?} is not allowed for {fighter:?} source {symbol:?}"
                        ));
                    }
                    format!("*{choice}")
                }
            };
            ("get_int", symbol, op, right)
        }
        MoveCondition::WorkFloat { symbol, op, value } => {
            validate_source(fighter, symbol, IdentifierKind::WorkFloat)?;
            if !value.is_finite() {
                return Err("native float comparison requires a finite value".into());
            }
            ("get_float", symbol, op, crate::acmd::num(*value))
        }
        _ => return Err("expected a numeric native branch".into()),
    };
    Ok(format!(
        "if WorkModule::{method}(agent.module_accessor, *{symbol}) {} {right} {{",
        op.token()
    ))
}

/// Import only the exact canonical read/comparison shape. Unreviewed literal
/// spellings, attributes, casts, compound tests and extra arguments remain
/// preserved source so no-edit round trips retain their original headers.
pub fn import(fighter: &str, source: &str) -> Option<MoveCondition> {
    let body = source.strip_prefix("if WorkModule::")?.strip_suffix(" {")?;
    let (method, rest) = body.split_once("(agent.module_accessor, *")?;
    let (symbol, comparison) = rest.split_once(") ")?;
    let (operator, right) = comparison.split_once(' ')?;
    let op = NativeComparison::ALL
        .into_iter()
        .find(|op| op.token() == operator)?;
    let condition = match method {
        "get_int" => {
            let value = if let Some(token) = right.strip_prefix('*') {
                if integer_choices(fighter, symbol)
                    .iter()
                    .any(|allowed| allowed.symbol == token)
                {
                    NativeIntOperand::Choice(token.into())
                } else {
                    let entry = registry::FIGHTER_KINDS
                        .iter()
                        .find(|entry| entry.symbol == token)?;
                    NativeIntOperand::FighterKind(entry.fighter.into())
                }
            } else {
                NativeIntOperand::Literal(right.parse().ok()?)
            };
            MoveCondition::WorkInt {
                symbol: symbol.into(),
                op,
                value,
            }
        }
        "get_float" => MoveCondition::WorkFloat {
            symbol: symbol.into(),
            op,
            value: right.parse().ok()?,
        },
        _ => return None,
    };
    (header(fighter, &condition).ok()?.as_str() == source).then_some(condition)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOUD_FLOAT: &str = "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE";
    const RIDLEY_INT: &str = "FIGHTER_RIDLEY_INSTANCE_WORK_ID_INT_DISABLE_SPECIAL_LW_FINISH_COUNT";

    #[test]
    fn exact_native_numeric_comparisons_keep_type_operator_and_symbolic_rhs() {
        for op in NativeComparison::ALL {
            for condition in [
                MoveCondition::WorkInt {
                    symbol: COMMON_INT_READS[1].into(),
                    op,
                    value: NativeIntOperand::Literal(-17),
                },
                MoveCondition::WorkInt {
                    symbol: COMMON_INT_READS[0].into(),
                    op,
                    value: NativeIntOperand::FighterKind("kirby".into()),
                },
                MoveCondition::WorkFloat {
                    symbol: CLOUD_FLOAT.into(),
                    op,
                    value: -12.5,
                },
            ] {
                let source = header("cloud", &condition).unwrap();
                let recovered = import("cloud", &source).unwrap();
                assert_eq!(
                    serde_json::to_value(&recovered).unwrap(),
                    serde_json::to_value(&condition).unwrap()
                );
                assert_eq!(header("cloud", &recovered).unwrap(), source);
            }
        }
        let independent = "if WorkModule::get_int(agent.module_accessor, *FIGHTER_RIDLEY_INSTANCE_WORK_ID_INT_DISABLE_SPECIAL_LW_FINISH_COUNT) <= 0 {";
        let recovered = import("ridley", independent).unwrap();
        assert!(
            matches!(recovered, MoveCondition::WorkInt { symbol, op: NativeComparison::Lte, value: NativeIntOperand::Literal(0) } if symbol == RIDLEY_INT)
        );
    }

    #[test]
    fn malformed_or_unreviewed_native_comparisons_are_not_normalized() {
        let valid =
            format!("if WorkModule::get_float(agent.module_accessor, *{CLOUD_FLOAT}) >= 100.0 {{");
        assert!(import("cloud", &valid).is_some());
        for bad in [
            valid.replace("100.0", "100"),
            valid.replace("100.0", "1e2"),
            valid.replace("100.0", "100.0f32"),
            valid.replace("100.0", "f32::NAN"),
            valid.replace("100.0", "NaN"),
            valid.replace("100.0", "inf"),
            valid.replace("100.0", "100.0 + injected()"),
            valid.replace("100.0", "100.0 && other()"),
            valid.replace("100.0", "(100.0)"),
            valid.replace("agent.module_accessor", "other.module_accessor"),
            valid.replace("WorkModule::", "OtherModule::"),
            valid.replace("get_float", "get_int64"),
            valid.replace(") >=", ", 1) >="),
            valid.replace(CLOUD_FLOAT, "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_TERM"),
            valid.replace(CLOUD_FLOAT, "0x4C"),
            format!("#[cfg(feature = \"x\")] {valid}"),
        ] {
            assert!(import("cloud", &bad).is_none(), "{bad}");
        }
        for right in [
            "0x1",
            "1i32",
            "01",
            "+1",
            "1 + 2",
            "*UNKNOWN_KIND",
            "2147483648",
        ] {
            assert!(import(
                "cloud",
                &format!(
                    "if WorkModule::get_int(agent.module_accessor, *{}) == {right} {{",
                    COMMON_INT_READS[1]
                )
            )
            .is_none());
        }
    }

    #[test]
    fn native_numeric_sources_fail_closed_without_expanding_write_authority() {
        assert!(validate_source("ridley", RIDLEY_INT, IdentifierKind::WorkInt).is_ok());
        assert!(validate_source("cloud", RIDLEY_INT, IdentifierKind::WorkInt).is_err());
        assert!(validate_source("cloud", CLOUD_FLOAT, IdentifierKind::WorkInt).is_err());
        assert!(validate_source("cloud", COMMON_INT_READS[0], IdentifierKind::WorkFloat).is_err());
        assert!(validate_source("unknown", COMMON_INT_READS[0], IdentifierKind::WorkInt).is_err());
        assert!(registry::resolve("cloud", COMMON_INT_READS[0], IdentifierKind::WorkInt).is_err());
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(header(
                "cloud",
                &MoveCondition::WorkFloat {
                    symbol: CLOUD_FLOAT.into(),
                    op: NativeComparison::Eq,
                    value
                }
            )
            .is_err());
        }
        assert!(header(
            "cloud",
            &MoveCondition::WorkInt {
                symbol: COMMON_INT_READS[0].into(),
                op: NativeComparison::Eq,
                value: NativeIntOperand::FighterKind("kirby; injected()".into())
            }
        )
        .is_err());
    }

    #[test]
    fn bayonetta_enum_choices_are_closed_to_the_exact_source_and_fighter() {
        assert_eq!(
            integer_choices("bayonetta", BAYONETTA_COSTUME_KIND),
            BAYONETTA_COSTUME_CHOICES
        );
        assert!(integer_choices("bayonetta", COMMON_INT_READS[0]).is_empty());
        assert!(integer_choices("bayonetta", COMMON_INT_READS[1]).is_empty());
        assert!(
            integer_choices("bayonetta", "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_OTHER").is_empty()
        );
        assert!(integer_choices("ike", BAYONETTA_COSTUME_KIND).is_empty());

        let choice = BAYONETTA_COSTUME_CHOICES[0].symbol;
        for allowed in BAYONETTA_COSTUME_CHOICES {
            let condition = MoveCondition::WorkInt {
                symbol: BAYONETTA_COSTUME_KIND.into(),
                op: NativeComparison::Ne,
                value: NativeIntOperand::Choice(allowed.symbol.into()),
            };
            let source = format!(
                "if WorkModule::get_int(agent.module_accessor, *{BAYONETTA_COSTUME_KIND}) != *{} {{",
                allowed.symbol
            );
            assert_eq!(header("bayonetta", &condition).unwrap(), source);
            let imported = import("bayonetta", &source).unwrap();
            assert_eq!(
                serde_json::to_value(&imported).unwrap(),
                serde_json::to_value(&condition).unwrap()
            );
            assert_eq!(header("bayonetta", &imported).unwrap(), source);
            assert_eq!(
                serde_json::from_value::<NativeIntOperand>(
                    serde_json::to_value(NativeIntOperand::Choice(allowed.symbol.into())).unwrap()
                )
                .unwrap(),
                NativeIntOperand::Choice(allowed.symbol.into())
            );
        }

        for (fighter, symbol, choice) in [
            ("bayonetta", COMMON_INT_READS[0], choice),
            ("bayonetta", COMMON_INT_READS[1], choice),
            (
                "bayonetta",
                "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_OTHER",
                choice,
            ),
            ("ike", BAYONETTA_COSTUME_KIND, choice),
            (
                "bayonetta",
                BAYONETTA_COSTUME_KIND,
                "FIGHTER_IKE_COSTUME_KIND_DEFAULT",
            ),
        ] {
            let condition = MoveCondition::WorkInt {
                symbol: symbol.into(),
                op: NativeComparison::Eq,
                value: NativeIntOperand::Choice(choice.into()),
            };
            assert!(header(fighter, &condition).is_err());
        }
        for bad in [
            "*FIGHTER_BAYONETTA_COSTUME_KIND_UNKNOWN",
            "FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT",
            "*FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT + injected()",
            "*FIGHTER_IKE_COSTUME_KIND_DEFAULT",
        ] {
            let source = format!(
                "if WorkModule::get_int(agent.module_accessor, *{BAYONETTA_COSTUME_KIND}) == {bad} {{"
            );
            assert!(import("bayonetta", &source).is_none(), "{source}");
        }
    }

    #[test]
    fn chrom_charge_condition_keeps_effect_values_and_repeat_siblings() {
        use crate::data::EffectMacro;
        use crate::move_nodes::{
            effects,
            model::{MoveGraph, NodeKind},
            timeline,
        };

        // Opening branch and repeat from chrom/SpecialAirNLoop.txt.
        let source = r#"unsafe extern "C" fn effect_specialairnloop(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("chrom_volcano_hold_1"), Hash40::new("sword1"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::EFFECT_FOLLOW(agent, Hash40::new("chrom_volcano_hold_2"), Hash40::new("sword1"), 0, 0, 0, 0, 0, 0, 1, true);
    }
    if WorkModule::get_int(agent.module_accessor, *FIGHTER_ROY_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL) == 0 {
        if macros::is_excute(agent) {
            macros::EFFECT_FOLLOW(agent, Hash40::new("chrom_sword_purple"), Hash40::new("sword1"), 0, 0, 0, 0, 0, 0, 1, true);
        }
    }
    for _ in 0..6 {
    if macros::is_excute(agent) {
        macros::FLASH(agent, 0.392, 0, 0.941, 0.1);
    }
    wait(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::COL_NORMAL(agent);
    }
    wait(agent.lua_state_agent, 2.0);
}
}"#;
        let script = crate::acmd::parse_effect_script(source);
        let mut track = effects::import_for_fighter("chrom", &script);
        let NodeKind::Branch {
            condition,
            then_nodes,
            else_nodes,
        } = &track.nodes[1].kind
        else {
            panic!("charge read must be typed")
        };
        assert!(
            matches!(condition, MoveCondition::WorkInt { symbol, op: NativeComparison::Eq, value: NativeIntOperand::Literal(0) } if symbol == ROY_READY_LEVEL)
        );
        assert!(else_nodes.is_empty());
        let [node] = then_nodes.as_slice() else {
            panic!("one guarded execute")
        };
        let NodeKind::EffectExecute { commands } = &node.kind else {
            panic!("guarded effect must be editable")
        };
        assert!(
            matches!(commands.as_slice(), [EffectMacro::Effect { effect_name, spawn_func, bone_name, offset, rotation, scale: 1.0, extra_args, .. }]
            if effect_name == "chrom_sword_purple" && spawn_func == "EFFECT_FOLLOW" && bone_name == "sword1"
                && *offset == [0.0; 3] && *rotation == [0.0; 3] && extra_args == &["true"])
        );
        let NodeKind::Repeat { count: 6, body, .. } = &track.nodes[2].kind else {
            panic!("repeat remains a sibling")
        };
        assert!(matches!(body[1].kind, NodeKind::Wait { frames: 4.0 }));
        assert!(matches!(body[3].kind, NodeKind::Wait { frames: 2.0 }));
        let NodeKind::EffectExecute { commands } = &body[0].kind else {
            panic!()
        };
        assert!(
            matches!(commands.as_slice(), [EffectMacro::Color { command, color }]
            if command == "FLASH" && color.rgba == Some([0.392, 0.0, 0.941, 0.1]))
        );
        let graph = MoveGraph {
            tracks: vec![track.clone()],
            ..Default::default()
        };
        let restored = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        let compiled = timeline::compile("chrom", "specialairnloop", &restored).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "specialairnloop");
        syn::parse_file(&emitted).unwrap();
        assert_eq!(
            serde_json::to_value(crate::acmd::parse_effect_script(&emitted)).unwrap(),
            serde_json::to_value(&script).unwrap()
        );

        let NodeKind::Branch {
            condition: MoveCondition::WorkInt { value, .. },
            ..
        } = &mut track.nodes[1].kind
        else {
            panic!()
        };
        *value = NativeIntOperand::Literal(1);
        let edited = MoveGraph {
            tracks: vec![track],
            ..Default::default()
        };
        let compiled = timeline::compile("chrom", "specialairnloop", &edited).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "specialairnloop");
        assert!(emitted.contains("WORK_INT_READY_LEVEL) == 1 {"));
        assert!(!emitted.contains("WORK_INT_READY_LEVEL) == 0 {"));
        for owner in ["ike", "marth", "unknown"] {
            let track = effects::import_for_fighter(owner, &script);
            assert!(crate::move_nodes::test_support::untyped_condition(
                &track.nodes[1].kind
            ));
        }
        let unknown = crate::acmd::parse_effect_script(&source.replace("get_int(", "get_int64("));
        assert!(crate::move_nodes::test_support::untyped_condition(
            &effects::import_for_fighter("chrom", &unknown).nodes[1].kind
        ));
        assert!(registry::resolve("chrom", ROY_READY_LEVEL, IdentifierKind::WorkInt).is_err());
    }

    #[test]
    fn only_known_fighter_owned_status_integer_reads_fall_back_from_registry() {
        assert_eq!(status_int_reads("ike"), IKE_STATUS_INT_READS);
        assert_eq!(status_int_reads("roy"), ROY_STATUS_INT_READS);
        assert!(status_int_reads("marth").is_empty());
        assert!(status_int_reads("unknown").is_empty());
        assert!(validate_source("ike", IKE_READY_LEVEL, IdentifierKind::WorkInt).is_ok());
        assert!(validate_source("roy", ROY_READY_LEVEL, IdentifierKind::WorkInt).is_ok());
        assert!(validate_source("roy", IKE_READY_LEVEL, IdentifierKind::WorkInt).is_err());
        assert!(validate_source("ike", ROY_READY_LEVEL, IdentifierKind::WorkInt).is_err());
        assert!(validate_source("ike", IKE_READY_LEVEL, IdentifierKind::WorkFloat).is_err());
        assert!(validate_source("unknown", IKE_READY_LEVEL, IdentifierKind::WorkInt).is_err());

        for (fighter, symbol) in [("ike", IKE_READY_LEVEL), ("roy", ROY_READY_LEVEL)] {
            assert!(registry::resolve(fighter, symbol, IdentifierKind::WorkInt).is_err());
            for marker in ["START", "TERM"] {
                let owner = fighter.to_ascii_uppercase();
                assert!(validate_source(
                    fighter,
                    &format!("FIGHTER_{owner}_STATUS_SPECIAL_N_WORK_INT_{marker}"),
                    IdentifierKind::WorkInt
                )
                .is_err());
            }
            for value in [0, 1, 2, 3] {
                let source = format!(
                    "if WorkModule::get_int(agent.module_accessor, *{symbol}) == {value} {{"
                );
                assert!(import(fighter, &source).is_some(), "{source}");
            }
            assert!(import(
                fighter,
                &format!(
                    "if WorkModule::get_int(agent.module_accessor, *{symbol}) == 2147483648 {{"
                )
            )
            .is_none());
            let foreign = if fighter == "ike" { "roy" } else { "ike" };
            assert!(import(
                foreign,
                &format!("if WorkModule::get_int(agent.module_accessor, *{symbol}) == 0 {{")
            )
            .is_none());
            assert!(header(
                fighter,
                &MoveCondition::WorkInt {
                    symbol: symbol.into(),
                    op: NativeComparison::Eq,
                    value: NativeIntOperand::Choice(
                        "FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT".into()
                    ),
                }
            )
            .is_err());
        }
    }
}
