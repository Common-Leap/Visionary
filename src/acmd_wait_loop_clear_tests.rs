//! `wait_loop_clear` as a typed statement and as the every-motion-loop Repeat it closes.
use super::*;
use crate::data::ExcuteStmt;
use crate::move_nodes::{
    model::{MoveCommand, MoveGraph, MoveNode, MoveTrack, NodeKind, Track},
    timeline,
};

/// The first two hits of Fox's rapid jab, verbatim:
/// SSBU-Dumped-Scripts smashline/lua2cpp_fox/fox/Attack100.txt lines 1-23.
const FOX_ATTACK100: &str = r#"unsafe extern "C" fn game_attack100(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("kneel"), 0.6, 361, 20, 0, 7, 2.8, 5.0, 0.0, 0.7, Some(-2.0), Some(0.0), Some(0.0), 0.5, 0.6, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
        macros::ATTACK(agent, 1, 0, Hash40::new("top"), 0.6, 361, 20, 0, 7, 4.8, 0.0, 8.0, 14.5, Some(0.0), Some(8.0), Some(8.0), 0.5, 0.6, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
        macros::ATK_SET_SHIELD_SETOFF_MUL_arg3(agent, 0, 1, 6.0);
    }
    wait(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECK);
    }
    macros::wait_loop_clear(agent);
    frame(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("kneel"), 0.6, 361, 20, 0, 7, 2.8, 5.0, 0.0, 0.7, Some(-2.0), Some(0.0), Some(0.0), 0.5, 0.6, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
        macros::ATTACK(agent, 1, 0, Hash40::new("top"), 0.6, 361, 20, 0, 7, 4.8, 0.0, 8.0, 14.5, Some(0.0), Some(8.0), Some(8.0), 0.5, 0.6, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
        macros::ATK_SET_SHIELD_SETOFF_MUL_arg3(agent, 0, 1, 6.0);
    }
    wait(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECK);
    }
}
"#;

/// The rest of the same function, verbatim: Attack100.txt lines 24-58.
const FOX_ATTACK100_REST: &str = r#"    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("kneel"), 0.6, 361, 20, 0, 7, 2.8, 5.0, 0.0, 0.7, Some(-2.0), Some(0.0), Some(0.0), 0.5, 0.6, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
        macros::ATTACK(agent, 1, 0, Hash40::new("top"), 0.6, 361, 20, 0, 7, 4.8, 0.0, 8.0, 14.5, Some(0.0), Some(8.0), Some(8.0), 0.5, 0.6, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
        macros::ATK_SET_SHIELD_SETOFF_MUL_arg3(agent, 0, 1, 6.0);
    }
    wait(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECK);
    }
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("kneel"), 0.6, 361, 20, 0, 7, 2.8, 5.0, 0.0, 0.7, Some(-2.0), Some(0.0), Some(0.0), 0.5, 0.6, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
        macros::ATTACK(agent, 1, 0, Hash40::new("top"), 0.6, 361, 20, 0, 7, 4.8, 0.0, 8.0, 14.5, Some(0.0), Some(8.0), Some(8.0), 0.5, 0.6, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
        macros::ATK_SET_SHIELD_SETOFF_MUL_arg3(agent, 0, 1, 6.0);
    }
    wait(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECK);
    }
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("kneel"), 0.6, 361, 20, 0, 7, 2.8, 5.0, 0.0, 0.7, Some(-2.0), Some(0.0), Some(0.0), 0.5, 0.6, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
        macros::ATTACK(agent, 1, 0, Hash40::new("top"), 0.6, 361, 20, 0, 7, 4.8, 0.0, 8.0, 14.5, Some(0.0), Some(8.0), Some(8.0), 0.5, 0.6, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
        macros::ATK_SET_SHIELD_SETOFF_MUL_arg3(agent, 0, 1, 6.0);
    }
    wait(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECK);
    }
    frame(agent.lua_state_agent, 10.0);
}
"#;

/// Donkey Kong's run expression, verbatim:
/// SSBU-Dumped-Scripts smashline/lua2cpp_donkey/donkey/Run.txt lines 34-45.
const DONKEY_RUN_EXPRESSION: &str = r#"unsafe extern "C" fn expression_run(agent: &mut L2CAgentBase) {
    wait_loop_clear();
    if macros::is_excute(agent) {
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_TOP, 6, true);
    }
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_run_hv"), 0, false, *BATTLE_OBJECT_ID_INVALID as u32);
    }
    frame(agent.lua_state_agent, 22.0);
    ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_run_hv"), 0, false, *BATTLE_OBJECT_ID_INVALID as u32);
}
"#;

fn graph(track: MoveTrack) -> MoveGraph {
    MoveGraph {
        tracks: vec![track],
        ..Default::default()
    }
}

fn compile_one(graph: &MoveGraph) -> AcmdScript {
    timeline::compile("fox", "attack_100", graph)
        .unwrap()
        .remove(0)
        .1
}

fn attacks(nodes: &mut [MoveNode]) -> Vec<&mut crate::data::AttackCall> {
    let mut out = Vec::new();
    for node in nodes {
        match &mut node.kind {
            NodeKind::Execute { commands } => {
                out.extend(commands.iter_mut().filter_map(|command| match command {
                    ExcuteStmt::Attack(call) => Some(call),
                    _ => None,
                }))
            }
            NodeKind::MixedExecute { commands } => {
                out.extend(commands.iter_mut().filter_map(|command| match command {
                    MoveCommand::Editable(ExcuteStmt::Attack(call)) => Some(call),
                    _ => None,
                }))
            }
            NodeKind::Repeat { body, .. } => out.extend(attacks(body)),
            _ => {}
        }
    }
    out
}

fn assert_syntax(text: &str) {
    syn::parse_file(text).unwrap_or_else(|error| panic!("{error}\n{text}"));
}

/// The frame of every root `frame` call and the attack damage of every hit, in body order.
fn shape(stmts: &[AcmdStmt]) -> Vec<String> {
    let mut out = Vec::new();
    for stmt in stmts {
        match stmt {
            AcmdStmt::Frame(frame) => out.push(format!("F{frame}")),
            AcmdStmt::Excute(commands) => {
                if commands
                    .iter()
                    .any(|command| matches!(command, ExcuteStmt::Attack(_)))
                {
                    out.push("HIT".into());
                }
            }
            _ => {}
        }
    }
    out
}

/// The dump writes Fox's jab as `hit; wait_loop_clear; frame(2) hit`. Re-rolled, the hit that
/// came first is the body's tail, and the whole script is one motion loop.
#[test]
fn dumped_rapid_jab_is_rerolled_into_an_editable_motion_loop() {
    let parsed = parse_acmd_script(FOX_ATTACK100);
    let [AcmdStmt::Loop {
        count: 1,
        body,
        motion_loop: crate::data::MotionLoop::Clear,
    }] = parsed.stmts.as_slice()
    else {
        panic!("{:?}", parsed.stmts);
    };
    assert_eq!(shape(body), ["F2", "HIT", "HIT"]);
    assert!(!body
        .iter()
        .any(|stmt| matches!(stmt, AcmdStmt::WaitLoopClear)));

    let mut track = timeline::import_for_fighter("fox", Track::Game, &parsed);
    assert!(
        track.baseline.is_empty(),
        "nothing in the jab is preserved: {:?}",
        track.baseline
    );
    assert!(matches!(
        track.nodes.as_slice(),
        [MoveNode {
            kind: NodeKind::Repeat {
                motion_loop: crate::data::MotionLoop::Clear,
                count: 1,
                ..
            },
            ..
        }]
    ));
    let hits = attacks(&mut track.nodes);
    assert_eq!(hits.len(), 4);
    for hit in hits {
        assert_eq!(hit.damage, 0.6);
        hit.damage = 3.0;
    }
    let compiled = compile_one(&graph(track));
    let text = preview_game_fn(&compiled, "attack_100");
    assert_syntax(&text);
    assert!(text.contains("    loop {\n"), "{text}");
    assert!(
        text.contains("        macros::wait_loop_clear(agent);\n    }\n}"),
        "{text}"
    );
    assert_eq!(text.matches("wait_loop_clear").count(), 1, "{text}");
    assert_eq!(
        text.matches("Hash40::new(\"kneel\"), 3.0,").count(),
        2,
        "{text}"
    );

    let reparsed = parse_acmd_script(&text);
    crate::move_nodes::test_support::assert_json_eq!(&compiled, &reparsed);
    let reimported = timeline::import_for_fighter("fox", Track::Game, &reparsed);
    crate::move_nodes::test_support::assert_json_eq!(&compiled, &compile_one(&graph(reimported)));
}

/// The whole Fox game script: the dangling `frame(10)` the dump ends on is where the leading hit
/// runs once re-rolled, keeping the two-frame spacing (2, 4, 6, 8, 10).
#[test]
fn rerolled_fox_jab_keeps_its_hit_spacing() {
    let text = format!(
        "{}{}",
        FOX_ATTACK100.trim_end().trim_end_matches('}'),
        FOX_ATTACK100_REST
    );
    let parsed = parse_acmd_script(&text);
    let [AcmdStmt::Loop { body, .. }] = parsed.stmts.as_slice() else {
        panic!();
    };
    assert_eq!(
        shape(body),
        ["F2", "HIT", "F4", "HIT", "F6", "HIT", "F8", "HIT", "F10", "HIT"]
    );
}

/// Expression scripts open with the wait (the dump's `wait_loop_clear();`), so nothing rotates.
#[test]
fn dumped_expression_loop_with_a_leading_wait_is_wrapped_and_exported_with_arguments() {
    let parsed = parse_expression_script(DONKEY_RUN_EXPRESSION);
    let [AcmdStmt::Loop {
        body,
        motion_loop: crate::data::MotionLoop::Clear,
        ..
    }] = parsed.stmts.as_slice()
    else {
        panic!("{:?}", parsed.stmts);
    };
    assert!(matches!(
        body.as_slice(),
        [AcmdStmt::Excute(_), AcmdStmt::Frame(10.0), ..]
    ));
    let track = timeline::import_for_fighter("donkey", Track::Expression, &parsed);
    assert!(matches!(
        track.nodes[0].kind,
        NodeKind::Repeat {
            motion_loop: crate::data::MotionLoop::Clear,
            ..
        }
    ));
    let compiled = timeline::compile("donkey", "run", &graph(track))
        .unwrap()
        .remove(0)
        .1;
    let text = preview_expression_fn(&compiled, "run");
    assert_syntax(&text);
    assert!(
        text.contains("        macros::wait_loop_clear(agent);\n    }\n}"),
        "{text}"
    );
    assert!(!text.contains("wait_loop_clear();"), "{text}");
    crate::move_nodes::test_support::assert_json_eq!(&compiled, &parse_expression_script(&text));
}

/// Paired: a rotation that would run `frame` calls backwards is not one ordered body, so the
/// script stays as written and the wait stays an editable root block. Its twin, with the
/// frames in order, re-rolls. A second root wait also leaves the script flat.
#[test]
fn scripts_that_are_not_one_rotated_body_stay_flat() {
    let script = |before: f32, after: f32| {
        format!(
            "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n    frame(agent.lua_state_agent, {before:.1});\n    macros::wait_loop_clear(agent);\n    frame(agent.lua_state_agent, {after:.1});\n}}\n"
        )
    };
    let rolled = parse_acmd_script(&script(5.0, 3.0));
    assert!(
        matches!(rolled.stmts.as_slice(), [AcmdStmt::Loop { motion_loop: crate::data::MotionLoop::Clear, body, .. }]
        if matches!(body.as_slice(), [AcmdStmt::Frame(3.0), AcmdStmt::Frame(5.0)]))
    );

    let flat = parse_acmd_script(&script(3.0, 5.0)).stmts;
    assert!(
        matches!(
            flat.as_slice(),
            [
                AcmdStmt::Frame(3.0),
                AcmdStmt::WaitLoopClear,
                AcmdStmt::Frame(5.0)
            ]
        ),
        "{flat:?}"
    );
    let track = timeline::import_for_fighter("fox", Track::Game, &AcmdScript { stmts: flat });
    assert!(matches!(track.nodes[1].kind, NodeKind::NativeWaitLoopClear));

    let twice = "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {\n    macros::wait_loop_clear(agent);\n    frame(agent.lua_state_agent, 2.0);\n    macros::wait_loop_clear(agent);\n}\n";
    assert!(matches!(
        parse_acmd_script(twice).stmts.as_slice(),
        [
            AcmdStmt::WaitLoopClear,
            AcmdStmt::Frame(2.0),
            AcmdStmt::WaitLoopClear
        ]
    ));
}

/// Paired: the same `loop` is typed only while its body ends by yielding to the motion.
#[test]
fn a_loop_without_a_trailing_wait_loop_clear_stays_preserved() {
    let typed = "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {\n    loop {\n        frame(agent.lua_state_agent, 2.0);\n        macros::wait_loop_clear(agent);\n    }\n}\n";
    let untyped = "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {\n    loop {\n        macros::wait_loop_clear(agent);\n        frame(agent.lua_state_agent, 2.0);\n    }\n}\n";
    assert!(matches!(
        parse_acmd_script(typed).stmts.as_slice(),
        [AcmdStmt::Loop {
            motion_loop: crate::data::MotionLoop::Clear,
            ..
        }]
    ));
    let parsed = parse_acmd_script(untyped);
    assert!(
        matches!(parsed.stmts.as_slice(), [AcmdStmt::RawBlock { header, .. }] if header == "loop {")
    );
    let track = timeline::import_for_fighter("fox", Track::Game, &parsed);
    assert!(matches!(
        track.nodes.as_slice(),
        [MoveNode {
            kind: NodeKind::Preserved { .. },
            ..
        }]
    ));
}

/// Paired: placement rules. The accepted twin of every rejected graph compiles.
#[test]
fn motion_loop_constructs_are_root_game_or_expression_and_the_loop_comes_last() {
    let node = |id, kind| MoveNode { id, kind };
    let track = |track, nodes| {
        graph(MoveTrack {
            track,
            nodes,
            baseline: vec![],
            effect_execute_baseline: vec![],
        })
    };
    let repeat = |id| {
        node(
            id,
            NodeKind::Repeat {
                count: 1,
                body: vec![node(id + 10, NodeKind::Wait { frames: 2.0 })],
                motion_loop: crate::data::MotionLoop::Clear,
            },
        )
    };

    assert!(timeline::compile(
        "fox",
        "x",
        &track(
            Track::Game,
            vec![node(1, NodeKind::Frame { frame: 1.0 }), repeat(2)]
        )
    )
    .is_ok());
    let after = timeline::compile(
        "fox",
        "x",
        &track(
            Track::Game,
            vec![repeat(2), node(1, NodeKind::Frame { frame: 1.0 })],
        ),
    )
    .unwrap_err();
    assert!(
        after
            .iter()
            .any(|error| error.reason.contains("must be the last block")),
        "{after:?}"
    );

    assert!(timeline::compile(
        "fox",
        "x",
        &track(
            Track::Expression,
            vec![node(1, NodeKind::NativeWaitLoopClear)]
        )
    )
    .is_ok());
    let sound = timeline::compile(
        "fox",
        "x",
        &track(Track::Sound, vec![node(1, NodeKind::NativeWaitLoopClear)]),
    )
    .unwrap_err();
    assert!(
        sound
            .iter()
            .any(|error| error.reason.contains("Game or Expression")),
        "{sound:?}"
    );

    let nested = timeline::compile(
        "fox",
        "x",
        &track(
            Track::Game,
            vec![node(
                1,
                NodeKind::Repeat {
                    count: 2,
                    body: vec![node(2, NodeKind::NativeWaitLoopClear)],
                    motion_loop: crate::data::MotionLoop::No,
                },
            )],
        ),
    )
    .unwrap_err();
    assert!(
        nested
            .iter()
            .any(|error| error.reason.contains("direct root")),
        "{nested:?}"
    );

    let mut counted = repeat(2);
    if let NodeKind::Repeat { count, .. } = &mut counted.kind {
        *count = 3;
    }
    let counted = timeline::compile("fox", "x", &track(Track::Game, vec![counted])).unwrap_err();
    assert!(
        counted
            .iter()
            .any(|error| error.reason.contains("count must be 1")),
        "{counted:?}"
    );

    // The loop body is compiled as the root, but cannot wait for the motion loop twice.
    let mut doubled = repeat(2);
    if let NodeKind::Repeat { body, .. } = &mut doubled.kind {
        body.push(node(30, NodeKind::NativeWaitLoopClear));
    }
    let doubled = timeline::compile("fox", "x", &track(Track::Game, vec![doubled])).unwrap_err();
    assert!(
        doubled
            .iter()
            .any(|error| error.reason.contains("already waits")),
        "{doubled:?}"
    );
}

#[test]
fn only_exact_wait_loop_clear_spellings_are_typed() {
    for line in [
        "macros::wait_loop_clear(agent);",
        "wait_loop_clear();",
        "wait_loop_clear(agent);",
        "smash::app::sv_animcmd::wait_loop_clear(agent.lua_state_agent);",
    ] {
        let text =
            format!("unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n    {line}\n}}\n");
        // A lone root wait is a motion loop with an empty body.
        assert!(
            matches!(parse_acmd_script(&text).stmts.as_slice(), [AcmdStmt::Loop { motion_loop: crate::data::MotionLoop::Clear, body, .. }] if body.is_empty()),
            "{line}"
        );
    }
    for line in [
        "wait_loop_clear(other);",
        "let x = wait_loop_clear(agent);",
        "macros::wait_loop_clear(agent)",
    ] {
        let text =
            format!("unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n    {line}\n}}\n");
        assert!(
            matches!(
                parse_acmd_script(&text).stmts.as_slice(),
                [AcmdStmt::Raw(_)]
            ),
            "{line}"
        );
    }
}
