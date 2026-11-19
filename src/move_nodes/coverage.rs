//! Deterministic accounting for a saved move track, shown in the node editor. Preserved source
//! blocks are counted without claiming they are fully interpreted.

use super::model::{MoveCommand, MoveNode, MoveTrack, NodeKind, Track};

/// Counts one saved track's node structure and authored command entries.
///
/// Nodes inside repeats are counted once as authored structure, regardless of the repeat count.
/// Both branch arms are counted because both are present in the saved recipe. These are source
/// structure counts, not predicted runtime execution totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackCoverage {
    pub track: Track,
    pub node_counts: NodeKindCounts,
    /// ACMD calls in editable execute blocks, measured bare-command nodes, and motion-rate nodes.
    pub editable_command_count: u64,
    /// Immutable `Preserved` nodes; their source is not treated as fully interpreted.
    pub preserved_block_count: u64,
    /// Commands retained as immutable source within mixed or wholly preserved execute blocks.
    pub preserved_command_count: u64,
    /// Read-only decompiler lines that cannot be real SDK calls; neither editable nor preserved.
    pub opaque_source_line_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeKindCounts {
    pub total: u64,
    pub frame: u64,
    pub native_execute_frame: u64,
    pub native_motion_loop_sync: u64,
    pub native_wait_loop_clear: u64,
    pub wait: u64,
    pub motion_rate: u64,
    pub bare_command: u64,
    pub execute: u64,
    pub mixed_execute: u64,
    pub effect_execute: u64,
    pub mixed_effect_execute: u64,
    pub repeat: u64,
    pub branch: u64,
    pub preserved: u64,
}

/// Summarize a track by recursively visiting all saved node content.
pub fn track_summary(track: &MoveTrack) -> TrackCoverage {
    let mut summary = TrackCoverage {
        track: track.track,
        ..TrackCoverage::default()
    };
    count_nodes(&track.nodes, &mut summary, false);
    summary
}

fn count_nodes(nodes: &[MoveNode], summary: &mut TrackCoverage, execute_context: bool) {
    for node in nodes {
        summary.node_counts.total = inc(summary.node_counts.total);
        match &node.kind {
            NodeKind::Frame { .. } => summary.node_counts.frame = inc(summary.node_counts.frame),
            NodeKind::NativeExecuteFrame { .. } => {
                summary.node_counts.native_execute_frame =
                    inc(summary.node_counts.native_execute_frame);
                summary.editable_command_count = inc(summary.editable_command_count);
            }
            NodeKind::NativeWaitLoopClear => {
                summary.node_counts.native_wait_loop_clear =
                    inc(summary.node_counts.native_wait_loop_clear);
                summary.editable_command_count = inc(summary.editable_command_count);
            }
            NodeKind::NativeMotionLoopSync => {
                summary.node_counts.native_motion_loop_sync =
                    inc(summary.node_counts.native_motion_loop_sync);
                summary.editable_command_count = inc(summary.editable_command_count);
            }
            NodeKind::Wait { .. } => summary.node_counts.wait = inc(summary.node_counts.wait),
            NodeKind::MotionRate { .. } => {
                summary.node_counts.motion_rate = inc(summary.node_counts.motion_rate);
                summary.editable_command_count = inc(summary.editable_command_count);
            }
            NodeKind::BareCommand { command } => {
                summary.node_counts.bare_command = inc(summary.node_counts.bare_command);
                count_command(summary, command_is_editable(command));
            }
            NodeKind::Execute { commands } => {
                summary.node_counts.execute = inc(summary.node_counts.execute);
                for command in commands {
                    count_command(summary, command_is_editable(command));
                }
            }
            NodeKind::MixedExecute { commands } => {
                summary.node_counts.mixed_execute = inc(summary.node_counts.mixed_execute);
                for command in commands {
                    match command {
                        MoveCommand::Editable(command) => {
                            count_command(summary, command_is_editable(command));
                        }
                        MoveCommand::Preserved(_) => {
                            summary.preserved_command_count = inc(summary.preserved_command_count);
                        }
                    }
                }
            }
            NodeKind::EffectExecute { commands } => {
                summary.node_counts.effect_execute = inc(summary.node_counts.effect_execute);
                for command in commands {
                    count_command(summary, effect_command_is_editable(command));
                }
            }
            NodeKind::MixedEffectExecute { commands, .. } => {
                summary.node_counts.mixed_effect_execute =
                    inc(summary.node_counts.mixed_effect_execute);
                for command in commands {
                    match command {
                        super::model::EffectCommandSlot::Editable(command) => {
                            count_command(summary, effect_command_is_editable(command));
                        }
                        super::model::EffectCommandSlot::Preserved(_) => {
                            summary.preserved_command_count = inc(summary.preserved_command_count);
                        }
                    }
                }
            }
            NodeKind::Repeat {
                body, motion_loop, ..
            } => {
                summary.node_counts.repeat = inc(summary.node_counts.repeat);
                // An every-motion-loop repeat carries the native wait that ends each cycle.
                match motion_loop {
                    crate::data::MotionLoop::No => {}
                    crate::data::MotionLoop::Clear => {
                        summary.node_counts.native_wait_loop_clear =
                            inc(summary.node_counts.native_wait_loop_clear);
                        summary.editable_command_count = inc(summary.editable_command_count);
                    }
                    crate::data::MotionLoop::SyncMotion => {
                        summary.node_counts.native_motion_loop_sync =
                            inc(summary.node_counts.native_motion_loop_sync);
                        summary.editable_command_count = inc(summary.editable_command_count);
                    }
                }
                count_nodes(body, summary, execute_context);
            }
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                condition,
            } => {
                summary.node_counts.branch = inc(summary.node_counts.branch);
                count_nodes(
                    then_nodes,
                    summary,
                    execute_context || matches!(condition, super::model::MoveCondition::IsExecute),
                );
                count_nodes(else_nodes, summary, execute_context);
            }
            NodeKind::Preserved { statement } => {
                summary.node_counts.preserved = inc(summary.node_counts.preserved);
                summary.preserved_block_count = inc(summary.preserved_block_count);
                if let crate::data::AcmdStmt::Excute(commands) = statement {
                    summary.preserved_command_count =
                        add(summary.preserved_command_count, count(commands.len()));
                } else if execute_context && matches!(statement, crate::data::AcmdStmt::Bare(_)) {
                    summary.preserved_command_count = inc(summary.preserved_command_count);
                }
            }
        }
    }
}

fn count_command(summary: &mut TrackCoverage, editable: bool) {
    if editable {
        summary.editable_command_count = inc(summary.editable_command_count);
    } else {
        summary.opaque_source_line_count = inc(summary.opaque_source_line_count);
    }
}

// Source lines are typed transport for unresolved decompiler output, not editable calls.
fn command_is_editable(command: &crate::data::ExcuteStmt) -> bool {
    if let crate::data::ExcuteStmt::Effect(command) = command {
        return effect_command_is_editable(command);
    }
    !matches!(command, crate::data::ExcuteStmt::NativeCall(call) if call.source_line.is_some())
}

fn effect_command_is_editable(command: &crate::data::EffectMacro) -> bool {
    !matches!(command, crate::data::EffectMacro::NativeCall(call) if call.source_line.is_some())
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn inc(value: u64) -> u64 {
    value.saturating_add(1)
}

fn add(left: u64, right: u64) -> u64 {
    left.saturating_add(right)
}

#[cfg(test)]
mod tests {
    use crate::data::{AcmdStmt, ExcuteStmt};

    use super::*;
    use crate::move_nodes::model::{MoveCondition, MoveNode, NodeKind};

    fn node(id: u32, kind: NodeKind) -> MoveNode {
        MoveNode { id, kind }
    }

    #[test]
    fn recursively_counts_typed_commands_and_preserved_blocks() {
        let preserved = AcmdStmt::RawBlock {
            header: "if unknown_condition() {".into(),
            body: vec![AcmdStmt::Excute(vec![ExcuteStmt::ClearAll])],
        };
        let track = MoveTrack {
            track: Track::Game,
            effect_execute_baseline: Vec::new(),
            nodes: vec![
                node(1, NodeKind::Frame { frame: 3.0 }),
                node(
                    2,
                    NodeKind::Repeat {
                        motion_loop: crate::data::MotionLoop::No,
                        count: 4,
                        body: vec![node(
                            3,
                            NodeKind::Branch {
                                condition: MoveCondition::Grounded,
                                then_nodes: vec![
                                    node(
                                        4,
                                        NodeKind::Execute {
                                            commands: vec![ExcuteStmt::Clear(0)],
                                        },
                                    ),
                                    node(
                                        5,
                                        NodeKind::Preserved {
                                            statement: preserved.clone(),
                                        },
                                    ),
                                ],
                                else_nodes: vec![node(
                                    6,
                                    NodeKind::Execute {
                                        commands: vec![ExcuteStmt::ClearAll],
                                    },
                                )],
                            },
                        )],
                    },
                ),
            ],
            baseline: vec![preserved],
        };

        let summary = track_summary(&track);
        assert_eq!(summary.node_counts.total, 6);
        assert_eq!(summary.node_counts.frame, 1);
        assert_eq!(summary.node_counts.repeat, 1);
        assert_eq!(summary.node_counts.branch, 1);
        assert_eq!(summary.node_counts.execute, 2);
        assert_eq!(summary.node_counts.preserved, 1);
        assert_eq!(summary.editable_command_count, 2);
        assert_eq!(summary.preserved_block_count, 1);
    }

    #[test]
    fn newly_typed_native_statements_count_as_editable_not_preserved() {
        let rate_summary = track_summary(&MoveTrack {
            track: Track::Game,
            effect_execute_baseline: Vec::new(),
            nodes: vec![node(1, NodeKind::MotionRate { rate: 0.75 })],
            baseline: Vec::new(),
        });
        let bare_summary = track_summary(&MoveTrack {
            track: Track::Sound,
            effect_execute_baseline: Vec::new(),
            nodes: vec![node(
                1,
                NodeKind::BareCommand {
                    command: Box::new(ExcuteStmt::Sound(crate::data::SoundCall {
                        func: "PLAY_SE".into(),
                        sounds: vec!["se_common_swing_01".into()],
                        tail: None,
                    })),
                },
            )],
            baseline: Vec::new(),
        });
        assert_eq!(rate_summary.node_counts.motion_rate, 1);
        assert_eq!(rate_summary.node_counts.preserved, 0);
        assert_eq!(rate_summary.preserved_block_count, 0);
        assert_eq!(rate_summary.editable_command_count, 1);
        assert_eq!(bare_summary.node_counts.bare_command, 1);
        assert_eq!(bare_summary.node_counts.preserved, 0);
        assert_eq!(bare_summary.preserved_block_count, 0);
        assert_eq!(bare_summary.editable_command_count, 1);
    }

    #[test]
    fn mixed_execute_reports_editable_and_preserved_commands_separately() {
        let mixed = track_summary(&MoveTrack {
            track: Track::Game,
            effect_execute_baseline: Vec::new(),
            nodes: vec![node(
                1,
                NodeKind::MixedExecute {
                    commands: vec![
                        MoveCommand::Editable(ExcuteStmt::ClearAll),
                        MoveCommand::Preserved(ExcuteStmt::Raw("unknown();".into())),
                    ],
                },
            )],
            baseline: vec![AcmdStmt::Excute(vec![
                ExcuteStmt::ClearAll,
                ExcuteStmt::Raw("unknown();".into()),
            ])],
        });
        assert_eq!(mixed.node_counts.mixed_execute, 1);
        assert_eq!(mixed.node_counts.preserved, 0);
        assert_eq!(mixed.editable_command_count, 1);
        assert_eq!(mixed.preserved_command_count, 1);
        assert_eq!(mixed.preserved_block_count, 0);

        let whole = track_summary(&MoveTrack {
            track: Track::Game,
            effect_execute_baseline: Vec::new(),
            nodes: vec![node(
                1,
                NodeKind::Preserved {
                    statement: AcmdStmt::Excute(vec![
                        ExcuteStmt::ClearAll,
                        ExcuteStmt::Raw("unknown();".into()),
                    ]),
                },
            )],
            baseline: vec![],
        });
        assert_eq!(whole.preserved_command_count, 2);
        assert_eq!(whole.preserved_block_count, 1);
    }
}

/// Editable versus preserved across the vanilla corpus. Ignored: it reads the pinned dump,
/// `VISIONARY_CORPUS` (default `~/.cache/builds/ssbu_scripts/SSBU-Dumped-Scripts/smashline`
/// at `crate::acmd::DUMP_REV`). Run with `cargo test corpus_census -- --ignored --nocapture`.
#[cfg(test)]
mod census {
    use super::*;
    use std::collections::BTreeMap;

    /// What a preserved item is, for ranking: its callee, or the head of its block.
    fn key(text: &str) -> String {
        let text = text.trim();
        let callee: String = text
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '!'))
            .collect();
        if callee.len() > 2 {
            return callee;
        }
        text.chars().take(48).collect()
    }

    fn stmt_text(stmt: &crate::data::AcmdStmt) -> String {
        use crate::data::AcmdStmt;
        match stmt {
            AcmdStmt::RawBlock { header, body } if header.contains("is_excute") => format!(
                "block is_excute[{}]",
                body.iter().map(stmt_text).collect::<Vec<_>>().join(",")
            ),
            AcmdStmt::RawBlock { header, .. } => format!("block {}", key(header)),
            AcmdStmt::Raw(line) => key(line),
            // Which command keeps a whole `is_excute` block preserved.
            AcmdStmt::Excute(commands) => format!(
                "Excute[{}]",
                commands.iter().map(excute_text).collect::<Vec<_>>().join(",")
            ),
            other => key(&format!("{other:?}")),
        }
    }

    fn excute_text(stmt: &crate::data::ExcuteStmt) -> String {
        match stmt {
            crate::data::ExcuteStmt::Raw(line) => key(line),
            other => format!("{other:?}").split(['(', ' ', '{']).next().unwrap_or("").into(),
        }
    }

    fn effect_text(stmt: &crate::data::EffectMacro) -> String {
        match stmt {
            crate::data::EffectMacro::Raw(line) => key(line),
            other => format!("{other:?}").split(['(', ' ', '{']).next().unwrap_or("").into(),
        }
    }

    /// The untyped lines inside a preserved block, which are what typing would have to cover.
    fn inner(
        stmt: &crate::data::AcmdStmt,
        ranking: &mut BTreeMap<String, u64>,
        samples: &mut BTreeMap<String, String>,
    ) {
        use crate::data::{AcmdStmt, ExcuteStmt};
        let mut note = |line: &str| {
            let key = format!("in  {}", key(line));
            samples.entry(key.clone()).or_insert_with(|| line.chars().take(300).collect());
            *ranking.entry(key).or_default() += 1;
        };
        match stmt {
            AcmdStmt::Raw(line) => note(line),
            AcmdStmt::Excute(commands) => {
                for command in commands {
                    if let ExcuteStmt::Raw(line) = command {
                        note(line);
                    }
                }
            }
            AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. } => {
                for stmt in body {
                    inner(stmt, ranking, samples);
                }
            }
            _ => {}
        }
    }

    fn note_raw(line: Option<String>, ranking: &mut BTreeMap<String, u64>, samples: &mut BTreeMap<String, String>) {
        if let Some(line) = line {
            let line = line.trim_start_matches("SourceLine { source: \"").to_owned();
            let key = format!("raw {}", key(&line));
            samples.entry(key.clone()).or_insert_with(|| line.chars().take(300).collect());
            *ranking.entry(key).or_default() += 1;
        }
    }

    fn walk(nodes: &[MoveNode], ranking: &mut BTreeMap<String, u64>, samples: &mut BTreeMap<String, String>) {
        fn raw_line(command: &crate::data::ExcuteStmt) -> Option<String> {
            match command {
                crate::data::ExcuteStmt::NativeCall(call) => call.source_line.as_ref().map(|l| format!("{l:?}")),
                _ => None,
            }
        }
        for node in nodes {
            match &node.kind {
                NodeKind::BareCommand { command } => note_raw(raw_line(command), ranking, samples),
                NodeKind::Execute { commands } => {
                    for command in commands {
                        note_raw(raw_line(command), ranking, samples);
                    }
                }
                NodeKind::EffectExecute { commands } => {
                    for command in commands {
                        if let crate::data::EffectMacro::NativeCall(call) = command {
                            note_raw(call.source_line.as_ref().map(|l| format!("{l:?}")), ranking, samples);
                        }
                    }
                }
                NodeKind::MixedExecute { commands } => {
                    for command in commands {
                        if let MoveCommand::Editable(command) = command {
                            note_raw(raw_line(command), ranking, samples);
                        }
                        if let MoveCommand::Preserved(command) = command {
                            let key = format!("cmd {}", excute_text(command));
                            samples.entry(key.clone()).or_insert_with(|| format!("{command:?}").chars().take(300).collect());
                            *ranking.entry(key).or_default() += 1;
                        }
                    }
                }
                NodeKind::MixedEffectExecute { commands, .. } => {
                    for command in commands {
                        if let super::super::model::EffectCommandSlot::Editable(
                            crate::data::EffectMacro::NativeCall(call),
                        ) = command
                        {
                            note_raw(call.source_line.as_ref().map(|l| format!("{l:?}")), ranking, samples);
                        }
                        if let super::super::model::EffectCommandSlot::Preserved(command) = command {
                            let key = format!("cmd {}", effect_text(command));
                            samples.entry(key.clone()).or_insert_with(|| format!("{command:?}").chars().take(300).collect());
                            *ranking.entry(key).or_default() += 1;
                        }
                    }
                }
                NodeKind::Preserved { statement } => {
                    inner(statement, ranking, samples);
                    let key = format!("blk {}", stmt_text(statement));
                    samples.entry(key.clone()).or_insert_with(|| format!("{statement:?}").chars().take(300).collect());
                    *ranking.entry(key).or_default() += 1;
                }
                NodeKind::Repeat { body, .. } => walk(body, ranking, samples),
                NodeKind::Branch { then_nodes, else_nodes, .. } => {
                    walk(then_nodes, ranking, samples);
                    walk(else_nodes, ranking, samples);
                }
                _ => {}
            }
        }
    }

    #[test]
    #[ignore]
    fn corpus_census() {
        let root = std::env::var_os("VISIONARY_CORPUS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap()
                    .join(".cache/builds/ssbu_scripts/SSBU-Dumped-Scripts/smashline")
            });
        let (mut functions, mut editable) = (0u64, 0u64);
        let (mut commands, mut blocks, mut opaque, mut broken) = (0u64, 0u64, 0u64, 0u64);
        let mut ranking: BTreeMap<String, u64> = BTreeMap::new();
        let mut sole: BTreeMap<String, u64> = BTreeMap::new();
        let mut samples = BTreeMap::new();
        let mut dirs: Vec<_> = std::fs::read_dir(&root).expect("corpus").flatten().collect();
        dirs.sort_by_key(|entry| entry.path());
        for package in dirs {
            let Some(fighter) = package
                .file_name()
                .to_string_lossy()
                .strip_prefix("lua2cpp_")
                .map(str::to_owned)
            else {
                continue;
            };
            let mut agents: Vec<_> = std::fs::read_dir(package.path()).unwrap().flatten().collect();
            agents.sort_by_key(|entry| entry.path());
            for agent in agents {
                let mut files: Vec<_> = std::fs::read_dir(agent.path()).unwrap().flatten().collect();
                files.sort_by_key(|entry| entry.path());
                for file in files {
                    let Ok(text) = std::fs::read_to_string(file.path()) else {
                        continue;
                    };
                    for function in text.split("unsafe extern \"C\" fn ").skip(1) {
                        let body = format!("unsafe extern \"C\" fn {function}");
                        let track = if function.starts_with("game_") {
                            Track::Game
                        } else if function.starts_with("effect_") {
                            Track::Effect
                        } else if function.starts_with("sound_") {
                            Track::Sound
                        } else if function.starts_with("expression_") {
                            Track::Expression
                        } else {
                            continue;
                        };
                        let imported = match track {
                            Track::Effect => super::super::effects::import_for_fighter(
                                &fighter,
                                &crate::acmd::parse_effect_script(&body),
                            ),
                            _ => {
                                let parsed = match track {
                                    Track::Game => crate::acmd::parse_acmd_script(&body),
                                    Track::Sound => crate::acmd::parse_sound_script(&body),
                                    _ => crate::acmd::parse_expression_script(&body),
                                };
                                super::super::timeline::import_for_fighter(&fighter, track, &parsed)
                            }
                        };
                        let summary = track_summary(&imported);
                        functions += 1;
                        // A dump that is not Rust (decompiler `L2CValue::operator` conditions,
                        // stray `else`) cannot compile however it is typed; rank only the rest.
                        if syn::parse_file(&body).is_err() {
                            broken += 1;
                            continue;
                        }
                        if summary.preserved_block_count == 0
                            && summary.preserved_command_count == 0
                            && summary.opaque_source_line_count == 0
                        {
                            editable += 1;
                        }
                        commands += summary.preserved_command_count;
                        blocks += summary.preserved_block_count;
                        opaque += summary.opaque_source_line_count;
                        // Which single kind of item is all that keeps this function from being
                        // fully editable: typing that kind flips the function.
                        let mut own = BTreeMap::new();
                        walk(&imported.nodes, &mut own, &mut samples);
                        let blockers: Vec<_> = own.keys().filter(|key| !key.starts_with("in ")).collect();
                        if let [only] = blockers.as_slice() {
                            *sole.entry((*only).clone()).or_default() += 1;
                        }
                        for (key, count) in own {
                            *ranking.entry(key).or_default() += count;
                        }
                    }
                }
            }
        }
        println!(
            "CENSUS functions {functions} not_rust {broken} fully_editable {editable} preserved_commands {commands} preserved_blocks {blocks} opaque_lines {opaque}"
        );
        let mut ranked: Vec<_> = ranking.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (name, count) in ranked.iter().take(80) {
            println!("RANK {count:>6} {name}\n       {}", samples.get(name).map_or("", String::as_str));
        }
        let mut sole: Vec<_> = sole.into_iter().collect();
        sole.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (name, count) in sole.iter().take(40) {
            println!("SOLE {count:>6} {name}");
        }
        assert!(functions > 70_000, "corpus incomplete: {functions} functions");
    }

    /// Every vanilla effect script with a facing test: both arms reach the flat call list, and
    /// writing that list back gives Rust that parses and reads back to the same calls.
    #[test]
    #[ignore]
    fn facing_effects_round_trip() {
        let root = dirs::home_dir()
            .unwrap()
            .join(".cache/builds/ssbu_scripts/SSBU-Dumped-Scripts/smashline");
        let (mut checked, mut spawns) = (0, 0);
        let mut failures = Vec::new();
        for package in std::fs::read_dir(&root).unwrap().flatten() {
            for agent in std::fs::read_dir(package.path()).into_iter().flatten().flatten() {
                for file in std::fs::read_dir(agent.path()).into_iter().flatten().flatten() {
                    let Ok(text) = std::fs::read_to_string(file.path()) else { continue };
                    for function in text.split("unsafe extern \"C\" fn ").skip(1) {
                        // Decompiler-broken conditions (`L2CValue::operator`) never wrote back.
                        if !function.starts_with("effect_")
                            || !function.contains(crate::data::FACING_LEFT_GUARD)
                            || function.contains("methodlib::")
                        {
                            continue;
                        }
                        let body = format!("unsafe extern \"C\" fn {function}");
                        // A dump that is not Rust to begin with cannot be expected to write back.
                        if syn::parse_file(&body).is_err() {
                            continue;
                        }
                        let script = crate::acmd::parse_effect_script(&body);
                        let (calls, residue) = script.to_effect_calls_and_residue();
                        let facing: Vec<_> = calls
                            .iter()
                            .filter(|call| crate::data::facing_arm_of(call.guard.as_deref()).is_some())
                            .collect();
                        let name = format!("{}", file.path().display());
                        spawns += facing.len();
                        checked += 1;
                        let written = crate::acmd::preview_effect_fn(&calls, "x", &[], &residue);
                        if let Err(error) = syn::parse_file(&written) {
                            failures.push(format!("{name}: does not parse: {error}\n{written}"));
                            continue;
                        }
                        let again = crate::acmd::parse_effect_script(&written).to_effect_calls();
                        // The writer orders spawns by frame; the dump sometimes does not.
                        let key = |calls: &[crate::data::EffectCall]| -> Vec<_> {
                            let mut keys: Vec<_> = calls
                                .iter()
                                .map(|c| (c.effect_name.clone(), c.active_start, c.guard.clone(), c.rotation.map(f32::to_bits)))
                                .collect();
                            keys.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            keys
                        };
                        if key(&again) != key(&calls) {
                            failures.push(format!("{name}: reads back differently\n{written}"));
                        }
                    }
                }
            }
        }
        println!("FACING {checked} scripts, {spawns} facing spawns, {} failures", failures.len());
        for failure in failures.iter().take(10) {
            println!("{failure}\n");
        }
        // Three dumps put an `else {` after an unrelated block; they wrote back broken before
        // facing tests were read too, and still do.
        let known = ["trail/AppealHiR.txt", "trail/AppealHiL.txt", "bayonetta/AttackAirN.txt"];
        failures.retain(|failure| !known.iter().any(|name| failure.contains(name)));
        assert!(failures.is_empty(), "{}", failures.join("\n\n"));
        assert!(spawns > 1000, "only {spawns} facing spawns reached the flat list");
    }
}
