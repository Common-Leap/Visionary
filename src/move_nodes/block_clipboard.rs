//! In-memory copy/paste for editable move-node subtrees.
//!
//! Imported immutable source witnesses are intentionally excluded. Pasting is
//! transactional and uses the ordinary timeline compiler as its target-owner
//! validation boundary.

use std::collections::BTreeMap;

use super::model::{MoveGraph, MoveNode, MoveTrack, NodeKind, Track};

const MAX_NESTING: usize = 8;
const MAX_NODES: usize = 10_000;
const MAX_COMMANDS_PER_NODE: usize = 256;
const MAX_REPEAT: u32 = 100;
const MAX_LAYOUT_ITEMS: usize = 10_000;
const MAX_LAYOUT_COORDINATE: f32 = 1_000_000.0;
const PASTE_OFFSET: f32 = 32.0;

#[derive(Debug, Clone)]
pub struct BlockClipboard {
    origin_fighter: String,
    track: Track,
    root: MoveNode,
    positions: BTreeMap<u32, [f32; 2]>,
}

impl BlockClipboard {
    pub fn capture(fighter: &str, graph: &MoveGraph, node_id: u32) -> Result<Self, String> {
        validate_donor(fighter)?;
        preflight_graph(graph)?;
        // Full compilation catches malformed donor graphs, including duplicate
        // IDs outside the selected subtree and owner-bound typed identifiers.
        super::timeline::compile(fighter, "clipboard_capture", graph)
            .map_err(|errors| format_errors("donor graph is invalid", errors))?;
        let mut found = Vec::new();
        for track in &graph.tracks {
            find_node(track.track, &track.nodes, node_id, &mut found);
        }
        if found.len() != 1 {
            return Err(if found.is_empty() {
                format!("node id {node_id} does not exist")
            } else {
                format!("node id {node_id} is not globally unique")
            });
        }
        let (track, root) = found.pop().expect("one matching node");
        let mut count = 0;
        validate_subtree(root, 0, &mut count)?;
        let mut positions = BTreeMap::new();
        collect_subtree_positions(root, &graph.positions, &mut positions)?;
        Ok(Self {
            origin_fighter: fighter.to_owned(),
            track,
            root: root.clone(),
            positions,
        })
    }

    pub fn track(&self) -> Track {
        self.track
    }

    pub fn origin_fighter(&self) -> &str {
        &self.origin_fighter
    }

    #[cfg(test)]
    fn paste(
        &self,
        fighter: &str,
        move_name: &str,
        graph: &mut MoveGraph,
        track: Track,
        after: Option<u32>,
    ) -> Result<u32, String> {
        let (candidate, root_id) = self.candidate(fighter, move_name, graph, track, after)?;
        *graph = candidate;
        Ok(root_id)
    }

    /// Build and validate a paste candidate without changing the supplied graph.
    /// Callers can run whole-character validation on the returned candidate
    /// before committing it to a project.
    pub fn candidate(
        &self,
        fighter: &str,
        move_name: &str,
        graph: &MoveGraph,
        track: Track,
        after: Option<u32>,
    ) -> Result<(MoveGraph, u32), String> {
        validate_donor(fighter)?;
        if track != self.track {
            return Err(format!(
                "clipboard track {:?} cannot be pasted on {:?}",
                self.track, track
            ));
        }
        preflight_graph(graph)?;
        if let Some(after_id) = after {
            let target_exists = graph
                .tracks
                .iter()
                .filter(|candidate| candidate.track == track)
                .any(|candidate| {
                    let mut found = Vec::new();
                    find_node(track, &candidate.nodes, after_id, &mut found);
                    !found.is_empty()
                });
            if !target_exists {
                return Err(format!(
                    "target node id {after_id} does not exist in track {track:?}"
                ));
            }
        }
        super::timeline::compile(fighter, move_name, graph)
            .map_err(|errors| format_errors("target graph is invalid", errors))?;
        let existing_ids = all_ids(graph)?;
        // Bounds above are deliberately checked before cloning either graph.
        let mut candidate = graph.clone();
        let target_track = unique_track_mut(&mut candidate, track)?;
        let mut next_id = existing_ids.iter().copied().max().unwrap_or(0);
        let mut cloned = self.root.clone();
        let mut id_map = BTreeMap::new();
        assign_fresh_ids(&mut cloned, &existing_ids, &mut next_id, &mut id_map)?;
        let root_id = cloned.id;

        match after {
            Some(after_id) => {
                let siblings = find_sibling_region_mut(&mut target_track.nodes, after_id)
                    .ok_or_else(|| {
                        format!("target node id {after_id} does not exist in track {track:?}")
                    })?;
                let index = siblings
                    .iter()
                    .position(|node| node.id == after_id)
                    .ok_or_else(|| {
                        format!("target node id {after_id} is not a sibling in its region")
                    })?;
                siblings.insert(index + 1, cloned);
            }
            None => target_track.nodes.push(cloned),
        }

        if candidate
            .positions
            .len()
            .saturating_add(self.positions.len())
            > MAX_LAYOUT_ITEMS
        {
            return Err(format!(
                "pasted layout would exceed {MAX_LAYOUT_ITEMS} entries"
            ));
        }
        for (old_id, position) in &self.positions {
            let new_id = id_map
                .get(old_id)
                .ok_or_else(|| format!("clipboard layout references unknown node id {old_id}"))?;
            let moved = [position[0] + PASTE_OFFSET, position[1] + PASTE_OFFSET];
            if moved
                .iter()
                .any(|value| !value.is_finite() || value.abs() > MAX_LAYOUT_COORDINATE)
            {
                return Err(format!(
                    "pasted layout for node {new_id} is outside valid bounds"
                ));
            }
            candidate.positions.insert(format!("node:{new_id}"), moved);
        }
        preflight_graph(&candidate)?;
        super::timeline::compile(fighter, move_name, &candidate)
            .map_err(|errors| format_errors("pasted graph is invalid for target", errors))?;
        Ok((candidate, root_id))
    }
}

fn validate_donor(fighter: &str) -> Result<(), String> {
    crate::gimmicks::registry::fighter_kind_symbol(fighter)
        .map(|_| ())
        .ok_or_else(|| {
            format!(
                "block reuse requires a registered engine donor, got {:?}",
                fighter.chars().take(64).collect::<String>()
            )
        })
}

/// Bound every recursive traversal and large witness before compiler walks,
/// ID discovery, or cloning. This validates structure only; it never repairs
/// layout, references, or immutable source baselines.
pub(super) fn preflight_graph(graph: &MoveGraph) -> Result<(), String> {
    if graph.tracks.len() > 4 {
        return Err("move graph contains more than four tracks".into());
    }
    let mut seen_tracks = Vec::with_capacity(4);
    let mut ids = std::collections::BTreeSet::new();
    let mut pending = Vec::new();
    let mut count = 0usize;
    for track in &graph.tracks {
        if seen_tracks.contains(&track.track) {
            return Err(format!(
                "move graph contains duplicate {:?} tracks",
                track.track
            ));
        }
        seen_tracks.push(track.track);
        if track.baseline.len() > MAX_NODES {
            return Err(format!(
                "{:?} baseline exceeds {MAX_NODES} statements",
                track.track
            ));
        }
        if track.effect_execute_baseline.len() > MAX_NODES {
            return Err(format!(
                "{:?} effect witness count exceeds {MAX_NODES}",
                track.track
            ));
        }
        for (index, witness) in track.effect_execute_baseline.iter().enumerate() {
            if witness.is_empty() || witness.len() > MAX_COMMANDS_PER_NODE {
                return Err(format!(
                    "{:?} effect witness {index} exceeds command bounds",
                    track.track
                ));
            }
        }
        preflight_acmd_statements(&track.baseline)?;
        pending.push((track.nodes.as_slice(), 0usize));
    }
    while let Some((nodes, depth)) = pending.pop() {
        if depth > MAX_NESTING {
            return Err(format!("move graph nesting exceeds {MAX_NESTING}"));
        }
        count = count
            .checked_add(nodes.len())
            .ok_or("move graph node count overflow")?;
        if count > MAX_NODES {
            return Err(format!("move graph exceeds {MAX_NODES} nodes"));
        }
        for node in nodes {
            if !ids.insert(node.id) {
                return Err(format!("move graph has duplicate node id {}", node.id));
            }
            match &node.kind {
                NodeKind::Execute { commands } if commands.len() > MAX_COMMANDS_PER_NODE => {
                    return Err(format!("node {} exceeds command bound", node.id));
                }
                NodeKind::EffectExecute { commands } if commands.len() > MAX_COMMANDS_PER_NODE => {
                    return Err(format!("node {} exceeds command bound", node.id));
                }
                NodeKind::MixedExecute { commands } if commands.len() > MAX_COMMANDS_PER_NODE => {
                    return Err(format!("node {} exceeds command bound", node.id));
                }
                NodeKind::MixedEffectExecute {
                    commands,
                    source_commands,
                } if commands.len() > MAX_COMMANDS_PER_NODE
                    || source_commands.len() > MAX_COMMANDS_PER_NODE =>
                {
                    return Err(format!(
                        "node {} exceeds mixed effect command bound",
                        node.id
                    ));
                }
                NodeKind::Repeat {
                    count: repeats,
                    body,
                    ..
                } => {
                    if !(1..=MAX_REPEAT).contains(repeats) {
                        return Err(format!("node {} has invalid repeat count", node.id));
                    }
                    if body.len() > MAX_NODES {
                        return Err("repeat body exceeds node bound".into());
                    }
                    pending.push((body, depth + 1));
                }
                NodeKind::Preserved { statement } => {
                    preflight_acmd_statements(std::slice::from_ref(statement))?;
                }
                NodeKind::Branch {
                    condition,
                    then_nodes,
                    else_nodes,
                    ..
                } => {
                    if !super::compound_conditions::within_limits(condition) {
                        return Err(format!(
                            "node {} condition exceeds structural bounds",
                            node.id
                        ));
                    }
                    if then_nodes.len() > MAX_NODES || else_nodes.len() > MAX_NODES {
                        return Err("branch arm exceeds node bound".into());
                    }
                    pending.push((else_nodes, depth + 1));
                    pending.push((then_nodes, depth + 1));
                }
                _ => {}
            }
        }
    }
    let layout_errors = super::flow::validate_layout(graph);
    if !layout_errors.is_empty() {
        return Err(format_errors("move graph layout is invalid", layout_errors));
    }
    Ok(())
}

fn preflight_acmd_statements(stmts: &[crate::data::AcmdStmt]) -> Result<(), String> {
    const MAX_STATEMENTS: usize = 100_000;
    const MAX_DEPTH: usize = 64;
    const MAX_WORK: u64 = crate::data::MAX_PREVIEW_EXPANDED_WORK;
    const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
    let mut pending = vec![(stmts, 0usize, 1u64)];
    let mut statements = 0usize;
    let mut work = 0u64;
    let mut text_bytes = 0usize;
    while let Some((list, depth, multiplier)) = pending.pop() {
        if depth > MAX_DEPTH {
            return Err(format!("ACMD witness nesting exceeds {MAX_DEPTH}"));
        }
        for stmt in list {
            statements = statements
                .checked_add(1)
                .ok_or("ACMD witness count overflow")?;
            if statements > MAX_STATEMENTS {
                return Err("ACMD witness exceeds statement bound".into());
            }
            let (amount, child_multiplier) = match stmt {
                crate::data::AcmdStmt::Loop { count, body, .. } => {
                    if body.len() > MAX_NODES {
                        return Err("ACMD loop body exceeds statement bound".into());
                    }
                    let iterations = (*count as u64).saturating_add(1);
                    let local = 1u64.saturating_add(if body.is_empty() { iterations } else { 0 });
                    (local, Some(multiplier.saturating_mul(iterations)))
                }
                crate::data::AcmdStmt::RawBlock { header, .. } => {
                    if header.len() > MAX_TEXT_BYTES {
                        return Err("ACMD raw header exceeds text bound".into());
                    }
                    text_bytes = text_bytes
                        .checked_add(header.len())
                        .ok_or("ACMD text size overflow")?;
                    (1, Some(multiplier))
                }
                crate::data::AcmdStmt::Excute(commands) => {
                    if commands.len() > MAX_COMMANDS_PER_NODE {
                        return Err("ACMD witness execute exceeds command bound".into());
                    }
                    (1u64.saturating_add(commands.len() as u64), None)
                }
                crate::data::AcmdStmt::Raw(raw) => {
                    if raw.len() > MAX_TEXT_BYTES {
                        return Err("ACMD raw statement exceeds text bound".into());
                    }
                    text_bytes = text_bytes
                        .checked_add(raw.len())
                        .ok_or("ACMD text size overflow")?;
                    (1, None)
                }
                crate::data::AcmdStmt::Frame(_)
                | crate::data::AcmdStmt::NativeExecuteFrame(_)
                | crate::data::AcmdStmt::NativeMotionLoopSync
                | crate::data::AcmdStmt::Wait(_)
                | crate::data::AcmdStmt::WaitLoopClear
                | crate::data::AcmdStmt::MotionRate(_) => (1, None),
                crate::data::AcmdStmt::Bare(_) => (2, None),
            };
            work = work.saturating_add(multiplier.saturating_mul(amount));
            if work > MAX_WORK {
                return Err("ACMD witness exceeds preview work bound".into());
            }
            if text_bytes > MAX_TEXT_BYTES {
                return Err("ACMD raw witness text exceeds aggregate text bound".into());
            }
            if let Some(child_multiplier) = child_multiplier {
                let children = match stmt {
                    crate::data::AcmdStmt::Loop { body, .. }
                    | crate::data::AcmdStmt::RawBlock { body, .. } => body,
                    _ => unreachable!(),
                };
                pending.push((children, depth + 1, child_multiplier));
            }
        }
    }
    Ok(())
}

fn format_errors(prefix: &str, errors: Vec<super::model::NodeError>) -> String {
    let detail = errors
        .into_iter()
        .take(8)
        .map(|error| format!("{}: {}", error.subject, error.reason))
        .collect::<Vec<_>>()
        .join("; ");
    format!("{prefix}: {detail}")
}

fn find_node<'a>(
    track: Track,
    nodes: &'a [MoveNode],
    id: u32,
    found: &mut Vec<(Track, &'a MoveNode)>,
) {
    for node in nodes {
        if node.id == id {
            found.push((track, node));
        }
        match &node.kind {
            NodeKind::Repeat { body, .. } => find_node(track, body, id, found),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                find_node(track, then_nodes, id, found);
                find_node(track, else_nodes, id, found);
            }
            _ => {}
        }
    }
}

fn validate_subtree(node: &MoveNode, depth: usize, count: &mut usize) -> Result<(), String> {
    if depth > MAX_NESTING {
        return Err(format!(
            "copied subtree exceeds nesting limit {MAX_NESTING}"
        ));
    }
    *count = (*count)
        .checked_add(1)
        .ok_or("subtree node count overflow")?;
    if *count > MAX_NODES {
        return Err(format!("copied subtree exceeds node limit {MAX_NODES}"));
    }
    match &node.kind {
        NodeKind::Preserved { .. } => {
            return Err(format!("node {} is immutable preserved source", node.id))
        }
        NodeKind::MixedExecute { .. } | NodeKind::MixedEffectExecute { .. } => {
            return Err(format!(
                "node {} contains immutable source witnesses",
                node.id
            ));
        }
        NodeKind::Execute { commands } if commands.len() > MAX_COMMANDS_PER_NODE => {
            return Err(format!(
                "node {} exceeds command limit {MAX_COMMANDS_PER_NODE}",
                node.id
            ));
        }
        NodeKind::EffectExecute { commands } if commands.len() > MAX_COMMANDS_PER_NODE => {
            return Err(format!(
                "node {} exceeds command limit {MAX_COMMANDS_PER_NODE}",
                node.id
            ));
        }
        NodeKind::Repeat { body, .. } => {
            for child in body {
                validate_subtree(child, depth + 1, count)?;
            }
        }
        NodeKind::Branch {
            then_nodes,
            else_nodes,
            ..
        } => {
            for child in then_nodes.iter().chain(else_nodes) {
                validate_subtree(child, depth + 1, count)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn collect_subtree_positions(
    node: &MoveNode,
    positions: &BTreeMap<String, [f32; 2]>,
    out: &mut BTreeMap<u32, [f32; 2]>,
) -> Result<(), String> {
    if let Some(position) = positions.get(&format!("node:{}", node.id)) {
        if position
            .iter()
            .any(|value| !value.is_finite() || value.abs() > MAX_LAYOUT_COORDINATE)
        {
            return Err(format!("layout for copied node {} is invalid", node.id));
        }
        out.insert(node.id, *position);
    }
    match &node.kind {
        NodeKind::Repeat { body, .. } => {
            for child in body {
                collect_subtree_positions(child, positions, out)?;
            }
        }
        NodeKind::Branch {
            then_nodes,
            else_nodes,
            ..
        } => {
            for child in then_nodes.iter().chain(else_nodes) {
                collect_subtree_positions(child, positions, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn all_ids(graph: &MoveGraph) -> Result<std::collections::BTreeSet<u32>, String> {
    let mut ids = std::collections::BTreeSet::new();
    for track in &graph.tracks {
        collect_ids(&track.nodes, &mut ids)?;
    }
    Ok(ids)
}

fn collect_ids(
    nodes: &[MoveNode],
    ids: &mut std::collections::BTreeSet<u32>,
) -> Result<(), String> {
    for node in nodes {
        if !ids.insert(node.id) {
            return Err(format!("graph contains duplicate node id {}", node.id));
        }
        match &node.kind {
            NodeKind::Repeat { body, .. } => collect_ids(body, ids)?,
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                collect_ids(then_nodes, ids)?;
                collect_ids(else_nodes, ids)?;
            }
            _ => {}
        }
    }
    if ids.len() > MAX_NODES {
        return Err(format!("graph exceeds node limit {MAX_NODES}"));
    }
    Ok(())
}

fn assign_fresh_ids(
    node: &mut MoveNode,
    existing: &std::collections::BTreeSet<u32>,
    next: &mut u32,
    map: &mut BTreeMap<u32, u32>,
) -> Result<(), String> {
    let old = node.id;
    loop {
        *next = (*next).checked_add(1).ok_or("node id space exhausted")?;
        if !existing.contains(&*next) {
            break;
        }
    }
    node.id = *next;
    map.insert(old, node.id);
    match &mut node.kind {
        NodeKind::Repeat { body, .. } => {
            for child in body {
                assign_fresh_ids(child, existing, next, map)?;
            }
        }
        NodeKind::Branch {
            then_nodes,
            else_nodes,
            ..
        } => {
            for child in then_nodes.iter_mut().chain(else_nodes) {
                assign_fresh_ids(child, existing, next, map)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn unique_track_mut(graph: &mut MoveGraph, track: Track) -> Result<&mut MoveTrack, String> {
    let mut matches = graph
        .tracks
        .iter_mut()
        .filter(|candidate| candidate.track == track);
    let result = matches
        .next()
        .ok_or_else(|| format!("target graph has no {track:?} track"))?;
    if matches.next().is_some() {
        return Err(format!("target graph has duplicate {track:?} tracks"));
    }
    Ok(result)
}

fn find_sibling_region_mut(nodes: &mut Vec<MoveNode>, id: u32) -> Option<&mut Vec<MoveNode>> {
    if nodes.iter().any(|node| node.id == id) {
        return Some(nodes);
    }
    for node in nodes {
        match &mut node.kind {
            NodeKind::Repeat { body, .. } => {
                if let Some(found) = find_sibling_region_mut(body, id) {
                    return Some(found);
                }
            }
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                if let Some(found) = find_sibling_region_mut(then_nodes, id) {
                    return Some(found);
                }
                if let Some(found) = find_sibling_region_mut(else_nodes, id) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::move_nodes::model::{MoveCondition, MoveTrack};

    fn node(id: u32, kind: NodeKind) -> MoveNode {
        MoveNode { id, kind }
    }
    fn graph(track: Track, nodes: Vec<MoveNode>) -> MoveGraph {
        MoveGraph {
            tracks: vec![MoveTrack {
                track,
                nodes,
                baseline: vec![],
                effect_execute_baseline: vec![],
            }],
            ..MoveGraph::default()
        }
    }

    #[test]
    fn deep_copy_rekeys_all_nodes_and_inserts_after_target_in_same_region() {
        let source = graph(
            Track::Game,
            vec![
                node(1, NodeKind::Wait { frames: 1.0 }),
                node(
                    2,
                    NodeKind::Branch {
                        condition: MoveCondition::Grounded,
                        then_nodes: vec![node(
                            3,
                            NodeKind::Repeat {
                                motion_loop: crate::data::MotionLoop::No,
                                count: 2,
                                body: vec![node(4, NodeKind::Wait { frames: 2.0 })],
                            },
                        )],
                        else_nodes: vec![node(5, NodeKind::Frame { frame: 3.0 })],
                    },
                ),
            ],
        );
        let mut source = source;
        source.positions.insert("node:2".into(), [5.0, 8.0]);
        source.positions.insert("node:4".into(), [9.0, 10.0]);
        let clipboard = BlockClipboard::capture("mario", &source, 2).unwrap();
        let mut target = graph(
            Track::Game,
            vec![node(
                10,
                NodeKind::Branch {
                    condition: MoveCondition::Grounded,
                    then_nodes: vec![node(11, NodeKind::Wait { frames: 1.0 })],
                    else_nodes: vec![],
                },
            )],
        );
        let root = clipboard
            .paste("mario", "attack11", &mut target, Track::Game, Some(11))
            .unwrap();
        assert_eq!(root, 12);
        let NodeKind::Branch { then_nodes, .. } = &target.tracks[0].nodes[0].kind else {
            panic!()
        };
        assert_eq!(
            then_nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
            vec![11, 12]
        );
        let NodeKind::Branch {
            then_nodes: copied_then,
            else_nodes: copied_else,
            ..
        } = &then_nodes[1].kind
        else {
            panic!()
        };
        let NodeKind::Repeat { body, .. } = &copied_then[0].kind else {
            panic!()
        };
        let ids = [root, copied_then[0].id, body[0].id, copied_else[0].id];
        assert_eq!(ids, [12, 13, 14, 15]);
        assert_eq!(target.positions["node:12"], [37.0, 40.0]);
        assert_eq!(target.positions["node:14"], [41.0, 42.0]);
    }

    #[test]
    fn track_mismatch_and_owner_mismatch_are_rejected_atomically() {
        let clipboard = BlockClipboard::capture(
            "sonic",
            &graph(
                Track::Game,
                vec![node(
                    1,
                    NodeKind::Branch {
                        condition: MoveCondition::ArticleExists {
                            symbol: "FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP".into(),
                        },
                        then_nodes: vec![],
                        else_nodes: vec![],
                    },
                )],
            ),
            1,
        )
        .unwrap();
        let mut target = graph(Track::Game, vec![node(8, NodeKind::Wait { frames: 1.0 })]);
        let original = target.tracks[0].nodes[0].id;
        assert!(clipboard
            .paste("sonic", "attack11", &mut target, Track::Effect, None)
            .is_err());
        assert_eq!(target.tracks[0].nodes[0].id, original);
        assert!(clipboard
            .paste("mario", "attack11", &mut target, Track::Game, None)
            .is_err());
        assert_eq!(target.tracks[0].nodes.len(), 1);
    }

    #[test]
    fn donor_dependent_execute_action_is_rejected_for_foreign_target() {
        let source = graph(
            Track::Game,
            vec![node(
                1,
                NodeKind::Execute {
                    commands: vec![crate::data::ExcuteStmt::WorkModuleIncInt(
                        crate::data::WorkModuleIncIntCall {
                            slot: "*FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_BAT_WITH_IN_STATE"
                                .into(),
                        },
                    )],
                },
            )],
        );
        let clipboard = BlockClipboard::capture("bayonetta", &source, 1).unwrap();
        let target = graph(Track::Game, vec![node(5, NodeKind::Wait { frames: 1.0 })]);
        assert!(clipboard
            .candidate("mario", "attack11", &target, Track::Game, None)
            .is_err());
        assert_eq!(target.tracks[0].nodes.len(), 1);
    }

    #[test]
    fn generic_cross_fighter_copy_and_root_append_work() {
        let clipboard = BlockClipboard::capture(
            "mario",
            &graph(Track::Game, vec![node(1, NodeKind::Wait { frames: 1.0 })]),
            1,
        )
        .unwrap();
        assert_eq!(clipboard.origin_fighter(), "mario");
        let mut target = graph(Track::Game, vec![node(9, NodeKind::Frame { frame: 2.0 })]);
        assert_eq!(
            clipboard
                .paste("sonic", "attack11", &mut target, Track::Game, None)
                .unwrap(),
            10
        );
        assert_eq!(
            target.tracks[0]
                .nodes
                .iter()
                .map(|node| node.id)
                .collect::<Vec<_>>(),
            vec![9, 10]
        );
    }

    #[test]
    fn generic_blocks_still_require_a_registered_donor() {
        let source = graph(Track::Game, vec![node(1, NodeKind::Wait { frames: 1.0 })]);
        for unknown in ["unknown", "", "Mario", "mario_2", "mario "] {
            assert!(BlockClipboard::capture(unknown, &source, 1).is_err());
        }
        let clipboard = BlockClipboard::capture("mario", &source, 1).unwrap();
        assert!(clipboard
            .candidate("unknown", "attack11", &source, Track::Game, None)
            .is_err());
    }

    #[test]
    fn preserved_subtrees_duplicate_ids_id_overflow_and_invalid_layout_fail_closed() {
        let preserved = graph(
            Track::Game,
            vec![node(
                1,
                NodeKind::Preserved {
                    statement: crate::data::AcmdStmt::Wait(1.0),
                },
            )],
        );
        assert!(BlockClipboard::capture("mario", &preserved, 1).is_err());

        let duplicate = graph(
            Track::Game,
            vec![node(
                1,
                NodeKind::Branch {
                    condition: MoveCondition::Grounded,
                    then_nodes: vec![node(1, NodeKind::Wait { frames: 1.0 })],
                    else_nodes: vec![],
                },
            )],
        );
        assert!(BlockClipboard::capture("mario", &duplicate, 1).is_err());

        let clipboard = BlockClipboard::capture(
            "mario",
            &graph(Track::Game, vec![node(1, NodeKind::Wait { frames: 1.0 })]),
            1,
        )
        .unwrap();
        let mut overflow = graph(
            Track::Game,
            vec![node(u32::MAX, NodeKind::Frame { frame: 1.0 })],
        );
        assert!(clipboard
            .paste("mario", "attack11", &mut overflow, Track::Game, None)
            .is_err());

        let mut invalid = graph(Track::Game, vec![node(1, NodeKind::Wait { frames: 1.0 })]);
        invalid.positions.insert("node:1".into(), [f32::NAN, 0.0]);
        assert!(BlockClipboard::capture("mario", &invalid, 1).is_err());

        let too_deep =
            (0..=MAX_NESTING)
                .rev()
                .fold(node(90, NodeKind::Wait { frames: 1.0 }), |child, id| {
                    node(
                        id as u32,
                        NodeKind::Repeat {
                            motion_loop: crate::data::MotionLoop::No,
                            count: 1,
                            body: vec![child],
                        },
                    )
                });
        assert!(BlockClipboard::capture("mario", &graph(Track::Game, vec![too_deep]), 0).is_err());
    }

    #[test]
    fn iterative_preflight_rejects_deep_oversized_and_cross_track_duplicate_graphs() {
        let too_deep =
            (0..=MAX_NESTING)
                .rev()
                .fold(node(90, NodeKind::Wait { frames: 1.0 }), |child, id| {
                    node(
                        id as u32,
                        NodeKind::Repeat {
                            motion_loop: crate::data::MotionLoop::No,
                            count: 1,
                            body: vec![child],
                        },
                    )
                });
        assert!(preflight_graph(&graph(Track::Game, vec![too_deep])).is_err());

        let too_many = (1..=MAX_NODES as u32 + 1)
            .map(|id| node(id, NodeKind::Wait { frames: 1.0 }))
            .collect();
        assert!(preflight_graph(&graph(Track::Game, too_many)).is_err());

        let mut multi = graph(Track::Game, vec![node(7, NodeKind::Wait { frames: 1.0 })]);
        multi.tracks.push(MoveTrack {
            track: Track::Sound,
            nodes: vec![node(7, NodeKind::Wait { frames: 1.0 })],
            baseline: vec![],
            effect_execute_baseline: vec![],
        });
        assert!(preflight_graph(&multi).is_err());

        let zero_repeat = graph(
            Track::Game,
            vec![node(
                1,
                NodeKind::Repeat {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 0,
                    body: vec![],
                },
            )],
        );
        assert!(preflight_graph(&zero_repeat).is_err());

        let preserved = (0..=64)
            .rev()
            .fold(crate::data::AcmdStmt::Wait(1.0), |body, _| {
                crate::data::AcmdStmt::RawBlock {
                    header: "if true {".into(),
                    body: vec![body],
                }
            });
        let preserved_graph = graph(
            Track::Game,
            vec![node(
                1,
                NodeKind::Preserved {
                    statement: preserved,
                },
            )],
        );
        // The source witness itself is preflighted before timeline comparison.
        assert!(preflight_graph(&preserved_graph).is_err());

        let nested_mixed = node(
            2,
            NodeKind::Branch {
                condition: MoveCondition::Grounded,
                then_nodes: vec![node(3, NodeKind::MixedExecute { commands: vec![] })],
                else_nodes: vec![],
            },
        );
        let mut subtree_count = 0;
        assert!(validate_subtree(&nested_mixed, 0, &mut subtree_count).is_err());
    }

    #[test]
    fn candidate_rejects_aggregate_node_overflow_and_missing_after_atomically() {
        let clipboard = BlockClipboard::capture(
            "mario",
            &graph(Track::Game, vec![node(1, NodeKind::Wait { frames: 1.0 })]),
            1,
        )
        .unwrap();
        let target = graph(
            Track::Game,
            (1..=MAX_NODES as u32)
                .map(|id| node(id, NodeKind::Wait { frames: 1.0 }))
                .collect(),
        );
        assert!(clipboard
            .candidate("mario", "attack11", &target, Track::Game, None)
            .is_err());
        assert_eq!(target.tracks[0].nodes.len(), MAX_NODES);

        let small = graph(Track::Game, vec![node(1, NodeKind::Wait { frames: 1.0 })]);
        assert!(clipboard
            .candidate("mario", "attack11", &small, Track::Game, Some(99))
            .is_err());
        assert_eq!(small.tracks[0].nodes.len(), 1);
    }

    #[test]
    fn layout_offset_overflow_fails_and_unrelated_layout_keys_are_preserved() {
        let mut source = graph(Track::Game, vec![node(1, NodeKind::Wait { frames: 1.0 })]);
        source
            .positions
            .insert("node:1".into(), [MAX_LAYOUT_COORDINATE, 0.0]);
        let clipboard = BlockClipboard::capture("mario", &source, 1).unwrap();
        let mut target = graph(
            Track::Game,
            vec![
                node(5, NodeKind::Wait { frames: 1.0 }),
                node(6, NodeKind::Frame { frame: 2.0 }),
            ],
        );
        target.positions.insert("node:5".into(), [3.0, 4.0]);
        target.positions.insert("node:6".into(), [7.0, 8.0]);
        let before = target.positions.clone();
        assert!(clipboard
            .candidate("mario", "attack11", &target, Track::Game, Some(5))
            .is_err());
        assert_eq!(target.positions, before);

        source.positions.insert("node:1".into(), [10.0, 20.0]);
        let clipboard = BlockClipboard::capture("mario", &source, 1).unwrap();
        let (pasted, root) = clipboard
            .candidate("mario", "attack11", &target, Track::Game, Some(5))
            .unwrap();
        assert_eq!(pasted.positions["node:5"], [3.0, 4.0]);
        assert_eq!(pasted.positions["node:6"], [7.0, 8.0]);
        assert_eq!(pasted.positions[&format!("node:{root}")], [42.0, 52.0]);
    }

    #[test]
    fn paste_failure_does_not_mutate_graph() {
        let clipboard = BlockClipboard::capture(
            "sonic",
            &graph(
                Track::Game,
                vec![node(
                    1,
                    NodeKind::Branch {
                        condition: MoveCondition::ArticleExists {
                            symbol: "FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP".into(),
                        },
                        then_nodes: vec![],
                        else_nodes: vec![],
                    },
                )],
            ),
            1,
        )
        .unwrap();
        let mut target = graph(Track::Game, vec![node(5, NodeKind::Wait { frames: 1.0 })]);
        let before = serde_json::to_value(&target).unwrap();
        assert!(clipboard
            .paste("mario", "attack11", &mut target, Track::Game, None)
            .is_err());
        assert_eq!(serde_json::to_value(&target).unwrap(), before);
    }

    #[test]
    fn unknown_node_layout_keys_reject_before_any_id_or_position_changes() {
        let mut source = graph(Track::Game, vec![node(1, NodeKind::Wait { frames: 1.0 })]);
        source.positions.insert("node:1".into(), [10.0, 20.0]);
        let clipboard = BlockClipboard::capture("mario", &source, 1).unwrap();
        let mut target = graph(Track::Game, vec![node(5, NodeKind::Frame { frame: 2.0 })]);
        target.positions.insert("node:6".into(), [100.0, 200.0]);
        target
            .positions
            .insert(format!("node:{}", u32::MAX), [300.0, 400.0]);
        let before = serde_json::to_value(&target).unwrap();
        let error = clipboard
            .candidate("mario", "attack11", &target, Track::Game, None)
            .unwrap_err();
        assert!(error.contains("position key does not identify an existing"));
        assert_eq!(serde_json::to_value(&target).unwrap(), before);
        source.positions.clear();
        let clipboard = BlockClipboard::capture("mario", &source, 1).unwrap();
        assert!(clipboard
            .candidate("mario", "attack11", &target, Track::Game, None)
            .is_err());
        assert_eq!(
            serde_json::to_value(&target).unwrap(),
            before,
            "no stale layout inheritance"
        );
    }
}
