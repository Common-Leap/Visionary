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
