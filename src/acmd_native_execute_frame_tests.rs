//! Exact native timing, source preservation and authoring-boundary regressions.
use super::*;
use crate::move_nodes::{
    effects,
    model::{MoveGraph, MoveNode, MoveTrack, NodeKind, Track},
    timeline,
};

fn prefix(track: Track) -> &'static str {
    match track {
        Track::Game => "game",
        Track::Effect => "effect",
        Track::Sound => "sound",
        Track::Expression => "expression",
    }
}

fn source(track: Track, body: &str) -> String {
    format!(
        "unsafe extern \"C\" fn {}_nativeframecontract(agent: &mut L2CAgentBase) {{\n{body}\n}}",
        prefix(track)
    )
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
        Track::Game => preview_game_fn(script, "nativeframecontract"),
        Track::Expression => preview_expression_fn(script, "nativeframecontract"),
        Track::Sound => preview_sound_fn(script, "nativeframecontract"),
        Track::Effect => preview_effect_structure_fn(script, "nativeframecontract"),
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
fn native_timing_parser_requires_exact_path_receiver_and_bounded_float() {
    for path in [
        "execute",
        "sv_animcmd::execute",
        "smash::app::sv_animcmd::execute",
    ] {
        for (token, value) in [
            ("0.0", 0.0_f32),
            ("-0.0", -0.0),
            ("7.25f32", 7.25),
            ("1e-9", 1e-9),
            ("1_000_000.0", 1_000_000.0),
        ] {
            let line = format!("{path}(agent.lua_state_agent, {token});");
            assert_eq!(
                parse_native_execute_frame_call(&line).unwrap().to_bits(),
                value.to_bits(),
                "{line}"
            );
        }
    }
    assert_eq!(
        parse_native_execute_frame_call(
            "execute (\n agent /* receiver */ . lua_state_agent,\n 7.0,\n);"
        ),
        Some(7.0)
    );
    for line in [
        "other::execute(agent.lua_state_agent, 7.0);",
        "execute::<f32>(agent.lua_state_agent, 7.0);",
        "agent.execute(agent.lua_state_agent, 7.0);",
        "execute!(agent.lua_state_agent, 7.0);",
        "execute(other.lua_state_agent, 7.0);",
        "execute(agent.module_accessor, 7.0);",
        "execute(lua_state, 7.0);",
        "execute(agent.lua_state_agent, 7);",
        "execute(agent.lua_state_agent, 7.0f64);",
        "execute(agent.lua_state_agent, -1.0);",
        "execute(agent.lua_state_agent, 1000001.0);",
        "execute(agent.lua_state_agent, f32::NAN);",
        "execute(agent.lua_state_agent, 1e100);",
        "execute(agent.lua_state_agent, 3.0 + 4.0);",
        "execute(agent.lua_state_agent, (7.0));",
        "execute(agent.lua_state_agent, 7.0 as f32);",
        "execute(agent.lua_state_agent, 7.0, false);",
        "#[cfg(any())] execute(agent.lua_state_agent, 7.0);",
        "execute(agent.lua_state_agent, 7.0); frame(agent.lua_state_agent, 9.0);",
        "let value = execute(agent.lua_state_agent, 7.0);",
        "// execute(agent.lua_state_agent, 7.0);",
    ] {
        assert!(parse_native_execute_frame_call(line).is_none(), "{line}");
    }
}

#[test]
fn native_timing_roundtrips_and_remains_source_only_on_three_tracks() {
    for track in [Track::Game, Track::Effect, Track::Expression] {
        let text = source(track, "    frame(agent.lua_state_agent, 3.0);\n    execute(agent.lua_state_agent, 7.5);\n    wait(agent.lua_state_agent, 2.0);");
        let original = parse(track, &text);
        assert!(matches!(
            original.stmts.as_slice(),
            [
                AcmdStmt::Frame(3.0),
                AcmdStmt::NativeExecuteFrame(7.5),
                AcmdStmt::Wait(2.0)
            ]
        ));
        let imported = import(track, &text);
        assert!(imported.baseline.is_empty());
        assert!(matches!(
            imported.nodes[1].kind,
            NodeKind::NativeExecuteFrame { frame: 7.5 }
        ));
        let mut recipe = MoveGraph {
            tracks: vec![imported],
            ..Default::default()
        };
        let serialized = serde_json::to_value(&recipe).unwrap();
        let restored: MoveGraph = serde_json::from_value(serialized.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), serialized);
        let compiled = timeline::compile("mario", "nativeframecontract", &restored).unwrap();
        let emitted = emit(track, &compiled[0].1);
        assert!(syn::parse_file(&emitted).is_ok(), "{emitted}");
        assert_eq!(emitted.matches("sv_animcmd::execute(").count(), 1);
        assert!(
            !emitted.contains("is_excute"),
            "no Execute wrapper may be invented"
        );
        assert_eq!(
            serde_json::to_value(parse(track, &emitted)).unwrap(),
            serde_json::to_value(original).unwrap()
        );
        assert_eq!(
            crate::move_nodes::live_support::source_only_tracks(&recipe).collect::<Vec<_>>(),
            vec![track]
        );
        let summary = crate::move_nodes::coverage::track_summary(&recipe.tracks[0]);
        assert_eq!(summary.node_counts.native_execute_frame, 1);
        assert_eq!(summary.editable_command_count, 1);
        assert_eq!(summary.preserved_block_count, 0);
        // The native frame remains editable without changing either neighboring wait.
        recipe.tracks[0].nodes[1].kind = NodeKind::NativeExecuteFrame { frame: 9.25 };
        let edited = timeline::compile("mario", "nativeframecontract", &recipe).unwrap();
        assert!(matches!(
            edited[0].1.stmts.as_slice(),
            [
                AcmdStmt::Frame(3.0),
                AcmdStmt::NativeExecuteFrame(9.25),
                AcmdStmt::Wait(2.0)
            ]
        ));
    }
}

#[test]
fn unsupported_native_timing_scope_is_preserved_without_flattening() {
    let call = "execute(agent.lua_state_agent, 7.0);";
    for track in [Track::Game, Track::Effect, Track::Expression] {
        for body in [
            format!("    if macros::is_excute(agent) {{\n        {call}\n    }}"),
            format!("    for _ in 0..2 {{\n        {call}\n    }}"),
            format!("    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD) {{\n        {call}\n    }}"),
            format!("    unsafe {{\n        {call}\n    }}"),
            format!("    #[cfg(any())]\n    {call}"),
            format!("    #[doc = \"scope\"] {call}"),
            format!("    {call} frame(agent.lua_state_agent, 9.0);"),
            format!("    frame(agent.lua_state_agent, 9.0); {call}"),
        ] {
            let text = source(track, &body);
            let parsed = parse(track, &text);
            assert!(!serde_json::to_string(&parsed).unwrap().contains("NativeExecuteFrame"), "{text}");
            let imported = import(track, &text);
            let recipe = MoveGraph { tracks: vec![imported], ..Default::default() };
            let compiled = timeline::compile("mario", "nativeframecontract", &recipe).unwrap();
            let emitted = emit(track, &compiled[0].1);
            assert!(emitted.contains(call), "native statement was dropped: {emitted}");
            if body.contains("9.0") { assert!(emitted.contains("frame(agent.lua_state_agent, 9.0);")); }
            assert_eq!(serde_json::to_value(parse(track, &emitted)).unwrap(), serde_json::to_value(parsed).unwrap(), "{text}\n{emitted}");
        }
    }
    let sound = source(Track::Sound, &format!("    {call}"));
    assert!(!serde_json::to_string(&parse(Track::Sound, &sound))
        .unwrap()
        .contains("NativeExecuteFrame"));
}

#[test]
fn native_timing_rejects_forged_tracks_nesting_and_numeric_values() {
    let timing = || MoveNode {
        id: 2,
        kind: NodeKind::NativeExecuteFrame { frame: 7.0 },
    };
    assert!(timeline::compile(
        "mario",
        "nativeframecontract",
        &graph(Track::Sound, vec![timing()])
    )
    .is_err());
    for track in [Track::Game, Track::Effect, Track::Expression] {
        for kind in [
            NodeKind::Repeat {
                motion_loop: crate::data::MotionLoop::No,
                count: 2,
                body: vec![timing()],
            },
            NodeKind::Branch {
                condition: crate::move_nodes::model::MoveCondition::Grounded,
                then_nodes: vec![timing()],
                else_nodes: vec![],
            },
        ] {
            assert!(timeline::compile(
                "mario",
                "nativeframecontract",
                &graph(track, vec![MoveNode { id: 1, kind }])
            )
            .is_err());
        }
        for frame in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -1.0,
            1_000_001.0,
        ] {
            assert!(timeline::compile(
                "mario",
                "nativeframecontract",
                &graph(
                    track,
                    vec![MoveNode {
                        id: 1,
                        kind: NodeKind::NativeExecuteFrame { frame }
                    }]
                )
            )
            .is_err());
        }
        for frame in [0.0, -0.0, 1e-9, 1_000_000.0] {
            let recipe = graph(
                track,
                vec![MoveNode {
                    id: 1,
                    kind: NodeKind::NativeExecuteFrame { frame },
                }],
            );
            let compiled = timeline::compile("mario", "nativeframecontract", &recipe).unwrap();
            let text = emit(track, &compiled[0].1);
            assert!(
                matches!(parse(track, &text).stmts.as_slice(), [AcmdStmt::NativeExecuteFrame(value)] if value.to_bits() == frame.to_bits())
            );
        }
    }
}

#[test]
fn native_timing_does_not_reinterpret_local_bindings_or_helper_names() {
    let call = "execute(agent.lua_state_agent, 7.0);";
    for track in [Track::Game, Track::Effect, Track::Expression] {
        for binding in [
            "fn execute(_: u64, _: f32) {}",
            "let execute = |_: u64, _: f32| {};",
            "let (execute, other) = (callback, 0);",
            "let agent = other;",
            "mod sv_animcmd {}",
            "mod smash {}",
            "use other::execute;",
            "use other::{execute, remaining};",
            "use other::callback as execute;",
            "use other as smash;",
            "use other::*;",
            "unsafe extern \"C\" { fn execute(state: u64, frame: f32); }",
        ] {
            let text = source(track, &format!("    {binding}\n    {call}"));
            assert!(
                !serde_json::to_string(&parse(track, &text))
                    .unwrap()
                    .contains("NativeExecuteFrame"),
                "{text}"
            );
        }
        let text = source(track, &format!("    fn helper(agent: &mut L2CAgentBase) {{ let execute = 0; }}\n    let ordinary = 0;\n    {call}"));
        assert!(
            matches!(
                parse(track, &text).stmts.last(),
                Some(AcmdStmt::NativeExecuteFrame(7.0))
            ),
            "a helper's private scope cannot shadow the ACMD receiver: {text}"
        );
    }
}

#[test]
fn native_timing_has_a_distinct_flow_kind_and_cannot_be_flat_effect_residue() {
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
                kind: NodeKind::NativeExecuteFrame { frame: 7.0 },
            },
        ],
    );
    let projection = crate::move_nodes::flow::project(&recipe);
    assert_eq!(
        projection.nodes[2].kind,
        crate::move_nodes::flow::FlowNodeKind::NativeExecuteFrame
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
    timeline::compile("mario", "nativeframecontract", &recipe).unwrap();
    let text = source(Track::Effect, "    execute(agent.lua_state_agent, 7.0);");
    let parsed = parse_effect_script(&text);
    let (calls, residue) = parsed.to_effect_calls_and_residue();
    assert!(calls.is_empty() && residue.is_empty());
    assert_eq!(
        unexportable_effect_lines(&parsed),
        ["smash::app::sv_animcmd::execute(agent.lua_state_agent, 7.0);"]
    );
}
