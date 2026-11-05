//! Execute-owned alternatives lifted from the pinned public ACMD corpus.
use super::{
    effects,
    model::{MoveCondition, MoveGraph, MoveNode, NodeKind, Track},
    timeline,
};
use crate::{
    acmd,
    data::{AcmdScript, AcmdStmt, ExcuteStmt},
};

// bayonetta/Attack11.txt
const EFFECT: &str = r#"unsafe extern "C" fn effect_attack11(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 7.0);
    if macros::is_excute(agent) {
        macros::FOOT_EFFECT(agent, Hash40::new("null"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1.1, 0, 0, 0, 0, 0, 0, false);
    }
    if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {
        if macros::is_excute(agent) {
            macros::EFFECT_FOLLOW_WORK(agent, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_EFFECT_KIND_BAYONETTA_ATTACK_LINE1, Hash40::new("top"), -2.5, 10.5, -3, 0, 0, 0, 1, false);
        }
        else {
        if macros::is_excute(agent) {
            macros::EFFECT_FOLLOW_WORK(agent, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_EFFECT_KIND_BAYONETTA_ATTACK_LINE1, Hash40::new("top"), 2.5, 10.5, -3, 0, 0, 0, 1, false);
        }
    }
}
if macros::is_excute(agent) {
    EffectModule::set_disable_render_offset_last(agent.module_accessor);
}
frame(agent.lua_state_agent, 9.0);
if macros::is_excute(agent) {
    macros::EFFECT_DETACH_KIND_WORK(agent, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_EFFECT_KIND_BAYONETTA_ATTACK_LINE1, -1);
}
frame(agent.lua_state_agent, 14.0);
if WorkModule::get_int(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND) != *FIGHTER_BAYONETTA_COSTUME_KIND_BAYONETTA_1 {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_COLOR(agent, Hash40::new("bayonetta_beretta_rotation"), Hash40::new("havel"), 0, 0, 0, 0, 0, 0, 1, true, 0.059, 0.38, 1);
    }
    else {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_COLOR(agent, Hash40::new("bayonetta_beretta_rotation"), Hash40::new("havel"), 0, 0, 0, 0, 0, 0, 1, true, 1, 0.118, 0.118);
    }
}
}
frame(agent.lua_state_agent, 30.0);
if macros::is_excute(agent) {
macros::EFFECT_OFF_KIND(agent, Hash40::new("bayonetta_beretta_rotation"), true, true);
}
}"#;

// buddy/SpecialSStart.txt
const EXPRESSION: &str = r#"unsafe extern "C" fn expression_specialsstart(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_LR);
    }
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_BUDDY_STATUS_SPECIAL_S_FLAG_FAIL) {
        if macros::is_excute(agent) {
            ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_73_bodyattack_kamae"), 0, false, *BATTLE_OBJECT_ID_INVALID as u32);
        }
        else {
        if macros::is_excute(agent) {
            ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_smashhold1"), 18, true, *BATTLE_OBJECT_ID_INVALID as u32);
        }
    }
}
}"#;

// eflame/AppealHiL.txt
const ELEMENT: &str = r#"unsafe extern "C" fn game_appealhil(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_ELEMENT_INSTANCE_WORK_ID_FLAG_ENABLE_CHANGE) {
        if macros::is_excute(agent) {
            ArticleModule::generate_article(agent.module_accessor, *FIGHTER_EFLAME_GENERATE_ARTICLE_DIVER, false, -1);
            ArticleModule::change_motion(agent.module_accessor, *FIGHTER_EFLAME_GENERATE_ARTICLE_DIVER, Hash40::new("f_appeal_hi_l"), false, -1.0);
        }
    }
}"#;

// sonic/AppealSL.txt
const SONIC: &str = r#"unsafe extern "C" fn effect_appealsl(agent: &mut L2CAgentBase) {
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_SONIC_INSTANCE_WORK_FLAG_DISABLE_RUN_TRACE) {
        if macros::is_excute(agent) {
            EFFECT_FOLLOW_arg11(Hash40::new("sonic_appealruntrace"), Hash40::new("top"), 0, 0, 0, 180, 0, 0, -0.85, true, *FIGHTER_INSTANCE_WORK_ID_INT_COLOR);
            macros::LAST_EFFECT_SET_WORK_INT(agent, *FIGHTER_STATUS_WIN_WORK_INT_EFFECT_HANDLE);
            macros::LAST_EFFECT_SET_RATE(agent, 0.76);
        }
    }
    frame(agent.lua_state_agent, 70.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_HANDLE(agent, *FIGHTER_STATUS_WIN_WORK_INT_EFFECT_HANDLE);
    }
}"#;

fn assert_same<T: serde::Serialize, U: serde::Serialize>(left: &T, right: &U) {
    assert_eq!(
        serde_json::to_value(left).unwrap(),
        serde_json::to_value(right).unwrap()
    );
}

fn roundtrip(fighter: &str, track: Track, source: &str) -> super::model::MoveTrack {
    let (script, imported) = if track == Track::Effect {
        let effect = acmd::parse_effect_script(source);
        let script = AcmdScript {
            stmts: effect
                .stmts
                .iter()
                .map(effects::effect_stmt_to_acmd)
                .collect(),
        };
        (script, effects::import_for_fighter(fighter, &effect))
    } else {
        let script = if track == Track::Game {
            acmd::parse_acmd_script(source)
        } else {
            acmd::parse_expression_script(source)
        };
        let imported = timeline::import_for_fighter(fighter, track, &script);
        (script, imported)
    };
    let graph = MoveGraph {
        tracks: vec![imported.clone()],
        ..Default::default()
    };
    let restored = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
    let compiled = timeline::compile(fighter, "fixture", &restored).unwrap();
    // Compare with the parse-side witness as well as reparsing emitted source.
    if track != Track::Effect {
        assert_same(&compiled[0].1, &script);
    }
    let emitted = if track == Track::Effect {
        acmd::preview_effect_structure_fn(&compiled[0].1, "fixture")
    } else if track == Track::Game {
        acmd::preview_game_fn(&compiled[0].1, "fixture")
    } else {
        acmd::preview_expression_fn(&compiled[0].1, "fixture")
    };
    syn::parse_file(&emitted).unwrap();
    if track == Track::Effect {
        assert_same(
            &acmd::parse_effect_script(&emitted),
            &acmd::parse_effect_script(source),
        );
    } else if track == Track::Game {
        assert_same(&acmd::parse_acmd_script(&emitted), &script);
    } else {
        assert_same(&acmd::parse_expression_script(&emitted), &script);
    }
    imported
}

fn branches(nodes: &[MoveNode]) -> Vec<(&MoveCondition, &[MoveNode], &[MoveNode])> {
    let mut result = Vec::new();
    for node in nodes {
        if let NodeKind::Branch {
            condition,
            then_nodes,
            else_nodes,
        } = &node.kind
        {
            result.push((condition, then_nodes.as_slice(), else_nodes.as_slice()));
            result.extend(branches(then_nodes));
            result.extend(branches(else_nodes));
        }
    }
    result
}

#[test]
fn corpus_execute_alternatives_preserve_owner_and_effect_payloads() {
    let parsed = acmd::parse_effect_script(EFFECT);
    assert!(!parsed
        .to_effect_calls()
        .iter()
        .any(|call| call.spawn_func == "EFFECT_FOLLOW_COLOR"));
    assert!(acmd::unexportable_effect_lines(&parsed)
        .iter()
        .any(|line| line == "if macros::is_excute(agent) {"));
    let imported = roundtrip("bayonetta", Track::Effect, EFFECT);
    let all = branches(&imported.nodes);
    let execute = all
        .iter()
        .filter(|(condition, _, _)| matches!(condition, MoveCondition::IsExecute))
        .collect::<Vec<_>>();
    assert_eq!(execute.len(), 1);
    // The LR and costume guards have no alternate: each alternate is inside its Execute.
    assert!(all
        .iter()
        .filter(|(condition, _, _)| !matches!(condition, MoveCondition::IsExecute))
        .all(|(_, _, other)| other.is_empty()));
    // Work-selected effects keep their original mixed witness and its identity lock.
    let (_, work_then, _) = all
        .iter()
        .find(|(condition, _, _)| matches!(condition, MoveCondition::NativeLrNegative))
        .unwrap();
    let NodeKind::MixedEffectExecute {
        source_commands, ..
    } = &work_then[0].kind
    else {
        panic!()
    };
    assert!(
        matches!(&source_commands[0], crate::data::EffectMacro::EffectFollowWork { work_symbol, offset, scale, .. }
        if work_symbol == "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_EFFECT_KIND_BAYONETTA_ATTACK_LINE1" && *offset == [-2.5, 10.5, -3.0] && *scale == 1.0)
    );
    let NodeKind::BareCommand { command } = &execute[0].1[0].kind else {
        panic!()
    };
    assert!(
        matches!(command.as_ref(), ExcuteStmt::Effect(command) if matches!(command.as_ref(), crate::data::EffectMacro::Effect { offset, scale, extra_args, .. }
        if *offset == [0.0, 0.0, 0.0] && *scale == 1.0 && extra_args == &["true", "0.059", "0.38", "1"]))
    );

    assert!(execute.iter().all(|(_, _, other)| !other.is_empty()));
    // An effect lifted from the true arm cannot become an unguarded authored command.
    let forged = MoveGraph {
        tracks: vec![super::model::MoveTrack {
            nodes: execute[0].1.to_vec(),
            baseline: Vec::new(),
            ..imported.clone()
        }],
        ..Default::default()
    };
    assert!(timeline::compile("bayonetta", "fixture", &forged).is_err());
}

#[test]
fn corpus_camera_request_keeps_execute_alternative_and_opaque_operands() {
    // brave/FinalStart.txt: the camera-flag block, including its Execute-owned alternate.
    let source = r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {
        frame(agent.lua_state_agent, 9.0);
        if macros::is_excute(agent) {
            macros::FT_SET_FINAL_FEAR_FACE(agent, 50);
            macros::REQ_FINAL_START_CAMERA(agent, Hash40::new("d04finalstart.nuanmb"), true);
        }
        else {
        if macros::is_excute(agent) {
            PostureModule::scale(agent.module_accessor, 4, 0);
            0x23a020(-1586326874, 2.1);
            CAM_ZOOM_IN_arg5(0, 0);
            macros::FT_START_CUTIN(agent);
        }
    }
}
}"#;
    let imported = roundtrip("brave", Track::Game, source);
    let all = branches(&imported.nodes);
    assert!(matches!(&imported.nodes[0].kind, NodeKind::Branch {
        condition: MoveCondition::WorkFlag { symbol, expected: false }, else_nodes, ..
    } if symbol == "FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA" && else_nodes.is_empty()));
    let execute = all
        .iter()
        .filter(|(condition, _, _)| matches!(condition, MoveCondition::IsExecute))
        .collect::<Vec<_>>();
    assert_eq!(execute.len(), 1);
    let (_, then_nodes, else_nodes) = execute[0];
    assert!(
        matches!(&then_nodes[0].kind, NodeKind::BareCommand { command }
        if matches!(command.as_ref(), ExcuteStmt::FinalFearFace(call) if call.argument == "50"))
    );
    assert!(
        matches!(&then_nodes[1].kind, NodeKind::BareCommand { command }
        if matches!(command.as_ref(), ExcuteStmt::NativeCall(call)
            if call.callee == "macros::REQ_FINAL_START_CAMERA"
                && call.arguments == ["Hash40::new(\"d04finalstart.nuanmb\")", "true"]))
    );
    let NodeKind::MixedExecute { commands } = &else_nodes[0].kind else {
        panic!()
    };
    let preserved = commands
        .iter()
        .filter_map(|command| match command {
            super::model::MoveCommand::Preserved(ExcuteStmt::Raw(raw)) => Some(raw.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        preserved,
        [
            "PostureModule::scale(agent.module_accessor, 4, 0);",
            "0x23a020(-1586326874, 2.1);",
            "CAM_ZOOM_IN_arg5(0, 0);",
        ]
    );
    let unknown = source.replace(
        "!WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA)",
        "unknown_condition(agent)",
    );
    let imported = roundtrip("brave", Track::Game, &unknown);
    // The unknown guard stays source text and grants its body no fear-face authority.
    let all = branches(&imported.nodes);
    assert!(
        matches!(all[0].0, MoveCondition::Source { header } if header == "if unknown_condition(agent) {")
    );
    assert!(!format!("{:?}", imported.nodes).contains("FinalFearFace"));
}

#[test]
fn corpus_expression_execute_alternative_retains_rumble_values() {
    let imported = roundtrip("buddy", Track::Expression, EXPRESSION);
    let all = branches(&imported.nodes);
    let (_, then_nodes, else_nodes) = all
        .iter()
        .find(|(condition, _, _)| matches!(condition, MoveCondition::IsExecute))
        .unwrap();
    let NodeKind::BareCommand { command } = &then_nodes[0].kind else {
        panic!()
    };
    // Values are checked before compilation, so a parse-side omission cannot pass unnoticed.
    let emitted = acmd::emit_stmts(&[AcmdStmt::Bare(command.clone())], "").join("\n");
    assert!(emitted.contains("rbkind_73_bodyattack_kamae"));
    assert!(emitted.contains("0, false, *BATTLE_OBJECT_ID_INVALID as u32"));
    let NodeKind::Execute { commands } = &else_nodes[0].kind else {
        panic!()
    };
    let emitted = acmd::emit_stmts(&[AcmdStmt::Excute(commands.clone())], "").join("\n");
    assert!(emitted.contains("rbkind_smashhold1"));
    assert!(emitted.contains("18, true, *BATTLE_OBJECT_ID_INVALID as u32"));
}

#[test]
fn execute_branch_rejects_attributes_and_keeps_source_witness() {
    let positive = roundtrip("buddy", Track::Expression, EXPRESSION);
    assert!(branches(&positive.nodes)
        .iter()
        .any(|(condition, _, _)| matches!(condition, MoveCondition::IsExecute)));
    let changed = EXPRESSION.replace(
        "        if macros::is_excute(agent) {",
        "        #[cfg(any())]\n        if macros::is_excute(agent) {",
    );
    let imported = roundtrip("buddy", Track::Expression, &changed);
    assert!(!branches(&imported.nodes)
        .iter()
        .any(|(condition, _, _)| matches!(condition, MoveCondition::IsExecute)));
    let unknown_owner = EFFECT.replace("macros::is_excute(agent)", "other::is_excute(agent)");
    let imported = roundtrip("bayonetta", Track::Effect, &unknown_owner);
    assert!(!branches(&imported.nodes)
        .iter()
        .any(|(condition, _, _)| matches!(condition, MoveCondition::IsExecute)));
}

#[test]
fn owned_read_only_guards_preserve_native_values_and_reject_other_owners() {
    use super::native_work;
    for (fighter, source, track, symbol) in [
        (
            "eflame",
            ELEMENT,
            Track::Game,
            "FIGHTER_ELEMENT_INSTANCE_WORK_ID_FLAG_ENABLE_CHANGE",
        ),
        (
            "sonic",
            SONIC,
            Track::Effect,
            "FIGHTER_SONIC_INSTANCE_WORK_FLAG_DISABLE_RUN_TRACE",
        ),
    ] {
        let script = if track == Track::Game {
            acmd::parse_acmd_script(source)
        } else {
            AcmdScript {
                stmts: acmd::parse_effect_script(source)
                    .stmts
                    .iter()
                    .map(effects::effect_stmt_to_acmd)
                    .collect(),
            }
        };
        let imported = if track == Track::Game {
            timeline::import_for_fighter(fighter, track, &script)
        } else {
            effects::import_for_fighter(fighter, &acmd::parse_effect_script(source))
        };
        assert!(
            matches!(&imported.nodes[0].kind, NodeKind::Branch { condition: MoveCondition::WorkFlag { symbol: actual, .. }, .. } if actual == symbol)
        );
        assert_eq!(
            native_work::validate_condition_flag(fighter, &format!("*{symbol}")).unwrap(),
            symbol
        );
        assert!(native_work::validate_flag(fighter, &format!("*{symbol}")).is_err());
        assert!(native_work::validate_condition_flag("mario", &format!("*{symbol}")).is_err());
        assert!(
            native_work::validate_condition_flag(fighter, &format!("*{symbol}_UNKNOWN")).is_err()
        );
        let values = serde_json::to_string(&imported).unwrap();
        if fighter == "eflame" {
            assert!(values.contains("FIGHTER_EFLAME_GENERATE_ARTICLE_DIVER"));
            assert!(values.contains("f_appeal_hi_l"));
        } else {
            assert!(values.contains("sonic_appealruntrace"));
            assert!(values.contains("0.76"));
        }
        let graph = MoveGraph {
            tracks: vec![imported],
            ..Default::default()
        };
        let compiled = timeline::compile(fighter, "fixture", &graph).unwrap();
        if track == Track::Effect {
            let emitted = acmd::preview_effect_structure_fn(&compiled[0].1, "fixture");
            assert_same(
                &acmd::parse_effect_script(&emitted),
                &acmd::parse_effect_script(source),
            );
        } else {
            assert_same(&compiled[0].1, &script);
        }
    }
}

#[test]
fn corpus_pikmin_numeric_branch_exposes_typed_children() {
    // Vanilla pikmin/AttackAirF.txt, complete Effect function.
    let source = r#"unsafe extern "C" fn effect_attackairf(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 2.0);
    if WorkModule::get_int(agent.module_accessor, *FIGHTER_PIKMIN_STATUS_ATTACK_AIR_WORK_INT_HAVE_PIKMIN) == 1 {
        frame(agent.lua_state_agent, 7.0);
    }
}"#;
    let imported = roundtrip("pikmin", Track::Effect, source);
    let NodeKind::Branch {
        condition: MoveCondition::WorkInt { symbol, .. },
        then_nodes,
        ..
    } = &imported.nodes[1].kind
    else {
        panic!("corpus comparison must be editable");
    };
    assert_eq!(
        symbol,
        "FIGHTER_PIKMIN_STATUS_ATTACK_AIR_WORK_INT_HAVE_PIKMIN"
    );
    assert!(matches!(&then_nodes[0].kind, NodeKind::Frame { frame } if *frame == 7.0));
}

#[test]
fn corpus_weapon_branches_expose_effect_commands() {
    // Vanilla tantan_ring/AttackFly.txt, complete Effect function.
    let source = r#"unsafe extern "C" fn effect_attackfly(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *WEAPON_TANTAN_RING_INSTANCE_WORK_ID_FLAG_IS_AIR) {
        if WorkModule::is_flag(agent.module_accessor, *WEAPON_TANTAN_RING_INSTANCE_WORK_ID_FLAG_IS_LONG) {
            if macros::is_excute(agent) {
                macros::EFFECT_FOLLOW(agent, Hash40::new("tantan_wepon3_fly"), Hash40::new("cakram1"), 0, 0, 0, 0, 0, 0, 1, true);
            }
            frame(agent.lua_state_agent, 8.0);
            if macros::is_excute(agent) {
                macros::EFFECT_OFF_KIND(agent, Hash40::new("tantan_wepon3_fly"), false, false);
            }
            else {
            if WorkModule::is_flag(agent.module_accessor, *WEAPON_TANTAN_RING_INSTANCE_WORK_ID_FLAG_IS_LONG) {
                if macros::is_excute(agent) {
                    macros::EFFECT_FOLLOW(agent, Hash40::new("tantan_wepon3_fly"), Hash40::new("cakram1"), 0, 0, 0, 0, 0, 0, 1, true);
                }
                frame(agent.lua_state_agent, 9.0);
                if macros::is_excute(agent) {
                    macros::EFFECT_OFF_KIND(agent, Hash40::new("tantan_wepon3_fly"), false, false);
                }
                else {
                if macros::is_excute(agent) {
                    macros::EFFECT_FOLLOW(agent, Hash40::new("tantan_wepon3_fly_s"), Hash40::new("cakram1"), 0, 0, 0, 0, 0, 0, 1, true);
                }
                frame(agent.lua_state_agent, 4.0);
                if macros::is_excute(agent) {
                    macros::EFFECT_OFF_KIND(agent, Hash40::new("tantan_wepon3_fly_s"), false, false);
                }
                else {
                if macros::is_excute(agent) {
                    macros::EFFECT_FOLLOW(agent, Hash40::new("tantan_wepon3_fly_s"), Hash40::new("cakram1"), 0, 0, 0, 0, 0, 0, 1, true);
                }
                frame(agent.lua_state_agent, 7.0);
                if macros::is_excute(agent) {
                    macros::EFFECT_OFF_KIND(agent, Hash40::new("tantan_wepon3_fly_s"), false, false);
                }
            }
        }
    }
}
}
}
}"#;
    let imported = roundtrip("tantan_ring", Track::Effect, source);
    let NodeKind::Branch { then_nodes, .. } = &imported.nodes[0].kind else {
        panic!("outer weapon guard must be typed");
    };
    let NodeKind::Branch { then_nodes, .. } = &then_nodes[0].kind else {
        panic!("inner weapon guard must be typed");
    };
    assert!(
        matches!(&then_nodes[0].kind, NodeKind::EffectExecute { commands } if commands.len() == 1 && !matches!(commands[0], crate::data::EffectMacro::Raw(_)))
    );
}
