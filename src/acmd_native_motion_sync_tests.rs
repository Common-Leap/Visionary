//! Closed native synchronization syntax, structural fidelity and source-only gates.
use super::*;
use crate::move_nodes::{
    effects,
    model::{MoveGraph, MoveNode, MoveTrack, NodeKind, Track},
    timeline,
};

const CALL: &str = "smash::app::sv_animcmd::wait_loop_sync_mot(agent.lua_state_agent);";
const TRACKS: [Track; 3] = [Track::Effect, Track::Sound, Track::Expression];

fn source(track: Track, body: &str) -> String {
    let prefix = match track {
        Track::Game => "game",
        Track::Effect => "effect",
        Track::Sound => "sound",
        Track::Expression => "expression",
    };
    format!("unsafe extern \"C\" fn {prefix}_motionsynccontract(agent: &mut L2CAgentBase) {{\n{body}\n}}")
}

fn parse(track: Track, text: &str) -> AcmdScript {
    match track {
        Track::Game => parse_acmd_script(text),
        Track::Expression => parse_expression_script(text),
        Track::Sound => parse_sound_script(text),
        Track::Effect => AcmdScript {
            stmts: parse_effect_script(text)
                .stmts
                .iter()
                .map(effects::effect_stmt_to_acmd)
                .collect(),
        },
    }
}

fn import(track: Track, text: &str) -> MoveTrack {
    if track == Track::Effect {
        effects::import_for_fighter("mario", &parse_effect_script(text))
    } else {
        timeline::import_for_fighter("mario", track, &parse(track, text))
    }
}

fn emit(track: Track, script: &AcmdScript) -> String {
    match track {
        Track::Game => preview_game_fn(script, "motionsynccontract"),
        Track::Expression => preview_expression_fn(script, "motionsynccontract"),
        Track::Sound => preview_sound_fn(script, "motionsynccontract"),
        Track::Effect => preview_effect_structure_fn(script, "motionsynccontract"),
    }
}

fn graph(track: Track, nodes: Vec<MoveNode>) -> MoveGraph {
    MoveGraph {
        tracks: vec![MoveTrack {
            track,
            nodes,
            baseline: vec![],
            effect_execute_baseline: vec![],
        }],
        ..Default::default()
    }
}

#[test]
fn motion_sync_requires_exact_native_path_and_context() {
    for path in [
        "wait_loop_sync_mot",
        "sv_animcmd::wait_loop_sync_mot",
        "smash::app::sv_animcmd::wait_loop_sync_mot",
    ] {
        assert!(parse_native_motion_loop_sync_call(&format!(
            "{path}(agent.lua_state_agent);"
        )));
    }
    assert!(parse_native_motion_loop_sync_call("wait_loop_sync_mot();"));
    assert!(parse_native_motion_loop_sync_call(
        "wait_loop_sync_mot(\n agent /* context */ . lua_state_agent,\n);"
    ));
    for call in [
        "sv_animcmd::wait_loop_sync_mot();",
        "smash::app::sv_animcmd::wait_loop_sync_mot();",
        "other::wait_loop_sync_mot(agent.lua_state_agent);",
        "::wait_loop_sync_mot(agent.lua_state_agent);",
        "wait_loop_sync_mot::<u64>(agent.lua_state_agent);",
        "wait_loop_sync_mot!(agent.lua_state_agent);",
        "agent.wait_loop_sync_mot();",
        "wait_loop_sync_mot(other.lua_state_agent);",
        "wait_loop_sync_mot(agent.module_accessor);",
        "wait_loop_sync_mot(lua_state);",
        "wait_loop_sync_mot(agent.lua_state_agent as u64);",
        "wait_loop_sync_mot((agent.lua_state_agent));",
        "wait_loop_sync_mot(agent.lua_state_agent, false);",
        "wait_loop_sync_mot(agent.lua_state_agent, false, 3.0);",
        "#[cfg(any())] wait_loop_sync_mot();",
        "wait_loop_sync_mot()",
        "wait_loop_sync_mot(); frame(agent.lua_state_agent, 9.0);",
        "let x = wait_loop_sync_mot();",
        "// wait_loop_sync_mot();",
    ] {
        assert!(!parse_native_motion_loop_sync_call(call), "{call}");
    }
}

/// The dump writes a motion-looped script as `wait_loop_sync_mot; body`; it re-rolls into the
/// every-motion-loop Repeat and exports as `loop { body; <stack-wrapped wait> }`.
#[test]
fn motion_sync_imports_saves_compiles_and_reparses_on_presentation_tracks() {
    for track in TRACKS {
        for call in ["wait_loop_sync_mot();", CALL] {
            let text = source(track, &format!("    {call}\n    frame(agent.lua_state_agent, 3.0);\n    wait(agent.lua_state_agent, 2.0);"));
            let parsed = parse(track, &text);
            assert!(
                matches!(
                    parsed.stmts.as_slice(),
                    [AcmdStmt::Loop { count: 1, body, motion_loop: crate::data::MotionLoop::SyncMotion }]
                        if matches!(body.as_slice(), [AcmdStmt::Frame(3.0), AcmdStmt::Wait(2.0)])
                ),
                "{text}"
            );
            let imported = import(track, &text);
            assert!(imported.baseline.is_empty());
            assert!(matches!(
                imported.nodes[0].kind,
                NodeKind::Repeat {
                    motion_loop: crate::data::MotionLoop::SyncMotion,
                    count: 1,
                    ..
                }
            ));
            let recipe = MoveGraph {
                tracks: vec![imported],
                ..Default::default()
            };
            let json = serde_json::to_value(&recipe).unwrap();
            assert_eq!(
                json["tracks"][0]["nodes"][0]["kind"]["repeat"]["motion_loop"],
                "sync_motion"
            );
            let restored: MoveGraph = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(serde_json::to_value(&restored).unwrap(), json);
            let compiled = timeline::compile("mario", "motionsynccontract", &restored).unwrap();
            let emitted = emit(track, &compiled[0].1);
            assert!(syn::parse_file(&emitted).is_ok(), "{emitted}");
            assert_eq!(emitted.matches(CALL).count(), 1);
            assert!(emitted.contains("    loop {\n") && emitted.contains("        agent.clear_lua_stack();\n        smash::app::sv_animcmd::wait_loop_sync_mot(agent.lua_state_agent);\n        agent.pop_lua_stack(1);\n    }\n}"), "{emitted}");
            assert!(!emitted.contains("is_excute") && !emitted.contains("for _"));
            assert_eq!(
                serde_json::to_value(parse(track, &emitted)).unwrap(),
                serde_json::to_value(&parsed).unwrap()
            );
            assert_eq!(
                crate::move_nodes::live_support::source_only_tracks(&recipe).collect::<Vec<_>>(),
                vec![track]
            );
            let summary = crate::move_nodes::coverage::track_summary(&recipe.tracks[0]);
            assert_eq!(summary.node_counts.native_motion_loop_sync, 1);
            assert_eq!(summary.editable_command_count, 1);
            assert_eq!(summary.preserved_block_count, 0);
        }
    }
}

#[test]
fn motion_sync_rejects_unsupported_scopes_without_dropping_source() {
    for track in TRACKS {
        for body in [
            "    if macros::is_excute(agent) {\n        wait_loop_sync_mot();\n    }",
            "    for _ in 0..2 {\n        wait_loop_sync_mot();\n    }",
            "    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD) {\n        wait_loop_sync_mot();\n    }",
            "    unsafe {\n        wait_loop_sync_mot();\n    }",
            "    #[cfg(any())]\n    wait_loop_sync_mot();",
            "    #[doc = \"scope\"] wait_loop_sync_mot();",
            "    wait_loop_sync_mot(); frame(agent.lua_state_agent, 9.0);",
            "    frame(agent.lua_state_agent, 9.0); wait_loop_sync_mot();",
            "    for _ in 0..2 {\n        wait_loop_sync_mot(); frame(agent.lua_state_agent, 9.0);\n    }",
            "    let agent = other;\n    wait_loop_sync_mot(); frame(agent.lua_state_agent, 9.0);",
        ] {
            let text = source(track, body);
            let parsed = parse(track, &text);
            assert!(!serde_json::to_string(&parsed).unwrap().contains("NativeMotionLoopSync"), "{text}");
            let recipe = MoveGraph { tracks: vec![import(track, &text)], ..Default::default() };
            let compiled = timeline::compile("mario", "motionsynccontract", &recipe).unwrap();
            let emitted = emit(track, &compiled[0].1);
            assert!(emitted.contains("wait_loop_sync_mot();"), "{emitted}");
            if body.contains("9.0") { assert!(emitted.contains("frame(agent.lua_state_agent, 9.0);"), "{emitted}"); }
            assert_eq!(serde_json::to_value(parse(track, &emitted)).unwrap(), serde_json::to_value(parsed).unwrap(), "{text}\n{emitted}");
        }
    }
    let text = source(
        Track::Game,
        "    wait_loop_sync_mot(); frame(agent.lua_state_agent, 9.0);",
    );
    let parsed = parse(Track::Game, &text);
    assert!(
        matches!(parsed.stmts.as_slice(), [AcmdStmt::Raw(line)] if line.contains("wait_loop_sync_mot();") && line.contains("frame("))
    );
}

#[test]
fn motion_sync_does_not_reinterpret_shadowed_names_or_widen_sound_authority() {
    for track in TRACKS {
        for binding in [
            "fn wait_loop_sync_mot() {}",
            "let wait_loop_sync_mot = || {};",
            "let (wait_loop_sync_mot, x) = (callback, 0);",
            "let agent = other;",
            "mod sv_animcmd {}",
            "mod smash {}",
            "use other::wait_loop_sync_mot;",
            "use other::{wait_loop_sync_mot, x};",
            "use other::callback as wait_loop_sync_mot;",
            "use other as smash;",
            "use other::*;",
            "unsafe extern \"C\" { fn wait_loop_sync_mot(); }",
        ] {
            let text = source(track, &format!("    {binding}\n    wait_loop_sync_mot();"));
            assert!(
                !serde_json::to_string(&parse(track, &text))
                    .unwrap()
                    .contains("NativeMotionLoopSync"),
                "{text}"
            );
        }
        let text = source(track, "    fn helper(agent: &mut L2CAgentBase) { let wait_loop_sync_mot = 0; }\n    wait_loop_sync_mot();");
        assert!(matches!(
            parse(track, &text).stmts.last(),
            Some(AcmdStmt::NativeMotionLoopSync)
        ));
    }
    let text = source(Track::Sound, "    wait_loop_sync_mot();\n    execute(agent.lua_state_agent, 7.0);\n    if macros::is_excute(agent) {\n        VisibilityModule::set_whole(agent.module_accessor, false);\n    }");
    let parsed = parse(Track::Sound, &text);
    assert!(matches!(
        parsed.stmts.as_slice(),
        [AcmdStmt::Loop {
            motion_loop: crate::data::MotionLoop::SyncMotion,
            ..
        }]
    ));
    let json = serde_json::to_string(&parsed).unwrap();
    assert!(
        !json.contains("NativeExecuteFrame") && !json.contains("WholeVisibility"),
        "{json}"
    );
    // Permissions are independent: admitting Sound synchronization does not enable
    // Game/Expression-only root native command families.
    assert!(json.contains("VisibilityModule::set_whole"));
}

#[test]
fn motion_sync_rejects_forged_tracks_and_nested_graphs() {
    let sync = || MoveNode {
        id: 2,
        kind: NodeKind::NativeMotionLoopSync,
    };
    assert!(timeline::compile(
        "mario",
        "motionsynccontract",
        &graph(Track::Game, vec![sync()])
    )
    .is_err());
    for track in TRACKS {
        for kind in [
            NodeKind::Repeat {
                motion_loop: crate::data::MotionLoop::No,
                count: 2,
                body: vec![sync()],
            },
            NodeKind::Branch {
                condition: crate::move_nodes::model::MoveCondition::Grounded,
                then_nodes: vec![sync()],
                else_nodes: vec![],
            },
        ] {
            let errors = timeline::compile(
                "mario",
                "motionsynccontract",
                &graph(track, vec![MoveNode { id: 1, kind }]),
            )
            .unwrap_err();
            assert!(errors
                .iter()
                .any(|error| error.reason.contains("direct root")));
        }
    }
}

#[test]
fn motion_sync_has_distinct_flow_coverage_and_no_flat_effect_residue() {
    let mut recipe = graph(
        Track::Effect,
        vec![
            MoveNode {
                id: 1,
                kind: NodeKind::Frame { frame: 3.0 },
            },
            MoveNode {
                id: 2,
                kind: NodeKind::Wait { frames: 2.0 },
            },
            MoveNode {
                id: 3,
                kind: NodeKind::NativeMotionLoopSync,
            },
        ],
    );
    assert_eq!(
        crate::move_nodes::flow::project(&recipe).nodes[2].kind,
        crate::move_nodes::flow::FlowNodeKind::NativeMotionLoopSync
    );
    crate::move_nodes::flow::reconnect_after(&mut recipe.tracks[0].nodes, 1, 3).unwrap();
    assert_eq!(
        recipe.tracks[0]
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        [1, 3, 2]
    );
    timeline::compile("mario", "motionsynccontract", &recipe).unwrap();
    let parsed = parse_effect_script(&source(Track::Effect, "    wait_loop_sync_mot();"));
    let (calls, residue) = parsed.to_effect_calls_and_residue();
    assert!(calls.is_empty() && residue.is_empty());
    assert_eq!(unexportable_effect_lines(&parsed), [CALL]);
}

/// Steve's run effect, verbatim: SSBU-Dumped-Scripts smashline/lua2cpp_pickel/pickel/Run.txt
/// lines 1-9. HDR's hand-written equivalent (HewDraw-Remix fighters/pickel/src/acmd/other.rs,
/// `effect_run`) is `loop { frame(14) FOOT_EFFECT; frame(29) FOOT_EFFECT; wait_loop_sync_mot }`.
const STEVE_RUN_EFFECT: &str = r#"unsafe extern "C" fn effect_run(agent: &mut L2CAgentBase) {
    wait_loop_sync_mot();
    frame(agent.lua_state_agent, 14.0);
    if macros::is_excute(agent) {
        macros::FOOT_EFFECT(agent, Hash40::new("sys_run_smoke"), Hash40::new("top"), 3, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
    }
    frame(agent.lua_state_agent, 29.0);
    macros::FOOT_EFFECT(agent, Hash40::new("sys_run_smoke"), Hash40::new("top"), 3, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
}
"#;

/// Zelda's dizzy sound, verbatim: smashline/lua2cpp_zelda/zelda/FuraFura.txt lines 1-9. Two
/// waits around a decompiled branch: no single rotation, so it must stay as written.
const ZELDA_FURAFURA_SOUND: &str = r#"unsafe extern "C" fn sound_furafura(agent: &mut L2CAgentBase) {
    wait_loop_sync_mot();
    WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FURAFURA_VOICE_FINAL);
    0x10db70(false, true);
    if macros::is_excute(agent) {
        macros::PLAY_STATUS(agent, Hash40::new("vc_zelda_furafura"));
    }
    wait_loop_sync_mot();
}
"#;

#[test]
fn dumped_run_effect_rerolls_into_the_loop_hdr_writes() {
    let parsed = parse_effect_script(STEVE_RUN_EFFECT);
    let [crate::data::EffectStmt::Loop {
        count: 1,
        body,
        motion_loop: crate::data::MotionLoop::SyncMotion,
    }] = parsed.stmts.as_slice()
    else {
        panic!("{:?}", parsed.stmts);
    };
    assert!(
        matches!(
            body.as_slice(),
            [
                crate::data::EffectStmt::Frame(14.0),
                crate::data::EffectStmt::Excute(_),
                crate::data::EffectStmt::Frame(29.0),
                _
            ]
        ),
        "{body:?}"
    );
    let track = import(Track::Effect, STEVE_RUN_EFFECT);
    assert!(track.baseline.is_empty());
    assert!(matches!(
        track.nodes.as_slice(),
        [MoveNode {
            kind: NodeKind::Repeat {
                motion_loop: crate::data::MotionLoop::SyncMotion,
                ..
            },
            ..
        }]
    ));
    let compiled = timeline::compile(
        "pickel",
        "run",
        &MoveGraph {
            tracks: vec![track],
            ..Default::default()
        },
    )
    .unwrap();
    let emitted = emit(Track::Effect, &compiled[0].1);
    assert!(syn::parse_file(&emitted).is_ok(), "{emitted}");
    assert!(
        emitted.contains("    loop {\n        frame(agent.lua_state_agent, 14.0);"),
        "{emitted}"
    );
    assert_eq!(
        emitted.matches("wait_loop_sync_mot").count(),
        1,
        "{emitted}"
    );
    let reparsed = parse_effect_script(&emitted);
    assert_eq!(
        serde_json::to_value(&reparsed).unwrap(),
        serde_json::to_value(&parsed).unwrap()
    );
}

/// Paired: one root wait re-rolls (above); two root waits do not, and neither does a rotation
/// that would run `frame` calls backwards. Its twin with ordered frames re-rolls.
#[test]
fn sync_scripts_that_are_not_one_rotated_body_stay_flat() {
    let parsed = parse_sound_script(ZELDA_FURAFURA_SOUND);
    let waits = parsed
        .stmts
        .iter()
        .filter(|stmt| matches!(stmt, AcmdStmt::NativeMotionLoopSync))
        .count();
    assert_eq!(waits, 2, "{:?}", parsed.stmts);
    assert!(!parsed
        .stmts
        .iter()
        .any(|stmt| matches!(stmt, AcmdStmt::Loop { .. })));

    let script = |before: f32, after: f32| {
        source(Track::Sound, &format!("    frame(agent.lua_state_agent, {before:.1});\n    wait_loop_sync_mot();\n    frame(agent.lua_state_agent, {after:.1});"))
    };
    assert!(
        matches!(parse(Track::Sound, &script(5.0, 3.0)).stmts.as_slice(),
        [AcmdStmt::Loop { motion_loop: crate::data::MotionLoop::SyncMotion, body, .. }]
            if matches!(body.as_slice(), [AcmdStmt::Frame(3.0), AcmdStmt::Frame(5.0)]))
    );
    assert!(matches!(
        parse(Track::Sound, &script(3.0, 5.0)).stmts.as_slice(),
        [
            AcmdStmt::Frame(3.0),
            AcmdStmt::NativeMotionLoopSync,
            AcmdStmt::Frame(5.0)
        ]
    ));
}

#[test]
fn saved_motion_loop_flags_read_back_unchanged() {
    for (saved, kind) in [
        ("false", crate::data::MotionLoop::No),
        ("true", crate::data::MotionLoop::Clear),
        ("\"sync_motion\"", crate::data::MotionLoop::SyncMotion),
    ] {
        let json = format!("{{\"Loop\":{{\"count\":1,\"body\":[],\"motion_loop\":{saved}}}}}");
        let stmt: AcmdStmt = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(stmt, AcmdStmt::Loop { motion_loop, .. } if motion_loop == kind),
            "{json}"
        );
        if kind.is_loop() {
            assert_eq!(serde_json::to_string(&stmt).unwrap(), json);
        }
    }
}
