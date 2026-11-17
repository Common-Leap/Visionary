//! Source fixtures lifted unchanged from the public ACMD corpus.
use super::{
    coverage, effects,
    model::{MoveCondition, MoveGraph, MoveNode, MoveTrack, NodeKind, Track},
    test_support::assert_json_eq,
    timeline,
};
use crate::{
    acmd,
    data::{AcmdScript, AttackHeightAllKind, ExcuteStmt},
};

// trail/TurnDash.txt: expression_turndash
const ITEM: &str = r#"unsafe extern "C" fn expression_turndash(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_LR, 2);
    }
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_NONE, 2);
    }
    frame(agent.lua_state_agent, 7.0);
    if macros::is_excute(agent) {
        ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_dash"), 0, false, *BATTLE_OBJECT_ID_INVALID as u32);
    }
    frame(agent.lua_state_agent, 20.0);
    if macros::is_excute(agent) {
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_LR);
    }
    frame(agent.lua_state_agent, 32.0);
    if ItemModule::is_have_item(agent.module_accessor, 0) {
        if macros::is_excute(agent) {
            macros::FT_MOTION_INTP_WAIT_ITEM(agent, false, 0.001);
        }
    }
    frame(agent.lua_state_agent, 37.0);
    if macros::is_excute(agent) {
        macros::FT_MOTION_INTP_WAIT_ITEM(agent, false, 0.05);
    }
    frame(agent.lua_state_agent, 41.0);
    if macros::is_excute(agent) {
        macros::FT_MOTION_INTP_WAIT_ITEM(agent, false, 0.11);
    }
    frame(agent.lua_state_agent, 46.0);
    if macros::is_excute(agent) {
        macros::FT_MOTION_INTP_WAIT_ITEM(agent, false, 0);
    }
}"#;

// trail/SpecialLwStart.txt: expression_speciallwstart
const ITEM_LOOP: &str = r#"unsafe extern "C" fn expression_speciallwstart(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_LR);
        ItemModule::set_have_item_visibility(agent.module_accessor, false, 0);
    }
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_81_special_lw"), 0, true, *BATTLE_OBJECT_ID_INVALID as u32);
    }
    frame(agent.lua_state_agent, 19.0);
    for _ in 0..15 {
    if macros::is_excute(agent) {
        ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_81_speciallw_land"), 0, true, *BATTLE_OBJECT_ID_INVALID as u32);
    }
    wait(agent.lua_state_agent, 2.0);
}
frame(agent.lua_state_agent, 46.0);
if ItemModule::is_have_item(agent.module_accessor, 0) {
    if macros::is_excute(agent) {
        macros::FT_MOTION_INTP_WAIT_ITEM(agent, true, 0.05);
    }
}
frame(agent.lua_state_agent, 50.0);
if macros::is_excute(agent) {
    macros::FT_MOTION_INTP_WAIT_ITEM(agent, true, 0.07);
}
frame(agent.lua_state_agent, 60.0);
if macros::is_excute(agent) {
    ItemModule::set_have_item_visibility(agent.module_accessor, true, 0);
}
frame(agent.lua_state_agent, 63.0);
if macros::is_excute(agent) {
    macros::FT_MOTION_INTP_WAIT_ITEM(agent, true, 0);
}
if macros::is_excute(agent) {
    ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_81_landingairlwlight"), 0, false, *BATTLE_OBJECT_ID_INVALID as u32);
}
wait(agent.lua_state_agent, 2.0);
}"#;

// daisy/AttackLw4.txt: game_attacklw4
const HEIGHT_LOOP: &str = r#"unsafe extern "C" fn game_attacklw4(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD);
    }
    frame(agent.lua_state_agent, 6.0);
    for _ in 0..6 {
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 2.0, 0, 100, 20, 0, 2.5, -2.5, 2.0, 0.0, Some(2.5), Some(2.0), Some(0.0), 0.5, 0.5, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_cutup"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_CUTUP, *ATTACK_REGION_BODY);
        macros::ATTACK(agent, 1, 0, Hash40::new("top"), 2.0, 180, 100, 25, 0, 3.2, -2.5, 2.0, 7.5, Some(2.5), Some(2.0), Some(7.5), 0.5, 0.5, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_cutup"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_CUTUP, *ATTACK_REGION_BODY);
        macros::ATTACK(agent, 2, 0, Hash40::new("top"), 2.0, 180, 100, 25, 0, 3.2, -2.5, 2.0, -7.5, Some(2.5), Some(2.0), Some(-7.5), 0.5, 0.5, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_cutup"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_CUTUP, *ATTACK_REGION_BODY);
        AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_LOW), false);
    }
    wait(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
    }
    wait(agent.lua_state_agent, 2.0);
}
if macros::is_excute(agent) {
    macros::ATTACK(agent, 0, 0, Hash40::new("top"), 3.0, 361, 180, 0, 40, 4.3, 0.0, 1.8, 7.5, Some(0.0), Some(1.8), Some(-7.5), 3.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_cutup"), *ATTACK_SOUND_LEVEL_L, *COLLISION_SOUND_ATTR_CUTUP, *ATTACK_REGION_BODY);
    AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_LOW), false);
}
wait(agent.lua_state_agent, 2.0);
if macros::is_excute(agent) {
    AttackModule::clear_all(agent.module_accessor);
}
}"#;

fn imported(fighter: &str, track: Track, source: &str) -> (AcmdScript, MoveTrack) {
    let parsed = match track {
        Track::Game => acmd::parse_acmd_script(source),
        Track::Sound => acmd::parse_sound_script(source),
        Track::Effect => {
            let parsed = acmd::parse_effect_script(source);
            return (
                AcmdScript {
                    stmts: parsed
                        .stmts
                        .iter()
                        .map(effects::effect_stmt_to_acmd)
                        .collect(),
                },
                effects::import_for_fighter(fighter, &parsed),
            );
        }
        _ => acmd::parse_expression_script(source),
    };
    let nodes = timeline::import_for_fighter(fighter, track, &parsed);
    (parsed, nodes)
}

fn roundtrip(fighter: &str, track: Track, source: &str) -> MoveTrack {
    let (parsed, imported) = imported(fighter, track, source);
    let graph = MoveGraph {
        tracks: vec![imported.clone()],
        ..Default::default()
    };
    assert!(super::live_support::source_only_tracks(&graph).any(|value| value == track));
    let restored = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
    let compiled = timeline::compile(fighter, "fixture", &restored).unwrap();
    // EffectStmt's transport uses RawBlock for Execute; compilation uses AcmdStmt::Excute.
    // Compare EffectScript below, where both representations have the same structural IR.
    if track != Track::Effect {
        assert_json_eq!(&compiled[0].1, &parsed);
    }
    let emitted = match track {
        Track::Game => acmd::preview_game_fn(&compiled[0].1, "fixture"),
        Track::Sound => acmd::preview_sound_fn(&compiled[0].1, "fixture"),
        Track::Effect => acmd::preview_effect_structure_fn(&compiled[0].1, "fixture"),
        _ => acmd::preview_expression_fn(&compiled[0].1, "fixture"),
    };
    syn::parse_file(&emitted).unwrap();
    if track == Track::Effect {
        assert_json_eq!(
            acmd::parse_effect_script(&emitted),
            acmd::parse_effect_script(source)
        );
    } else {
        assert_json_eq!(self::imported(fighter, track, &emitted).0, parsed);
    }
    imported
}

fn all_nodes(nodes: &[MoveNode]) -> Vec<&MoveNode> {
    let mut out = Vec::new();
    for node in nodes {
        out.push(node);
        match &node.kind {
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                out.extend(all_nodes(then_nodes));
                out.extend(all_nodes(else_nodes));
            }
            NodeKind::Repeat { body, .. } => out.extend(all_nodes(body)),
            _ => {}
        }
    }
    out
}

#[test]
fn corpus_held_item_guard_keeps_slot_timing_and_interpolation_values() {
    // Pinned skyline-smash 258a566 cpp.rs: ItemModule::is_have_item(accessor, c_int) -> bool.
    // No return-value guess or smash-script wrapper is involved.
    for (source, frame, first, second, args) in [
        (ITEM, 32.0, 0.001, 0.05, "false"),
        (ITEM_LOOP, 46.0, 0.05, 0.07, "true"),
    ] {
        let imported = roundtrip("trail", Track::Expression, source);
        let coverage = coverage::track_summary(&imported);
        assert_eq!(coverage.preserved_block_count, 0);
        assert_eq!(coverage.preserved_command_count, 0);
        assert_eq!(coverage.opaque_source_line_count, 0);
        let branch_index = imported
            .nodes
            .iter()
            .position(|node| {
                matches!(
                    node.kind,
                    NodeKind::Branch {
                        condition: MoveCondition::ItemHeld,
                        ..
                    }
                )
            })
            .unwrap();
        assert!(
            matches!(imported.nodes[branch_index - 1].kind, NodeKind::Frame { frame: actual } if actual == frame)
        );
        let NodeKind::Branch {
            then_nodes,
            else_nodes,
            ..
        } = &imported.nodes[branch_index].kind
        else {
            panic!()
        };
        assert!(else_nodes.is_empty());
        let NodeKind::Execute { commands } = &then_nodes[0].kind else {
            panic!()
        };
        let [ExcuteStmt::NativeCall(call)] = commands.as_slice() else {
            panic!()
        };
        assert_eq!(call.callee, "macros::FT_MOTION_INTP_WAIT_ITEM");
        assert_eq!(call.arguments, [args.to_owned(), acmd::num(first)]);
        let NodeKind::Execute { commands } = &imported.nodes[branch_index + 2].kind else {
            panic!()
        };
        assert!(
            matches!(commands.as_slice(), [ExcuteStmt::NativeCall(call)] if call.arguments == [args.to_owned(), acmd::num(second)])
        );
        if source == ITEM_LOOP {
            assert!(all_nodes(&imported.nodes)
                .iter()
                .any(|node| matches!(node.kind, NodeKind::Repeat { count: 15, .. })));
        }
    }
}

#[test]
fn corpus_held_item_guard_rejects_changed_receiver_slot_arity_and_attributes() {
    const HEADER: &str = "if ItemModule::is_have_item(agent.module_accessor, 0) {";
    for header in [
        "if ItemModule::is_have_item(other.module_accessor, 0) {",
        "if ItemModule::is_have_item(agent.module_accessor, 1) {",
        "if ItemModule::is_have_item(agent.module_accessor, 0, false) {",
        "if ItemModule::is_have_item(agent.module_accessor) {",
        "if ItemModule::is_have_item(agent.module_accessor, slot) {",
        "#[cfg(any())]\n    if ItemModule::is_have_item(agent.module_accessor, 0) {",
    ] {
        let source = ITEM.replace(HEADER, header);
        let imported = roundtrip("trail", Track::Expression, &source);
        assert!(
            !all_nodes(&imported.nodes).iter().any(|node| matches!(
                node.kind,
                NodeKind::Branch {
                    condition: MoveCondition::ItemHeld,
                    ..
                }
            )),
            "{header}"
        );
        assert!(
            all_nodes(&imported.nodes)
                .iter()
                .any(|node| super::test_support::untyped_condition(&node.kind)),
            "{header}"
        );
    }
    assert!(timeline::import_condition(None, HEADER).is_none());
    assert!(timeline::import_condition(Some("unknown"), HEADER).is_none());
    let (_, imported) = imported("trail", Track::Expression, ITEM);
    for track in [Track::Game, Track::Sound, Track::Effect] {
        let mut moved = imported.clone();
        moved.track = track;
        let errors = timeline::compile(
            "trail",
            "fixture",
            &MoveGraph {
                tracks: vec![moved],
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.reason.contains("held-item")));
        let prefix = match track {
            Track::Game => "game",
            Track::Sound => "sound",
            Track::Effect => "effect",
            _ => unreachable!(),
        };
        let source = ITEM.replace("expression_turndash", &format!("{prefix}_turndash"));
        let (parsed, imported) = self::imported("trail", track, &source);
        assert!(!parsed.stmts.is_empty());
        assert!(!all_nodes(&imported.nodes).iter().any(|node| matches!(
            node.kind,
            NodeKind::Branch {
                condition: MoveCondition::ItemHeld,
                ..
            }
        )));
    }
}

#[test]
fn corpus_attack_height_in_repeat_keeps_count_calls_and_attack_payloads() {
    // Pinned skyline-smash 258a566 cpp.rs: set_attack_height_all(accessor, AttackHeight, bool).
    let imported = roundtrip("daisy", Track::Game, HEIGHT_LOOP);
    let summary = coverage::track_summary(&imported);
    assert_eq!(summary.preserved_command_count, 0);
    assert_eq!(summary.preserved_block_count, 0);
    let nodes = all_nodes(&imported.nodes);
    assert!(nodes
        .iter()
        .any(|node| matches!(node.kind, NodeKind::Repeat { count: 6, .. })));
    let mut heights = 0;
    let mut payloads = Vec::new();
    for node in nodes {
        if let NodeKind::Execute { commands } = &node.kind {
            for command in commands {
                match command {
                    ExcuteStmt::AttackModuleSetHeightAll { height, arg3 } => {
                        assert!(matches!(height, AttackHeightAllKind::Low));
                        assert!(!arg3);
                        heights += 1;
                    }
                    ExcuteStmt::Attack(call) => payloads.push((
                        call.id,
                        call.damage,
                        call.angle,
                        call.size,
                        call.capsule_end,
                    )),
                    _ => {}
                }
            }
        }
    }
    assert_eq!(heights, 2);
    assert_eq!(
        payloads,
        [
            (0, 2.0, 0, 2.5, Some([2.5, 2.0, 0.0])),
            (1, 2.0, 180, 3.2, Some([2.5, 2.0, 7.5])),
            (2, 2.0, 180, 3.2, Some([2.5, 2.0, -7.5])),
            (0, 3.0, 361, 4.3, Some([0.0, 1.8, -7.5])),
        ]
    );
}

#[test]
fn corpus_nested_attack_height_rejects_changed_abi_or_attributed_calls() {
    const CALL: &str = "AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_LOW), false);";
    for call in [
        CALL.replace("agent.module_accessor", "other.module_accessor"),
        CALL.replace("false", "true"),
        CALL.replace(", false", ""),
        CALL.replace("ATTACK_HEIGHT_LOW", "ATTACK_HEIGHT_MIDDLE"),
        format!("#[cfg(any())]\n        {CALL}"),
    ] {
        let source = HEIGHT_LOOP.replace(CALL, &call);
        let imported = roundtrip("daisy", Track::Game, &source);
        assert!(coverage::track_summary(&imported).preserved_command_count >= 2);
        assert!(!all_nodes(&imported.nodes).iter().any(|node| matches!(&node.kind, NodeKind::Execute { commands } if commands.iter().any(|command| matches!(command, ExcuteStmt::AttackModuleSetHeightAll { .. })))));
    }
    for track in [Track::Expression, Track::Sound, Track::Effect] {
        let prefix = match track {
            Track::Expression => "expression",
            Track::Sound => "sound",
            Track::Effect => "effect",
            _ => unreachable!(),
        };
        let source = HEIGHT_LOOP.replace("game_attacklw4", &format!("{prefix}_attacklw4"));
        let (parsed, imported) = self::imported("daisy", track, &source);
        assert!(!parsed.stmts.is_empty());
        let graph = MoveGraph {
            tracks: vec![imported],
            ..Default::default()
        };
        let compiled = timeline::compile("daisy", "fixture", &graph).unwrap();
        assert!(!serde_json::to_string(&compiled)
            .unwrap()
            .contains("AttackModuleSetHeightAll"));
    }
}

// bayonetta/FuraFura.txt: sound_furafura
const OPAQUE_SOUND: &str = r#"unsafe extern "C" fn sound_furafura(agent: &mut L2CAgentBase) {
    wait_loop_sync_mot();
    WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FURAFURA_VOICE_FINAL);
    0x1cb400(false, true);
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        macros::PLAY_STATUS(agent, Hash40::new("vc_bayonetta_furafura"));
    }
    wait_loop_sync_mot();
}"#;

// buddy/AttackAirHi.txt: effect_attackairhi
const OPAQUE_COLOR: &str = r#"unsafe extern "C" fn effect_attackairhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 7.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("buddy_air_hi"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
    }
    if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 0)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.8, 0.3, 0);
        }
    }
    if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 1)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.7, 0.35, 0.1);
        }
    }
    if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 2)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.8, 0.2, 0.5);
        }
    }
    if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 3)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.5, 0.2, 0.35);
        }
    }
    if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 4)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.35, 0.35, 0.35);
        }
    }
    if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 5)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.4, 0.3, 0.1);
        }
    }
    if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 6)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 1, 0.2, 0.2);
        }
    }
    if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 7)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.1, 0.05, 0.1);
        }
    }
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_NO_STOP(agent, Hash40::new("buddy_air_hi_flash"), Hash40::new("top"), 0, 23, 0, 0, 0, 0, 1, true);
    }
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_WORK(agent, *FIGHTER_BUDDY_INSTANCE_WORK_ID_INT_EFFECT_KIND_FLYING, Hash40::new("top"), 0, 18, 0, 0, 0, 0, 0.9, true);
    }
    frame(agent.lua_state_agent, 13.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND_WORK(agent, *FIGHTER_BUDDY_INSTANCE_WORK_ID_INT_EFFECT_KIND_FLYING, false, true);
    }
}"#;

#[test]
fn corpus_unknown_address_calls_and_color_guards_keep_their_opaque_semantics() {
    let sound = roundtrip("bayonetta", Track::Sound, OPAQUE_SOUND);
    assert!(sound.baseline.iter().any(|statement| matches!(statement, crate::data::AcmdStmt::Raw(line) if line == "0x1cb400(false, true);")));
    let mut effect = roundtrip("buddy", Track::Effect, OPAQUE_COLOR);
    // buddy/AttackAirHi.txt lines 46-83, read by eye, not through the parser.
    const TINTS: [[f32; 3]; 8] = [
        [0.8, 0.3, 0.0],
        [0.7, 0.35, 0.1],
        [0.8, 0.2, 0.5],
        [0.5, 0.2, 0.35],
        [0.35, 0.35, 0.35],
        [0.4, 0.3, 0.1],
        [1.0, 0.2, 0.2],
        [0.1, 0.05, 0.1],
    ];
    let guarded = effect
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| match &node.kind {
            NodeKind::Branch {
                condition: MoveCondition::Source { header },
                then_nodes,
                else_nodes,
            } if else_nodes.is_empty() => Some((index, header.as_str(), then_nodes.as_slice())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(guarded.len(), 8);
    for (slot, ((_, header, body), tint)) in guarded.iter().zip(TINTS).enumerate() {
        // The unresolvable address call stays the source's own text; the tint under it is typed.
        assert_eq!(
            *header,
            format!("if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, {slot})){{")
        );
        assert!(
            matches!(body, [MoveNode { kind: NodeKind::EffectExecute { commands }, .. }]
            if matches!(commands.as_slice(), [crate::data::EffectMacro::LastEffectSetColor { rgb }] if *rgb == tint))
        );
    }
    let summary = coverage::track_summary(&effect);
    assert_eq!(summary.preserved_block_count, 0);
    assert_eq!(summary.editable_command_count, 12);
    assert_eq!(summary.opaque_source_line_count, 0);

    // Retint slot 3: only that value moves, and every address header is re-emitted byte-exact.
    let (slot3, _, _) = guarded[3];
    let NodeKind::Branch { then_nodes, .. } = &mut effect.nodes[slot3].kind else {
        panic!()
    };
    let NodeKind::EffectExecute { commands } = &mut then_nodes[0].kind else {
        panic!()
    };
    commands[0] = crate::data::EffectMacro::LastEffectSetColor {
        rgb: [0.25, 0.5, 0.75],
    };
    let graph = MoveGraph {
        tracks: vec![effect.clone()],
        ..Default::default()
    };
    let compiled = timeline::compile("buddy", "attackairhi", &graph).unwrap();
    let emitted = acmd::preview_effect_structure_fn(&compiled[0].1, "attackairhi");
    let lines = emitted.lines().map(str::trim).collect::<Vec<_>>();
    let at = lines
        .iter()
        .position(|line| *line == "if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 3)){")
        .unwrap();
    assert_eq!(
        lines[at + 2],
        "macros::LAST_EFFECT_SET_COLOR(agent, 0.25, 0.5, 0.75);"
    );
    assert!(!emitted.contains("0.5, 0.2, 0.35"));
    let source_headers = OPAQUE_COLOR
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("if(0x"));
    assert!(source_headers.eq(lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("if(0x"))));

    // A header the import did not witness cannot be authored, and a witnessed one cannot vanish.
    let mut forged = effect.clone();
    let NodeKind::Branch {
        condition: MoveCondition::Source { header },
        ..
    } = &mut forged.nodes[slot3].kind
    else {
        panic!()
    };
    *header = "if(0x18cc20(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 9)){".into();
    let errors = timeline::compile(
        "buddy",
        "attackairhi",
        &MoveGraph {
            tracks: vec![forged],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(errors
        .iter()
        .any(|error| error.reason.contains("source condition does not match")));
    let mut dropped = effect;
    dropped.nodes.remove(slot3);
    let errors = timeline::compile(
        "buddy",
        "attackairhi",
        &MoveGraph {
            tracks: vec![dropped],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(errors
        .iter()
        .any(|error| error.reason.contains("baseline source")));
}

#[test]
fn unresolved_address_conditions_keep_their_header_and_type_the_block_under_it() {
    // SSBU-Dumped-Scripts 59004685, smashline paths and original line numbers below.
    // All three addresses resolve to PLT0 (the shared resolver) in the matching
    // 13.0.1 NROs: robot=nro72, dolly=nro80, pickel=nro71. GOT[2] has no
    // symbol relocation, so neither the address nor the arguments identify a getter.
    for (fighter, source) in [
        // lua2cpp_robot/robot/AttackDash.txt:24-35
        (
            "robot",
            r#"unsafe extern "C" fn game_fixture(agent: &mut L2CAgentBase) {
    if(0x1050b0(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 0)){
        if WorkModule::is_flag(agent.module_accessor, *FIGHTER_ROBOT_INSTANCE_WORK_ID_FLAG_REGION_JP) {
            if macros::is_excute(agent) {
                macros::LAST_EFFECT_SET_COLOR(agent, 0.1, 0.01, 0);
            }
            else {
            if macros::is_excute(agent) {
                macros::LAST_EFFECT_SET_COLOR(agent, 0.196, 0.196, 0.216);
            }
        }
    }
}
}"#,
        ),
        // lua2cpp_dolly/dolly/SpecialAirSFEnd.txt:3-12
        (
            "dolly",
            r#"unsafe extern "C" fn game_fixture(agent: &mut L2CAgentBase) {
    if(0x2508e0(*FIGHTER_DOLLY_STATUS_SPECIAL_COMMON_WORK_INT_STRENGTH, *FIGHTER_DOLLY_STRENGTH_W)){
        if macros::is_excute(agent) {
            MotionModule::set_rate(agent.module_accessor, 1);
        }
        else {
        if macros::is_excute(agent) {
            MotionModule::set_rate(agent.module_accessor, 0.8);
        }
    }
}
}"#,
        ),
        // lua2cpp_pickel/pickel/AttackDash.txt:10-16
        (
            "pickel",
            r#"unsafe extern "C" fn game_fixture(agent: &mut L2CAgentBase) {
    if(0x370870(*FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_HAVE_CRAFT_WEAPON_MATERIAL_KIND, *FIGHTER_PICKEL_MATERIAL_KIND_DIAMOND)){
        if macros::is_excute(agent) {
            macros::ATTACK(agent, 0, 0, Hash40::new("top"), 14.04, 50, 66, 0, 68, 5.8, 0.0, 8.8, 4.2, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 1, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_cutup"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_OBJECT);
            macros::ATTACK(agent, 1, 0, Hash40::new("top"), 14.04, 50, 66, 0, 68, 5.8, 0.0, 8.8, 9.4, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 1, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_cutup"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_OBJECT);
            WorkModule::set_float(agent.module_accessor, 4.5, *FIGHTER_PICKEL_INSTANCE_WORK_ID_FLOAT_ATTACK_DURABILITY);
        }
    }
}"#,
        ),
    ] {
        // The header stays the dump's own text; the blocks under it import as typed nodes.
        let imported = roundtrip(fighter, Track::Game, source);
        let header = source.lines().nth(1).unwrap().trim();
        assert!(matches!(
            imported.nodes.as_slice(),
            [MoveNode { kind: NodeKind::Branch { condition: MoveCondition::Source { header: kept }, .. }, .. }] if kept == header
        ));
        if fighter != "pickel" {
            continue;
        }
        // pickel/AttackDash.txt: both diamond hitboxes deal 14.04, read off the corpus line.
        let mut edited = imported;
        let NodeKind::Branch { then_nodes, .. } = &mut edited.nodes[0].kind else {
            panic!()
        };
        let [MoveNode {
            kind: NodeKind::Execute { commands },
            ..
        }] = then_nodes.as_mut_slice()
        else {
            panic!("{then_nodes:?}")
        };
        let damages = commands
            .iter()
            .filter_map(|command| match command {
                ExcuteStmt::Attack(call) => Some(call.damage),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(damages, [14.04, 14.04]);
        let ExcuteStmt::Attack(call) = &mut commands[0] else {
            panic!()
        };
        call.damage = 16.5;
        let compiled = timeline::compile(
            fighter,
            "fixture",
            &MoveGraph {
                tracks: vec![edited],
                ..Default::default()
            },
        )
        .unwrap();
        let emitted = acmd::preview_game_fn(&compiled[0].1, "fixture");
        assert!(emitted.lines().any(|line| line.trim() == header));
        assert!(emitted.contains(r#"Hash40::new("top"), 16.5, 50, 66"#));
        assert_eq!(emitted.matches("14.04").count(), 1);
    }
}

// rosetta/Final.txt lines 1-39: Execute guards holding an unresolved address call, each
// with an alternate the dump wrote as a separate `else {`, inside a typed LR branch.
const EXECUTE_ALTERNATE: &str = r#"unsafe extern "C" fn game_final(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 7, 20, 0, 0, 0);
        macros::SLOW_OPPONENT(agent, 15, 70);
    }
    frame(agent.lua_state_agent, 1.0);
    macros::FT_MOTION_RATE(agent, 0.5);
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {
        if macros::is_excute(agent) {
            macros::FT_SET_FINAL_FEAR_FACE(agent, 55);
        }
    }
    if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {
        if macros::is_excute(agent) {
            macros::REQ_FINAL_START_CAMERA(agent, Hash40::new("d04final02.nuanmb"), true);
        }
        else {
        if macros::is_excute(agent) {
            PostureModule::scale(agent.module_accessor, 4, 0);
            0x11e570(1469557358, 0.9);
            CAM_ZOOM_IN_arg5(0, 0);
            camera!(agent, *MA_MSC_CMD_CAMERA_CAM_OFFSET, 0, 13);
            camera!(agent, *MA_MSC_CMD_CAMERA_CAM_RECT, 0, 0, 50, 0);
        }
        else {
        if macros::is_excute(agent) {
            macros::REQ_FINAL_START_CAMERA(agent, Hash40::new("d04final.nuanmb"), true);
        }
        if macros::is_excute(agent) {
            macros::FT_START_CUTIN(agent);
        }
    }
}
}
frame(agent.lua_state_agent, 50.0);
if macros::is_excute(agent) {
macros::CAM_ZOOM_OUT(agent);
}
}"#;

// bayonetta/AttackS4.txt lines 55-83: timing inside an address-call branch's arms.
const ADDRESS_TIMING: &str = r#"unsafe extern "C" fn sound_attacks4(agent: &mut L2CAgentBase) {
    
        if WorkModule::get_int(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND) != *FIGHTER_BAYONETTA_COSTUME_KIND_BAYONETTA_1 {
            frame(agent.lua_state_agent, 2.0);
            if macros::is_excute(agent) {
                macros::PLAY_SE(agent, Hash40::new("se_bayonetta_wecked_chargefull"));
            }
        }
        frame(agent.lua_state_agent, 10.0);
        WorkModule::get_float(agent.module_accessor, *FIGHTER_STATUS_WORK_ID_FLOAT_RESERVE_HOLD_RATE);
        if(0x1cb400(-460198162, 1)){
            if macros::is_excute(agent) {
                macros::PLAY_SEQUENCE(agent, Hash40::new("seq_bayonetta_rnd_attack_smash_s01_jp"));
            }
            else {
            if macros::is_excute(agent) {
                macros::PLAY_SEQUENCE(agent, Hash40::new("seq_bayonetta_rnd_attack_smash_s02_jp"));
            }
            frame(agent.lua_state_agent, 34.0);
            if macros::is_excute(agent) {
                macros::PLAY_SE(agent, Hash40::new("se_bayonetta_loveisblue_spin"));
            }
            frame(agent.lua_state_agent, 90.0);
            if macros::is_excute(agent) {
                macros::PLAY_STEP(agent, Hash40::new("se_bayonetta_step_right_s"));
            }
        }
    }
}"#;

#[test]
fn corpus_source_else_arms_and_timing_under_address_headers_are_typed_and_editable() {
    let mut imported = roundtrip("rosetta", Track::Game, EXECUTE_ALTERNATE);
    let NodeKind::Branch {
        condition: MoveCondition::NativeLrNegative,
        then_nodes,
        ..
    } = &mut imported.nodes[4].kind
    else {
        panic!()
    };
    let NodeKind::Branch {
        condition: MoveCondition::IsExecute,
        else_nodes,
        ..
    } = &mut then_nodes[0].kind
    else {
        panic!()
    };
    // Line 25's `else {` follows the address-call Execute of lines 18-24 and stays its own text.
    let [MoveNode {
        kind: NodeKind::MixedExecute { .. },
        ..
    }, MoveNode {
        kind:
            NodeKind::Branch {
                condition: MoveCondition::Source { header },
                then_nodes: alternate,
                ..
            },
        ..
    }] = else_nodes.as_mut_slice()
    else {
        panic!("{else_nodes:?}")
    };
    assert_eq!(header, "else {");
    // Line 27: macros::REQ_FINAL_START_CAMERA(agent, Hash40::new("d04final.nuanmb"), true);
    let NodeKind::Execute { commands } = &mut alternate[0].kind else {
        panic!()
    };
    let [ExcuteStmt::NativeCall(call)] = commands.as_mut_slice() else {
        panic!()
    };
    assert_eq!(
        (call.callee.as_str(), call.arguments.as_slice()),
        (
            "macros::REQ_FINAL_START_CAMERA",
            &[
                "Hash40::new(\"d04final.nuanmb\")".to_owned(),
                "true".to_owned()
            ][..]
        )
    );
    call.arguments[1] = "false".into();
    let compiled = timeline::compile(
        "rosetta",
        "finalstart",
        &MoveGraph {
            tracks: vec![imported],
            ..Default::default()
        },
    )
    .unwrap();
    let emitted = acmd::preview_game_fn(&compiled[0].1, "finalstart");
    assert!(emitted.contains(
        r#"macros::REQ_FINAL_START_CAMERA(agent, Hash40::new("d04final.nuanmb"), false);"#
    ));
    assert!(emitted.contains(
        r#"macros::REQ_FINAL_START_CAMERA(agent, Hash40::new("d04final02.nuanmb"), true);"#
    ));
    assert_eq!(
        emitted
            .lines()
            .filter(|line| line.trim() == "else {")
            .count(),
        2
    );
    let mut sound = roundtrip("bayonetta", Track::Sound, ADDRESS_TIMING);
    let header = "if(0x1cb400(-460198162, 1)){";
    let address = sound.nodes.iter_mut().find(|node| matches!(&node.kind, NodeKind::Branch { condition: MoveCondition::Source { header: kept }, .. } if kept == header)).unwrap();
    // Lines 73 and 77: the frames inside the address branch's arm are timing nodes, not text.
    let frames = all_nodes(std::slice::from_ref(address))
        .iter()
        .filter_map(|node| match node.kind {
            NodeKind::Frame { frame } => Some(frame),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(frames, [34.0, 90.0]);
    fn retime(nodes: &mut [MoveNode]) -> bool {
        nodes.iter_mut().any(|node| match &mut node.kind {
            NodeKind::Frame { frame } if *frame == 34.0 => {
                *frame = 36.0;
                true
            }
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => retime(then_nodes) || retime(else_nodes),
            _ => false,
        })
    }
    assert!(retime(std::slice::from_mut(address)));
    let compiled = timeline::compile(
        "bayonetta",
        "attacks4",
        &MoveGraph {
            tracks: vec![sound],
            ..Default::default()
        },
    )
    .unwrap();
    let emitted = acmd::preview_sound_fn(&compiled[0].1, "attacks4");
    assert!(emitted.lines().any(|line| line.trim() == header));
    assert!(emitted.contains("frame(agent.lua_state_agent, 36.0);"));
    assert!(!emitted.contains("frame(agent.lua_state_agent, 34.0);"));
}
