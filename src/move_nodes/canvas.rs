//! Visual projection of the typed move/trigger IR. Wires edit execution order, not decoration.
#[path = "workspace_canvas.rs"]
mod workspace_canvas;
pub(in crate::move_nodes) use workspace_canvas::start_frames;
use workspace_canvas::{project_workspace, FrameMarks, Lane};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EditCommand {
    Copy,
    Duplicate,
    Delete,
}

use egui::{Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use super::flow::FlowPort;
use super::model::{MoveGraph, MoveNode, NodeKind, Track};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum OutputPort {
    Flow(FlowPort),
    Event,
    Value,
}

impl OutputPort {
    fn is_value(self) -> bool {
        self == Self::Value
    }
}

impl From<FlowPort> for OutputPort {
    fn from(port: FlowPort) -> Self {
        Self::Flow(port)
    }
}

#[derive(Default)]
pub struct CanvasState {
    pub selected: Option<String>,
    pub expanded: bool,
    pub(super) workspace: bool,
    pub(super) fighter: String,
    selection: std::collections::BTreeSet<String>,
    marquee: Option<Pos2>,
    collapsed: std::collections::BTreeSet<String>,
    edit_command: Option<EditCommand>,
    character_graphs: std::collections::BTreeMap<String, crate::mod_project::ControllerChart>,
    field_area: Option<Rect>,
    lanes: Vec<Lane>,
    /// Motion frame the preview is showing; drawn across the script lanes.
    pub(super) playhead: Option<f32>,
    /// Keep the playhead in view while the preview plays.
    pub(super) follow_playhead: bool,
    /// Frame picked on the ruler, for the preview to jump to.
    pub(super) scrub: Option<f32>,
    lane_menu: Option<Track>,
    lane_clear: Option<Track>,
    frame_marks: FrameMarks,
    /// Workspace Frame/Wait cards drawn narrow because the ruler already names their frame.
    compact_cards: std::collections::HashSet<String>,
    /// Field y where each character controller's band content starts.
    chart_tops: std::collections::BTreeMap<String, f32>,
    palette_drop: Option<(
        super::view::workspace::PaletteBlock,
        [f32; 2],
        Option<String>,
    )>,
    insertion_port: Option<(String, OutputPort)>,
    quick_add: Option<(Pos2, [f32; 2], String, OutputPort)>,
    pan: Vec2,
    zoom: f32,
    connecting: Option<(String, OutputPort)>,
    error: String,
    fit: bool,
    hover_target: Option<String>,
    hover_error: String,
    viewport: Option<Vec2>,
    search: String,
    center_on: Option<String>,
    /// Pan just enough to bring this block into view (after inserting it off-screen).
    pub(super) reveal: Option<String>,
}

struct Block {
    internal: &'static str,
    key: String,
    title: String,
    detail: String,
    position: [f32; 2],
    color: Color32,
    ports: Vec<OutputPort>,
    inputs: Vec<OutputPort>,
}

struct Wire {
    from: String,
    to: String,
    label: &'static str,
    port: OutputPort,
    input: OutputPort,
}

const TIME: Color32 = Color32::from_rgb(116, 171, 230);
const FLOW: Color32 = Color32::from_rgb(187, 145, 230);
const ACTION: Color32 = Color32::from_rgb(98, 199, 158);
const EVENT: Color32 = Color32::from_rgb(236, 181, 99);
const VALUE: Color32 = Color32::from_rgb(91, 203, 205);
const MIN_ZOOM: f32 = 0.0001;

impl CanvasState {
    pub fn center_selected(&mut self) {
        self.center_on = self.selected.clone();
    }

    fn reconcile_selection(&mut self, blocks: &[Block]) {
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| !blocks.iter().any(|block| block.key == *selected))
        {
            self.selected = None;
            self.center_on = None;
        }
    }

    pub fn reset(&mut self) {
        *self = Self {
            // Changing tracks clears transient selection/wiring, not the surrounding workspace.
            expanded: self.expanded,
            workspace: self.workspace,
            fit: true,
            ..Self::default()
        };
    }

    pub fn selected_node(&self) -> Option<u32> {
        self.selected
            .as_deref()?
            .strip_prefix("node:")?
            .parse()
            .ok()
    }

    pub(super) fn clear_errors(&mut self) {
        self.error.clear();
        self.hover_error.clear();
        self.hover_target = None;
        self.connecting = None;
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        graph: &mut MoveGraph,
        track: Option<Track>,
        height: f32,
    ) {
        if self.zoom == 0.0 {
            self.zoom = 1.0;
            self.fit = true;
        }
        let (mut blocks, mut wires) = if self.workspace {
            let (blocks, wires, lanes, frame_marks, compact) = project_workspace(graph);
            self.lanes = lanes;
            self.frame_marks = frame_marks;
            self.compact_cards = compact;
            (blocks, wires)
        } else {
            project_with_context(graph, track)
        };
        if self.workspace {
            self.project_characters(graph, &mut blocks, &mut wires);
        }
        for block in &mut blocks {
            if self.collapsed.contains(&block.key) {
                block.detail.clear();
            }
        }
        self.reconcile_selection(&blocks);
        if self.workspace
            && !ui.ctx().text_edit_focused()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F))
        {
            self.fit = true;
        }
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                self.zoom = (self.zoom / 1.25).max(MIN_ZOOM);
            }
            if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                self.zoom = (self.zoom * 1.25).min(1.8);
            }
            if ui.small_button("Fit").clicked() {
                self.fit = true;
            }
            if ui.small_button("Arrange").clicked() {
                for block in &blocks {
                    graph.positions.remove(&block.key);
                }
                self.fit = true;
            }
            if ui
                .add_enabled(self.selected.is_some(), egui::Button::new("Center").small())
                .on_hover_text("Center the selected block at a readable scale")
                .clicked()
            {
                self.center_on = self.selected.clone();
            }
            egui::ComboBox::from_id_salt("find_field_block")
                .selected_text("Find block")
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Name or value…")
                            .desired_width(180.0),
                    )
                    .request_focus();
                    let query = self.search.trim().to_ascii_lowercase();
                    egui::ScrollArea::vertical()
                        .max_height(240.0)
                        .show(ui, |ui| {
                            for block in &blocks {
                                if !format!("{} {} {}", block.title, block.detail, block.key)
                                    .to_ascii_lowercase()
                                    .contains(&query)
                                {
                                    continue;
                                }
                                if ui
                                    .selectable_label(
                                        self.selected.as_ref() == Some(&block.key),
                                        if block.detail.is_empty() {
                                            block.title.clone()
                                        } else {
                                            format!("{} · {}", block.title, block.detail)
                                        },
                                    )
                                    .clicked()
                                {
                                    self.selected = Some(block.key.clone());
                                    self.center_on = Some(block.key.clone());
                                    ui.close();
                                }
                            }
                        });
                });
            if !self.workspace
                && ui
                    .small_button(if self.expanded {
                        "Close field"
                    } else {
                        "Open field"
                    })
                    .clicked()
            {
                self.expanded = !self.expanded;
                self.fit = true;
            }
        });
        super::view::node_edit_error(ui, &self.error);
        let height = (height - if self.error.is_empty() { 0.0 } else { 30.0 }).max(1.0);
        let (area, background) = ui.allocate_exact_size(
            Vec2::new(ui.available_width().max(1.0), height),
            Sense::click_and_drag(),
        );
        if self
            .viewport
            .is_some_and(|size| (size - area.size()).length() > 32.0)
        {
            self.fit = true;
        }
        self.viewport = Some(area.size());
        self.field_area = Some(area);
        let painter = ui.painter().with_clip_rect(area);
        painter.rect_filled(area, 6.0, ui.visuals().extreme_bg_color);
        if blocks.is_empty() {
            painter.text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                "Pick a starter or drag a block here",
                FontId::proportional(14.0),
                ui.visuals().weak_text_color(),
            );
            // The workspace still has lanes to drop into.
            if !self.workspace {
                return;
            }
        }
        let positions: Vec<_> = blocks
            .iter()
            .map(|block| self.position(graph, &block.key).unwrap_or(block.position))
            .collect();
        if self.fit && !blocks.is_empty() {
            let min = positions
                .iter()
                .fold(Vec2::splat(f32::INFINITY), |p, next| {
                    p.min(Vec2::from(*next))
                })
                // The workspace keeps a left margin for input-port labels on the first column.
                .min(if self.workspace {
                    Vec2::new(-56.0, 0.0)
                } else {
                    Vec2::INFINITY
                });
            let max = positions
                .iter()
                .zip(blocks.iter())
                .fold(Vec2::splat(f32::NEG_INFINITY), |p, (position, block)| {
                    p.max(Vec2::from(*position) + self.card_size(block))
                });
            let bounds = max - min;
            let fitted = ((area.width() - 32.0) / bounds.x.max(1.0))
                .min((area.height() - 32.0) / bounds.y.max(1.0))
                // The workspace is narrow beside its 3D view, so Fit must be able to show a whole move.
                .clamp(if self.workspace { 0.45 } else { 0.85 }, 1.25);
            self.zoom = fitted;
            let content = bounds * fitted;
            let offset = Vec2::new(
                if content.x > area.width() - 32.0 {
                    16.0
                } else {
                    (area.width() - content.x) * 0.5
                },
                if self.workspace || content.y > area.height() - 32.0 {
                    16.0
                } else {
                    (area.height() - content.y) * 0.5
                },
            );
            self.pan = offset - min * fitted;
            self.fit = false;
        }
        if let Some(key) = self.center_on.take() {
            if let Some(index) = blocks.iter().position(|block| block.key == key) {
                self.zoom = 1.0;
                self.pan = area.size() * 0.5 - Vec2::from(positions[index]) - Vec2::new(90.0, 38.0);
            }
        }
        if let Some(key) = self.reveal.take() {
            if let Some(index) = blocks.iter().position(|block| block.key == key) {
                let min = self.pan + Vec2::from(positions[index]) * self.zoom;
                let max = min + self.card_size(&blocks[index]) * self.zoom;
                let margin = 24.0;
                let size = area.size();
                self.pan.x -= (max.x + margin - size.x).max(0.0) + (min.x - margin).min(0.0);
                self.pan.y -= (max.y + margin - size.y).max(0.0) + (min.y - margin).min(0.0);
            }
        }
        let space = self.workspace && ui.input(|input| input.key_down(egui::Key::Space));
        if background.drag_started()
            && self.connecting.is_none()
            && self.workspace
            && !space
            && !ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle))
        {
            self.marquee = ui.input(|input| input.pointer.press_origin());
            if !ui.input(|i| i.modifiers.shift) {
                self.selection.clear();
            }
        }
        if background.dragged() && self.connecting.is_none() && (self.marquee.is_none() || space) {
            self.pan += ui.input(|input| input.pointer.delta());
        }
        if background.clicked() && !ui.input(|i| i.modifiers.shift) {
            self.selected = None;
            self.selection.clear();
            // A click in the first lane's header row is a click on the frame ruler.
            let origin = area.min + self.pan;
            if let (true, Some(point), Some(first)) = (
                self.workspace,
                ui.input(|input| input.pointer.interact_pos()),
                self.lanes.first(),
            ) {
                let world = (point - origin) / self.zoom;
                if (first.top..first.top + 30.0).contains(&world.y) {
                    self.scrub = self
                        .frame_marks
                        .iter()
                        .rev()
                        .find(|(_, x)| *x - 6.0 <= world.x)
                        .map(|(frame, _)| *frame);
                }
            }
        }
        if self.workspace {
            if background.secondary_clicked() {
                let origin = area.min + self.pan;
                self.lane_menu = ui
                    .input(|input| input.pointer.interact_pos())
                    .and_then(|point| self.lane_track_at((point.y - origin.y) / self.zoom));
            }
            background.context_menu(|ui| match self.lane_menu {
                Some(track) => {
                    if ui
                        .button("Clear lane")
                        .on_hover_text(
                            "Remove every block from this script lane; Undo restores them",
                        )
                        .clicked()
                    {
                        self.lane_clear = Some(track);
                        ui.close();
                    }
                }
                None => {
                    ui.weak("Right-click a script lane to clear it.");
                }
            });
        }
        if background.hovered() {
            let wheel = ui.input(|input| input.smooth_scroll_delta.y);
            if wheel != 0.0 {
                let pointer = ui
                    .input(|input| input.pointer.hover_pos())
                    .unwrap_or(area.center());
                let old = self.zoom;
                self.zoom = (old * (wheel * 0.002).exp()).clamp(MIN_ZOOM, 1.8);
                let relative = pointer - area.min - self.pan;
                self.pan += relative * (1.0 - self.zoom / old);
            }
        }
        // An overview must not allocate one grid dot per world-space cell.
        // Keep density bounded even when fitting very large imported moves.
        let step = (24.0 * self.zoom).max(12.0);
        let grid = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let origin = area.min + self.pan;
        let start = Vec2::new(
            (origin.x - area.min.x).rem_euclid(step),
            (origin.y - area.min.y).rem_euclid(step),
        );
        for x in 0..(area.width() / step).ceil() as usize {
            for y in 0..(area.height() / step).ceil() as usize {
                painter.circle_filled(
                    area.min + start + Vec2::new(x as f32 * step, y as f32 * step),
                    0.8,
                    grid,
                );
            }
        }
        let rectangles: Vec<_> = positions
            .iter()
            .zip(&blocks)
            .map(|(position, block)| {
                Rect::from_min_size(
                    origin + Vec2::from(*position) * self.zoom,
                    self.card_size(block) * self.zoom,
                )
            })
            .collect();
        let mut block_indices = std::collections::HashMap::with_capacity(blocks.len());
        for (index, block) in blocks.iter().enumerate() {
            block_indices.entry(block.key.as_str()).or_insert(index);
        }
        for wire in wires {
            let Some(&from) = block_indices.get(wire.from.as_str()) else {
                continue;
            };
            let Some(&to) = block_indices.get(wire.to.as_str()) else {
                continue;
            };
            let a = output_point(&blocks[from], rectangles[from], wire.port);
            let b = input_point(&blocks[to], rectangles[to], wire.input);
            curve(
                &painter,
                a,
                b,
                if wire.port.is_value() {
                    VALUE
                } else {
                    blocks[from].color
                },
                self.zoom,
            );
            // Multi-output and typed ports already carry their own labels.
            // Repeating a wire label at the same socket makes it unreadable.
            // The workspace's left-to-right chains read as "next" without a label.
            if !wire.label.is_empty()
                && (!self.workspace || wire.label != "Next")
                && blocks[from].ports.len() == 1
                && matches!(wire.port, OutputPort::Flow(_))
            {
                painter.text(
                    a + Vec2::new(8.0, -10.0),
                    egui::Align2::LEFT_BOTTOM,
                    wire.label,
                    FontId::proportional(10.0 * self.zoom),
                    blocks[from].color,
                );
            }
        }
        for (index, lane) in self.lanes.iter().enumerate().filter(|_| self.workspace) {
            let band = Rect::from_x_y_ranges(
                area.x_range(),
                origin.y + lane.top * self.zoom..=origin.y + lane.bottom * self.zoom,
            );
            if !band.intersects(area) {
                continue;
            }
            if index % 2 == 1 {
                painter.rect_filled(band, 0.0, Color32::from_white_alpha(4));
            }
            painter.hline(area.x_range(), band.top(), Stroke::new(1.0, grid));
            if lane.track.is_some() {
                for (_, x) in &self.frame_marks {
                    let x = origin.x + (x - 6.0) * self.zoom;
                    if area.x_range().contains(x) {
                        painter.vline(
                            x,
                            band.y_range(),
                            Stroke::new(1.0, grid.gamma_multiply(0.6)),
                        );
                    }
                }
            }
            painter.text(
                Pos2::new(
                    area.left() + 10.0,
                    band.top().max(area.top()) + 4.0 * self.zoom.min(1.0),
                ),
                egui::Align2::LEFT_TOP,
                &lane.label,
                FontId::proportional((12.0 * self.zoom).clamp(7.0, 12.0)),
                ui.visuals().weak_text_color(),
            );
        }
        if let Some(first) = self.lanes.first().filter(|_| self.workspace) {
            // Frame ruler: sticks to the top edge once the first lane scrolls away.
            let y = (origin.y + first.top * self.zoom).max(area.top()) + 4.0 * self.zoom.min(1.0);
            for (frame, x) in &self.frame_marks {
                let x = origin.x + x * self.zoom;
                if x > area.left() + 70.0 && x < area.right() - 20.0 {
                    painter.text(
                        Pos2::new(x, y),
                        egui::Align2::LEFT_TOP,
                        format!("F{}", (frame * 100.0).round() / 100.0),
                        FontId::monospace((11.0 * self.zoom).clamp(7.0, 11.0)),
                        ui.visuals().weak_text_color(),
                    );
                }
            }
        }
        if let (true, Some(frame), Some(first)) =
            (self.workspace, self.playhead, self.frame_marks.first())
        {
            // Within a frame's columns the line advances in proportion to the next marked frame.
            let x = match self.frame_marks.iter().position(|(at, _)| *at > frame) {
                Some(0) => first.1,
                Some(next) => {
                    let ((from, start), (to, end)) =
                        (self.frame_marks[next - 1], self.frame_marks[next]);
                    start + (end - start) * (frame - from) / (to - from)
                }
                None => self
                    .frame_marks
                    .last()
                    .map_or(0.0, |(at, x)| x + (frame - at).min(1.0) * 231.0),
            };
            let mut x = origin.x + (x - 6.0) * self.zoom;
            if self.follow_playhead && !area.x_range().contains(x) {
                let target = area.left() + area.width() * 0.25;
                self.pan.x += target - x;
                x = target;
            }
            let scripts = self.lanes.iter().filter(|lane| lane.track.is_some());
            let (top, bottom) = scripts
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(top, bottom), lane| {
                    (top.min(lane.top), bottom.max(lane.bottom))
                });
            if area.x_range().contains(x) && top < bottom {
                painter.vline(
                    x,
                    (origin.y + top * self.zoom)..=(origin.y + bottom * self.zoom),
                    Stroke::new(2.0, Color32::from_rgb(235, 110, 90)),
                );
            }
        }
        if let Some(start) = self.marquee {
            if let Some(end) = ui.input(|i| i.pointer.hover_pos()) {
                let rect = Rect::from_two_pos(start, end).intersect(area);
                painter.rect_filled(
                    rect,
                    0.0,
                    ui.visuals().selection.bg_fill.gamma_multiply(0.15),
                );
                painter.rect_stroke(
                    rect,
                    0.0,
                    ui.visuals().selection.stroke,
                    egui::StrokeKind::Inside,
                );
                if ui.input(|i| i.pointer.any_released()) {
                    for (block, block_rect) in blocks.iter().zip(&rectangles) {
                        if rect.intersects(*block_rect) {
                            self.selection.insert(block.key.clone());
                        }
                    }
                    self.selected = self.selection.iter().next().cloned();
                    self.marquee = None;
                }
            }
        }
        if self.workspace
            && ui.input(|input| input.pointer.any_released())
            && ui.input(|input| {
                input
                    .pointer
                    .hover_pos()
                    .is_some_and(|point| area.contains(point))
            })
        {
            if let Some(payload) =
                egui::DragAndDrop::take_payload::<super::view::workspace::PaletteBlock>(ui.ctx())
            {
                if let Some(point) = ui.input(|input| input.pointer.hover_pos()) {
                    let mut nearest = None;
                    let mut distance = 44.0;
                    for (block, rect) in blocks.iter().zip(&rectangles) {
                        for port in &block.ports {
                            let d = point.distance(output_point(block, *rect, *port));
                            if d < distance {
                                nearest = Some((block.key.clone(), *port));
                                distance = d;
                            }
                        }
                    }
                    let world = (point - origin) / self.zoom;
                    self.palette_drop = Some((
                        (*payload).clone(),
                        [world.x, world.y],
                        nearest.as_ref().map(|(key, _)| key.clone()),
                    ));
                    self.insertion_port = nearest;
                }
            }
        }
        let mut drop_target = None;
        let mut connect_from = None;
        for (block, rect) in blocks.iter().zip(&rectangles) {
            let rect = *rect;
            if !area.intersects(rect.expand(10.0)) {
                continue;
            }
            let response = ui.interact(
                rect.shrink2(Vec2::new(12.0, 0.0) * self.zoom),
                ui.id().with((&block.key, "card")),
                Sense::click_and_drag(),
            );
            let internal = block.internal;
            let help = super::plain::block_help(
                block
                    .title
                    .split(' ')
                    .next()
                    .filter(|word| *word == "If")
                    .unwrap_or(&block.title),
            )
            .unwrap_or("");
            let response = if self.workspace || !internal.is_empty() {
                response.on_hover_text(
                    [block.title.as_str(), block.detail.as_str(), help]
                        .iter()
                        .filter(|text| !text.is_empty())
                        .copied()
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            } else {
                response
            };
            if response.clicked() {
                self.selected = Some(block.key.clone());
                if ui.input(|i| i.modifiers.shift) {
                    if !self.selection.insert(block.key.clone()) {
                        self.selection.remove(&block.key);
                    }
                } else {
                    self.selection.clear();
                    self.selection.insert(block.key.clone());
                }
            }
            if self.workspace {
                response.context_menu(|ui| {
                    self.selected = Some(block.key.clone());
                    for (label, command) in [
                        ("Copy", EditCommand::Copy),
                        ("Duplicate", EditCommand::Duplicate),
                        ("Delete", EditCommand::Delete),
                    ] {
                        if ui.button(label).clicked() {
                            self.edit_command = Some(command);
                            ui.close();
                        }
                    }
                    if ui
                        .button(if self.collapsed.contains(&block.key) {
                            "Expand"
                        } else {
                            "Collapse"
                        })
                        .clicked()
                    {
                        if !self.collapsed.insert(block.key.clone()) {
                            self.collapsed.remove(&block.key);
                        }
                        ui.close();
                    }
                });
            }
            if response.dragged() && !space {
                self.selected = Some(block.key.clone());
                if !self.selection.contains(&block.key) {
                    self.selection.clear();
                    self.selection.insert(block.key.clone());
                }
                let delta = ui.input(|input| input.pointer.delta()) / self.zoom;
                for (member, member_position) in blocks
                    .iter()
                    .zip(&positions)
                    .filter(|(member, _)| self.selection.contains(&member.key))
                    .map(|(member, position)| (member.key.clone(), *position))
                    .collect::<Vec<_>>()
                {
                    self.set_position(
                        graph,
                        &member,
                        [
                            (member_position[0] + delta.x).clamp(-100_000.0, 100_000.0),
                            (member_position[1] + delta.y).clamp(-100_000.0, 100_000.0),
                        ],
                    );
                }
            } else if response.dragged() && space {
                self.pan += ui.input(|i| i.pointer.delta());
            }
            let selected =
                self.selected.as_ref() == Some(&block.key) || self.selection.contains(&block.key);
            painter.rect_filled(rect, 5.0, ui.visuals().widgets.inactive.bg_fill);
            painter.rect_stroke(
                rect,
                5.0,
                Stroke::new(
                    if selected { 2.0 } else { 1.0 },
                    if selected {
                        block.color
                    } else {
                        ui.visuals().widgets.inactive.bg_stroke.color
                    },
                ),
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    rect.left_top() + Vec2::new(8.0, 3.0),
                    rect.right_top() + Vec2::new(-8.0, 3.0),
                ],
                Stroke::new(3.0, block.color),
            );
            let text_painter = painter.with_clip_rect(rect.shrink(8.0 * self.zoom).intersect(area));
            let title = text_painter.layout(
                block.title.clone(),
                FontId::proportional(14.0 * self.zoom),
                ui.visuals().text_color(),
                rect.width() - 24.0 * self.zoom,
            );
            let detail_y = (20.0 * self.zoom + title.size().y).min(58.0 * self.zoom);
            text_painter.galley(
                rect.min + Vec2::new(12.0, 16.0) * self.zoom,
                title,
                Color32::PLACEHOLDER,
            );
            let detail = text_painter.layout(
                block.detail.clone(),
                FontId::proportional(11.0 * self.zoom),
                ui.visuals().text_color().gamma_multiply(0.65),
                rect.width() - 24.0 * self.zoom,
            );
            text_painter.galley(
                rect.min + Vec2::new(12.0 * self.zoom, detail_y),
                detail,
                Color32::PLACEHOLDER,
            );
            let handles = block
                .inputs
                .iter()
                .map(|port| (false, *port, input_point(block, rect, *port)))
                .chain(
                    block
                        .ports
                        .iter()
                        .map(|port| (true, *port, output_point(block, rect, *port))),
                );
            for (output, flow_port, point) in handles {
                let count = if output {
                    block.ports.len()
                } else {
                    block.inputs.len()
                };
                let hit_size = if count > 1 {
                    (rect.height() / (count + 1) as f32 * 0.8).clamp(6.0, 20.0)
                } else {
                    20.0
                };
                let port = Rect::from_center_size(point, Vec2::splat(hit_size));
                let response = ui.interact(
                    port,
                    ui.id().with((&block.key, output, flow_port)),
                    Sense::drag(),
                );
                let color = if flow_port.is_value() {
                    VALUE
                } else {
                    block.color
                };
                paint_port(&painter, point, flow_port, color, 5.0 * self.zoom.max(0.6));
                response
                    .clone()
                    .on_hover_text(if output && block.key.starts_with("event-field:") {
                        "Incoming event field. Drag to a matching Value or Condition input on this event's receiver. Values are local to this event, not shared variables."
                    } else if output && flow_port.is_value() {
                        "Read this value when the action runs. Drag to a same-type Value input. Expressions are copied, not linked by reference."
                    } else if !output && flow_port.is_value() {
                        "Value input. Connect a Counter to a Counter, or a Gauge to a Gauge. No implicit conversions."
                    } else if output && flow_port == OutputPort::Event {
                        "Drag to a named-event receiver. Events are queued, not called recursively."
                    } else if flow_port == OutputPort::Event {
                        "Named-event input. Connect an Emit output, not an execution wire."
                    } else if output && flow_port == OutputPort::Flow(FlowPort::Then) {
                        "These blocks run when the answer is yes. Drag to the block that should run."
                    } else if output && flow_port == OutputPort::Flow(FlowPort::Else) {
                        "These blocks run when the answer is no. Drag to the block that should run."
                    } else if output && flow_port == OutputPort::Flow(FlowPort::Repeat) {
                        "These blocks repeat. Drag to the first block of the loop."
                    } else if output {
                        "Drag to the block that should run next"
                    } else {
                        "The block before this one connects here"
                    });
                if (count > 1 || flow_port.is_value() || flow_port == OutputPort::Event)
                    && (output || flow_port.is_value() || flow_port == OutputPort::Event)
                    && (!output
                        || count > 1
                        || selected
                        || response.hovered()
                        || self
                            .connecting
                            .as_ref()
                            .is_some_and(|(key, _)| key == &block.key))
                {
                    let name = match flow_port {
                        OutputPort::Flow(FlowPort::Next) => {
                            if count > 1 {
                                "After"
                            } else {
                                "Next"
                            }
                        }
                        OutputPort::Flow(FlowPort::Then) => "Yes",
                        OutputPort::Flow(FlowPort::Else) => "No",
                        OutputPort::Flow(FlowPort::Repeat) => "Loop",
                        OutputPort::Event => {
                            if output {
                                "Emit"
                            } else {
                                "Event"
                            }
                        }
                        OutputPort::Value => "Value",
                    };
                    painter.text(
                        point + Vec2::new(if output { 10.0 } else { -10.0 } * self.zoom, 0.0),
                        if output {
                            egui::Align2::LEFT_CENTER
                        } else {
                            egui::Align2::RIGHT_CENTER
                        },
                        name,
                        FontId::proportional(9.0 * self.zoom),
                        color,
                    );
                }
                if output && response.drag_started() {
                    connect_from = Some((block.key.clone(), flow_port));
                }
                if !output
                    && self.connecting.is_some()
                    && ui
                        .input(|input| input.pointer.hover_pos())
                        .is_some_and(|pointer| port.contains(pointer))
                {
                    painter.circle_stroke(point, 10.0, Stroke::new(2.0, Color32::WHITE));
                    drop_target = Some(block.key.clone());
                }
            }
        }
        if let Some(from) = connect_from {
            self.connecting = Some(from);
            self.hover_target = None;
            self.hover_error.clear();
        }
        if let Some((from, flow_port)) = self.connecting.clone() {
            if drop_target != self.hover_target {
                self.hover_error = if let Some(to) = &drop_target {
                    let character_before = self.character_graphs.clone();
                    let result = self.reconnect_any(&mut graph.clone(), &from, flow_port, to);
                    self.character_graphs = character_before;
                    result.err().unwrap_or_default()
                } else {
                    String::new()
                };
                self.hover_target = drop_target.clone();
            }
            if let Some(index) = blocks.iter().position(|block| block.key == from) {
                let position = self
                    .position(graph, &from)
                    .unwrap_or(blocks[index].position);
                let rect = Rect::from_min_size(
                    origin + Vec2::from(position) * self.zoom,
                    self.card_size(&blocks[index]) * self.zoom,
                );
                if let Some(end) = ui.input(|input| input.pointer.hover_pos()) {
                    curve(
                        &painter,
                        output_point(&blocks[index], rect, flow_port),
                        end,
                        Color32::WHITE,
                        self.zoom,
                    );
                }
            }
            if ui.input(|input| input.pointer.any_released()) {
                if let Some(to) = drop_target {
                    self.error = self
                        .reconnect_any(graph, &from, flow_port, &to)
                        .err()
                        .unwrap_or_default();
                } else if self.workspace {
                    if let Some(point) = ui
                        .input(|input| input.pointer.hover_pos())
                        .filter(|point| area.contains(*point))
                    {
                        let world = (point - origin) / self.zoom;
                        self.quick_add = Some((point, [world.x, world.y], from, flow_port));
                    }
                }
                self.connecting = None;
                self.hover_error.clear();
            }
        }
        if !self.hover_error.is_empty() {
            ui.colored_label(Color32::LIGHT_RED, &self.hover_error);
        }
    }
}

impl CanvasState {
    /// Selects a block from outside the field and brings it into view.
    pub(super) fn focus_node(&mut self, id: u32) {
        let key = format!("node:{id}");
        self.reveal = Some(key.clone());
        self.select(key);
    }
}

/// What the block with this id does, in one sentence.
pub(super) fn node_help(node: &MoveNode) -> Option<&'static str> {
    let mut blocks = Vec::new();
    project_nodes(std::slice::from_ref(node), 0, &mut 0, &mut blocks);
    let title = blocks[0].title.as_str();
    super::plain::block_help(if title.starts_with("If ") {
        "If"
    } else {
        title
    })
}

/// A block's name and detail as one line of the move's story.
pub(super) fn story_caption(node: &MoveNode) -> String {
    let mut blocks = Vec::new();
    project_nodes(std::slice::from_ref(node), 0, &mut 0, &mut blocks);
    let block = &blocks[0];
    if block.detail.is_empty() {
        block.title.clone()
    } else {
        format!("{}: {}", block.title, block.detail)
    }
}

const CARD: f32 = 180.0;
const COMPACT_CARD: f32 = 96.0;

impl CanvasState {
    fn card_size(&self, block: &Block) -> Vec2 {
        let width = if self.compact_cards.contains(&block.key) {
            COMPACT_CARD
        } else if block.key.contains(":state/") {
            220.0
        } else {
            CARD
        };
        Vec2::new(width, block_height(block))
    }
}

fn block_height(block: &Block) -> f32 {
    if block.key.contains(":state/") {
        96.0
    } else {
        76.0
    }
}

fn paint_port(painter: &egui::Painter, point: Pos2, port: OutputPort, color: Color32, radius: f32) {
    if port.is_value() {
        painter.circle_filled(point, radius, color);
    } else {
        painter.rect_filled(
            Rect::from_center_size(point, Vec2::splat(radius * 2.0)),
            1.0,
            color,
        );
    }
}

fn curve(painter: &egui::Painter, a: Pos2, b: Pos2, color: Color32, zoom: f32) {
    let reach = ((b.x - a.x).abs() * 0.5).max(35.0 * zoom);
    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
        [a, a + Vec2::new(reach, 0.0), b - Vec2::new(reach, 0.0), b],
        false,
        Color32::TRANSPARENT,
        Stroke::new(1.8, color),
    ));
}

fn output_point(block: &Block, rect: Rect, port: OutputPort) -> Pos2 {
    let index = block
        .ports
        .iter()
        .position(|candidate| *candidate == port)
        .unwrap_or(0);
    Pos2::new(
        rect.right(),
        rect.top() + rect.height() * (index + 1) as f32 / (block.ports.len() + 1) as f32,
    )
}

fn input_point(block: &Block, rect: Rect, port: OutputPort) -> Pos2 {
    let index = block
        .inputs
        .iter()
        .position(|candidate| *candidate == port)
        .unwrap_or(0);
    Pos2::new(
        rect.left(),
        rect.top() + rect.height() * (index + 1) as f32 / (block.inputs.len() + 1) as f32,
    )
}

#[cfg(test)]
fn project(graph: &MoveGraph, track: Option<Track>) -> (Vec<Block>, Vec<Wire>) {
    project_with_context(graph, track)
}

fn project_with_context(graph: &MoveGraph, track: Option<Track>) -> (Vec<Block>, Vec<Wire>) {
    let (mut blocks, wires) = project_structure(graph, track);
    for block in &mut blocks {
        if let Some(position) = graph.positions.get(&block.key) {
            block.position = *position;
        }
    }
    (blocks, wires)
}

/// Blocks at the place the script's structure gives them, ignoring where the user dragged them.
fn project_structure(graph: &MoveGraph, track: Option<Track>) -> (Vec<Block>, Vec<Wire>) {
    let mut blocks = Vec::new();
    let mut wires = Vec::new();
    let mut row = 0;
    for source in graph
        .tracks
        .iter()
        .filter(|source| Some(source.track) == track)
    {
        project_nodes(&source.nodes, 0, &mut row, &mut blocks);
    }
    for edge in super::flow::project(graph)
        .edges
        .into_iter()
        .filter(|edge| Some(edge.track) == track)
    {
        let label = match edge.port {
            super::flow::FlowPort::Next => "Next",
            super::flow::FlowPort::Then => "Yes",
            super::flow::FlowPort::Else => "No",
            super::flow::FlowPort::Repeat => "Loop",
        };
        wires.push(Wire {
            from: format!("node:{}", edge.source),
            to: format!("node:{}", edge.target),
            label,
            port: edge.port.into(),
            input: FlowPort::Next.into(),
        });
    }
    (blocks, wires)
}

pub(super) fn effect_caption(command: &crate::data::EffectMacro) -> (&'static str, String) {
    use crate::data::{EffectControl, EffectMacro};
    match command {
        EffectMacro::NativeCall(call) => (
            call.signature()
                .map_or("Native call", |signature| signature.caption),
            call.arguments.join(" · "),
        ),
        EffectMacro::Effect {
            effect_name,
            bone_name,
            ..
        } => ("Spawn effect", format!("{effect_name} · {bone_name}")),
        EffectMacro::EffectFollowWork {
            work_symbol,
            bone_name,
            ..
        } => (
            "Follow native effect",
            format!(
                "{} · {bone_name} · Source export",
                super::view::native_effect_slot_label(work_symbol)
            ),
        ),
        EffectMacro::EffectOffKind { effect_name, .. } => ("Stop effect", effect_name.clone()),
        EffectMacro::AfterImage {
            effect_name,
            bone_name,
            bone_name2,
            ..
        } => (
            "Sword trail",
            format!(
                "{effect_name} · {bone_name} → {}",
                bone_name2.as_deref().unwrap_or(bone_name)
            ),
        ),
        EffectMacro::AfterImageOff { arg } => ("End sword trail", format!("Native control {arg}")),
        EffectMacro::LastEffectSetRate { rate } => {
            ("Effect rate", format!("{rate}× · last effect"))
        }
        EffectMacro::LastEffectSetColor { .. } => ("Effect color", "Last effect".into()),
        EffectMacro::LastParticleSetColor { .. } => ("Particle color", "Last particle".into()),
        EffectMacro::LastEffectSetAlpha { alpha } => {
            ("Effect opacity", format!("{alpha} · last effect"))
        }
        EffectMacro::LastEffectSetOffsetToCameraFlat { offset } => {
            ("Effect camera offset", offset.to_string())
        }
        EffectMacro::LastEffectSetScaleW { values } => (
            "Effect scale",
            format!("{} components · last effect", values.len()),
        ),
        EffectMacro::Control(EffectControl::DetachKind { effect_name, .. }) => {
            ("Detach effect", effect_name.clone())
        }
        EffectMacro::Control(EffectControl::SetDisableRenderOffsetLast) => (
            "Disable render offset",
            "Last effect · source export".into(),
        ),
        EffectMacro::Control(EffectControl::EnableSyncInitPosLast) => (
            "Sync initial position",
            "Last effect · source export".into(),
        ),
        EffectMacro::Control(EffectControl::ReqScreen { preset_id }) => (
            "Screen effect",
            format!(
                "{} · Source export",
                preset_id
                    .strip_prefix("bg_")
                    .unwrap_or(preset_id)
                    .replace('_', " ")
            ),
        ),
        EffectMacro::Control(EffectControl::OffKindWork { preset_id }) => (
            "Native effect off",
            format!(
                "{} · Source export",
                crate::data::EffectOffKindWorkPreset::by_id(preset_id)
                    .map(super::view::native_effect_off_label)
                    .unwrap_or_else(|| "Unsupported saved preset".into())
            ),
        ),
        EffectMacro::Control(EffectControl::SetDisableCameraDepthInfluence { value }) => (
            "Camera depth influence",
            format!(
                "{} · Source export",
                if *value { "Disabled" } else { "Enabled" }
            ),
        ),
        EffectMacro::Color { command, .. } => (
            "Fighter color",
            command.to_ascii_lowercase().replace('_', " "),
        ),
        _ => ("Imported effect", "Native command".into()),
    }
}

pub(super) fn condition_caption(condition: &super::model::MoveCondition) -> String {
    super::plain::condition_sentence(condition)
}

pub(super) fn command_caption(command: &crate::data::ExcuteStmt) -> (&'static str, String) {
    use crate::data::{ArticleLifecycleCall, ExcuteStmt, ExpressionCall, HurtTarget};
    match command {
        ExcuteStmt::NativeCall(call) => (
            call.signature()
                .map_or("Native call", |signature| signature.caption),
            call.arguments.join(" · "),
        ),
        ExcuteStmt::GimmickChartEvent { chart_id, name } => {
            ("Send event", format!("{chart_id} · {name}"))
        }
        ExcuteStmt::GimmickChartEventValues {
            chart_id,
            name,
            arguments,
        } => (
            "Send event",
            format!("{chart_id} · {name} · {} inputs", arguments.len()),
        ),
        ExcuteStmt::WorkFlag(call) => (
            "Native flag",
            format!(
                "{} {}",
                super::plain::symbol_words(&call.flag),
                if matches!(call.action, crate::data::WorkFlagAction::On) {
                    "on"
                } else {
                    "off"
                }
            ),
        ),
        ExcuteStmt::Attack(call) => (
            "Create hitbox",
            format!(
                "{}% damage · {} · slot {}",
                call.damage, call.bone_name, call.id
            ),
        ),
        ExcuteStmt::AttackAbs(call) => (
            if call.kind.trim_start_matches('*') == "FIGHTER_ATTACK_ABSOLUTE_KIND_THROW" {
                "Define throw damage"
            } else {
                "Define absolute attack"
            },
            format!("ID {} · {}% · angle {}", call.id, call.damage, call.angle),
        ),
        ExcuteStmt::AttackAbsApplyThrow => ("Apply throw", "The held opponent".into()),
        ExcuteStmt::WorkTransitionTerm(call)
            if call.receiver == crate::data::WorkTransitionTermReceiver::AgentAccessor
                && call.action == crate::data::WorkTransitionTermAction::Enable
                && call.transition_term == super::native_work::DASH_TO_RUN_TERM =>
        {
            ("Allow dash → run", "Source export".into())
        }
        ExcuteStmt::WorkTransitionTerm(call) => {
            let title = match call.action {
                crate::data::WorkTransitionTermAction::Enable => "Allow transition",
                crate::data::WorkTransitionTermAction::Unable => "Disable transition",
                crate::data::WorkTransitionTermAction::EnableGroup => "Allow transition group",
                crate::data::WorkTransitionTermAction::UnableGroupEx => "Disable transition group",
            };
            let detail = super::native_work::transition_term_label(&call.transition_term)
                .map_or_else(|| "Preserved source".into(), str::to_owned);
            (title, detail)
        }
        ExcuteStmt::SlowWhole(crate::data::SlowWholeCall::Set { multiplier, frames }) => {
            if crate::data::SLOW_WHOLE_SET_PAIRS
                .iter()
                .any(|pair| pair.multiplier == *multiplier && pair.frames == *frames)
            {
                (
                    "Set slowdown",
                    if *frames == 0 {
                        format!("{multiplier}x slower until cleared")
                    } else {
                        format!("{multiplier}x slower for {frames} frames")
                    },
                )
            } else {
                ("Native slowdown", "Preserved source".into())
            }
        }
        ExcuteStmt::SlowWhole(crate::data::SlowWholeCall::Clear) => {
            ("Clear slowdown", "Source export".into())
        }
        ExcuteStmt::SlowOpponent(call) => (
            "Slow opponents",
            format!("{} / {} · Source export", call.slow_mul, call.slow_time),
        ),
        ExcuteStmt::CheckFinishCamera(call) => (
            "Finish camera",
            format!("({}, {}) · Source export", call.arg1, call.arg2),
        ),
        ExcuteStmt::CheckFinalStartCamera(call) => (
            "Final-start camera",
            format!("({}) · Source export", call.args.join(", ")),
        ),
        ExcuteStmt::FinalFearFace(call) => (
            "Final fear face",
            format!("Arg {} · Source export", call.argument),
        ),
        ExcuteStmt::OptionalHitFeedback(call) => (
            match call.kind {
                crate::data::OptionalHitFeedbackKind::Effect => "Optional hit effect",
                crate::data::OptionalHitFeedbackKind::Sound => "Optional hit sound",
            },
            format!(
                "Arg {} · {} · Source export",
                call.arg2,
                match &call.hash {
                    crate::data::Hash40Literal::Name(name) => name.clone(),
                    crate::data::Hash40Literal::Raw(raw) => format!("Raw 0x{raw:x}"),
                }
            ),
        ),
        ExcuteStmt::WorkModuleSet(call)
            if super::native_work::is_source_only_work_module_set(call) =>
        {
            if let Some(descriptor) = super::native_work::source_work_module_set_descriptors()
                .iter()
                .find(|descriptor| {
                    call.receiver == "agent.module_accessor"
                        && descriptor.kind == call.kind
                        && descriptor.slot == call.slot
                        && descriptor.values.contains(&call.value.as_str())
                })
            {
                return (
                    descriptor.label,
                    format!(
                        "{} · Source export",
                        super::view::source_work_value_label(&call.value)
                    ),
                );
            }
            let known_shape = call.kind == crate::data::WorkModuleSetKind::Int
                && call.receiver == "agent.module_accessor"
                && (call.slot == super::native_work::ITEM_GRASS_PULL_NEXT_STEP_SLOT
                    && super::native_work::ITEM_GRASS_PULL_STEP_VALUES
                        .contains(&call.value.as_str())
                    || call.slot == super::native_work::THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT
                        && call.value
                            == super::native_work::THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_VALUE);
            if !known_shape {
                ("Native setter", "Preserved source".into())
            } else if call.slot == super::native_work::ITEM_GRASS_PULL_NEXT_STEP_SLOT {
                (
                    "Item pull step",
                    format!(
                        "{} · Source export",
                        call.value
                            .trim_start_matches("*FIGHTER_ITEM_GRASS_PULL_STEP_")
                    ),
                )
            } else {
                ("Item pull offset limit", "4 · Source export".into())
            }
        }
        ExcuteStmt::Catch(call) => (
            "Create grab",
            format!("{} · slot {}", call.bone_name, call.id),
        ),
        ExcuteStmt::Search(call) => ("Search volume", format!("ID {}", call.id)),
        ExcuteStmt::Clear(id) => ("Clear hitbox", format!("Slot {id}")),
        ExcuteStmt::AttackModuleAddReactionFrame { id, frames } => {
            ("Reaction frames", format!("ID {id} · {frames:+}"))
        }
        ExcuteStmt::AttackModuleAddReactionFrameRevised { id, frames } => {
            ("Reaction frames · revised", format!("ID {id} · {frames:+}"))
        }
        ExcuteStmt::ClearAll => ("Clear hitboxes", String::new()),
        ExcuteStmt::AttackModuleSetDamageShakeScale(scale) => (
            "Damage shake scale",
            format!("{} · Source export", scale.source_token()),
        ),
        ExcuteStmt::AttackModuleSetNoUniqEffectAll(pair) => {
            let (arg2, arg3) = pair.source_args();
            (
                "Unique attack effects",
                format!("{arg2}, {arg3} · Source export"),
            )
        }
        ExcuteStmt::AttackModuleSetTargetCategory(id) => (
            "Attack target filter",
            format!("ID {} · NO_IF · Source export", id.attack_id()),
        ),
        ExcuteStmt::AttackModuleSetSize(preset) => (
            "Hitbox size",
            format!(
                "ID {} · {} · Source export",
                preset.attack_id(),
                preset.size_token()
            ),
        ),
        ExcuteStmt::HitModuleSetStatusAllNormal => {
            ("All hurtboxes normal", "Normal · Source export".into())
        }
        ExcuteStmt::AttackModuleSetVecTargetPos(preset) => {
            let (id, x, y, _) = preset.source_parts();
            (
                "Vector target",
                format!("ID {id} · ({x}, {y}) · Source export"),
            )
        }
        ExcuteStmt::GrabClearAll => ("Clear grabs", "All grab IDs".into()),
        ExcuteStmt::ReverseLr => ("Reverse facing", String::new()),
        ExcuteStmt::ShootItemBullet => ("Fire held item", "Source export".into()),
        ExcuteStmt::StartInfoFlashEye => ("Eye flash", "Source export".into()),
        ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind } => (
            "Attack reference joint",
            super::view::attack_reference_joint_label(*kind),
        ),
        ExcuteStmt::CameraZoomOut => ("Zoom camera out", "Source export".into()),
        ExcuteStmt::StartCutIn => ("Start cut-in", "Source export".into()),
        ExcuteStmt::CaptureCutCommon => (
            "Default catch setup",
            "Own Execute guard · source export".into(),
        ),
        ExcuteStmt::FixedJostleArea(crate::data::FixedJostleAreaCall::EnableFix { arg2, arg3 }) => {
            ("Body collision area", format!("{arg2} · {arg3}"))
        }
        ExcuteStmt::FixedJostleArea(crate::data::FixedJostleAreaCall::EnableFixXy {
            arg2,
            arg3,
            arg4,
            arg5,
        }) => (
            "Body collision area XY",
            format!("{arg2} · {arg3} · {arg4} · {arg5}"),
        ),
        ExcuteStmt::HitResetAll => ("Reset hurtboxes", "Restore reception state".into()),
        ExcuteStmt::HitStatus { target, status } => {
            let target = match target {
                HurtTarget::Whole => "Whole fighter".into(),
                HurtTarget::Bone(joint) => format!("Joint {joint}"),
                HurtTarget::Group(group) => format!("Group {group}"),
            };
            let state = match status.trim().trim_start_matches('*') {
                "HIT_STATUS_NORMAL" | "0" => "Normal",
                "HIT_STATUS_INVINCIBLE" | "1" => "Invincible",
                "HIT_STATUS_XLU" | "2" => "Intangible",
                "HIT_STATUS_OFF" | "3" => "Off",
                other => other,
            };
            ("Hurtbox status", format!("{target} · {state}"))
        }
        ExcuteStmt::Wind(wind) => (
            "Wind area",
            format!(
                "{} · ID {}",
                if wind.is_radial() {
                    "Radial"
                } else {
                    "Rectangle"
                },
                wind.args.first().copied().unwrap_or_default()
            ),
        ),
        ExcuteStmt::EraseWind(id) => ("Erase wind", format!("ID {id}")),
        ExcuteStmt::VisibilitySetInt64 { group, status } => {
            ("Model visibility", format!("{group} / {status}"))
        }
        ExcuteStmt::VisibilitySetWhole { visible } => (
            "Whole model visibility",
            if *visible { "Visible" } else { "Hidden" }.into(),
        ),
        ExcuteStmt::VisibilitySetDefaultInt64 { group } => {
            ("Model default", format!("{group} · Source export"))
        }
        ExcuteStmt::ArticleLifecycle(call) => {
            let (title, symbol) = match call {
                ArticleLifecycleCall::AddMotionPartial { preset } => {
                    return ("Open/close sword", preset.label());
                }
                ArticleLifecycleCall::Generate { symbol } => ("Generate article", symbol),
                ArticleLifecycleCall::GenerateEnabled { symbol } => {
                    ("Generate enabled article", symbol)
                }
                ArticleLifecycleCall::ChangeStatus {
                    symbol,
                    status,
                    if_present,
                } => {
                    return (
                        "Article state",
                        super::view::article_state_label(symbol, status, *if_present),
                    );
                }
                ArticleLifecycleCall::RemoveAll { symbol } => ("Remove article", symbol),
                ArticleLifecycleCall::SetVisibilityWhole { symbol, visible } => {
                    return (
                        "Article visibility",
                        format!(
                            "{} · {}",
                            super::plain::object_name(symbol),
                            if *visible { "Visible" } else { "Hidden" }
                        ),
                    );
                }
                ArticleLifecycleCall::Shoot { symbol, target }
                | ArticleLifecycleCall::ShootExist { symbol, target } => {
                    return (
                        if matches!(call, ArticleLifecycleCall::ShootExist { .. }) {
                            "Launch if present"
                        } else {
                            "Launch article"
                        },
                        format!(
                            "{} · {}",
                            super::plain::object_name(symbol),
                            super::view::article_target_label(*target)
                        ),
                    );
                }
                ArticleLifecycleCall::ChangeMotion { symbol, motion } => {
                    return (
                        "Article motion",
                        format!(
                            "{} · {motion}",
                            symbol
                                .split_once("GENERATE_ARTICLE_")
                                .map_or(symbol.as_str(), |(_, name)| name)
                        ),
                    );
                }
            };
            (title, super::plain::object_name(symbol))
        }
        ExcuteStmt::Expression(ExpressionCall::ControlModuleSetRumble {
            duration, looped, ..
        }) => (
            "Rumble pattern",
            format!(
                "Duration {duration} · {}",
                if looped == "true" { "Loop" } else { "Once" }
            ),
        ),
        ExcuteStmt::Expression(ExpressionCall::RumbleHit { .. }) => {
            ("Hit rumble", "Native feedback".into())
        }
        ExcuteStmt::Expression(ExpressionCall::Quake { kind }) => (
            "Camera shake",
            kind.trim_start_matches('*')
                .trim_start_matches("CAMERA_QUAKE_KIND_")
                .into(),
        ),
        ExcuteStmt::Sound(call) => (
            if call.func == "STOP_SE" {
                "Stop sound"
            } else {
                "Play sound"
            },
            call.sounds.join(" / "),
        ),
        command => ("Engine action", variant_name(command)),
    }
}

/// "WorkFlag(..)" → "Work flag": a readable name for commands without a dedicated caption.
fn variant_name(command: &impl std::fmt::Debug) -> String {
    let debug = format!("{command:?}");
    let variant = debug.split(['(', ' ', '{']).next().unwrap_or_default();
    let mut name = String::new();
    for (index, character) in variant.chars().enumerate() {
        if character.is_uppercase() && index > 0 {
            name.push(' ');
            name.extend(character.to_lowercase());
        } else {
            name.push(character);
        }
    }
    name
}

/// The short name a multi-command block lists for one command.
fn command_name(command: &crate::data::ExcuteStmt) -> String {
    match command_caption(command) {
        ("Engine action", detail) => super::plain::block_name(&detail).to_owned(),
        (title, _) => super::plain::block_name(title).to_owned(),
    }
}

fn project_nodes(nodes: &[MoveNode], depth: usize, row: &mut usize, blocks: &mut Vec<Block>) {
    for node in nodes {
        let key = format!("node:{}", node.id);
        let (title, mut detail, color) = match &node.kind {
            NodeKind::Frame { frame } => ("At motion frame", format!("Frame {frame}"), TIME),
            NodeKind::NativeExecuteFrame { frame } => {
                ("Execute at frame", format!("Frame {frame}"), TIME)
            }
            NodeKind::NativeWaitLoopClear => (
                "Wait for motion loop",
                "Native wait_loop_clear · source export".into(),
                TIME,
            ),
            NodeKind::NativeMotionLoopSync => (
                "Sync motion loop",
                "Native timing · source export".into(),
                TIME,
            ),
            NodeKind::Wait { frames } => ("Wait", format!("{frames} motion frames"), TIME),
            NodeKind::MotionRate { rate } => ("Motion rate", format!("{rate}"), TIME),
            NodeKind::BareCommand { command }
                if matches!(command.as_ref(), crate::data::ExcuteStmt::CaptureCutCommon) =>
            {
                (
                    "Default catch setup",
                    "Own Execute guard · source export".into(),
                    ACTION,
                )
            }
            NodeKind::BareCommand { .. } => {
                ("Top-level command", "Runs outside Execute".into(), ACTION)
            }
            NodeKind::Execute { commands }
                if matches!(
                    commands.as_slice(),
                    [crate::data::ExcuteStmt::GimmickEvent { .. }]
                ) =>
            {
                (
                    "Send event",
                    match &commands[0] {
                        crate::data::ExcuteStmt::GimmickEvent { name } => format!("main · {name}"),
                        _ => unreachable!(),
                    },
                    EVENT,
                )
            }
            NodeKind::Execute { commands } if commands.len() == 1 => {
                let (title, detail) = command_caption(&commands[0]);
                (title, detail, ACTION)
            }
            NodeKind::Execute { commands } => (
                "Actions",
                summarize(commands.iter().map(command_name), 0),
                ACTION,
            ),
            NodeKind::MixedExecute { commands } => (
                "Imported actions",
                summarize(
                    commands.iter().filter_map(|command| match command {
                        super::model::MoveCommand::Editable(command) => Some(command_name(command)),
                        super::model::MoveCommand::Preserved(_) => None,
                    }),
                    commands
                        .iter()
                        .filter(|command| {
                            matches!(command, super::model::MoveCommand::Preserved(_))
                        })
                        .count(),
                ),
                ACTION,
            ),
            NodeKind::EffectExecute { commands } if commands.len() == 1 => {
                let (title, detail) = effect_caption(&commands[0]);
                (title, detail, ACTION)
            }
            NodeKind::EffectExecute { commands } => (
                "Visual effects",
                summarize(
                    commands
                        .iter()
                        .map(|command| effect_caption(command).0.to_string()),
                    0,
                ),
                ACTION,
            ),
            NodeKind::MixedEffectExecute { commands, .. } => (
                "Imported effects",
                summarize(
                    commands.iter().filter_map(|command| match command {
                        super::model::EffectCommandSlot::Editable(command) => {
                            Some(effect_caption(command).0.to_string())
                        }
                        super::model::EffectCommandSlot::Preserved(_) => None,
                    }),
                    commands
                        .iter()
                        .filter(|command| {
                            matches!(command, super::model::EffectCommandSlot::Preserved(_))
                        })
                        .count(),
                ),
                ACTION,
            ),
            NodeKind::Branch { condition, .. } => ("If", condition_caption(condition), FLOW),
            NodeKind::Repeat { motion_loop, .. } if motion_loop.is_loop() => {
                ("Repeat", "Every motion loop".into(), FLOW)
            }
            NodeKind::Repeat { count, .. } => ("Repeat", format!("{count} times"), FLOW),
            NodeKind::Preserved { .. } => (
                "Native structure",
                "Preserved · not fully editable".into(),
                Color32::GRAY,
            ),
        };
        // A command without its own caption is better named by what it is than by "Engine action".
        let internal = match title {
            "Engine action" => "Engine action",
            "Execute at frame" => "Native execute frame · source export",
            _ => "",
        };
        let title = if title == "Engine action" {
            super::plain::block_name(&std::mem::take(&mut detail)).to_owned()
        } else if title == "If" {
            format!("If {}", std::mem::take(&mut detail))
        } else {
            super::plain::block_name(title).into()
        };
        let detail = super::plain::detail(detail);
        blocks.push(Block {
            internal,
            key: key.clone(),
            title,
            detail,
            position: [depth as f32 * 230.0, *row as f32 * 110.0],
            color,
            ports: match &node.kind {
                NodeKind::Execute { commands }
                    if matches!(
                        commands.as_slice(),
                        [crate::data::ExcuteStmt::GimmickEvent { .. }]
                    ) =>
                {
                    vec![FlowPort::Next.into(), OutputPort::Event]
                }
                // Next sits mid-card, level with the chain, so it cannot be
                // mistaken for a wire leaving Then.
                NodeKind::Branch { .. } => vec![
                    FlowPort::Then.into(),
                    FlowPort::Next.into(),
                    FlowPort::Else.into(),
                ],
                NodeKind::Repeat { .. } => vec![FlowPort::Next.into(), FlowPort::Repeat.into()],
                _ => vec![FlowPort::Next.into()],
            },
            inputs: vec![FlowPort::Next.into()],
        });
        *row += 1;
        let children: Vec<(&str, &[MoveNode])> = match &node.kind {
            NodeKind::Repeat { body, .. } => vec![("Body", body)],
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => vec![("Then", then_nodes), ("Else", else_nodes)],
            _ => Vec::new(),
        };
        for (_, body) in children {
            project_nodes(body, depth + 1, row, blocks);
        }
    }
}

/// "Hitbox ×2 · Smash-hold flag · 1 preserved": what a multi-command block does, in order.
fn summarize(titles: impl Iterator<Item = String>, preserved: usize) -> String {
    let mut counted: Vec<(String, usize)> = Vec::new();
    for title in titles {
        match counted.iter_mut().find(|(seen, _)| *seen == title) {
            Some((_, count)) => *count += 1,
            None => counted.push((title, 1)),
        }
    }
    let mut parts: Vec<String> = counted
        .into_iter()
        .map(|(title, count)| {
            if count > 1 {
                format!("{title} ×{count}")
            } else {
                title
            }
        })
        .collect();
    if preserved > 0 {
        parts.push(format!("{preserved} kept from the game"));
    }
    parts.join(" · ")
}

fn reconnect(
    graph: &mut MoveGraph,
    from: &str,
    port: impl Into<OutputPort>,
    to: &str,
) -> Result<(), String> {
    let OutputPort::Flow(port) = port.into() else {
        return Err("Connect a move execution output or a controller event receiver.".into());
    };
    let source = from
        .strip_prefix("node:")
        .and_then(|id| id.parse().ok())
        .ok_or("Select a move block")?;
    let target = to
        .strip_prefix("node:")
        .and_then(|id| id.parse().ok())
        .ok_or("Select a move block")?;
    let track = graph
        .tracks
        .iter_mut()
        .find(|track| contains(&track.nodes, source) && contains(&track.nodes, target))
        .ok_or("Connect blocks on the same move track")?;
    super::flow::reconnect_port(&mut track.nodes, source, port, target)
}
pub fn contains(nodes: &[MoveNode], id: u32) -> bool {
    nodes.iter().any(|node| {
        node.id == id
            || match &node.kind {
                NodeKind::Repeat { body, .. } => contains(body, id),
                NodeKind::Branch {
                    then_nodes,
                    else_nodes,
                    ..
                } => contains(then_nodes, id) || contains(else_nodes, id),
                _ => false,
            }
    })
}

pub fn prune_positions(graph: &mut MoveGraph) {
    let keys = super::flow::known_position_keys(graph);
    graph.positions.retain(|key, _| keys.contains(key));
}

#[cfg(test)]
mod tests {
    use crate::move_nodes::test_support as fixture;
    use crate::move_nodes::test_support::assert_json_eq;

    #[test]
    fn removed_card_selection_and_pending_center_are_cleared_without_program_edits() {
        let graph = graph();
        let before = serde_json::to_value(&graph).unwrap();
        let (blocks, _) = project(&graph, Some(Track::Game));
        let mut canvas = CanvasState {
            selected: Some("node:999999".into()),
            center_on: Some("node:999999".into()),
            ..Default::default()
        };
        canvas.reconcile_selection(&blocks);
        assert!(canvas.selected.is_none());
        assert!(canvas.center_on.is_none());
        assert_eq!(serde_json::to_value(graph).unwrap(), before);
    }

    #[test]
    fn valid_card_selection_survives_reprojection_including_offscreen_cards() {
        let graph = graph();
        let (blocks, _) = project(&graph, Some(Track::Game));
        for block in &blocks {
            let mut canvas = CanvasState {
                selected: Some(block.key.clone()),
                center_on: Some(block.key.clone()),
                pan: egui::vec2(-100_000.0, -100_000.0),
                ..Default::default()
            };
            canvas.reconcile_selection(&blocks);
            assert_eq!(canvas.selected.as_deref(), Some(block.key.as_str()));
            assert_eq!(canvas.center_on.as_deref(), Some(block.key.as_str()));
        }
    }

    use super::*;
    use crate::data::ExcuteStmt;

    fn graph() -> MoveGraph {
        MoveGraph {
            tracks: vec![fixture::track(
                Track::Game,
                vec![
                    fixture::frame(1, 1.0),
                    fixture::wait(2, 2.0),
                    fixture::execute(3, vec![ExcuteStmt::ClearAll]),
                ],
            )],
            ..Default::default()
        }
    }

    #[test]
    fn track_reset_keeps_the_workspace_open_but_clears_transient_graph_state() {
        for expanded in [false, true] {
            let mut state = CanvasState {
                expanded,
                selected: Some("node:3".into()),
                connecting: Some(("node:3".into(), OutputPort::Event)),
                pan: egui::vec2(20.0, 40.0),
                zoom: 2.5,
                search: "old track".into(),
                error: "old error".into(),
                hover_error: "old hover".into(),
                viewport: Some(egui::vec2(100.0, 100.0)),
                ..Default::default()
            };
            state.reset();
            assert_eq!(state.expanded, expanded);
            assert!(state.fit);
            assert!(state.selected.is_none());
            assert!(state.connecting.is_none());
            assert!(state.viewport.is_none());
            assert_eq!(state.pan, egui::Vec2::ZERO);
            assert_eq!(state.zoom, 0.0);
            assert!(
                state.search.is_empty() && state.error.is_empty() && state.hover_error.is_empty()
            );
        }
    }

    #[test]
    fn mixed_effect_cards_keep_one_execute_group_with_clear_editable_counts() {
        let script = crate::data::EffectScript {
            stmts: vec![crate::data::EffectStmt::Excute(vec![
                crate::data::EffectMacro::LastEffectSetRate { rate: 0.5 },
                crate::data::EffectMacro::Raw("native_effect(agent);".into()),
            ])],
        };
        let track = super::super::effects::import_for_fighter("mario", &script);
        let node_key = format!("node:{}", track.nodes[0].id);
        let graph = MoveGraph {
            tracks: vec![track],
            ..Default::default()
        };
        let before = serde_json::to_value(&graph).unwrap();
        let (blocks, _) = project(&graph, Some(Track::Effect));
        let matching: Vec<_> = blocks
            .iter()
            .filter(|block| block.key == node_key)
            .collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].title, "Visual effects");
        assert_eq!(matching[0].detail, "Effect rate · 1 kept from the game");
        assert_eq!(serde_json::to_value(&graph).unwrap(), before);
    }

    fn frame(
        ctx: &egui::Context,
        state: &mut CanvasState,
        graph: &mut MoveGraph,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| state.ui(ui, graph, Some(Track::Game), 400.0),
        )
    }

    fn label(shape: &egui::epaint::Shape, wanted: &str) -> Option<Pos2> {
        match shape {
            egui::epaint::Shape::Text(text) if text.galley.text() == wanted => Some(text.pos),
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().find_map(|shape| label(shape, wanted))
            }
            _ => None,
        }
    }

    fn point(output: &egui::FullOutput, wanted: &str) -> Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| label(&shape.shape, wanted))
            .expect("block rendered")
    }

    fn drag(
        ctx: &egui::Context,
        state: &mut CanvasState,
        graph: &mut MoveGraph,
        start: Pos2,
        end: Pos2,
    ) {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            ctx,
            state,
            graph,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        frame(
            ctx,
            state,
            graph,
            vec![egui::Event::PointerMoved(start + Vec2::new(10.0, 0.0))],
        );
        frame(ctx, state, graph, vec![egui::Event::PointerMoved(end)]);
        frame(ctx, state, graph, vec![button(end, false)]);
    }

    #[test]
    fn imported_timing_has_readable_title_and_internal_hover_name() {
        let mut node = fixture::wait(1, 1.0);
        node.kind = NodeKind::NativeExecuteFrame { frame: 12.0 };
        let mut blocks = Vec::new();
        project_nodes(&[node], 0, &mut 0, &mut blocks);
        assert_eq!(blocks[0].title, "At frame");
        assert_eq!(blocks[0].detail, "Frame 12");
        assert_eq!(blocks[0].internal, "Native execute frame · source export");
    }

    #[test]
    fn fit_large_field_stays_readable_and_starts_at_beginning() {
        let ctx = egui::Context::default();
        let mut state = CanvasState {
            fit: true,
            zoom: 1.0,
            ..Default::default()
        };
        let mut graph = graph();
        graph.tracks[0].nodes = (1..=200).map(|id| fixture::wait(id, 1.0)).collect();
        graph.positions.insert("node:200".into(), [9000.0, 30000.0]);
        let saved = graph.positions.clone();
        let output = frame(&ctx, &mut state, &mut graph, vec![]);
        assert_eq!(state.zoom, 0.85);
        assert_eq!(state.pan, Vec2::new(16.0, 16.0));
        assert!(
            output.shapes.len() < 10_000,
            "overview grid grew with world-space extent"
        );
        assert_eq!(graph.positions, saved, "Fit changed the stored layout");
    }

    #[test]
    fn dragging_a_block_never_changes_the_workspace_layout() {
        let graph = MoveGraph {
            tracks: vec![super::super::model::MoveTrack {
                track: Track::Game,
                nodes: vec![
                    MoveNode {
                        id: 1,
                        kind: NodeKind::Frame { frame: 5.0 },
                    },
                    MoveNode {
                        id: 2,
                        kind: NodeKind::Frame { frame: 9.0 },
                    },
                ],
                baseline: Vec::new(),
                effect_execute_baseline: Vec::new(),
            }],
            ..MoveGraph::default()
        };
        let (_, _, lanes, marks, _) = project_workspace(&graph);
        let mut dragged = graph.clone();
        dragged.positions.insert("node:1".into(), [5000.0, 3000.0]);
        let (_, _, moved_lanes, moved_marks, _) = project_workspace(&dragged);
        assert_eq!(marks, moved_marks);
        assert_eq!(lanes[0].bottom, moved_lanes[0].bottom);
    }

    #[test]
    fn pointer_drag_moves_a_card_and_layout_roundtrips() {
        let ctx = egui::Context::default();
        let mut state = CanvasState::default();
        let mut graph = graph();
        frame(&ctx, &mut state, &mut graph, vec![]);
        let output = frame(&ctx, &mut state, &mut graph, vec![]);
        let center = point(&output, "Wait") + Vec2::new(78.0, 22.0) * state.zoom;
        drag(
            &ctx,
            &mut state,
            &mut graph,
            center,
            center + Vec2::new(80.0, 20.0),
        );
        assert_eq!(state.selected.as_deref(), Some("node:2"));
        assert!(graph.positions["node:2"][0] > 40.0);
        let restored: MoveGraph =
            serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        assert_json_eq!(&restored, &graph);
        assert!(super::super::timeline::compile("mario", "attack11", &restored).is_ok());
    }

    #[test]
    fn find_block_search_stays_open_while_editing_and_centers_the_selected_card() {
        let ctx = egui::Context::default();
        let mut state = CanvasState::default();
        let mut graph = graph();
        frame(&ctx, &mut state, &mut graph, vec![]);
        let output = frame(&ctx, &mut state, &mut graph, vec![]);
        let click = |state: &mut CanvasState, graph: &mut MoveGraph, pos: Pos2| {
            frame(
                &ctx,
                state,
                graph,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                ],
            );
            frame(
                &ctx,
                state,
                graph,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            );
            frame(&ctx, state, graph, vec![])
        };
        let output = click(
            &mut state,
            &mut graph,
            point(&output, "Find block") + Vec2::new(15.0, 6.0),
        );
        let output = click(
            &mut state,
            &mut graph,
            point(&output, "Name or value…") + Vec2::new(15.0, 6.0),
        );
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| label(&shape.shape, "Name or value…").is_some()),
            "clicking the search field closed the popup"
        );
        frame(
            &ctx,
            &mut state,
            &mut graph,
            vec![egui::Event::Text("wait".into())],
        );
        let output = frame(&ctx, &mut state, &mut graph, vec![]);
        assert_eq!(state.search, "wait");
        let output = click(
            &mut state,
            &mut graph,
            point(&output, "Wait · 2 motion frames") + Vec2::new(15.0, 6.0),
        );
        assert_eq!(state.selected.as_deref(), Some("node:2"));
        assert!(
            output
                .shapes
                .iter()
                .all(|shape| label(&shape.shape, "Wait · 2 motion frames").is_none()),
            "choosing a result did not close the popup"
        );
        assert_eq!(state.zoom, 1.0);
        assert!(
            graph.positions.is_empty(),
            "navigation changed the saved graph layout"
        );
    }

    #[test]
    fn pointer_wire_drag_changes_real_execution_order() {
        let ctx = egui::Context::default();
        let mut state = CanvasState::default();
        let mut graph = graph();
        frame(&ctx, &mut state, &mut graph, vec![]);
        let output = frame(&ctx, &mut state, &mut graph, vec![]);
        let source = point(&output, "At frame") + Vec2::new(168.0, 22.0) * state.zoom;
        let target = point(&output, "End all hitboxes") + Vec2::new(-12.0, 22.0) * state.zoom;
        drag(&ctx, &mut state, &mut graph, source, target);
        assert_eq!(
            graph.tracks[0]
                .nodes
                .iter()
                .map(|node| node.id)
                .collect::<Vec<_>>(),
            [1, 3, 2]
        );
        assert!(state.error.is_empty(), "{}", state.error);
    }
}
