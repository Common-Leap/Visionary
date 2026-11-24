//! Parts made from parts: how an effect's parts nest, the tree every list of them draws, and the
//! part menu.
//!
//! The list is flat and parent-first, and `EmitterDef::depth` says which part each is made from.
//! The game plays one level of that: a part on its own, and at most 16 parts made from it.

use super::*;

/// How far a part sits in from the one it is made from.
const INDENT: f32 = 16.0;

/// How one effect's parts nest, by their places in its list.
#[derive(Clone, Default)]
pub(super) struct PartTree {
    names: Vec<String>,
    depths: Vec<u8>,
}

impl PartTree {
    pub fn of(emitters: &[EmitterDef]) -> Self {
        let names: Vec<&str> = emitters.iter().map(|e| e.name.as_str()).collect();
        Self {
            names: part_list_names(&names),
            depths: emitters.iter().map(|e| e.depth).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.depths.len()
    }

    /// The part's name as its list shows it.
    pub fn name(&self, i: usize) -> &str {
        self.names.get(i).map_or("", String::as_str)
    }

    pub fn depth(&self, i: usize) -> u8 {
        self.depths.get(i).copied().unwrap_or(0)
    }

    /// The part `i` is made from.
    pub fn parent(&self, i: usize) -> Option<usize> {
        let depth = self.depth(i);
        (0..i).rev().find(|&k| self.depths[k] < depth)
    }

    /// The part on its own that `i` is, or is made from.
    pub fn root(&self, i: usize) -> usize {
        std::iter::successors(Some(i), |&k| self.parent(k))
            .last()
            .unwrap_or(i)
    }

    /// The part and everything made from it.
    pub fn family(&self, i: usize) -> std::ops::Range<usize> {
        let depth = self.depth(i);
        let end = (i + 1..self.len()).find(|&k| self.depths[k] <= depth);
        i..end.unwrap_or(self.len().max(i))
    }

    /// The parts made from `i`.
    pub fn children(&self, i: usize) -> Vec<usize> {
        self.family(i)
            .skip(1)
            .filter(|&k| self.parent(k) == Some(i))
            .collect()
    }

    /// The part `i` trades places with when it moves one place `later` (or earlier): the next
    /// one made from the same part, or on its own like it.
    pub fn sibling(&self, i: usize, later: bool) -> Option<usize> {
        let (depth, parent) = (self.depth(i), self.parent(i));
        let mine = |k: &usize| self.depth(*k) == depth && self.parent(*k) == parent;
        if later {
            Some(self.family(i).end).filter(|k| *k < self.len() && mine(k))
        } else {
            (0..i).rev().find(mine)
        }
    }

    /// The name with the part it is made from before it, for a line of text: "Clock 1 › Trace 1".
    pub fn path(&self, i: usize) -> String {
        match self.parent(i) {
            Some(parent) => format!("{} › {}", self.name(parent), self.name(i)),
            None => self.name(i).to_owned(),
        }
    }

    /// The name with how many parts are made from it, when some are: "Clock 1 (2)".
    pub fn counted(&self, i: usize) -> String {
        match self.children(i).len() {
            0 => self.name(i).to_owned(),
            n => format!("{} ({n})", self.name(i)),
        }
    }

    /// "Clock 1 and its child part", "Clock 1 and its 2 child parts".
    pub fn with_children(&self, i: usize) -> String {
        match self.family(i).len().saturating_sub(1) {
            0 => self.name(i).to_owned(),
            1 => format!("{} and its child part", self.name(i)),
            n => format!("{} and its {n} child parts", self.name(i)),
        }
    }

    /// The names of the parts made from `i`, or None when there are none.
    pub fn children_names(&self, i: usize) -> Option<String> {
        let children = self.children(i);
        let names: Vec<&str> = children.iter().map(|&k| self.name(k)).collect();
        (!names.is_empty()).then(|| names.join(", "))
    }

    /// What a part's hover says of the parts around it: what it is made from and how, or what is
    /// made from it.
    pub fn hover(&self, i: usize, em: &EmitterDef) -> String {
        let mut lines = String::new();
        if let Some(parent) = self.parent(i) {
            lines += "\n";
            lines += &made_from_words(em, self.name(parent));
        }
        if let Some(children) = self.children_names(i) {
            lines += &format!("\nMade from it: {children}");
        }
        lines
    }

    /// The hover of "Show only this part".
    pub fn solo_hover(&self, i: usize) -> String {
        match (self.parent(i), self.children_names(i)) {
            (Some(parent), _) => format!(
                "Hide the effect's other parts in the view. {} still runs, not drawn, as this \
                 part is made from it.",
                self.name(parent)
            ),
            (None, Some(children)) => format!(
                "Hide the effect's other parts in the view. The parts made from this one stay: \
                 {children}."
            ),
            (None, None) => {
                "Hide the effect's other parts in the view while you edit this one.".to_owned()
            }
        }
    }
}

fn is_on(em: &EmitterDef, id: &str) -> bool {
    attr(em, id).is_some_and(|v| v.as_bool())
}

/// What the game does for a part made from another (Smash 0x8d70): how it comes out of the
/// parent's particles, when in their life, and for how long.
fn made_from_words(em: &EmitterDef, parent: &str) -> String {
    let mut words = if is_on(em, "child_inheritance.enable_emitter_particle") {
        format!("Made from {parent}: the game starts one for each of its particles")
    } else {
        format!("Made from {parent}: its particles come out of each particle of {parent}")
    };
    let timing = attr(em, "emission.timing").map_or(0, |v| v.as_i64());
    if timing > 0 {
        words += &format!(", {timing}% into that particle's life");
    }
    if is_on(em, "emission.is_emit_dist_enabled") {
        words += ", as that particle moves";
    }
    if is_on(em, "emission.is_one_time") {
        let frames = attr(em, "emission.duration").map_or(0, |v| v.as_i64());
        words += &match frames {
            1 => ", for 1 frame".to_owned(),
            n => format!(", for {n} frames"),
        };
    }
    words
}

/// The settings of what a part takes from the one it is made from, those the game reads
/// (`child_inheritance.color_scale` and `.draw_path` have no reader).
pub(super) fn taken_rows(em: &EmitterDef) -> Vec<usize> {
    let unread = [
        "child_inheritance.color_scale",
        "child_inheritance.draw_path",
    ];
    crate::eff_attrs::table()
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            a.group == crate::eff_attrs::G_INHERIT
                && !unread.contains(&a.id)
                && em.attrs.get(*i).copied().flatten().is_some()
        })
        .map(|(i, _)| i)
        .collect()
}

/// The names of those switched on.
pub(super) fn switches_on(em: &EmitterDef, rows: &[usize]) -> Vec<&'static str> {
    let table = crate::eff_attrs::table();
    rows.iter()
        .filter(|&&i| {
            matches!(table[i].kind, AttrKind::Flag) && em.attrs[i].is_some_and(|v| v.as_bool())
        })
        .map(|&i| table[i].label)
        .collect()
}

/// What a part takes from the one it is made from, open: the switches and rates themselves.
pub(super) fn draw_taken(
    ui: &mut Ui,
    em: &mut EmitterDef,
    pr: &EmitterSnapshot,
    parent: &str,
    textures: &[TextureInfo],
) -> bool {
    let rows = taken_rows(em);
    if rows.is_empty() {
        return false;
    }
    // Not what is drawn first, nor how many there are: those take nothing.
    let mut on = switches_on(em, &rows);
    on.retain(|name| !["Pre-draw", "Emitter per particle"].contains(name));
    let mut title = egui::RichText::new(match on.is_empty() {
        true => format!("Taken from {parent}: nothing"),
        false => format!("Taken from {parent}: {}", on.join(", ")),
    });
    let differs = |i: &usize| match (em.attrs[*i], pr.attrs.get(*i).copied().flatten()) {
        (Some(now), Some(was)) => !now.same(was),
        _ => false,
    };
    if rows.iter().any(differs) {
        title = title.color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60));
    }
    ui.add_space(6.0);
    let mut changed = false;
    egui::CollapsingHeader::new(wrapped_header(ui, title.strong()))
        .id_salt("part_taken")
        .default_open(true)
        .show(ui, |ui| {
            changed = attr_rows_by_index(ui, "part_taken_rows", em, pr, &rows, textures);
        });
    changed
}

/// The lines joining a row to the one it is made from, with the room they take before its name.
fn tree_lines(ui: &mut Ui, depth: u8, last: bool) {
    if depth == 0 {
        return;
    }
    let height = ui.spacing().interact_size.y;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(
            INDENT * f32::from(depth) - ui.spacing().item_spacing.x,
            height,
        ),
        egui::Sense::hover(),
    );
    // Up across the gap between the rows, to the row above.
    let left = rect.left() + INDENT * f32::from(depth - 1);
    paint_branch(
        ui,
        egui::pos2(left, rect.top() - ui.spacing().item_spacing.y - 2.0),
        rect.center().y,
        if last {
            rect.center().y
        } else {
            rect.bottom() + 2.0
        },
    );
}

/// One branch of a tree: down from `top` to `to`, and across at `at`.
fn paint_branch(ui: &Ui, top: egui::Pos2, at: f32, to: f32) {
    let stroke = egui::Stroke::new(1.0, ui.visuals().weak_text_color());
    let x = top.x + 7.0;
    ui.painter().vline(x, top.y..=to, stroke);
    ui.painter().hline(x..=top.x + INDENT - 3.0, at, stroke);
}

/// [`tree_lines`] for part `i` of a list of every part.
pub(super) fn part_lines(ui: &mut Ui, tree: &PartTree, i: usize) {
    tree_lines(ui, tree.depth(i), tree.sibling(i, true).is_none());
}

/// [`tree_lines`] for part `i` of a list of the parts `listed`.
pub(super) fn listed_lines(ui: &mut Ui, tree: &PartTree, i: usize, listed: &[usize]) {
    let parent = tree.parent(i);
    let last = !listed.iter().any(|&k| k > i && tree.parent(k) == parent);
    tree_lines(ui, tree.depth(i), last);
}

/// Where the part tabs go.
pub(super) struct TabPlaces {
    /// Each tab's left edge, and which line it is on.
    pub at: Vec<(f32, usize)>,
    /// How many times the columns wrapped, and how many lines they take.
    pub rows: usize,
    pub lines: usize,
}

/// A part on its own starts a column with the parts made from it under it, a step in; columns
/// run left to right and wrap to a new row.
pub(super) fn tab_places(widths: &[f32], depths: &[u8], room: f32, gap: f32) -> TabPlaces {
    let mut at = Vec::with_capacity(widths.len());
    let (mut x, mut line, mut tallest, mut rows) = (0.0f32, 0, 0, 0);
    let mut i = 0;
    while i < widths.len() {
        let end = (i + 1..widths.len())
            .find(|&k| depths[k] == 0)
            .unwrap_or(widths.len());
        let width = (i..end)
            .map(|k| widths[k] + INDENT * f32::from(depths[k]))
            .fold(0.0, f32::max);
        if rows == 0 || x + width > room && x > 0.0 {
            x = 0.0;
            line += tallest;
            tallest = 0;
            rows += 1;
        }
        for (down, k) in (i..end).enumerate() {
            at.push((x + INDENT * f32::from(depths[k]), line + down));
        }
        tallest = tallest.max(end - i);
        x += width + gap;
        i = end;
    }
    TabPlaces {
        at,
        rows,
        lines: line + tallest,
    }
}

/// The lines the part tabs may take in a panel `height` tall before the parts are picked from a
/// list instead, so the settings under them keep their room in a small window.
pub(super) fn tab_lines_allowed(height: f32, line: f32) -> usize {
    ((height * 0.25 / line.max(1.0)) as usize).clamp(2, 6)
}

/// How wide a button holding `text` is.
pub(super) fn button_width(ui: &Ui, text: &str, style: egui::TextStyle) -> f32 {
    let font = style.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, egui::Color32::WHITE)
        .size()
        .x
        + 2.0 * ui.spacing().button_padding.x
}

/// Draw the part tabs at `places`, and after them the part row's two small buttons, each with
/// whether it can be clicked. Returns each tab's response and each button's.
pub(super) fn draw_tabs(
    ui: &mut Ui,
    tree: &PartTree,
    places: &TabPlaces,
    widths: &[f32],
    tabs: Vec<(egui::RichText, String)>,
    selected: usize,
    buttons: [(&str, bool); 2],
) -> (Vec<egui::Response>, [egui::Response; 2]) {
    let gap = ui.spacing().item_spacing;
    let height = ui.spacing().interact_size.y;
    let room = ui.available_width();
    let step = height + gap.y;
    let (area, _) = ui.allocate_exact_size(
        egui::vec2(room, places.lines as f32 * step - gap.y),
        egui::Sense::hover(),
    );
    // Placed by hand in a child of their own: each one put moves the row under the tabs up to it.
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(area));
    let ui = &mut ui;
    let rect = |k: usize| {
        let (x, line) = places.at[k];
        let min = area.min + egui::vec2(x, line as f32 * step);
        // A name wider than the panel is cut at its edge.
        let width = widths[k].min((room - x).max(height));
        egui::Rect::from_min_size(min, egui::vec2(width, height))
    };
    for i in 0..tree.len() {
        // One line down from the part it is made from, however many are between.
        if let Some(parent) = tree.parent(i) {
            let (own, from) = (rect(i), rect(parent));
            paint_branch(
                ui,
                egui::pos2(own.left() - INDENT, from.bottom() + 1.0),
                own.center().y,
                own.center().y,
            );
        }
    }
    let responses = tabs
        .into_iter()
        .enumerate()
        .map(|(i, (text, hover))| {
            ui.put(
                rect(i),
                egui::Button::selectable(selected == i, text).truncate(),
            )
            .on_hover_text(hover)
        })
        .collect();
    let buttons = std::array::from_fn(|b| {
        let (label, enabled) = buttons[b];
        let place = egui::UiBuilder::new()
            .max_rect(rect(tree.len() + b))
            .layout(egui::Layout::left_to_right(egui::Align::Center));
        ui.scope_builder(place, |ui| {
            ui.add_enabled(enabled, egui::Button::new(label).small())
        })
        .inner
    });
    (responses, buttons)
}

/// What the part menu asked for.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PartAction {
    Rename(String),
    Reset,
    Copy,
    Paste,
    PasteAsChild,
    Duplicate,
    Remove,
    DrawEarlier,
    DrawLater,
    MakeFromAbove,
    MakeOnItsOwn,
}

/// One part's menu.
pub(super) struct PartMenu<'a> {
    pub set_idx: usize,
    pub part: usize,
    pub tree: &'a PartTree,
    /// The part's name in the file.
    pub file_name: &'a str,
    pub edited: bool,
    pub clip: Option<&'a PartClip>,
    pub clip_blocked: Option<&'a str>,
}

impl PartMenu<'_> {
    /// Draw the menu. Every command closes it; the name field keeps it open while it is typed in.
    pub fn show(
        &self,
        ui: &mut Ui,
        renaming: &mut Option<(usize, usize, String)>,
    ) -> Option<PartAction> {
        let (tree, i) = (self.tree, self.part);
        let mut action = None;
        // The name in words, typed over in place: kept from the frame the menu opened.
        if !matches!(renaming, Some((s, p, _)) if *s == self.set_idx && *p == i) {
            *renaming = Some((self.set_idx, i, part_words(self.file_name)));
        }
        if let Some((_, _, typed)) = renaming.as_mut() {
            let field = ui
                .add(egui::TextEdit::singleline(typed).desired_width(160.0))
                .on_hover_text(format!(
                    "The part's name. Enter to keep it.\n\nIn the file: {}",
                    self.file_name
                ));
            if field.lost_focus() && typed.trim() != part_words(self.file_name) {
                action = Some(PartAction::Rename(typed.clone()));
                if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    ui.close();
                }
            }
        }
        let mut command = |ui: &mut Ui, label: &str, able: Result<String, String>, does| {
            let button = ui.add_enabled(able.is_ok(), egui::Button::new(label));
            let button = match &able {
                Ok(hover) if hover.is_empty() => button,
                Ok(hover) => button.on_hover_text(hover),
                Err(why) if why.is_empty() => button,
                Err(why) => button.on_disabled_hover_text(why),
            };
            if button.clicked() {
                action = Some(does);
                ui.close();
            }
        };
        let when = |able: bool, hover: &str, why: &str| {
            if able {
                Ok(hover.to_owned())
            } else {
                Err(why.to_owned())
            }
        };
        ui.separator();
        command(
            ui,
            "Reset this part",
            when(
                self.edited,
                "Put this part back as the game has it",
                "This part has the game's values",
            ),
            PartAction::Reset,
        );
        ui.separator();
        command(
            ui,
            "Copy",
            Ok(format!(
                "Copy {}, with {} edits (Ctrl+C)",
                tree.with_children(i),
                if tree.children(i).is_empty() {
                    "its"
                } else {
                    "their"
                }
            )),
            PartAction::Copy,
        );
        let paste = |as_child: bool| paste_state(self.clip, self.clip_blocked, tree, i, as_child);
        command(ui, "Paste", paste(false), PartAction::Paste);
        command(ui, "Paste as child", paste(true), PartAction::PasteAsChild);
        command(
            ui,
            "Duplicate",
            when(
                room_to_copy(&tree.depths, i),
                &format!("Add another part just like {}", tree.with_children(i)),
                &format!(
                    "The game holds at most {} parts made from one part",
                    crate::eff_validate::MAX_CHILDREN
                ),
            ),
            PartAction::Duplicate,
        );
        let (remove, hover) = match tree.children_names(i) {
            None => (
                "Remove".to_owned(),
                "Stop this part from playing".to_owned(),
            ),
            Some(names) => (
                match tree.family(i).len() - 1 {
                    1 => "Remove with its child part".to_owned(),
                    n => format!("Remove with its {n} child parts"),
                },
                format!("Stop this part, and {names} made from it, from playing"),
            ),
        };
        command(ui, &remove, Ok(hover), PartAction::Remove);
        ui.separator();
        // A part made from another moves among that part's own.
        for (label, later, does) in [
            ("Draw earlier", false, PartAction::DrawEarlier),
            ("Draw later", true, PartAction::DrawLater),
        ] {
            let able = match (tree.sibling(i, later), tree.parent(i)) {
                (Some(_), _) if tree.children(i).is_empty() => Ok(String::new()),
                (Some(_), _) => Ok("Its child parts move with it".to_owned()),
                (None, Some(parent)) => Err(format!(
                    "It is the {} part made from {}",
                    if later { "last" } else { "first" },
                    tree.name(parent)
                )),
                (None, None) => Err(String::new()),
            };
            command(ui, label, able, does);
        }
        ui.separator();
        if let Some(parent) = tree.parent(i) {
            command(
                ui,
                "Make on its own",
                Ok(format!(
                    "No longer made from {}: it plays with the effect",
                    tree.name(parent)
                )),
                PartAction::MakeOnItsOwn,
            );
        } else {
            let above = (0..i).rev().find(|&k| tree.depth(k) == 0);
            let label = match above {
                Some(above) => format!("Make from {}", tree.name(above)),
                None => "Make from the part above".to_owned(),
            };
            let name = above.map_or("", |above| tree.name(above));
            command(
                ui,
                &label,
                when(
                    room_to_nest(&tree.depths, i),
                    &format!("Its particles come out of the particles of {name}"),
                    "Only a part on its own, with nothing made from it, can be made from \
                     another, and one part makes at most 16",
                ),
                PartAction::MakeFromAbove,
            );
        }
        action
    }
}

impl EffEditor {
    /// The open effect's name as the list shows it.
    pub(super) fn open_effect_name(&self) -> String {
        self.selected_entry
            .and_then(|i| entry_list_names(&self.entries).get(i).cloned())
            .unwrap_or_default()
    }

    /// The open set's parts, as a tree.
    pub(super) fn part_tree(&self, set_idx: usize) -> PartTree {
        self.ptcl
            .as_ref()
            .and_then(|p| p.emitter_sets.get(set_idx))
            .map(|set| PartTree::of(&set.emitters))
            .unwrap_or_default()
    }

    /// Do what the part menu asked for, as one undo step.
    ///
    /// The list is flat but the parts are a tree, carried by `depth` and position, so every one
    /// of these works on a part with everything made from it: a parent moved without its
    /// children would hand them to whichever part ended up above them.
    pub(super) fn part_action(&mut self, set_idx: usize, i: usize, action: PartAction) {
        let effect = self.open_effect_name();
        let tree = self.part_tree(set_idx);
        match action {
            PartAction::Copy => return self.copy_part(set_idx, i),
            PartAction::Paste | PartAction::PasteAsChild => {
                self.selected_emitter = i;
                return self.paste_part(set_idx, action == PartAction::PasteAsChild);
            }
            PartAction::Rename(_) => self.renaming = None,
            _ => {}
        }
        if i >= tree.len() {
            return;
        }
        self.history_close_step();
        if action == PartAction::Reset {
            self.reset_part_fields(set_idx, i);
            self.history.next_label = Some(self.reset_part_label(set_idx, i));
            return;
        }
        let Some(set) = self
            .ptcl
            .as_mut()
            .and_then(|p| p.emitter_sets.get_mut(set_idx))
        else {
            return;
        };
        let emitters = &mut set.emitters;
        let range = tree.family(i);
        let name = tree.path(i);
        let whole = tree.with_children(i);
        // The part open before stays the one open after, wherever it went.
        let mut open = emitters
            .get(self.selected_emitter)
            .map(|e| e.name.clone())
            .unwrap_or_default();
        let label = match action {
            PartAction::Rename(typed) => {
                let typed = add_part::part_name_from_typed(&typed);
                let taken: Vec<&str> = (0..emitters.len())
                    .filter(|k| *k != i)
                    .map(|k| emitters[k].name.as_str())
                    .collect();
                let new = add_part::unique_part_name(&typed, &taken);
                if new.is_empty() || new == emitters[i].name {
                    return;
                }
                let label = format!("Rename {name} to {}", part_words(&new));
                if open == emitters[i].name {
                    open = new.clone();
                }
                emitters[i].name = new;
                label
            }
            PartAction::Duplicate => {
                let mut copies: Vec<EmitterDef> = emitters[range.clone()].to_vec();
                // Named against the list AS IT GROWS, so duplicating a parent with two
                // identically-shaped children does not name two parts the same.
                let mut taken = emitters.clone();
                for copy in copies.iter_mut() {
                    copy.name = unique_emitter_name(&copy.name, &taken);
                    // `source_idx` keeps naming the part this was copied FROM, so the copy is
                    // measured from the same values and the exporter knows what to clone.
                    taken.push(copy.clone());
                }
                // The copy is what gets edited next.
                open = copies[0].name.clone();
                emitters.splice(range.end..range.end, copies);
                format!("Duplicate {whole} in {effect}")
            }
            PartAction::Remove => {
                let next = emitters
                    .get(range.end)
                    .or(i.checked_sub(1).and_then(|k| emitters.get(k)));
                if emitters[range.clone()].iter().any(|e| e.name == open) {
                    open = next.map(|e| e.name.clone()).unwrap_or_default();
                }
                emitters.drain(range);
                format!("Remove {whole} from {effect}")
            }
            PartAction::DrawEarlier | PartAction::DrawLater => {
                let later = action == PartAction::DrawLater;
                let Some(other) = tree.sibling(i, later) else {
                    return;
                };
                let other = tree.family(other);
                if later {
                    emitters[range.start..other.end].rotate_left(range.len());
                } else {
                    emitters[other.start..range.end].rotate_left(range.start - other.start);
                }
                let place = if later { "later" } else { "earlier" };
                format!("Draw {name} {place} in {effect}")
            }
            PartAction::MakeFromAbove => {
                if !room_to_nest(&tree.depths, i) {
                    return;
                }
                emitters[i].depth = 1;
                let above = tree.root(i - 1);
                format!("Make {name} from {} in {effect}", tree.name(above))
            }
            PartAction::MakeOnItsOwn => {
                let Some(parent) = tree.parent(i) else {
                    return;
                };
                // After what its parent makes: left where it was, the parts under it would
                // become its own.
                let end = tree.family(parent).end;
                emitters[range.start..end].rotate_left(range.len());
                shift_subtree_depth(emitters, end - range.len(), -1);
                format!("Make {name} on its own in {effect}")
            }
            PartAction::Reset | PartAction::Copy | PartAction::Paste | PartAction::PasteAsChild => {
                return
            }
        };
        self.selected_emitter = emitters.iter().position(|e| e.name == open).unwrap_or(0);
        self.touch();
        self.history.next_label = Some(label);
    }
}

#[cfg(test)]
pub(super) mod tests;
