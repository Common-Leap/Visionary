//! Structural flow projection and safe sibling reordering for the move canvas.
//!
//! Connections are derived from the typed move AST. This module does not accept
//! arbitrary endpoints or turn canvas edges into source code.

use std::collections::{BTreeSet, HashMap};

use super::model::{MoveGraph, MoveNode, MoveTrack, NodeError, NodeKind, Track};

const MAX_LAYOUT_ITEMS: usize = 10_000;
const MAX_LAYOUT_COORDINATE: f32 = 1_000_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlowPort {
    Next,
    Then,
    Else,
    Repeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowNodeKind {
    Frame,
    NativeExecuteFrame,
    NativeMotionLoopSync,
    NativeWaitLoopClear,
    Wait,
    MotionRate,
    BareCommand,
    Execute,
    MixedExecute,
    EffectExecute,
    MixedEffectExecute,
    Repeat,
    Branch,
    Preserved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowNode {
    pub id: u32,
    pub track: Track,
    pub kind: FlowNodeKind,
    /// Number of enclosing repeat/branch regions.
    pub depth: usize,
    /// The region that directly contains this node, when it is nested.
    pub parent: Option<(u32, FlowPort)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowEdge {
    pub track: Track,
    pub source: u32,
    pub target: u32,
    pub port: FlowPort,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlowProjection {
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
}

/// Project the ordered AST into displayable nodes and typed structural edges.
/// Sibling order creates `Next`; containers connect to the first node of each
/// child region using `Then`, `Else`, or `Repeat`.
pub fn project(graph: &MoveGraph) -> FlowProjection {
    let mut projection = FlowProjection::default();
    for track in &graph.tracks {
        project_sequence(track, &track.nodes, None, 0, &mut projection);
    }
    projection
}

fn project_sequence(
    track: &MoveTrack,
    nodes: &[MoveNode],
    parent: Option<(u32, FlowPort)>,
    depth: usize,
    projection: &mut FlowProjection,
) {
    for (index, node) in nodes.iter().enumerate() {
        projection.nodes.push(FlowNode {
            id: node.id,
            track: track.track,
            kind: flow_kind(&node.kind),
            depth,
            parent,
        });
        if let Some(next) = nodes.get(index + 1) {
            projection.edges.push(FlowEdge {
                track: track.track,
                source: node.id,
                target: next.id,
                port: FlowPort::Next,
            });
        }
        match &node.kind {
            NodeKind::Repeat { body, .. } => {
                project_entry(
                    track.track,
                    node.id,
                    body,
                    FlowPort::Repeat,
                    depth,
                    projection,
                );
                project_sequence(
                    track,
                    body,
                    Some((node.id, FlowPort::Repeat)),
                    depth + 1,
                    projection,
                );
            }
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                project_entry(
                    track.track,
                    node.id,
                    then_nodes,
                    FlowPort::Then,
                    depth,
                    projection,
                );
                project_entry(
                    track.track,
                    node.id,
                    else_nodes,
                    FlowPort::Else,
                    depth,
                    projection,
                );
                project_sequence(
                    track,
                    then_nodes,
                    Some((node.id, FlowPort::Then)),
                    depth + 1,
                    projection,
                );
                project_sequence(
                    track,
                    else_nodes,
                    Some((node.id, FlowPort::Else)),
                    depth + 1,
                    projection,
                );
            }
            _ => {}
        }
    }
}

fn project_entry(
    track: Track,
    source: u32,
    nodes: &[MoveNode],
    port: FlowPort,
    _depth: usize,
    projection: &mut FlowProjection,
) {
    if let Some(first) = nodes.first() {
        projection.edges.push(FlowEdge {
            track,
            source,
            target: first.id,
            port,
        });
    }
}

fn flow_kind(kind: &NodeKind) -> FlowNodeKind {
    match kind {
        NodeKind::Frame { .. } => FlowNodeKind::Frame,
        NodeKind::NativeExecuteFrame { .. } => FlowNodeKind::NativeExecuteFrame,
        NodeKind::NativeMotionLoopSync => FlowNodeKind::NativeMotionLoopSync,
        NodeKind::NativeWaitLoopClear => FlowNodeKind::NativeWaitLoopClear,
        NodeKind::Wait { .. } => FlowNodeKind::Wait,
        NodeKind::MotionRate { .. } => FlowNodeKind::MotionRate,
        NodeKind::BareCommand { .. } => FlowNodeKind::BareCommand,
        NodeKind::Execute { .. } => FlowNodeKind::Execute,
        NodeKind::MixedExecute { .. } => FlowNodeKind::MixedExecute,
        NodeKind::EffectExecute { .. } => FlowNodeKind::EffectExecute,
        NodeKind::MixedEffectExecute { .. } => FlowNodeKind::MixedEffectExecute,
        NodeKind::Repeat { .. } => FlowNodeKind::Repeat,
        NodeKind::Branch { .. } => FlowNodeKind::Branch,
        NodeKind::Preserved { .. } => FlowNodeKind::Preserved,
    }
}

/// Move `target` (with its children) immediately after `source`, into
/// `source`'s sequence if it lives in another region. Existing order among all
/// other blocks is retained. A block cannot be moved after its own descendant.
pub fn reconnect_after(nodes: &mut Vec<MoveNode>, source: u32, target: u32) -> Result<(), String> {
    if source == target {
        return Err("source and target must be different nodes".into());
    }
    let mut occurrences = HashMap::new();
    count_ids(nodes, &mut occurrences);
    for id in [source, target] {
        match occurrences.get(&id).copied().unwrap_or(0) {
            0 => return Err(format!("node ID {id} does not exist")),
            1 => {}
            _ => {
                return Err(format!(
                    "node ID {id} is ambiguous because it is duplicated"
                ));
            }
        }
    }
    if reorder_in_sequence(nodes, source, target) {
        return Ok(());
    }
    let target_node = find_node(nodes, target).expect("target counted above");
    if find_node(std::slice::from_ref(target_node), source).is_some() {
        return Err("A block cannot follow one of its own children.".into());
    }
    let mut updated = nodes.clone();
    let moved = take_node(&mut updated, target).expect("target counted above");
    insert_after(&mut updated, source, moved).expect("source survives removing a non-ancestor");
    *nodes = updated;
    Ok(())
}

fn insert_after(nodes: &mut Vec<MoveNode>, source: u32, moved: MoveNode) -> Result<(), MoveNode> {
    if let Some(index) = nodes.iter().position(|node| node.id == source) {
        nodes.insert(index + 1, moved);
        return Ok(());
    }
    let mut moved = moved;
    for node in nodes {
        let regions: Vec<&mut Vec<MoveNode>> = match &mut node.kind {
            NodeKind::Repeat { body, .. } => vec![body],
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => vec![then_nodes, else_nodes],
            _ => continue,
        };
        for region in regions {
            match insert_after(region, source, moved) {
                Ok(()) => return Ok(()),
                Err(back) => moved = back,
            }
        }
    }
    Err(moved)
}

fn count_ids(nodes: &[MoveNode], counts: &mut HashMap<u32, usize>) {
    for node in nodes {
        *counts.entry(node.id).or_default() += 1;
        match &node.kind {
            NodeKind::Repeat { body, .. } => count_ids(body, counts),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                count_ids(then_nodes, counts);
                count_ids(else_nodes, counts);
            }
            _ => {}
        }
    }
}

/// Connect a container's typed region to `target`, moving that block (and its
/// children) to the start of the region. Never duplicates execution or forms
/// an ancestor cycle. Rejected changes leave the original tree untouched.
pub fn reconnect_port(
    nodes: &mut Vec<MoveNode>,
    source: u32,
    port: FlowPort,
    target: u32,
) -> Result<(), String> {
    if port == FlowPort::Next {
        return reconnect_after(nodes, source, target);
    }
    let mut occurrences = HashMap::new();
    count_ids(nodes, &mut occurrences);
    if occurrences.values().any(|count| *count != 1) {
        return Err("Node IDs must be unique before connecting regions.".into());
    }
    let source_node = find_node(nodes, source).ok_or("Source block does not exist.")?;
    let target_node = find_node(nodes, target).ok_or("Target block does not exist.")?;
    if source == target || find_node(std::slice::from_ref(target_node), source).is_some() {
        return Err(
            "A block cannot contain itself or one of its ancestors. Use an explicit Repeat block."
                .into(),
        );
    }
    if !matches!(
        (&source_node.kind, port),
        (NodeKind::Repeat { .. }, FlowPort::Repeat)
            | (NodeKind::Branch { .. }, FlowPort::Then | FlowPort::Else)
    ) {
        return Err("This block does not have that execution-region port.".into());
    }
    let mut updated = nodes.clone();
    let moved = take_node(&mut updated, target).ok_or("Target block no longer exists.")?;
    let source = find_node_mut(&mut updated, source).ok_or("Source block no longer exists.")?;
    let destination = match (&mut source.kind, port) {
        (NodeKind::Repeat { body, .. }, FlowPort::Repeat) => body,
        (NodeKind::Branch { then_nodes, .. }, FlowPort::Then) => then_nodes,
        (NodeKind::Branch { else_nodes, .. }, FlowPort::Else) => else_nodes,
        _ => return Err("Execution-region port no longer exists.".into()),
    };
    destination.insert(0, moved);
    *nodes = updated;
    Ok(())
}

fn find_node(nodes: &[MoveNode], id: u32) -> Option<&MoveNode> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        let found = match &node.kind {
            NodeKind::Repeat { body, .. } => find_node(body, id),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => find_node(then_nodes, id).or_else(|| find_node(else_nodes, id)),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

pub(super) fn find_node_mut(nodes: &mut [MoveNode], id: u32) -> Option<&mut MoveNode> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        let found = match &mut node.kind {
            NodeKind::Repeat { body, .. } => find_node_mut(body, id),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => find_node_mut(then_nodes, id).or_else(|| find_node_mut(else_nodes, id)),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn take_node(nodes: &mut Vec<MoveNode>, id: u32) -> Option<MoveNode> {
    if let Some(index) = nodes.iter().position(|node| node.id == id) {
        return Some(nodes.remove(index));
    }
    for node in nodes {
        let found = match &mut node.kind {
            NodeKind::Repeat { body, .. } => take_node(body, id),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => take_node(then_nodes, id).or_else(|| take_node(else_nodes, id)),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn reorder_in_sequence(nodes: &mut Vec<MoveNode>, source: u32, target: u32) -> bool {
    let source_index = nodes.iter().position(|node| node.id == source);
    let target_index = nodes.iter().position(|node| node.id == target);
    if let (Some(source_index), Some(target_index)) = (source_index, target_index) {
        if target_index == source_index + 1 {
            return true;
        }
        let moved = nodes.remove(target_index);
        let source_index = nodes
            .iter()
            .position(|node| node.id == source)
            .expect("source sibling remains after removing target");
        nodes.insert(source_index + 1, moved);
        return true;
    }
    for node in nodes {
        match &mut node.kind {
            NodeKind::Repeat { body, .. } => {
                if reorder_in_sequence(body, source, target) {
                    return true;
                }
            }
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                if reorder_in_sequence(then_nodes, source, target)
                    || reorder_in_sequence(else_nodes, source, target)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Validate editor metadata against the typed graph. This deliberately does not
/// validate executable node semantics; callers must still run timeline/graph validation.
pub fn validate_layout(graph: &MoveGraph) -> Vec<NodeError> {
    let mut errors = Vec::new();
    let mut node_ids = BTreeSet::new();
    for track in &graph.tracks {
        collect_node_ids(&track.nodes, &mut node_ids, &mut errors);
    }
    let known_keys = known_position_keys(graph);

    if graph.positions.len() > MAX_LAYOUT_ITEMS {
        errors.push(layout_error(
            "positions",
            format!("layout item count exceeds {MAX_LAYOUT_ITEMS}"),
        ));
    }
    for (key, position) in &graph.positions {
        if !known_keys.contains(key) {
            errors.push(layout_error(
                format!("positions[{key:?}]"),
                "position key does not identify an existing node, trigger, action, variable or value",
            ));
        }
        if position
            .iter()
            .any(|coordinate| !coordinate.is_finite() || coordinate.abs() > MAX_LAYOUT_COORDINATE)
        {
            errors.push(layout_error(
                format!("positions[{key:?}]"),
                format!("coordinates must be finite and within ±{MAX_LAYOUT_COORDINATE}"),
            ));
        }
    }
    errors
}

/// All valid editor-position keys for this graph, including local variable
/// data ports. Canvas pruning and metadata validation share this inventory so
/// deleted references are not retained as orphaned positions.
pub fn known_position_keys(graph: &MoveGraph) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for track in &graph.tracks {
        collect_position_keys(&track.nodes, &mut keys);
    }
    keys
}

fn collect_position_keys(nodes: &[MoveNode], keys: &mut BTreeSet<String>) {
    for node in nodes {
        keys.insert(format!("node:{}", node.id));
        match &node.kind {
            NodeKind::Repeat { body, .. } => collect_position_keys(body, keys),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                collect_position_keys(then_nodes, keys);
                collect_position_keys(else_nodes, keys);
            }
            _ => {}
        }
    }
}

fn collect_node_ids(nodes: &[MoveNode], ids: &mut BTreeSet<u32>, errors: &mut Vec<NodeError>) {
    for node in nodes {
        if !ids.insert(node.id) {
            errors.push(layout_error(
                format!("node {}", node.id),
                "duplicate node ID makes layout keys ambiguous",
            ));
        }
        match &node.kind {
            NodeKind::Repeat { body, .. } => collect_node_ids(body, ids, errors),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                collect_node_ids(then_nodes, ids, errors);
                collect_node_ids(else_nodes, ids, errors);
            }
            _ => {}
        }
    }
}

fn layout_error(subject: impl Into<String>, reason: impl Into<String>) -> NodeError {
    NodeError {
        subject: subject.into(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{project, reconnect_after, validate_layout, FlowPort};

    #[test]
    fn regional_connections_move_subtrees_and_reject_cycles_without_mutation() {
        let mut nodes = vec![
            super::MoveNode {
                id: 1,
                kind: super::NodeKind::Branch {
                    condition: super::super::model::MoveCondition::Grounded,
                    then_nodes: Vec::new(),
                    else_nodes: Vec::new(),
                },
            },
            super::MoveNode {
                id: 2,
                kind: super::NodeKind::Repeat {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 2,
                    body: vec![super::MoveNode {
                        id: 3,
                        kind: super::NodeKind::Wait { frames: 1.0 },
                    }],
                },
            },
        ];
        super::reconnect_port(&mut nodes, 1, FlowPort::Then, 2).unwrap();
        assert_eq!(nodes.len(), 1);
        let before = serde_json::to_value(&nodes).unwrap();
        assert!(super::reconnect_port(&mut nodes, 2, FlowPort::Repeat, 1).is_err());
        assert_eq!(serde_json::to_value(&nodes).unwrap(), before);
        super::reconnect_port(&mut nodes, 1, FlowPort::Else, 2).unwrap();
        let super::NodeKind::Branch {
            then_nodes,
            else_nodes,
            ..
        } = &nodes[0].kind
        else {
            panic!("branch retained");
        };
        assert!(then_nodes.is_empty());
        assert_eq!(else_nodes[0].id, 2);
        assert_eq!(super::find_node(else_nodes, 3).unwrap().id, 3);
    }
    use crate::move_nodes::model::{
        MoveCondition, MoveGraph, MoveNode, MoveTrack, NodeKind, Track,
    };

    fn frame(id: u32) -> MoveNode {
        MoveNode {
            id,
            kind: NodeKind::Frame { frame: id as f32 },
        }
    }

    fn branch(id: u32, then_nodes: Vec<MoveNode>, else_nodes: Vec<MoveNode>) -> MoveNode {
        MoveNode {
            id,
            kind: NodeKind::Branch {
                condition: MoveCondition::Grounded,
                then_nodes,
                else_nodes,
            },
        }
    }

    #[test]
    fn reconnect_reorders_target_after_source_and_preserves_the_rest() {
        let mut nodes = vec![frame(1), frame(2), frame(3), frame(4)];
        reconnect_after(&mut nodes, 3, 1).unwrap();
        assert_eq!(
            nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
            [2, 3, 1, 4]
        );
    }

    #[test]
    fn reconnect_rejects_self_missing_and_ancestor_links() {
        let mut nodes = vec![branch(1, vec![frame(2)], vec![frame(3)])];
        assert!(reconnect_after(&mut nodes, 9, 9)
            .unwrap_err()
            .contains("different"));
        assert!(reconnect_after(&mut nodes, 1, 9)
            .unwrap_err()
            .contains("does not exist"));
        let before = format!("{nodes:?}");
        assert!(reconnect_after(&mut nodes, 2, 1)
            .unwrap_err()
            .contains("children"));
        assert_eq!(format!("{nodes:?}"), before);
    }

    #[test]
    fn reconnect_moves_a_block_across_regions() {
        // Into an arm: the wire a user draws to build a branch body one block at a time.
        let mut nodes = vec![branch(1, vec![frame(2)], vec![frame(3)]), frame(4)];
        reconnect_after(&mut nodes, 2, 4).unwrap();
        assert_eq!(
            format!("{nodes:?}"),
            format!(
                "{:?}",
                vec![branch(1, vec![frame(2), frame(4)], vec![frame(3)])]
            )
        );
        // Between arms.
        reconnect_after(&mut nodes, 2, 3).unwrap();
        assert_eq!(
            format!("{nodes:?}"),
            format!(
                "{:?}",
                vec![branch(1, vec![frame(2), frame(3), frame(4)], vec![])]
            )
        );
        // Out of an arm, after its own branch.
        reconnect_after(&mut nodes, 1, 2).unwrap();
        assert_eq!(
            format!("{nodes:?}"),
            format!(
                "{:?}",
                vec![branch(1, vec![frame(3), frame(4)], vec![]), frame(2)]
            )
        );
    }

    #[test]
    fn nested_reconnect_preserves_branch_contents_and_projection_ports() {
        let mut nodes = vec![branch(1, vec![frame(2), frame(3)], vec![frame(4)])];
        reconnect_after(&mut nodes, 3, 2).unwrap();
        let projection = project(&MoveGraph {
            tracks: vec![MoveTrack {
                track: Track::Game,
                effect_execute_baseline: Vec::new(),
                nodes,
                baseline: Vec::new(),
            }],
            ..MoveGraph::default()
        });
        assert_eq!(
            projection
                .nodes
                .iter()
                .map(|node| node.id)
                .collect::<Vec<_>>(),
            [1, 3, 2, 4]
        );
        assert!(projection
            .edges
            .iter()
            .any(|edge| edge.source == 1 && edge.target == 3 && edge.port == FlowPort::Then));
        assert!(projection
            .edges
            .iter()
            .any(|edge| edge.source == 3 && edge.target == 2 && edge.port == FlowPort::Next));
        assert!(projection
            .edges
            .iter()
            .any(|edge| edge.source == 1 && edge.target == 4 && edge.port == FlowPort::Else));
        assert_eq!(projection.nodes[1].parent, Some((1, FlowPort::Then)));
    }

    #[test]
    fn legacy_graph_defaults_positions_and_layout_rejects_bad_metadata() {
        let legacy: MoveGraph = serde_json::from_str(r#"{"version":1,"tracks":[]}"#).unwrap();
        assert!(legacy.positions.is_empty());

        let mut graph = MoveGraph::default();
        graph.tracks.push(MoveTrack {
            track: Track::Game,
            effect_execute_baseline: Vec::new(),
            nodes: vec![frame(7)],
            baseline: Vec::new(),
        });
        graph.positions = BTreeMap::from([
            ("node:7".to_string(), [f32::NAN, 0.0]),
            ("node:999".to_string(), [0.0, 0.0]),
            ("bad".to_string(), [0.0, 0.0]),
        ]);
        let errors = validate_layout(&graph);
        assert_eq!(errors.len(), 3);
    }
}
