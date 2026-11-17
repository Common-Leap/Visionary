//! Export preflight and project-scope coverage for move graph materialization.
use crate::move_nodes::test_support as fixture;
use crate::move_nodes::test_support::assert_json_eq;

use std::path::{Path, PathBuf};

use crate::data::{AcmdScript, AcmdStmt, EditRecord, EffectMacro, ExcuteStmt, Hitbox};
use crate::gimmicks::behavior::{Action, ConstValue, Expr, VarType};
use crate::gimmicks::hooks::HookName;
use crate::mod_export::{plan_normal_export, source_project};
use crate::mod_project::{
    scope_acmd_project, AcmdProjectScope, LiveTweak, ModProjectFile, MoveSourceSnapshot,
    PROJECT_VERSION,
};

use super::model::{
    InputButton, MoveCondition, MoveGraph, MoveNode, NativeComparison, NativeIntOperand, NodeKind,
    Track,
};

fn move_graph(track: Track, nodes: Vec<MoveNode>) -> MoveGraph {
    MoveGraph {
        version: super::model::SCHEMA_VERSION,
        tracks: vec![fixture::track(track, nodes)],
        ..MoveGraph::default()
    }
}

fn frame_node(id: u32, frame: f32) -> MoveNode {
    fixture::frame(id, frame)
}

fn wait_node(id: u32, frames: f32) -> MoveNode {
    fixture::wait(id, frames)
}

fn representative_lr_sign_condition() -> MoveCondition {
    MoveCondition::Not(Box::new(MoveCondition::Group(Box::new(
        MoveCondition::And {
            left: Box::new(MoveCondition::NativeLrNegative),
            right: Box::new(MoveCondition::Grounded),
        },
    ))))
}

fn live_tweak(effect_name: &str) -> LiveTweak {
    LiveTweak {
        effect_name: effect_name.into(),
        color: Some([1.1, 1.0, 1.0, 1.0]),
        speed: None,
    }
}

fn project_with_move_graphs(graphs: &[(&str, MoveGraph)]) -> ModProjectFile {
    let mut project = ModProjectFile {
        version: PROJECT_VERSION,
        name: "move_graph_export".into(),
        ..Default::default()
    };
    let fighter = project.fighters.entry("mario".into()).or_default();
    fighter.display = "Mario".into();
    for (name, graph) in graphs {
        fighter.move_graphs.insert((*name).into(), graph.clone());
    }
    project
}

fn source_text(source: &crate::mod_export::GeneratedSource) -> String {
    source
        .project
        .files
        .iter()
        .map(|file| file.contents.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn generated_attack_fn_has_nested_else_branch(source: &str) -> bool {
    let Ok(file) = syn::parse_file(source) else {
        return false;
    };
    file.items.iter().any(|item| {
        let syn::Item::Fn(function) = item else {
            return false;
        };
        if function.sig.ident != "game_attack11_costume" {
            return false;
        }
        function.block.stmts.iter().any(|statement| {
            let syn::Stmt::Expr(syn::Expr::If(outer), _) = statement else {
                return false;
            };
            let Some((_, else_branch)) = &outer.else_branch else {
                return false;
            };
            let syn::Expr::Block(else_block) = else_branch.as_ref() else {
                return false;
            };
            else_block
                .block
                .stmts
                .iter()
                .any(|statement| matches!(statement, syn::Stmt::Expr(syn::Expr::If(_), _)))
        })
    })
}

fn generated_effect_fn_has_else_branch(source: &str) -> bool {
    let Ok(file) = syn::parse_file(source) else {
        return false;
    };
    file.items.iter().any(|item| {
        let syn::Item::Fn(function) = item else {
            return false;
        };
        if function.sig.ident != "effect_attack11" {
            return false;
        }
        function.block.stmts.iter().any(|statement| {
            matches!(statement, syn::Stmt::Expr(syn::Expr::If(branch), _) if branch.else_branch.is_some())
        })
    })
}

fn simple_record(move_name: &str, script: AcmdScript) -> EditRecord {
    EditRecord {
        fighter: "mario".into(),
        fighter_display: "Mario".into(),
        move_name: move_name.into(),
        script,
        hitboxes_pristine: Vec::new(),
        hitboxes: Vec::new(),
        slot_scope: None,
        slot_scopes: Vec::new(),
    }
}

fn effect_spawn(name: &str) -> EffectMacro {
    EffectMacro::Effect {
        effect_name: name.into(),
        effect_name_alt: None,
        spawn_func: "EFFECT_FOLLOW".into(),
        bone_name: "top".into(),
        offset: [0.0, 1.0, 0.0],
        rotation: [0.0; 3],
        scale: 1.0,
        follows_bone: true,
        extra_args: Vec::new(),
        flip_axis: None,
    }
}

const CONTRACT_CATEGORY_ORIGINALS: &str = "unsafe extern \"C\" fn game_attack11(agent: &mut L2CAgentBase) {\n    // original game category\n    frame(agent.lua_state_agent, 2.0);\n}\n\
unsafe extern \"C\" fn effect_attack11(agent: &mut L2CAgentBase) {\n    // original effect category\n    frame(agent.lua_state_agent, 2.0);\n}\n\
unsafe extern \"C\" fn sound_attack11(agent: &mut L2CAgentBase) {\n    // original sound category\n    frame(agent.lua_state_agent, 2.0);\n}\n\
unsafe extern \"C\" fn expression_attack11(agent: &mut L2CAgentBase) {\n    // original expression category\n    frame(agent.lua_state_agent, 2.0);\n}\n";

fn assert_costume_category(acmd_source: &str, category: &str, move_name: &str) {
    let registered = format!("{category}_{move_name}");
    let authored = format!("{registered}_costume");
    let original = format!("{registered}_original");
    let function_marker = format!("unsafe extern \"C\" fn {registered}(");
    let start = acmd_source
        .find(&function_marker)
        .unwrap_or_else(|| panic!("missing dispatcher {registered}:\n{acmd_source}"));
    let dispatcher = crate::acmd::extract_function(&acmd_source[start..], &format!("{category}_"))
        .unwrap_or_else(|| panic!("could not extract dispatcher {registered}"));
    assert!(
        dispatcher.contains(&format!("{authored}(agent);")),
        "{dispatcher}"
    );
    assert!(
        dispatcher.contains(&format!("{original}(agent);")),
        "{dispatcher}"
    );
    assert!(acmd_source.contains(&format!("unsafe extern \"C\" fn {authored}(")));
    assert!(acmd_source.contains(&format!("unsafe extern \"C\" fn {original}(")));
    assert!(
        acmd_source.contains(&format!("agent.acmd(\"{registered}\", {registered},")),
        "the registered script must dispatch through its costume gate: {registered}"
    );
}

#[test]
fn authored_clear_and_reverse_are_registered_in_generated_source() {
    let graph = move_graph(
        Track::Game,
        vec![
            frame_node(1, 3.0),
            fixture::execute(2, vec![ExcuteStmt::ClearAll, ExcuteStmt::ReverseLr]),
        ],
    );
    let project = project_with_move_graphs(&[("attack11", graph)]);

    let generated = source_project(&project)
        .unwrap()
        .expect("authored game track exports");
    let text = source_text(&generated);

    assert!(
        text.contains("game_attack11"),
        "missing registered move:\n{text}"
    );
    assert!(
        text.contains("AttackModule::clear_all"),
        "missing authored clear:\n{text}"
    );
    assert!(
        text.contains("REVERSE_LR"),
        "missing authored reverse:\n{text}"
    );
}

#[test]
fn conflicting_legacy_script_is_rejected_by_both_source_and_normal_preflight() {
    let graph = move_graph(Track::Game, vec![frame_node(1, 4.0)]);
    let mut project = project_with_move_graphs(&[("attack11", graph)]);
    project.fighters.get_mut("mario").unwrap().acmd.insert(
        "attack11".into(),
        simple_record("attack11", AcmdScript::default()),
    );

    let source_error = match source_project(&project) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("conflicting legacy edit unexpectedly exported"),
    };
    let plan_error = match plan_normal_export(&project, None) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("conflicting legacy edit unexpectedly passed normal preflight"),
    };

    assert!(
        source_error.contains("differs from the compiled move graph"),
        "{source_error}"
    );
    assert!(
        plan_error.contains("differs from the compiled move graph"),
        "{plan_error}"
    );
}

#[test]
fn whole_project_and_move_scopes_keep_only_their_saved_graphs() {
    let attack11 = move_graph(Track::Game, vec![frame_node(1, 2.0)]);
    let attack12 = move_graph(Track::Game, vec![frame_node(2, 5.0)]);
    let project = project_with_move_graphs(&[("attack11", attack11), ("attack12", attack12)]);

    let all = scope_acmd_project(&project, AcmdProjectScope::All);
    let one = scope_acmd_project(
        &project,
        AcmdProjectScope::Move {
            fighter: "mario",
            move_name: "attack11",
        },
    );
    assert_eq!(all.fighters["mario"].move_graphs.len(), 2);
    assert_eq!(
        one.fighters["mario"]
            .move_graphs
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["attack11"]
    );

    let all_source = source_project(&all)
        .unwrap()
        .expect("whole project should export");
    let one_source = source_project(&one)
        .unwrap()
        .expect("one move should export");
    assert!(source_text(&all_source).contains("game_attack11"));
    assert!(source_text(&all_source).contains("game_attack12"));
    assert!(source_text(&one_source).contains("game_attack11"));
    assert!(!source_text(&one_source).contains("game_attack12"));
}

#[test]
fn all_scope_preserves_global_tweak_context_for_structured_effects() {
    let mut project = project_with_move_graphs(&[]);
    project
        .fighters
        .get_mut("mario")
        .unwrap()
        .effect_scripts
        .insert("attack11".into(), AcmdScript::default());
    project
        .fighters
        .entry("luigi".into())
        .or_default()
        .live_tweaks
        .push(live_tweak("sys_attack_flash"));

    let scoped = scope_acmd_project(&project, AcmdProjectScope::All);
    assert_eq!(
        scoped.fighters["luigi"].live_tweaks[0].effect_name,
        "sys_attack_flash"
    );
    let error = match source_project(&scoped) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("all-scope export must refuse structured effects with global tweaks"),
    };
    assert!(
        error.contains(
            "structured effect tracks cannot be combined with global live color/speed overrides"
        ),
        "{error}"
    );
}

#[test]
fn move_scope_preserves_global_tweaks_for_effect_graphs_across_fighters() {
    let mut project =
        project_with_move_graphs(&[("attack11", move_graph(Track::Effect, Vec::new()))]);
    project
        .fighters
        .entry("luigi".into())
        .or_default()
        .live_tweaks
        .push(live_tweak("sys_attack_flash"));

    let scoped = scope_acmd_project(
        &project,
        AcmdProjectScope::Move {
            fighter: "mario",
            move_name: "attack11",
        },
    );
    assert_eq!(scoped.fighters.len(), 1);
    assert_eq!(
        scoped.fighters["mario"].live_tweaks[0].effect_name,
        "sys_attack_flash"
    );
    let error = match source_project(&scoped) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("move-scope export must refuse structured effects with global tweaks"),
    };
    assert!(
        error.contains(
            "structured effect tracks cannot be combined with global live color/speed overrides"
        ),
        "{error}"
    );
}

#[test]
fn version_three_project_migrates_and_persists_graphs_at_current_version() {
    let graph = move_graph(Track::Game, vec![frame_node(7, 6.0)]);
    let mut project = project_with_move_graphs(&[("attack11", graph.clone())]);
    project.version = 3;
    let serialized = serde_json::to_string(&project).unwrap();
    let mut reloaded: ModProjectFile = serde_json::from_str(&serialized).unwrap();

    reloaded.migrate().unwrap();
    let persisted = serde_json::to_string(&reloaded).unwrap();
    let round_trip: ModProjectFile = serde_json::from_str(&persisted).unwrap();

    assert_eq!(round_trip.version, PROJECT_VERSION);
    let original = serde_json::to_value(graph).unwrap();
    let restored =
        serde_json::to_value(&round_trip.fighters["mario"].move_graphs["attack11"]).unwrap();
    assert_eq!(restored, original);
}

#[test]
fn future_graph_schema_is_rejected_before_any_source_is_returned() {
    let mut graph = move_graph(Track::Game, vec![frame_node(1, 2.0)]);
    graph.version += 100;
    let project = project_with_move_graphs(&[("attack11", graph)]);

    let error = match source_project(&project) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("future graph schema unexpectedly produced a source project"),
    };

    assert!(error.contains("unsupported schema"), "{error}");
}

fn representative_contract_project() -> ModProjectFile {
    use crate::gimmicks::native_inputs::{FighterValue, NativeInput};

    let branch = |id, condition, then_nodes, else_nodes| {
        fixture::node(
            id,
            NodeKind::Branch {
                condition,
                then_nodes,
                else_nodes,
            },
        )
    };
    let mut cloud = move_graph(
        Track::Game,
        vec![
            frame_node(1, 3.0),
            branch(
                2,
                MoveCondition::ButtonHeld {
                    button: InputButton::Special,
                },
                vec![fixture::execute(
                    3,
                    vec![ExcuteStmt::Attack(Hitbox::default().to_attack_call())],
                )],
                vec![],
            ),
            wait_node(4, 2.0),
            fixture::node(
                5,
                NodeKind::Repeat {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 2,
                    body: vec![
                        frame_node(6, 8.0),
                        fixture::execute(7, vec![ExcuteStmt::ClearAll]),
                    ],
                },
            ),
            branch(
                8,
                MoveCondition::Grounded,
                vec![frame_node(9, 12.0)],
                vec![branch(
                    10,
                    MoveCondition::Airborne,
                    vec![fixture::execute(11, vec![ExcuteStmt::ReverseLr])],
                    vec![],
                )],
            ),
            branch(
                12,
                MoveCondition::WorkFlag {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK".into(),
                    expected: true,
                },
                vec![frame_node(13, 16.0)],
                vec![],
            ),
            fixture::execute(
                14,
                (18..=40)
                    .chain(42..=49)
                    .chain([
                        52, 53, 64, 65, 66, 67, 68, 69, 72, 73, 77, 78, 79, 90, 100, 102, 103, 104,
                    ])
                    .map(|kind| super::view::context_command(kind, "cloud"))
                    .collect(),
            ),
            fixture::node(
                15,
                NodeKind::BareCommand {
                    command: Box::new(ExcuteStmt::CaptureCutCommon),
                },
            ),
            branch(
                16,
                representative_lr_sign_condition(),
                vec![frame_node(17, 17.0)],
                vec![],
            ),
            branch(
                18,
                MoveCondition::WorkInt {
                    symbol: "FIGHTER_INSTANCE_WORK_ID_INT_KIND".into(),
                    op: NativeComparison::Ne,
                    value: NativeIntOperand::FighterKind("kirby".into()),
                },
                vec![frame_node(19, 18.0)],
                vec![],
            ),
            branch(
                20,
                MoveCondition::WorkFloat {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                    op: NativeComparison::Gte,
                    value: 100.0,
                },
                vec![frame_node(21, 19.0)],
                vec![],
            ),
        ],
    );
    let effects = (7..=10)
        .chain(54..=62)
        .map(super::view::default_effect)
        .chain(crate::acmd::EFFECT_SPAWN_LAYOUTS.iter().map(|layout| {
            let mut command = super::view::default_effect(7);
            assert!(super::view::set_effect_spawn_variant(
                &mut command,
                layout.name
            ));
            command
        }))
        .chain(
            crate::acmd::parse_effect_script(crate::acmd::EFFECT_COMPATIBILITY_CORPUS_FIXTURE)
                .stmts
                .into_iter()
                .flat_map(|stmt| match stmt {
                    crate::data::EffectStmt::Excute(commands) => commands,
                    _ => panic!("expected an execute group"),
                }),
        )
        .chain(
            crate::data::COLOR_COMMANDS
                .iter()
                .map(|(command, transition, rgba)| EffectMacro::Color {
                    command: (*command).into(),
                    color: crate::data::ColorCall {
                        transition: transition.then_some(1.0),
                        rgba: rgba.then_some([1.0; 4]),
                    },
                }),
        )
        .collect();
    cloud.tracks.push(fixture::track(
        Track::Effect,
        vec![
            fixture::effect_execute(100, effects),
            branch(
                101,
                MoveCondition::ButtonHeld {
                    button: InputButton::Special,
                },
                vec![fixture::effect_execute(
                    102,
                    vec![
                        effect_spawn("sys_attack_line"),
                        EffectMacro::LastEffectSetColor {
                            rgb: [0.25, 0.5, 1.0],
                        },
                        EffectMacro::LastEffectSetRate { rate: 0.75 },
                    ],
                )],
                vec![fixture::effect_execute(
                    103,
                    vec![effect_spawn("sys_attack_impact")],
                )],
            ),
        ],
    ));
    cloud.tracks.push(fixture::track(
        Track::Sound,
        vec![fixture::execute(
            200,
            vec![super::view::default_command(11)],
        )],
    ));
    cloud.tracks.push(fixture::track(
        Track::Expression,
        vec![fixture::execute(
            300,
            [13, 14, 17, 20, 30, 28, 32, 34, 35, 36, 38, 51, 77, 79, 102]
                .into_iter()
                .map(|kind| super::view::context_command(kind, "cloud"))
                .collect(),
        )],
    ));
    let compiled = super::timeline::compile("cloud", "attack11", &cloud).unwrap();
    let mut project = ModProjectFile {
        name: "move_node_contract".into(),
        ..Default::default()
    };
    let fighter = project.fighters.entry("cloud".into()).or_default();
    fighter.display = "Cloud".into();
    fighter.behavior = fixture::controller_event("unused", vec![]);
    fighter.behavior.emission_plan.event_bindings.clear();
    fighter.behavior.vars = vec![
        fixture::var("shared_timer".into(), VarType::Timer, ConstValue::Int(0)),
        fixture::var("shared_counter".into(), VarType::Int, ConstValue::Int(0)),
    ];
    fighter.behavior.transitions[0].actions = vec![fixture::set(
        "shared_timer".into(),
        Expr::Const(ConstValue::Int(3)),
    )];
    fighter.behavior.emission_plan.bindings = vec![crate::gimmicks::codegen::HookBinding {
        hook: HookName::OnStockStart,
        article: None,
        transitions: vec![0],
    }];
    fighter.behavior.vars.push(fixture::var(
        "armed".into(),
        VarType::Flag,
        ConstValue::Flag(false),
    ));
    fighter.behavior.transitions[0].actions.push(Action::Raise {
        var: "armed".into(),
    });
    fighter
        .behavior
        .transitions
        .push(crate::gimmicks::behavior::Transition {
            from: "active".into(),
            to: "active".into(),
            on_frame: true,
            guard: crate::gimmicks::behavior::Predicate::And(
                Box::new(crate::gimmicks::behavior::Predicate::Expired(
                    "shared_timer".into(),
                )),
                Box::new(crate::gimmicks::behavior::Predicate::Flag("armed".into())),
            ),
            actions: vec![
                Action::Add {
                    var: "shared_counter".into(),
                    value: Expr::TimerRemaining("shared_timer".into()),
                },
                Action::Clear {
                    var: "armed".into(),
                },
            ],
        });
    fighter
        .behavior
        .emission_plan
        .bindings
        .push(crate::gimmicks::codegen::HookBinding {
            hook: HookName::OnChargeTick,
            article: None,
            transitions: vec![1],
        });
    let mut record = simple_record("attack11", compiled[0].1.clone());
    record.fighter = "cloud".into();
    record.fighter_display = "Cloud".into();
    record.slot_scopes = vec![8, 9];
    fighter.acmd.insert("attack11".into(), record);
    fighter.move_sources.insert(
        "attack11".into(),
        MoveSourceSnapshot {
            body: CONTRACT_CATEGORY_ORIGINALS.into(),
            captures: vec![],
        },
    );
    fighter.move_graphs.insert("attack11".into(), cloud);

    // Each row exercises a different native command API, rather than every catalog tuple.
    for (owner, name, kinds) in [
        ("kirby", "articlemotion", vec![41]),
        ("robot", "finishcamera", vec![93]),
        ("eflame", "finalcamera", vec![94]),
        (
            crate::data::FINAL_FEAR_FACE_PRESETS[0].fighter,
            "fearface",
            vec![95],
        ),
        ("gamewatch", "modeldefault", vec![97]),
        ("koopajr", "generateenabled", vec![98]),
        ("samus", "articlestate", vec![99]),
        ("miiswordsman", "waveangle", vec![70]),
        ("elight", "openmotion", vec![71]),
    ] {
        let commands = kinds
            .into_iter()
            .map(|kind| super::view::context_command(kind, owner))
            .collect();
        project
            .fighters
            .entry(owner.into())
            .or_default()
            .move_graphs
            .insert(
                name.into(),
                move_graph(Track::Game, vec![fixture::execute(1, commands)]),
            );
    }
    let preset = crate::data::ArticlePartialMotionPreset::ALL[0];
    let sword = move_graph(
        Track::Game,
        vec![branch(
            1,
            MoveCondition::ArticleExists {
                symbol: preset.article_symbol().into(),
            },
            vec![fixture::execute(
                2,
                vec![ExcuteStmt::ArticleLifecycle(
                    crate::data::ArticleLifecycleCall::AddMotionPartial { preset },
                )],
            )],
            vec![],
        )],
    );
    project
        .fighters
        .entry(preset.fighter().into())
        .or_default()
        .move_graphs
        .insert("swordpartial".into(), sword);
    for (owner, source) in [
        ("mario", "EffectModule::set_disable_render_offset_last(agent.module_accessor);\nEffectModule::is_exist_effect(agent.module_accessor, 0);".to_owned()),
        ("kirby", super::trails::TRAIL4_SOURCE_FIXTURE.to_owned()),
        (crate::data::EffectFollowWorkPreset::all()[0].fighter,
            format!("macros::EFFECT_FOLLOW_WORK(agent, *{}, Hash40::new(\"top\"), 0, 1, 2, 3, 4, 5, 1, true);",
                crate::data::EffectFollowWorkPreset::all()[0].work_symbol)),
    ] {
        let source = format!("unsafe extern \"C\" fn effect_native(agent: &mut L2CAgentBase) {{\nif macros::is_excute(agent) {{\n{source}\n}}\n}}");
        let track = super::effects::import_for_fighter(owner, &crate::acmd::parse_effect_script(&source));
        project.fighters.entry(owner.into()).or_default().move_graphs.insert("nativeeffect".into(),
            MoveGraph { tracks: vec![track], ..Default::default() });
    }
    for (owner, command) in [
        (
            "mario",
            EffectMacro::Control(crate::data::EffectControl::ReqScreen {
                preset_id: crate::data::EffectReqScreenPreset::all()[0].id.into(),
            }),
        ),
        (
            "mario",
            EffectMacro::Control(crate::data::EffectControl::SetDisableCameraDepthInfluence {
                value: true,
            }),
        ),
        (
            crate::data::EffectOffKindWorkPreset::all()[0].fighter,
            EffectMacro::Control(crate::data::EffectControl::OffKindWork {
                preset_id: crate::data::EffectOffKindWorkPreset::all()[0].id.into(),
            }),
        ),
    ]
    .into_iter()
    {
        let fighter = project.fighters.entry(owner.into()).or_default();
        let name = format!("effectcontrol{}", fighter.move_graphs.len());
        fighter.move_graphs.insert(
            name,
            move_graph(
                Track::Effect,
                vec![fixture::effect_execute(1, vec![command])],
            ),
        );
    }
    for (track, kind, name) in [
        (
            Track::Game,
            NodeKind::NativeExecuteFrame { frame: 7.5 },
            "nativeframe",
        ),
        (Track::Effect, NodeKind::NativeMotionLoopSync, "motionsync"),
    ] {
        project
            .fighters
            .entry("mario".into())
            .or_default()
            .move_graphs
            .insert(
                name.into(),
                move_graph(
                    track,
                    vec![
                        fixture::node(1, kind),
                        frame_node(2, 3.0),
                        wait_node(3, 2.0),
                    ],
                ),
            );
    }
    let event = |id| {
        fixture::execute(
            id,
            vec![ExcuteStmt::GimmickEvent {
                name: "timed_fireball".into(),
            }],
        )
    };
    let mario = project.fighters.entry("mario".into()).or_default();
    mario.display = "Mario".into();
    let graph = move_graph(
        Track::Game,
        vec![
            frame_node(1, 3.0),
            branch(
                2,
                MoveCondition::ButtonHeld {
                    button: InputButton::Special,
                },
                vec![event(3)],
                vec![event(4)],
            ),
            fixture::node(
                5,
                NodeKind::Repeat {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 100,
                    body: vec![event(6)],
                },
            ),
            frame_node(7, 20.0),
        ],
    );
    mario.behavior = fixture::controller_event(
        "timed_fireball",
        vec![Action::GenerateArticle {
            symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
        }],
    );
    mario
        .move_graphs
        .insert("conditionaleventcontract".into(), graph);
    mario.behavior.vars.push(fixture::var(
        "damage".into(),
        VarType::Float,
        ConstValue::Float(0.0),
    ));
    mario
        .behavior
        .transitions
        .push(crate::gimmicks::behavior::Transition {
            from: "active".into(),
            to: "active".into(),
            guard: crate::gimmicks::graph_view::always(),
            on_frame: true,
            actions: vec![fixture::set(
                "damage".into(),
                Expr::NativeFloat(NativeInput::Fighter {
                    value: FighterValue::Damage,
                    fighter: "mario".into(),
                }),
            )],
        });
    mario
        .behavior
        .emission_plan
        .bindings
        .push(crate::gimmicks::codegen::HookBinding {
            hook: HookName::OnChargeTick,
            article: None,
            transitions: vec![1],
        });
    project
}

fn clear_recipe_behavior(project: &mut ModProjectFile) {
    fn clear_saved_dispatch(statements: &mut Vec<AcmdStmt>) {
        for statement in statements.iter_mut() {
            match statement {
                AcmdStmt::Excute(commands) => {
                    commands.retain(|command| !matches!(command, ExcuteStmt::GimmickEvent { .. }));
                }
                AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. } => {
                    clear_saved_dispatch(body);
                }
                _ => {}
            }
        }
        statements.retain(
            |statement| !matches!(statement, AcmdStmt::Excute(commands) if commands.is_empty()),
        );
    }

    fn clear_dispatch(nodes: &mut Vec<MoveNode>) {
        for node in nodes.iter_mut() {
            match &mut node.kind {
                NodeKind::Execute { commands } => {
                    commands.retain(|command| !matches!(command, ExcuteStmt::GimmickEvent { .. }))
                }
                NodeKind::Repeat { body, .. } => clear_dispatch(body),
                NodeKind::Branch {
                    then_nodes,
                    else_nodes,
                    ..
                } => {
                    clear_dispatch(then_nodes);
                    clear_dispatch(else_nodes);
                }
                _ => {}
            }
        }
        nodes.retain(
            |node| !matches!(&node.kind, NodeKind::Execute { commands } if commands.is_empty()),
        );
    }
    for fighter in project.fighters.values_mut() {
        fighter.behavior = Default::default();
        // This source-only fixture deliberately removes character behavior. Keep its
        // saved category edit in agreement with the correspondingly stripped recipe.
        // The original combined-runtime fixture remains unchanged.
        for record in fighter.acmd.values_mut() {
            clear_saved_dispatch(&mut record.script.stmts);
        }
        for graph in fighter.move_graphs.values_mut() {
            for track in &mut graph.tracks {
                clear_dispatch(&mut track.nodes);
            }
        }
    }
}

#[test]
fn conditional_named_event_sdk_fixture_roundtrips_as_editable_control_flow() {
    let project = representative_contract_project();
    let expanded = super::project::expand(&project).unwrap();
    let script = &expanded.fighters["mario"].acmd["conditionaleventcontract"].script;
    let source = crate::acmd::preview_game_fn(script, "conditionaleventcontract");
    let parsed = crate::acmd::parse_acmd_script(&source);
    let imported = super::timeline::import_for_fighter("mario", Track::Game, &parsed);
    let sends_event = |nodes: &[MoveNode]| {
        matches!(nodes.first(), Some(MoveNode { kind: NodeKind::Execute { commands }, .. })
            if matches!(commands.as_slice(), [ExcuteStmt::GimmickEvent { name }] if name == "timed_fireball"))
    };
    assert!(matches!(imported.nodes.as_slice(),
        [MoveNode { kind: NodeKind::Frame { frame: 3.0 }, .. },
         MoveNode { kind: NodeKind::Branch { then_nodes, else_nodes, .. }, .. },
             MoveNode { kind: NodeKind::Repeat { count: 100, body, .. }, .. },
         MoveNode { kind: NodeKind::Frame { frame: 20.0 }, .. }]
        if sends_event(then_nodes) && sends_event(else_nodes) && sends_event(body)));
    let graph = MoveGraph {
        tracks: vec![imported.clone()],
        ..MoveGraph::default()
    };
    let compiled = super::timeline::compile("mario", "conditionaleventcontract", &graph)
        .unwrap()
        .remove(0)
        .1;
    let reparsed = crate::acmd::parse_acmd_script(&crate::acmd::preview_game_fn(
        &compiled,
        "conditionaleventcontract",
    ));
    let reimported = super::timeline::import_for_fighter("mario", Track::Game, &reparsed);
    assert_json_eq!(
        imported,
        reimported,
        "all three nested sends and their control flow must remain editable"
    );
}

#[test]
fn representative_nodes_render_for_skyline_contract_harness() {
    let project = representative_contract_project();
    let expanded = super::project::expand(&project).unwrap();
    for (owner, fighter) in &expanded.fighters {
        crate::gimmicks::live_protocol::program_from_graph(owner, &fighter.behavior).unwrap();
    }
    let effect_source = crate::acmd::preview_effect_structure_fn(
        &expanded.fighters["cloud"].effect_scripts["attack11"],
        "attack11",
    );
    assert!(generated_effect_fn_has_else_branch(&effect_source));
    let mut acmd_only = project.clone();
    clear_recipe_behavior(&mut acmd_only);
    let exported = source_project(&acmd_only).unwrap().unwrap();
    for file in &exported.project.files {
        if file.rel_path.ends_with(".rs") {
            syn::parse_file(&file.contents)
                .unwrap_or_else(|error| panic!("{}: {error}", file.rel_path));
        }
    }
    let cloud = exported
        .project
        .files
        .iter()
        .find(|file| file.rel_path == "src/cloud/acmd.rs")
        .unwrap();
    assert!(generated_attack_fn_has_nested_else_branch(&cloud.contents));
    for category in ["game", "effect", "sound", "expression"] {
        assert_costume_category(&cloud.contents, category, "attack11");
    }
    let Some(root) = std::env::var_os("VISIONARY_MOVE_NODE_CONTRACT_ROOT") else {
        return;
    };
    let fixture_root = PathBuf::from(root).canonicalize().unwrap();
    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    let cache_root = std::env::var_os("VISIONARY_BUILD_CACHE")
        .map(PathBuf::from)
        .or_else(|| dirs::cache_dir().map(|dir| dir.join("builds/visionary")))
        .unwrap()
        .canonicalize()
        .unwrap();
    assert!(!fixture_root.starts_with(&repository_root));
    assert!(fixture_root.starts_with(&cache_root));
    let plan = plan_normal_export(&project, Some(&fixture_root)).unwrap();
    assert!(plan.has_acmd_source && plan.generated_source.is_none());
    let runtime = plan.gimmick_runtime.unwrap();
    crate::gimmicks::adapter::export_combined_to_plugin(
        &fixture_root,
        runtime.artifact(),
        runtime.acmd_artifact(),
    )
    .unwrap();
    for (path, expected) in [
        (
            "src/slight/gimmicks/generated.rs",
            &runtime.artifact().source,
        ),
        (
            "src/slight/gimmicks/acmd_generated.rs",
            &runtime.acmd_artifact().source,
        ),
    ] {
        assert_eq!(
            &std::fs::read_to_string(fixture_root.join(path)).unwrap(),
            expected
        );
    }
}

#[test]
fn controller_only_requires_explicit_combined_plugin_selection() {
    let mut project = ModProjectFile::default();
    project.fighters.entry("mario".into()).or_default().behavior =
        fixture::controller_event("go", vec![]);
    let error = plan_normal_export(&project, None)
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("explicitly selected compatible plugin"),
        "{error}"
    );
    assert!(error.contains("mario (behavior)"), "{error}");
}
