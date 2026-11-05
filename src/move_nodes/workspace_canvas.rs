use super::*;
use crate::gimmicks::behavior::BehaviorGraph;
use crate::gimmicks::{
    field_canvas::FieldLayout,
    field_projection::{self, FieldEdgeKind, FieldRef, TransitionEndpoint},
};
use crate::mod_project::ControllerChart;
use crate::move_nodes::view::workspace::PaletteBlock;
use std::collections::{BTreeMap, HashSet};

/// One horizontal band of the workspace field: a script track, the move's events, or the character.
#[derive(Clone)]
pub(in crate::move_nodes) struct Lane {
    pub track: Option<Track>,
    pub label: String,
    pub top: f32,
    pub bottom: f32,
}

/// (motion frame, field x) of each shared frame column.
pub(in crate::move_nodes) type FrameMarks = Vec<(f32, f32)>;

const LANE_HEADER: f32 = 34.0;
const ROW_HEIGHT: f32 = 92.0;
const GAP: f32 = 32.0;

/// Frame and Wait cards repeat what the frame ruler shows, so the workspace draws them narrow.
fn compact_timing(block: &mut Block) -> bool {
    let title = match block.title.as_str() {
        "At frame" => block.detail.clone(),
        "Wait" => format!("Wait {}", block.detail.trim_end_matches(" motion frames")),
        _ => return false,
    };
    block.title = title;
    block.detail.clear();
    true
}

/// The motion frame each block starts on, following Frame/Wait/Repeat the way the script runs.
pub(in crate::move_nodes) fn start_frames(
    nodes: &[MoveNode],
    frame: &mut f32,
    out: &mut BTreeMap<u32, f32>,
) {
    for node in nodes {
        if let NodeKind::Frame { frame: at } | NodeKind::NativeExecuteFrame { frame: at } =
            &node.kind
        {
            *frame = frame.max(*at);
        }
        out.insert(node.id, *frame);
        match &node.kind {
            NodeKind::Wait { frames } => *frame += frames.max(0.0),
            NodeKind::Repeat { count, body, .. } => {
                let start = *frame;
                start_frames(body, frame, out);
                *frame = start + (*frame - start) * (*count).max(1) as f32;
            }
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                let mut other = *frame;
                start_frames(then_nodes, frame, out);
                start_frames(else_nodes, &mut other, out);
                *frame = frame.max(other);
            }
            _ => {}
        }
    }
}

/// Lays every track out as a swimlane sized to its deepest branch. All lanes share one frame
/// axis: blocks that start on the same motion frame line up vertically, and a frame gets as many
/// columns as its busiest lane needs. The move's own events and variables follow; character
/// controllers go below `lanes`. Also returns the x of each frame column for the ruler.
pub(in crate::move_nodes) fn project_workspace(
    graph: &MoveGraph,
) -> (
    Vec<Block>,
    Vec<Wire>,
    Vec<Lane>,
    FrameMarks,
    HashSet<String>,
) {
    let (_, mut wires) = project_structure(graph, None);
    let mut compact = HashSet::new();
    let mut timed_lanes = Vec::new();
    for (track, label) in [
        (Track::Game, crate::move_nodes::plain::LANES[0]),
        (Track::Effect, crate::move_nodes::plain::LANES[1]),
        (Track::Sound, crate::move_nodes::plain::LANES[2]),
        (Track::Expression, crate::move_nodes::plain::LANES[3]),
    ] {
        let mut frames = BTreeMap::new();
        for source in graph.tracks.iter().filter(|source| source.track == track) {
            start_frames(&source.nodes, &mut 0.0, &mut frames);
        }
        // Rows and columns come from the script. A dragged block keeps its own place and must not
        // stretch the lanes for everything else.
        let (timed, edges) = project_structure(graph, Some(track));
        let placed: Vec<(Block, usize, f32)> = timed
            .into_iter()
            .filter(|block| block.key.starts_with("node:"))
            .map(|mut block| {
                if compact_timing(&mut block) {
                    compact.insert(block.key.clone());
                }
                let depth = (block.position[0] / 230.0).round() as usize;
                let frame = block.key[5..]
                    .parse()
                    .ok()
                    .and_then(|id| frames.get(&id).copied())
                    .unwrap_or(0.0);
                (block, depth, frame)
            })
            .collect();
        wires.extend(
            edges
                .into_iter()
                .filter(|wire| wire.from.starts_with("node:")),
        );
        timed_lanes.push((track, label, placed));
    }
    let step = |block: &Block| {
        (if compact.contains(&block.key) {
            COMPACT_CARD
        } else {
            CARD
        }) + GAP
    };
    // Offset of each block within its frame column. Rows are nesting depths, so a branch body
    // starts right of the block that opens it instead of back at the column edge.
    let timed_lanes: Vec<_> = timed_lanes
        .into_iter()
        .map(|(track, label, placed)| {
            let mut used: BTreeMap<(usize, u32), f32> = BTreeMap::new();
            let mut previous: Option<(usize, u32, f32)> = None;
            let placed: Vec<_> = placed
                .into_iter()
                .map(|(block, depth, frame)| {
                    let mut offset = used.get(&(depth, frame.to_bits())).copied().unwrap_or(0.0);
                    if let Some((above, bits, after)) = previous {
                        if depth > above && bits == frame.to_bits() {
                            offset = offset.max(after);
                        }
                    }
                    let after = offset + step(&block);
                    used.insert((depth, frame.to_bits()), after);
                    previous = Some((depth, frame.to_bits(), after));
                    (block, depth, frame, offset)
                })
                .collect();
            (track, label, placed)
        })
        .collect();
    // Width of each frame column: the furthest any lane row reaches within that frame.
    let mut widths: Vec<(f32, f32)> = Vec::new();
    for (_, _, placed) in &timed_lanes {
        for (block, _, frame, offset) in placed {
            let reach = offset + step(block);
            match widths.iter_mut().find(|(seen, _)| seen == frame) {
                Some((_, width)) => *width = width.max(reach),
                None => widths.push((*frame, reach)),
            }
        }
    }
    widths.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut marks = Vec::new();
    let mut x = 0.0;
    for (frame, width) in &widths {
        marks.push((*frame, x));
        x += width;
    }
    let column = |frame: f32| {
        marks
            .iter()
            .find(|(at, _)| *at == frame)
            .map_or(0.0, |(_, x)| *x)
    };
    let mut blocks = Vec::new();
    let mut lanes = Vec::new();
    let mut top = 0.0;
    for (track, label, placed) in timed_lanes {
        let mut depth_max = 0;
        // An empty script stays a thin drop target so the lanes in use get the room.
        let rows = if placed.is_empty() { 0.4 } else { 1.0 };
        for (mut block, depth, frame, offset) in placed {
            block.position = [
                column(frame) + offset,
                top + LANE_HEADER + depth as f32 * ROW_HEIGHT,
            ];
            depth_max = depth_max.max(depth);
            blocks.push(block);
        }
        let bottom = top + LANE_HEADER + (depth_max as f32 + rows) * ROW_HEIGHT + 8.0;
        lanes.push(Lane {
            track: Some(track),
            label: label.into(),
            top,
            bottom,
        });
        top = bottom;
    }
    (blocks, wires, lanes, marks, compact)
}

fn character_ref(key: &str) -> Option<(String, FieldRef)> {
    let (chart, reference) = key.strip_prefix("character:")?.split_once(':')?;
    Some((chart.to_owned(), FieldRef::from_key(reference).ok()?))
}

impl CanvasState {
    pub(in crate::move_nodes) fn fit_to_view(&mut self) {
        self.fit = true;
    }
    /// The script track whose lane contains a field point, for drops and clicks.
    pub(in crate::move_nodes) fn lane_track_at(&self, y: f32) -> Option<Track> {
        self.lanes
            .iter()
            .find(|lane| (lane.top..lane.bottom).contains(&y))
            .and_then(|lane| lane.track)
    }
    /// Selects one block alone, as after inserting it, so no earlier group outline lingers.
    pub(in crate::move_nodes) fn select(&mut self, key: String) {
        self.selection.clear();
        self.selection.insert(key.clone());
        self.selected = Some(key);
    }
    pub(in crate::move_nodes) fn selected_character(&self) -> Option<(String, FieldRef)> {
        character_ref(self.selected.as_deref()?)
    }
    pub(in crate::move_nodes) fn set_character(
        &mut self,
        character: Option<(
            &BehaviorGraph,
            &BTreeMap<String, ControllerChart>,
            &FieldLayout,
        )>,
    ) {
        if let Some((main, charts, layout)) = character {
            self.character_graphs.clone_from(charts);
            if !main.states.is_empty() || !main.vars.is_empty() || !main.transitions.is_empty() {
                self.character_graphs.insert(
                    "main".into(),
                    ControllerChart {
                        graph: main.clone(),
                        layout: layout.clone(),
                    },
                );
            }
        } else {
            self.character_graphs.clear();
        }
    }
    pub(in crate::move_nodes) fn update_character_layout(
        &self,
        main: &mut BehaviorGraph,
        layout: &mut FieldLayout,
        charts: &mut BTreeMap<String, ControllerChart>,
    ) {
        for (id, chart) in &self.character_graphs {
            if id == "main" {
                *main = chart.graph.clone();
                *layout = chart.layout.clone();
            } else {
                charts.insert(id.clone(), chart.clone());
            }
        }
    }
    /// Controller layouts are stored relative to their own band, so they move with it
    /// when the lanes above grow or shrink.
    pub(in crate::move_nodes) fn place_character(&mut self, key: &str, position: [f32; 2]) {
        if let Some((id, reference)) = character_ref(key) {
            let top = self.chart_tops.get(&id).copied().unwrap_or_default();
            if let Some(chart) = self.character_graphs.get_mut(&id) {
                chart
                    .layout
                    .positions
                    .insert(reference.key(), [position[0], position[1] - top]);
            }
        }
    }
    pub(in crate::move_nodes) fn position(&self, graph: &MoveGraph, key: &str) -> Option<[f32; 2]> {
        if let Some((id, reference)) = character_ref(key) {
            let top = self.chart_tops.get(&id).copied().unwrap_or_default();
            self.character_graphs
                .get(&id)?
                .layout
                .positions
                .get(&reference.key())
                .map(|position| [position[0], position[1] + top])
        } else {
            graph.positions.get(key).copied()
        }
    }
    pub(in crate::move_nodes) fn set_position(
        &mut self,
        graph: &mut MoveGraph,
        key: &str,
        position: [f32; 2],
    ) {
        if key.starts_with("character:") {
            self.place_character(key, position);
        } else {
            graph.positions.insert(key.to_owned(), position);
        }
    }
    /// Projects each character controller into its own band below the move lanes.
    pub(super) fn project_characters(
        &mut self,
        graph: &MoveGraph,
        blocks: &mut Vec<Block>,
        wires: &mut Vec<Wire>,
    ) {
        let mut top = self.lanes.last().map_or(0.0, |lane| lane.bottom);
        let mut charts = std::mem::take(&mut self.character_graphs);
        for (chart_id, chart) in &mut charts {
            let Ok(projection) = field_projection::project(&chart.graph) else {
                continue;
            };
            let key = |reference: &FieldRef| format!("character:{chart_id}:{}", reference.key());
            // Default layout flows left to right: each node sits one column after the
            // furthest node feeding it, and a column stacks its nodes downward.
            let keys: Vec<String> = projection
                .nodes
                .iter()
                .map(|node| node.reference.key())
                .collect();
            let edges: Vec<(usize, usize)> = projection
                .edges
                .iter()
                .filter_map(|edge| {
                    let index = |reference: &Option<FieldRef>| {
                        let reference = reference.as_ref()?.key();
                        keys.iter().position(|key| *key == reference)
                    };
                    Some((index(&edge.from)?, index(&edge.to)?))
                })
                .filter(|(from, to)| from != to)
                .collect();
            let mut column = vec![0usize; keys.len()];
            for _ in 0..keys.len() {
                let mut changed = false;
                for (from, to) in &edges {
                    // Bounded relaxation: cycles stop growing once every node has been visited.
                    if column[*to] < column[*from] + 1 && column[*from] + 1 < keys.len() {
                        column[*to] = column[*from] + 1;
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            let mut rows = vec![0usize; keys.len()];
            let mut placement = Vec::with_capacity(keys.len());
            for column in &column {
                placement.push((*column, rows[*column]));
                rows[*column] += 1;
            }
            let tallest = rows.iter().copied().max().unwrap_or(1).max(1);
            let had_positions = !chart.layout.positions.is_empty();
            let anchor = self
                .selected
                .as_deref()
                .and_then(character_ref)
                .filter(|(id, _)| id == chart_id)
                .and_then(|(_, reference)| chart.layout.positions.get(&reference.key()).copied())
                .map(|p| [p[0] + 240.0, p[1]])
                .unwrap_or_else(|| {
                    let center = self.viewport.unwrap_or(Vec2::new(640.0, 480.0)) * 0.5;
                    let world = (center - self.pan) / self.zoom.max(0.85);
                    [world.x, (world.y - top - LANE_HEADER).max(0.0)]
                });
            let mut added = 0;
            for (index, reference) in keys.iter().enumerate() {
                chart
                    .layout
                    .positions
                    .entry(reference.clone())
                    .or_insert_with(|| {
                        if had_positions {
                            let position = [anchor[0], anchor[1] + added as f32 * 110.0];
                            added += 1;
                            position
                        } else {
                            [
                                placement[index].0 as f32 * 240.0,
                                placement[index].1 as f32 * 110.0,
                            ]
                        }
                    });
            }

            for (index, node) in projection.nodes.iter().enumerate() {
                let (title, mut detail, color) =
                    crate::gimmicks::field_canvas::describe(&chart.graph, &node.reference);
                if let Some(error) = projection
                    .diagnostics
                    .iter()
                    .find(|d| d.reference == node.reference)
                {
                    detail = format!("{detail} · {}", error.message);
                }
                blocks.push(Block {
                    internal: "",
                    key: key(&node.reference),
                    title: crate::move_nodes::plain::block_name(&title).to_owned(),
                    detail,
                    position: [
                        placement[index].0 as f32 * 240.0,
                        top + LANE_HEADER + placement[index].1 as f32 * 110.0,
                    ],
                    color,
                    ports: match node.reference {
                        FieldRef::Variable(_) => vec![OutputPort::Value],
                        _ => vec![FlowPort::Next.into()],
                    },
                    inputs: match node.reference {
                        FieldRef::SourceBinding(field_projection::SourceBindingRef::Event(_)) => {
                            vec![OutputPort::Event]
                        }
                        _ => vec![FlowPort::Next.into()],
                    },
                });
            }
            let stored = chart
                .layout
                .positions
                .values()
                .map(|position| position[1] + 100.0)
                .fold(0.0, f32::max);
            self.chart_tops.insert(chart_id.clone(), top + LANE_HEADER);
            let bottom = top + LANE_HEADER + (tallest as f32 * 110.0).max(stored);
            self.lanes.push(Lane {
                track: None,
                label: if chart_id == "main" {
                    "Character".into()
                } else {
                    format!("Character · {chart_id}")
                },
                top,
                bottom,
            });
            top = bottom;
            for edge in &projection.edges {
                if let (Some(from), Some(to)) = (&edge.from, &edge.to) {
                    wires.push(Wire {
                        from: key(from),
                        to: key(to),
                        label: "",
                        port: if matches!(from, FieldRef::Variable(_)) {
                            OutputPort::Value
                        } else {
                            FlowPort::Next.into()
                        },
                        input: if edge.kind == FieldEdgeKind::EventDispatch {
                            OutputPort::Event
                        } else {
                            FlowPort::Next.into()
                        },
                    });
                }
            }
            for (binding_index, binding) in chart
                .graph
                .emission_plan
                .event_bindings
                .iter()
                .enumerate()
                .filter(|(_, binding)| binding.fields.is_empty())
            {
                let target = key(&FieldRef::SourceBinding(
                    field_projection::SourceBindingRef::Event(binding_index),
                ));
                for track in &graph.tracks {
                    fn emitters(
                        nodes: &[MoveNode],
                        chart_id: &str,
                        name: &str,
                        out: &mut Vec<u32>,
                    ) {
                        for node in nodes {
                            match &node.kind {
                                NodeKind::Execute { commands } => {
                                    if commands.iter().any(|command| match command {
                                        crate::data::ExcuteStmt::GimmickEvent { name: event } => {
                                            chart_id == "main" && event == name
                                        }
                                        crate::data::ExcuteStmt::GimmickChartEvent {
                                            chart_id: chart,
                                            name: event,
                                        } => chart == chart_id && event == name,
                                        _ => false,
                                    }) {
                                        out.push(node.id);
                                    }
                                }
                                NodeKind::Branch {
                                    then_nodes,
                                    else_nodes,
                                    ..
                                } => {
                                    emitters(then_nodes, chart_id, name, out);
                                    emitters(else_nodes, chart_id, name, out);
                                }
                                NodeKind::Repeat { body, .. } => {
                                    emitters(body, chart_id, name, out)
                                }
                                _ => {}
                            }
                        }
                    }
                    let mut ids = Vec::new();
                    emitters(&track.nodes, chart_id, &binding.name, &mut ids);
                    for id in ids {
                        wires.push(Wire {
                            from: format!("node:{id}"),
                            to: target.clone(),
                            label: "Event",
                            port: OutputPort::Event,
                            input: OutputPort::Event,
                        });
                    }
                }
            }
        }
        self.character_graphs = charts;
    }
    pub(in crate::move_nodes) fn take_palette_drop(
        &mut self,
    ) -> Option<(PaletteBlock, [f32; 2], Option<String>)> {
        self.palette_drop.take()
    }
    pub(in crate::move_nodes) fn insertion_position(&self) -> [f32; 2] {
        let size = self
            .field_area
            .map_or(Vec2::new(600.0, 500.0), |area| area.size());
        let point = (size * 0.5 - self.pan) / self.zoom.max(0.001);
        [point.x, point.y]
    }
    pub(in crate::move_nodes) fn connect_inserted(
        &mut self,
        graph: &mut MoveGraph,
        from: &str,
        to: &str,
    ) -> Result<(), String> {
        let port = self
            .insertion_port
            .take()
            .filter(|(source, _)| source == from)
            .map_or(OutputPort::Flow(FlowPort::Next), |(_, port)| port);
        if from.starts_with("node:") && !to.starts_with("node:") && port != OutputPort::Event {
            return Err("A move execution output requires a block in the same track.".into());
        }
        self.reconnect_any(graph, from, port, to)
    }
    pub(in crate::move_nodes) fn quick_add_position(&self) -> Option<Pos2> {
        self.quick_add.as_ref().map(|(position, ..)| *position)
    }
    pub(in crate::move_nodes) fn quick_add_source(&self) -> Option<&str> {
        self.quick_add
            .as_ref()
            .map(|(_, _, source, _)| source.as_str())
    }
    pub(in crate::move_nodes) fn quick_add_accepts(&self, block: &PaletteBlock) -> bool {
        let Some((_, _, source, port)) = &self.quick_add else {
            return false;
        };
        match port {
            OutputPort::Flow(_) if source.starts_with("node:") => {
                matches!(block, PaletteBlock::Node(_) | PaletteBlock::Native(..))
            }
            _ => false,
        }
    }
    pub(in crate::move_nodes) fn cancel_quick_add(&mut self) {
        self.quick_add = None;
        self.insertion_port = None;
    }
    pub(in crate::move_nodes) fn finish_quick_add(&mut self) -> ([f32; 2], Option<String>) {
        if let Some((_, position, source, port)) = self.quick_add.take() {
            self.insertion_port = Some((source.clone(), port));
            (position, Some(source))
        } else {
            (self.insertion_position(), None)
        }
    }
    pub(in crate::move_nodes) fn take_lane_clear(&mut self) -> Option<Track> {
        self.lane_clear.take()
    }
    pub(in crate::move_nodes) fn take_edit_command(&mut self) -> Option<EditCommand> {
        self.edit_command.take()
    }
    pub(in crate::move_nodes) fn selection(&self) -> Vec<String> {
        if self.selection.is_empty() {
            self.selected.iter().cloned().collect()
        } else {
            self.selection.iter().cloned().collect()
        }
    }

    pub(super) fn reconnect_any(
        &mut self,
        graph: &mut MoveGraph,
        from: &str,
        port: OutputPort,
        to: &str,
    ) -> Result<(), String> {
        if from.starts_with("character:") && to.starts_with("character:") {
            return self.reconnect_character(&self.fighter.clone(), from, to);
        }
        if let Some((
            chart,
            FieldRef::SourceBinding(field_projection::SourceBindingRef::Event(index)),
        )) = character_ref(to)
        {
            if port != OutputPort::Event {
                return Err("Connect an Emit event output to this receiver.".into());
            }
            let binding = self
                .character_graphs
                .get(&chart)
                .and_then(|chart| chart.graph.emission_plan.event_bindings.get(index))
                .ok_or("Event receiver no longer exists")?;
            if !binding.fields.is_empty() {
                return Err(
                    "This receiver needs a typed payload; edit its event block's arguments.".into(),
                );
            }
            let id: u32 = from
                .strip_prefix("node:")
                .ok_or("Use a move event block for controller dispatch")?
                .parse()
                .map_err(|_| "Invalid block")?;
            let node = graph
                .tracks
                .iter_mut()
                .find_map(|track| super::super::flow::find_node_mut(&mut track.nodes, id))
                .ok_or("Move event block no longer exists")?;
            let NodeKind::Execute { commands } = &mut node.kind else {
                return Err("Choose an Emit event block".into());
            };
            let [command] = commands.as_mut_slice() else {
                return Err("Select a block containing only one event dispatch".into());
            };
            if !matches!(
                command,
                crate::data::ExcuteStmt::GimmickEvent { .. }
                    | crate::data::ExcuteStmt::GimmickChartEvent { .. }
            ) {
                return Err("Choose an Emit event block".into());
            }
            *command = if chart == "main" {
                crate::data::ExcuteStmt::GimmickEvent {
                    name: binding.name.clone(),
                }
            } else {
                crate::data::ExcuteStmt::GimmickChartEvent {
                    chart_id: chart,
                    name: binding.name.clone(),
                }
            };
            return Ok(());
        }
        reconnect(graph, from, port, to)
    }

    pub(in crate::move_nodes) fn reconnect_character(
        &mut self,
        fighter: &str,
        from: &str,
        to: &str,
    ) -> Result<(), String> {
        let (chart, source) = character_ref(from).ok_or("Choose a controller source")?;
        let (target_chart, target) = character_ref(to).ok_or("Choose a controller destination")?;
        if chart != target_chart {
            return Err("State and action connections stay in one controller; use named events between controllers.".into());
        }
        let controller = self
            .character_graphs
            .get_mut(&chart)
            .ok_or("Controller no longer exists")?;
        let graph = &controller.graph;
        let candidate = match (&source, &target) {
            (FieldRef::State(name), FieldRef::Transition(index)) => {
                field_projection::connect_state(
                    graph,
                    fighter,
                    name,
                    *index,
                    TransitionEndpoint::From,
                )
            }
            (FieldRef::Transition(index), FieldRef::State(name)) => {
                field_projection::connect_state(
                    graph,
                    fighter,
                    name,
                    *index,
                    TransitionEndpoint::To,
                )
            }
            (FieldRef::Variable(name), FieldRef::Action { transition, index }) => {
                field_projection::connect_variable(graph, fighter, name, *transition, *index)
            }
            _ => field_projection::connect_action_sequence(graph, fighter, &source, &target),
        }?;
        controller.graph = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;
    #[test]
    fn adding_character_node_preserves_existing_positions() {
        let mut canvas = CanvasState::default();
        let mut graph = BehaviorGraph {
            initial: "Start".into(),
            ..Default::default()
        };
        graph.states.push(crate::gimmicks::behavior::StateDef {
            name: "Start".into(),
            status: None,
        });
        canvas.character_graphs.insert(
            "main".into(),
            ControllerChart {
                graph,
                ..Default::default()
            },
        );
        canvas.project_characters(&MoveGraph::default(), &mut Vec::new(), &mut Vec::new());
        let saved = canvas.character_graphs["main"].layout.positions.clone();
        canvas
            .character_graphs
            .get_mut("main")
            .unwrap()
            .graph
            .states
            .push(crate::gimmicks::behavior::StateDef {
                name: "Next".into(),
                status: None,
            });
        canvas.project_characters(&MoveGraph::default(), &mut Vec::new(), &mut Vec::new());
        for (key, position) in saved {
            assert_eq!(
                canvas.character_graphs["main"].layout.positions[&key],
                position
            );
        }
        assert!(canvas.character_graphs["main"]
            .layout
            .positions
            .contains_key("state/Next"));
    }

    #[test]
    fn single_state_is_visible_with_or_without_status() {
        let render = |owned: bool| {
            let mut graph = BehaviorGraph {
                initial: "Start".into(),
                ..Default::default()
            };
            graph.states.push(crate::gimmicks::behavior::StateDef {
                name: "Start".into(),
                status: owned.then(|| crate::gimmicks::status_model::StatusBinding {
                    target: "FIGHTER_STATUS_KIND_SPECIAL_S".into(),
                    policy: crate::gimmicks::status_model::Policy::Owned,
                    roots: Default::default(),
                }),
            });
            let mut canvas = CanvasState::default();
            canvas.character_graphs.insert(
                "special".into(),
                ControllerChart {
                    graph,
                    ..Default::default()
                },
            );
            let mut blocks = Vec::new();
            let mut wires = Vec::new();
            canvas.project_characters(&MoveGraph::default(), &mut blocks, &mut wires);
            blocks
                .iter()
                .any(|block| block.key == "character:special:state/Start")
        };
        assert!(render(false));
        assert!(render(true));
    }
}
