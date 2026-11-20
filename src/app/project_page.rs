//! The Project page: one card per character the project touches, each with a button per
//! part (moves, effects, sounds, model, animations, values, select screen). A part's button
//! is coloured when the project changes it and grey while it is still the game's; opening
//! it shows where its files are, lets the author pick another folder for them, and holds
//! that part's tools.

use super::*;
use crate::project_parts::{Part, ProjectParts, OTHER_EFFECTS};
use crate::ui::{act, chip, look, Look};

/// The narrowest a card is laid out; wider windows fit more columns.
const CARD_WIDTH: f32 = 560.0;

/// Characters the game shows as one that are several fighters underneath. Each gets one card,
/// with its fighters to switch between: every part button then acts on the one picked.
const GROUPS: [(&str, &[(&str, &str)]); 3] = [
    ("Pokémon Trainer", &[("pzenigame", "Squirtle"), ("pfushigisou", "Ivysaur"), ("plizardon", "Charizard"), ("ptrainer", "Trainer")]),
    ("Pyra / Mythra", &[("eflame", "Pyra"), ("elight", "Mythra")]),
    ("Ice Climbers", &[("popo", "Popo"), ("nana", "Nana")]),
];

fn group_of(fighter: &str) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
    GROUPS.iter().find(|(_, members)| members.iter().any(|(member, _)| member.eq_ignore_ascii_case(fighter))).copied()
}

/// One card on the page.
#[derive(Clone)]
pub(super) struct Card {
    /// The fighter whose files the card is about (the donor, for a character you added).
    fighter: String,
    slots: Vec<u8>,
    slot: u8,
    name: String,
    base: String,
    /// The select screen entry, when the card is a character you added.
    roster_key: crate::roster::RosterKey,
    /// A character made in this project, on top of `fighter`.
    made_here: bool,
    /// The fighters this card stands for, with their names, when it is more than one.
    members: &'static [(&'static str, &'static str)],
}

/// Something a card asked for, done once the page is drawn.
enum PageAction {
    Select(String, u8),
    Open(String, Part),
    PickSlot(String, u8),
    Hide(String),
    Add(String),
    PickFolder(String, Part, u8),
    ForgetFolder(String, Part, u8),
    Reveal(PathBuf),
    EditMoves(String, bool),
    EditEffects(String),
    EditOtherEffect(String),
    OpenEffectFile,
    EditValues(String),
    /// Show another fighter of a card that stands for several: (card name, fighter).
    Member(String, String),
    /// Something a character made in this project asked for.
    Made(crate::roster::new_character::NewCharacterAction),
    EditGimmicks(String),
    EditSelectScreen(crate::roster::RosterKey),
    Files(String, u8, FileAction),
}

#[derive(Clone, Copy)]
enum FileAction {
    CopyGameModel,
    CopyGameCharacter,
    AddModelFolder,
    AddModelFiles,
    AddAnimations,
    AddSwing,
    AddEffects,
    AddSounds,
    Send,
    Stop,
}

/// Each part's colour on its button.
fn part_colour(part: Part) -> Color32 {
    match part {
        Part::Moves => Color32::from_rgb(255, 150, 70),
        Part::Effects => Color32::from_rgb(196, 120, 255),
        Part::Sounds => Color32::from_rgb(60, 200, 200),
        Part::Model => Color32::from_rgb(110, 205, 120),
        Part::Animations => Color32::from_rgb(95, 160, 255),
        Part::Gimmicks => Color32::from_rgb(255, 95, 95),
        Part::Values => Color32::from_rgb(240, 200, 70),
        Part::SelectScreen => Color32::from_rgb(255, 115, 160),
    }
}

/// Draw `part`'s icon in `rect`.
fn paint_part_icon(painter: &egui::Painter, rect: egui::Rect, part: Part, colour: Color32) {
    let c = rect.center();
    let s = rect.width() / 2.0;
    let stroke = egui::Stroke::new((s * 0.12).max(1.5), colour);
    let p = |x: f32, y: f32| c + egui::vec2(x * s, y * s);
    match part {
        Part::Moves => {
            // A game controller: body, d-pad, two buttons.
            let body = egui::Rect::from_min_max(p(-0.8, -0.4), p(0.8, 0.45));
            painter.rect_stroke(body, s * 0.35, stroke, egui::StrokeKind::Middle);
            painter.line_segment([p(-0.62, 0.02), p(-0.22, 0.02)], stroke);
            painter.line_segment([p(-0.42, -0.18), p(-0.42, 0.22)], stroke);
            painter.circle_filled(p(0.3, -0.06), s * 0.1, colour);
            painter.circle_filled(p(0.52, 0.14), s * 0.1, colour);
        }
        Part::Effects => {
            // A four-point sparkle and a small one.
            let star = |at: egui::Pos2, r: f32| {
                let w = r * 0.28;
                vec![
                    at + egui::vec2(0.0, -r),
                    at + egui::vec2(w, -w),
                    at + egui::vec2(r, 0.0),
                    at + egui::vec2(w, w),
                    at + egui::vec2(0.0, r),
                    at + egui::vec2(-w, w),
                    at + egui::vec2(-r, 0.0),
                    at + egui::vec2(-w, -w),
                ]
            };
            painter.add(egui::Shape::convex_polygon(star(p(-0.15, 0.1), s * 0.7), colour, egui::Stroke::NONE));
            painter.add(egui::Shape::convex_polygon(star(p(0.55, -0.5), s * 0.3), colour, egui::Stroke::NONE));
        }
        Part::Sounds => {
            // A speaker with sound waves.
            painter.add(egui::Shape::convex_polygon(
                vec![p(-0.8, -0.2), p(-0.5, -0.2), p(-0.15, -0.55), p(-0.15, 0.55), p(-0.5, 0.2), p(-0.8, 0.2)],
                colour,
                egui::Stroke::NONE,
            ));
            for r in [0.35_f32, 0.65] {
                let points: Vec<egui::Pos2> = (0..=8)
                    .map(|i| {
                        let a = -0.8 + 1.6 * i as f32 / 8.0;
                        p(-0.05 + r * a.cos(), r * a.sin())
                    })
                    .collect();
                painter.add(egui::Shape::line(points, stroke));
            }
        }
        Part::Model => {
            // A stick figure standing.
            painter.circle_stroke(p(0.0, -0.55), s * 0.22, stroke);
            painter.line_segment([p(0.0, -0.33), p(0.0, 0.25)], stroke);
            painter.line_segment([p(-0.5, -0.1), p(0.5, -0.1)], stroke);
            painter.line_segment([p(0.0, 0.25), p(-0.35, 0.8)], stroke);
            painter.line_segment([p(0.0, 0.25), p(0.35, 0.8)], stroke);
        }
        Part::Animations => {
            // A stick figure running, with motion lines.
            painter.circle_stroke(p(0.2, -0.6), s * 0.2, stroke);
            painter.line_segment([p(0.12, -0.4), p(-0.05, 0.2)], stroke);
            painter.line_segment([p(0.05, -0.15), p(0.5, 0.0)], stroke);
            painter.line_segment([p(0.05, -0.15), p(-0.3, -0.3)], stroke);
            painter.line_segment([p(-0.05, 0.2), p(0.35, 0.75)], stroke);
            painter.line_segment([p(-0.05, 0.2), p(-0.45, 0.6)], stroke);
            painter.line_segment([p(-0.85, -0.2), p(-0.5, -0.2)], stroke);
            painter.line_segment([p(-0.9, 0.1), p(-0.55, 0.1)], stroke);
        }
        Part::Gimmicks => {
            // A meter: a bar part filled, with a spark over it.
            let bar = egui::Rect::from_min_max(p(-0.8, 0.2), p(0.8, 0.65));
            painter.rect_stroke(bar, s * 0.12, stroke, egui::StrokeKind::Middle);
            painter.rect_filled(egui::Rect::from_min_max(p(-0.8, 0.2), p(0.15, 0.65)), s * 0.12, colour);
            painter.add(egui::Shape::convex_polygon(
                vec![p(0.1, -0.85), p(-0.35, -0.2), p(0.0, -0.2), p(-0.1, 0.05), p(0.4, -0.5), p(0.05, -0.5)],
                colour,
                egui::Stroke::NONE,
            ));
        }
        Part::Values => {
            // Three sliders.
            for (row, knob) in [(-0.45_f32, 0.3_f32), (0.0, -0.35), (0.45, 0.1)] {
                painter.line_segment([p(-0.75, row), p(0.75, row)], stroke);
                painter.circle_filled(p(knob, row), s * 0.16, colour);
            }
        }
        Part::SelectScreen => {
            // A portrait frame.
            let frame = egui::Rect::from_min_max(p(-0.65, -0.75), p(0.65, 0.75));
            painter.rect_stroke(frame, s * 0.1, stroke, egui::StrokeKind::Middle);
            painter.circle_filled(p(0.0, -0.18), s * 0.22, colour);
            painter.add(egui::Shape::convex_polygon(
                vec![p(-0.42, 0.62), p(-0.3, 0.2), p(0.3, 0.2), p(0.42, 0.62)],
                colour,
                egui::Stroke::NONE,
            ));
        }
    }
}

/// A square part button: solid in the part's colour when the project changes it, dark and
/// outlined while it is the game's. A ring when open; a dot when its files are in a folder of
/// the author's.
fn part_button(ui: &mut Ui, look: Look, part: Part, changed: bool, picked_folder: bool, open: bool, hover: &str) -> egui::Response {
    let size = egui::vec2(46.0, 46.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let colour = part_colour(part);
    let painter = ui.painter();
    let (fill, icon) = if changed { (colour, Color32::from_rgb(18, 18, 22)) } else { (look.sunk, look.dim) };
    painter.rect_filled(rect, 9.0, fill);
    if !changed {
        painter.rect_stroke(rect, 9.0, egui::Stroke::new(1.5, look.edge), egui::StrokeKind::Inside);
    }
    if open || response.hovered() {
        painter.rect_stroke(rect.expand(3.0), 11.0, egui::Stroke::new(2.0, if open { look.text } else { colour }), egui::StrokeKind::Inside);
    }
    paint_part_icon(painter, rect.shrink(10.0), part, icon);
    if picked_folder {
        let at = rect.right_top() + egui::vec2(-2.0, 2.0);
        painter.circle_filled(at, 6.0, look.card);
        painter.circle_filled(at, 4.5, look.text);
    }
    response.on_hover_text(hover)
}

/// The groups effects of no fighter are shown in, in order.
const EFFECT_GROUPS: [&str; 4] = ["Shared", "Items", "Stages", "Other"];

/// Which group an effect file belongs in, from its path in the game.
fn effect_group(rel: &str) -> &'static str {
    let rel = rel.replace('\\', "/").to_ascii_lowercase();
    let rel = rel.trim_start_matches("effect/");
    if rel.starts_with("system") || rel.starts_with("common") {
        "Shared"
    } else if rel.starts_with("item") {
        "Items"
    } else if rel.starts_with("stage") {
        "Stages"
    } else {
        "Other"
    }
}

fn section_title(ui: &mut Ui, look: Look, title: &str) {
    ui.label(RichText::new(title).size(21.0).strong().color(look.text));
}

#[derive(Clone, Copy)]
enum Tile {
    Part(Part),
    Mods,
}

/// A wide tile: icon, title and one line on what the project changes there.
fn game_tile(ui: &mut Ui, look: Look, icon: Tile, title: &str, line: &str, changed: bool, open: bool) -> egui::Response {
    let colour = match icon {
        Tile::Part(part) => part_colour(part),
        Tile::Mods => Color32::from_rgb(190, 150, 255),
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(300.0, 64.0), egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 12.0, look.card);
    let edge = if open { look.text } else if response.hovered() { colour } else { look.edge };
    painter.rect_stroke(rect, 12.0, egui::Stroke::new(2.0, edge), egui::StrokeKind::Inside);
    let badge = egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 10.0), egui::vec2(44.0, 44.0));
    let (fill, ink) = if changed { (colour, Color32::from_rgb(18, 18, 22)) } else { (look.sunk, colour) };
    painter.rect_filled(badge, 9.0, fill);
    match icon {
        Tile::Part(part) => paint_part_icon(painter, badge.shrink(10.0), part, ink),
        Tile::Mods => {
            // Three stacked boxes.
            let stroke = egui::Stroke::new(2.0, ink);
            for i in 0..3 {
                let y = badge.top() + 12.0 + i as f32 * 7.0;
                painter.rect_stroke(egui::Rect::from_min_size(egui::pos2(badge.left() + 11.0, y), egui::vec2(22.0, 5.0)), 1.5, stroke, egui::StrokeKind::Middle);
            }
        }
    }
    let x = badge.right() + 12.0;
    // The title sits alone until the project changes something here.
    if line.is_empty() {
        painter.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(18.0), look.text);
    } else {
        painter.text(egui::pos2(x, rect.top() + 12.0), egui::Align2::LEFT_TOP, title, egui::FontId::proportional(18.0), look.text);
        painter.text(egui::pos2(x, rect.top() + 36.0), egui::Align2::LEFT_TOP, line, egui::FontId::proportional(14.0), look.dim);
    }
    response
}

/// Where a part comes from, short, with its colour.
fn source_chip(from: &str) -> (String, Color32) {
    if from == "this project" {
        ("Project".into(), Color32::from_rgb(80, 220, 120))
    } else if let Some(name) = from.strip_prefix("your folder ") {
        (format!("Folder: {name}"), Color32::from_rgb(90, 175, 255))
    } else if let Some(name) = from.strip_prefix("your move code in ") {
        (format!("Code: {name}"), Color32::from_rgb(90, 175, 255))
    } else if let Some(name) = from.strip_prefix("the mod ") {
        (format!("Mod: {name}"), Color32::from_rgb(190, 150, 255))
    } else {
        ("Game".into(), Color32::from_rgb(150, 154, 164))
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

impl VisionaryApp {
    pub(super) fn workspace(&self) -> Option<PathBuf> {
        self.project_path.as_ref().and_then(|path| path.parent()).map(Path::to_path_buf)
    }

    /// The open project's characters and folders, read from beside the project when it changes.
    pub(super) fn project_parts(&mut self) -> ProjectParts {
        let workspace = self.workspace();
        if self.project_parts_cache.as_ref().map(|(at, _)| at) != workspace.as_ref() {
            let parts = workspace.as_deref().map(ProjectParts::load).unwrap_or_default();
            // A project that names its own move code links it on opening.
            if let Some(folder) = parts.link("", Part::Moves, 0) {
                if self.acmd_src.as_ref().map(|index| index.root.as_path()) != Some(folder) {
                    self.link_acmd_source(folder.to_path_buf());
                }
            }
            self.project_parts_cache = workspace.map(|at| (at, parts));
        }
        self.project_parts_cache.as_ref().map(|(_, parts)| parts.clone()).unwrap_or_default()
    }

    fn change_parts(&mut self, change: impl FnOnce(&mut ProjectParts)) {
        let Some(workspace) = self.workspace() else {
            self.state.status = "Save the project first.".into();
            return;
        };
        let mut parts = self.project_parts();
        change(&mut parts);
        if let Err(error) = parts.save(&workspace) {
            self.state.status = format!("Could not save parts.json: {error:#}");
        }
        self.project_parts_cache = Some((workspace, parts));
        // Folders feed the shown skin, so look at it again.
        self.preview_skin = None;
    }

    /// Every character the project touches: picked onto the page, edited anywhere, given a
    /// folder, or added as a new character.
    fn project_cards(&mut self) -> Vec<Card> {
        let parts = self.project_parts();
        let mut names: BTreeSet<String> = parts.characters.keys().cloned().collect();
        names.extend(self.state.edit_log.entries.iter().filter(|(_, moves)| !moves.is_empty()).map(|(name, _)| name.clone()));
        names.extend(self.move_graphs.iter().filter(|(_, graphs)| !graphs.is_empty()).map(|(name, _)| name.clone()));
        names.extend(self.eff_mods.iter().filter(|(_, eff)| !eff.is_empty()).map(|(name, _)| name.clone()));
        names.extend(self.roster.params.iter().filter(|(_, params)| !params.is_empty()).map(|(name, _)| name.clone()));
        names.extend(self.state.sound_script_edits.keys().filter_map(|key| key.split_once('/')).map(|(name, _)| name.to_owned()));
        names.extend(parts.links.iter().filter(|link| link.part != Part::Moves).map(|link| link.fighter.clone()));
        let mut cards: Vec<Card> = Vec::new();
        for name in names {
            // One card for all the fighters of a character, showing the one picked on it.
            let group = group_of(&name);
            let name = match group {
                Some((title, members)) => {
                    if cards.iter().any(|card| card.name == title) {
                        continue;
                    }
                    let picked = self.project_member.get(title).cloned();
                    picked.filter(|picked| members.iter().any(|(member, _)| member == picked)).unwrap_or(name)
                }
                None => name,
            };
            let Some(entry) = self.state.fighters.iter().find(|entry| entry.name.eq_ignore_ascii_case(&name)) else {
                continue;
            };
            let slot = parts.characters.get(&entry.name).copied().filter(|slot| entry.slots.contains(slot)).unwrap_or(entry.base_slot());
            cards.push(Card {
                fighter: entry.name.clone(),
                slots: entry.slots.clone(),
                slot,
                // "Ice Climbers · Popo" is the Ice Climbers on a card of their own.
                name: group.map_or_else(|| entry.display_name.split(" · ").next().unwrap_or(&entry.display_name).to_owned(), |(title, _)| title.to_owned()),
                base: if entry.is_modded() { "Mod fighter".into() } else { "Base game".into() },
                roster_key: crate::roster::RosterKey::fighter(&entry.name),
                made_here: false,
                members: group.map_or(&[], |(_, members)| members),
            });
        }
        for authored in &self.roster.project.authored {
            let donor = self.state.fighters.iter().find(|entry| entry.name.eq_ignore_ascii_case(&authored.donor));
            let slots = authored.all_slots();
            cards.push(Card {
                fighter: authored.donor.clone(),
                slot: slots.first().copied().unwrap_or(authored.slot),
                slots,
                name: authored.display_name.clone(),
                base: format!("Plays as {}", donor.map_or(authored.donor.clone(), |entry| entry.display_name.clone())),
                roster_key: authored.key.clone(),
                made_here: true,
                members: &[],
            });
        }
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        cards
    }

    /// Whether the project changes `part` of a card, where it comes from, and a line of detail.
    fn part_summary(&self, parts: &ProjectParts, card: &Card, part: Part) -> (bool, String, String) {
        let fighter = card.fighter.to_ascii_lowercase();
        let picked = parts.link(&fighter, part, card.slot);
        let from_folder = |default: &str| picked.map_or(default.to_owned(), |folder| format!("your folder {}", folder_name(folder)));
        match part {
            Part::Moves => {
                let moves = self.state.edit_log.entries.get(&fighter).map_or(0, HashMap::len);
                let graphs = self.move_graphs.get(&fighter).map_or(0, BTreeMap::len);
                let code = self.acmd_src.as_ref().map(|index| index.root.clone());
                let from = match &code {
                    Some(root) => format!("your move code in {}", folder_name(root)),
                    None if moves + graphs > 0 => "this project".into(),
                    None => "the game".into(),
                };
                (moves + graphs > 0, from, format!("{moves} moves edited, {graphs} with nodes"))
            }
            Part::Effects => {
                let edits = self.eff_mods.get(&fighter).filter(|eff| !eff.is_empty());
                let file = self.project_effect_file(&fighter);
                let detail = edits.map_or(String::new(), |eff| {
                    format!("{} effects edited, {} borrowed", eff.authored.len(), eff.transplants.len())
                });
                let from = match (&file, picked) {
                    (Some(_), Some(_)) => from_folder(""),
                    (Some(_), None) => "this project".into(),
                    (None, _) if edits.is_some() => "this project".into(),
                    (None, _) => "the game".into(),
                };
                (edits.is_some() || file.is_some(), from, detail)
            }
            Part::Sounds => {
                let scripts = self.state.sound_script_edits.keys().filter(|key| key.split_once('/').is_some_and(|(name, _)| name == fighter)).count();
                let files = self.workspace().and_then(|workspace| parts.dir(&workspace, &fighter, part, card.slot)).map_or(0, |dir| count_files(&dir));
                let from = if picked.is_some() { from_folder("") } else if scripts + files > 0 { "this project".into() } else { "the game".into() };
                (scripts + files > 0, from, format!("{scripts} moves edited, {files} files"))
            }
            Part::Model | Part::Animations => {
                let words = if part == Part::Model { "model" } else { "motion" };
                let found = self.skin_folder(&card.fighter, words, card.slot);
                let from = found.map_or("the game".to_owned(), |(_, from)| from);
                (from != "the game", from, String::new())
            }
            Part::Gimmicks => {
                let roster = &self.roster;
                // Editing a move with nodes saves an empty first gimmick; only ones that do
                // something count.
                let acts = |graph: &crate::gimmicks::behavior::BehaviorGraph| graph.transitions.iter().any(|step| !step.actions.is_empty());
                let main = roster.behaviors.get(&fighter).is_some_and(acts) as usize;
                let charts = roster.controller_charts.get(&fighter).map_or(0, |charts| charts.values().filter(|chart| acts(&chart.graph)).count());
                let meters = roster.meters.get(&fighter).map_or(0, Vec::len);
                let gimmicks = main + charts;
                let changed = gimmicks + meters > 0;
                let detail = if changed { format!("{gimmicks} gimmicks, {meters} meters") } else { String::new() };
                (changed, if changed { "this project".into() } else { "the game".into() }, detail)
            }
            Part::Values => {
                let values = self.roster.params.get(&fighter).map_or(0, |params| params.field_count());
                (values > 0, if values > 0 { "this project".into() } else { "the game".into() }, format!("{values} values changed"))
            }
            Part::SelectScreen => {
                let project = &self.roster.project;
                let changed = project.names.contains_key(&card.roster_key)
                    || project.ui_images.contains_key(&card.roster_key)
                    || project.authored.iter().any(|entry| entry.key == card.roster_key);
                (changed, if changed { "this project".into() } else { "the game".into() }, String::new())
            }
        }
    }

    /// The fighter's effect file in the project or a folder picked for its effects.
    fn project_effect_file(&self, fighter: &str) -> Option<PathBuf> {
        let workspace = self.workspace()?;
        let parts = self.project_parts_cache.as_ref().map(|(_, parts)| parts)?;
        let file = parts.dir(&workspace, fighter, Part::Effects, 0)?.join(format!("ef_{fighter}.eff"));
        file.is_file().then_some(file)
    }

    /// Project > Characters.
    pub(super) fn draw_project_page(&mut self, ui: &mut Ui) {
        let look = look(ui);
        ui.painter().rect_filled(ui.max_rect(), 0.0, look.page);
        let workspace = self.workspace();
        let mut actions = Vec::new();
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let title = if self.project_name.is_empty() { "Untitled project" } else { &self.project_name };
            ui.label(RichText::new(title).size(30.0).strong().color(look.text));
            ui.add_space(12.0);
            if let Some(dir) = &workspace {
                if act(ui, look, "Open folder", None).on_hover_text(dir.display().to_string()).clicked() {
                    actions.push(PageAction::Reveal(dir.clone()));
                }
            }
            if act(ui, look, "Projects", None).clicked() {
                self.project_hub.show = true;
            }
            if act(ui, look, "Export mod", Some(look.accent)).clicked() {
                self.export_full_mod();
            }
        });
        if workspace.is_none() {
            ui.label(RichText::new("Not saved yet").color(look.dim));
        }
        ui.add_space(10.0);
        let cards = self.project_cards();
        let parts = self.project_parts();
        let width = ui.available_width();
        let columns = ((width / CARD_WIDTH) as usize).clamp(1, 3);
        let card_width = (width / columns as f32 - 10.0).min(CARD_WIDTH * 1.25);

        section_title(ui, look, "Across the game");
        self.draw_game_tiles(ui, look, &parts, &mut actions);
        if self.project_open_part.as_ref().is_some_and(|(key, _)| key == OTHER_EFFECTS) {
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.set_width((card_width * columns as f32).min(width));
                self.draw_other_effects(ui, look, &parts, &mut actions);
            });
        }
        ui.add_space(18.0);

        ui.horizontal(|ui| {
            section_title(ui, look, "Characters");
            ui.add_space(12.0);
            let shown: BTreeSet<&str> = cards.iter().map(|card| card.fighter.as_str()).collect();
            egui::ComboBox::from_id_salt("project_add_character")
                .selected_text(RichText::new("Add character").strong().color(look.text))
                .width(200.0)
                .height(420.0)
                .show_ui(ui, |ui| {
                    let mut fighters: Vec<_> = self.state.fighters.iter().filter(|entry| !shown.contains(entry.name.as_str())).collect();
                    fighters.sort_by(|a, b| a.display_name.cmp(&b.display_name));
                    for entry in fighters {
                        if ui.selectable_label(false, &entry.display_name).clicked() {
                            actions.push(PageAction::Add(entry.name.clone()));
                        }
                    }
                });
            if act(ui, look, "New character", None).on_hover_text("Make a fighter of your own on top of a base one").clicked() {
                self.roster.focus_new_character();
            }
        });
        ui.add_space(6.0);
        for row in cards.chunks(columns) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for card in row {
                    ui.vertical(|ui| {
                        ui.set_width(card_width);
                        self.draw_character_card(ui, look, card, &parts, &mut actions);
                    });
                }
            });
            ui.add_space(10.0);
        }

        let dropped: Vec<PathBuf> = ui.ctx().input(|input| input.raw.dropped_files.iter().map(|file| file.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            let target = self
                .project_open_part
                .as_ref()
                .and_then(|(fighter, _)| cards.iter().find(|card| &card.fighter == fighter))
                .or(cards.first());
            match (&workspace, target) {
                (Some(workspace), Some(card)) => {
                    let (fighter, slot) = (card.fighter.clone(), card.slot);
                    self.import_dropped_files(workspace, &fighter, slot, dropped);
                }
                (None, _) => self.state.status = "Save the project before adding files.".into(),
                (_, None) => self.state.status = "Add a character first.".into(),
            }
        }
        for action in actions {
            self.run_page_action(action);
        }
    }

    fn card_frame(look: Look, selected: bool) -> egui::Frame {
        egui::Frame::new()
            .fill(look.card)
            .corner_radius(14.0)
            .inner_margin(egui::Margin::same(14))
            .stroke(egui::Stroke::new(2.0, if selected { look.accent } else { look.edge }))
    }

    fn draw_character_card(&mut self, ui: &mut Ui, look: Look, card: &Card, parts: &ProjectParts, actions: &mut Vec<PageAction>) {
        let selected = self.state.selected_fighter.and_then(|index| self.state.fighters.get(index)).is_some_and(|entry| entry.name == card.fighter);
        let open = self.project_open_part.as_ref().filter(|(fighter, _)| *fighter == card.fighter).map(|(_, part)| *part);
        Self::card_frame(look, selected).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let portrait = self.roster.portrait(ui.ctx(), &card.roster_key, &card.fighter, card.slot);
                let size = egui::vec2(112.0, 112.0);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                ui.painter().rect_filled(rect, 10.0, look.sunk);
                match portrait {
                    Some(texture) => {
                        egui::Image::new(&texture).corner_radius(10.0).paint_at(ui, rect);
                    }
                    None => paint_part_icon(ui.painter(), rect.shrink(32.0), Part::Model, look.dim),
                }
                ui.painter().rect_stroke(rect, 10.0, egui::Stroke::new(1.5, if response.hovered() { look.accent } else { look.edge }), egui::StrokeKind::Inside);
                if response.on_hover_text("Show in the editor").clicked() {
                    actions.push(PageAction::Select(card.fighter.clone(), card.slot));
                }
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&card.name).size(28.0).strong().color(look.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if card.made_here {
                                if act(ui, look, "Remove", None).on_hover_text("Take this character out of the project. Its files stay in its folder.").clicked() {
                                    actions.push(PageAction::Made(crate::roster::new_character::NewCharacterAction::Remove(card.roster_key.clone())));
                                }
                            } else if act(ui, look, "Hide", None).clicked() {
                                actions.push(PageAction::Hide(card.fighter.clone()));
                            }
                            egui::ComboBox::from_id_salt(("card_slot", &card.fighter, &card.name))
                                .selected_text(RichText::new(format!("c{:02}", card.slot)).strong().color(look.text))
                                .width(64.0)
                                .show_ui(ui, |ui| {
                                    for slot in &card.slots {
                                        if ui.selectable_label(*slot == card.slot, format!("c{slot:02}")).clicked() {
                                            actions.push(PageAction::PickSlot(card.fighter.clone(), *slot));
                                        }
                                    }
                                });
                        });
                    });
                    if card.members.is_empty() {
                        ui.label(RichText::new(&card.base).size(15.0).color(look.dim));
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            for (member, label) in card.members {
                                if self.fighter_index(member).is_none() {
                                    continue;
                                }
                                let shown = member.eq_ignore_ascii_case(&card.fighter);
                                if act(ui, look, label, shown.then_some(look.accent)).clicked() && !shown {
                                    actions.push(PageAction::Member(card.name.clone(), (*member).to_owned()));
                                }
                            }
                        });
                    }
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for part in Part::ALL {
                            let (changed, from, detail) = self.part_summary(parts, card, part);
                            let picked = parts.link(&card.fighter, part, card.slot).is_some();
                            let hover = if detail.is_empty() { format!("{}: {}", part.title(), source_chip(&from).0) } else { format!("{}: {}, {}", part.title(), source_chip(&from).0, detail) };
                            if part_button(ui, look, part, changed, picked, open == Some(part), &hover).clicked() {
                                actions.push(PageAction::Open(card.fighter.clone(), part));
                            }
                        }
                    });
                });
            });
            if let Some(part) = open {
                ui.add_space(10.0);
                self.draw_part_detail(ui, look, card, part, parts, actions);
            }
        });
    }

    fn draw_part_detail(&mut self, ui: &mut Ui, look: Look, card: &Card, part: Part, parts: &ProjectParts, actions: &mut Vec<PageAction>) {
        let (_, from, detail) = self.part_summary(parts, card, part);
        let fighter = &card.fighter;
        let slot = card.slot;
        let colour = part_colour(part);
        egui::Frame::new()
            .fill(look.sunk)
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.5, colour))
            .show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
            paint_part_icon(ui.painter(), rect, part, colour);
            ui.label(RichText::new(part.title()).size(19.0).strong().color(look.text));
            // A picked folder is named in full just below.
            if parts.link(fighter, part, slot).is_none() {
                let (label, tint) = source_chip(&from);
                chip(ui, &label, tint);
            }
            if !detail.is_empty() {
                ui.label(RichText::new(&detail).color(look.dim));
            }
        });
        if part.has_folder() {
            let workspace = self.workspace();
            let picked = parts.link(fighter, part, slot).map(Path::to_path_buf);
            let folder = if part == Part::Moves {
                self.acmd_src.as_ref().map(|index| index.root.clone())
            } else {
                workspace.as_deref().and_then(|workspace| parts.dir(workspace, fighter, part, slot))
            };
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Folder").strong().color(look.dim));
                let shown = match &folder {
                    Some(dir) => dir.display().to_string(),
                    None if part == Part::Moves => "Game scripts".into(),
                    None => "Save the project first".into(),
                };
                ui.label(RichText::new(shown).monospace().color(look.text));
            });
            ui.horizontal_wrapped(|ui| {
                let pick = if part == Part::Moves { "Link move code" } else { "Choose folder" };
                if act(ui, look, pick, None).clicked() {
                    actions.push(PageAction::PickFolder(fighter.clone(), part, slot));
                }
                if let Some(dir) = folder.clone() {
                    if act(ui, look, "Open", None).clicked() {
                        actions.push(PageAction::Reveal(dir));
                    }
                }
                if picked.is_some() || (part == Part::Moves && folder.is_some()) {
                    let words = if part == Part::Moves { "Unlink" } else { "Use project folder" };
                    if act(ui, look, words, None).clicked() {
                        actions.push(PageAction::ForgetFolder(fighter.clone(), part, slot));
                    }
                }
            });
        }
        ui.add_space(6.0);
        let files = |what| PageAction::Files(fighter.clone(), slot, what);
        let main = Some(colour);
        ui.horizontal_wrapped(|ui| match part {
            Part::Moves => {
                if act(ui, look, "Edit moves", main).clicked() {
                    actions.push(PageAction::EditMoves(fighter.clone(), false));
                }
                if act(ui, look, "Edit with nodes", None).clicked() {
                    actions.push(PageAction::EditMoves(fighter.clone(), true));
                }
                if card.made_here {
                    let alone = self.roster.edit_target() == Some(&card.roster_key);
                    let base = crate::data::fighter_display_name(fighter);
                    let (words, tip) = if alone {
                        ("Edits change only this character", format!("Click to make move edits change every {base} again."))
                    } else {
                        ("Edits change every costume", format!("Move edits change every {base}. Click to make them change only this character."))
                    };
                    if act(ui, look, words, alone.then_some(colour)).on_hover_text(tip).clicked() {
                        let target = (!alone).then(|| card.roster_key.clone());
                        actions.push(PageAction::Made(crate::roster::new_character::NewCharacterAction::SetEditTarget(target)));
                    }
                }
            }
            Part::Effects => {
                if act(ui, look, "Edit effects", main).clicked() {
                    actions.push(PageAction::EditEffects(fighter.clone()));
                }
                if act(ui, look, "Add .eff", None).clicked() {
                    actions.push(files(FileAction::AddEffects));
                }
            }
            Part::Sounds => {
                if act(ui, look, "Edit move sounds", main).clicked() {
                    actions.push(PageAction::EditMoves(fighter.clone(), false));
                }
                if act(ui, look, "Add sound files", None).clicked() {
                    actions.push(files(FileAction::AddSounds));
                }
            }
            Part::Model => {
                let busy = self.workspace_copy_rx.is_some() || self.state.data_root.is_none();
                if ui.add_enabled_ui(!busy, |ui| act(ui, look, "Copy from game", main)).inner.on_hover_text("Model and textures; files already here are kept").clicked() {
                    actions.push(files(FileAction::CopyGameModel));
                }
                if ui.add_enabled_ui(!busy, |ui| act(ui, look, "Copy model + animations", None)).inner.clicked() {
                    actions.push(files(FileAction::CopyGameCharacter));
                }
                if act(ui, look, "Add folder", None).clicked() {
                    actions.push(files(FileAction::AddModelFolder));
                }
                if act(ui, look, "Add files", None).clicked() {
                    actions.push(files(FileAction::AddModelFiles));
                }
            }
            Part::Animations => {
                if act(ui, look, "Add animations", main).clicked() {
                    actions.push(files(FileAction::AddAnimations));
                }
                if act(ui, look, "Add swing.prc", None).clicked() {
                    actions.push(files(FileAction::AddSwing));
                }
            }
            Part::Gimmicks => {
                if act(ui, look, "Edit gimmicks", main).clicked() {
                    actions.push(PageAction::EditGimmicks(fighter.clone()));
                }
            }
            Part::Values => {
                if act(ui, look, "Edit values", main).clicked() {
                    actions.push(PageAction::EditValues(fighter.clone()));
                }
            }
            Part::SelectScreen => {
                if act(ui, look, "Edit name and portraits", main).clicked() {
                    actions.push(PageAction::EditSelectScreen(card.roster_key.clone()));
                }
            }
        });
        if matches!(part, Part::Model | Part::Animations) {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if act(ui, look, "Show in game", None).on_hover_text("The fighter must not hold an item while it loads").clicked() {
                    actions.push(files(FileAction::Send));
                }
                if act(ui, look, "Stop", None).clicked() {
                    actions.push(files(FileAction::Stop));
                }
                let status = self.game_link.asset_bundle_status();
                if status.target == fighter.to_ascii_lowercase() && status.generation == self.workspace_asset_generation && self.workspace_asset_sent_slot == Some(slot) {
                    let state = if status.ready { "In game" } else if status.phase == "ready" { "Waiting for the game" } else { status.phase.as_str() };
                    chip(ui, &format!("{state} {}/{}", status.served, status.staged), look.accent);
                }
            });
        }
            });
    }

    /// Effects that belong to no fighter: common, item and stage effects.
    /// Effect files of no fighter the project edits, by where they live in the game.
    fn other_effect_edits(&self) -> Vec<(&'static str, String)> {
        let fighters: BTreeSet<&str> = self.state.fighters.iter().map(|entry| entry.name.as_str()).collect();
        let mut edited: Vec<(&'static str, String)> = self
            .eff_mods
            .iter()
            .filter(|(key, eff)| !fighters.contains(key.as_str()) && !eff.is_empty())
            .map(|(_, eff)| (effect_group(&eff.source_rel), eff.source_rel.clone()))
            .collect();
        edited.sort();
        edited
    }

    /// One tile per game-wide part of the project: effects of no fighter, the select screen,
    /// fighter values and the installed mods, each saying what the project changes there.
    fn draw_game_tiles(&mut self, ui: &mut Ui, look: Look, parts: &ProjectParts, actions: &mut Vec<PageAction>) {
        let edited = self.other_effect_edits();
        let picked = parts.link(OTHER_EFFECTS, Part::Effects, 0).is_some();
        let effects = {
            let mut groups: Vec<String> = Vec::new();
            for group in EFFECT_GROUPS {
                let n = edited.iter().filter(|(g, _)| *g == group).count();
                if n > 0 {
                    groups.push(format!("{group} {n}"));
                }
            }
            match (groups.is_empty(), picked) {
                (true, false) => String::new(),
                (true, true) => "From your folder".to_owned(),
                (false, _) => groups.join(", "),
            }
        };
        let summary = self.roster.export_summary();
        let screen = match (summary.order, summary.hidden, summary.names) {
            (0, 0, 0) => String::new(),
            (order, hidden, names) => [(order, "moved"), (hidden, "hidden"), (names, "renamed")]
                .iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, what)| format!("{n} {what}"))
                .collect::<Vec<_>>()
                .join(", "),
        };
        let values = match summary.params {
            0 => String::new(),
            1 => "1 value changed".to_owned(),
            n => format!("{n} values changed"),
        };
        let mods = match self.roster.enabled_roots().len() {
            0 => String::new(),
            1 => "1 turned on".to_owned(),
            n => format!("{n} turned on"),
        };
        let open = self.project_open_part.as_ref().is_some_and(|(key, _)| key == OTHER_EFFECTS);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
            let tiles: [(Tile, &str, &str, bool); 4] = [
                (Tile::Part(Part::Effects), "Effects", &effects, !edited.is_empty() || picked),
                (Tile::Part(Part::SelectScreen), "Select screen", &screen, summary.order + summary.hidden + summary.names > 0),
                (Tile::Part(Part::Values), "Fighter values", &values, summary.params > 0),
                (Tile::Mods, "Installed mods", &mods, false),
            ];
            for (index, (icon, title, line, changed)) in tiles.into_iter().enumerate() {
                if game_tile(ui, look, icon, title, line, changed, index == 0 && open).clicked() {
                    match index {
                        0 => actions.push(PageAction::Open(OTHER_EFFECTS.into(), Part::Effects)),
                        1 => self.roster.focus_character_select(),
                        2 => self.roster.focus_traits(),
                        _ => self.roster.focus_library(),
                    }
                }
            }
        });
    }

    fn draw_other_effects(&mut self, ui: &mut Ui, look: Look, parts: &ProjectParts, actions: &mut Vec<PageAction>) {
        let edited = self.other_effect_edits();
        let picked = parts.link(OTHER_EFFECTS, Part::Effects, 0).map(Path::to_path_buf);
        let colour = part_colour(Part::Effects);
        egui::Frame::new()
            .fill(look.card)
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.5, colour))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Effects of no fighter").size(19.0).strong().color(look.text));
                ui.add_space(6.0);
                for group in EFFECT_GROUPS {
                    let files: Vec<&String> = edited.iter().filter(|(g, _)| *g == group).map(|(_, rel)| rel).collect();
                    if files.is_empty() {
                        continue;
                    }
                    ui.label(RichText::new(group).strong().color(look.text));
                    for rel in files {
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.label(RichText::new(rel).monospace().color(look.text));
                            if act(ui, look, "Edit", None).clicked() {
                                actions.push(PageAction::EditOtherEffect(rel.clone()));
                            }
                        });
                    }
                    ui.add_space(4.0);
                }
                if let Some(dir) = &picked {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Folder").strong().color(look.dim));
                        ui.label(RichText::new(dir.display().to_string()).monospace().color(look.text));
                    });
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if act(ui, look, "Open .eff", Some(colour)).clicked() {
                        actions.push(PageAction::OpenEffectFile);
                    }
                    if act(ui, look, "Choose folder", None).on_hover_text("Laid out like the game's effect folder").clicked() {
                        actions.push(PageAction::PickFolder(OTHER_EFFECTS.into(), Part::Effects, 0));
                    }
                    if picked.is_some() && act(ui, look, "Use game files", None).clicked() {
                        actions.push(PageAction::ForgetFolder(OTHER_EFFECTS.into(), Part::Effects, 0));
                    }
                });
            });
    }

    fn fighter_index(&self, fighter: &str) -> Option<usize> {
        self.state.fighters.iter().position(|entry| entry.name.eq_ignore_ascii_case(fighter))
    }

    /// Pick the card's fighter in the editor, showing the costume picked on its card.
    fn focus_card(&mut self, fighter: &str, slot: Option<u8>) {
        let Some(index) = self.fighter_index(fighter) else { return };
        if self.state.selected_fighter != Some(index) {
            self.select_fighter(index);
        }
        if let Some(slot) = slot.or_else(|| self.project_parts().characters.get(fighter).copied()) {
            self.workspace_asset_slot = slot;
            self.show_skin(fighter, slot);
        }
    }

    fn run_page_action(&mut self, action: PageAction) {
        match action {
            PageAction::Select(fighter, slot) => self.focus_card(&fighter, Some(slot)),
            PageAction::Open(fighter, part) => {
                let again = self.project_open_part.as_ref() == Some(&(fighter.clone(), part));
                self.project_open_part = (!again).then_some((fighter, part));
            }
            PageAction::PickSlot(fighter, slot) => {
                self.change_parts(|parts| {
                    parts.characters.insert(fighter.clone(), slot);
                });
                if self.fighter_index(&fighter) == self.state.selected_fighter {
                    self.workspace_asset_slot = slot;
                    self.show_skin(&fighter, slot);
                }
            }
            PageAction::Hide(fighter) => {
                let members: Vec<&str> = group_of(&fighter).map_or(vec![fighter.as_str()], |(_, members)| members.iter().map(|(member, _)| *member).collect());
                self.change_parts(|parts| {
                    for member in &members {
                        parts.characters.remove(*member);
                    }
                });
            }
            PageAction::Member(card, fighter) => {
                if let Some((title, _)) = GROUPS.iter().find(|(title, _)| *title == card) {
                    self.project_member.insert(title, fighter.clone());
                }
                // The card stays on the page whichever of its fighters is shown.
                let slot = self.fighter_index(&fighter).map_or(0, |index| self.state.fighters[index].base_slot());
                self.change_parts(|parts| {
                    parts.characters.entry(fighter.clone()).or_insert(slot);
                });
                self.project_open_part = None;
            }
            PageAction::Add(fighter) => {
                let slot = self.fighter_index(&fighter).map_or(0, |index| self.state.fighters[index].base_slot());
                self.change_parts(|parts| {
                    parts.characters.insert(fighter.clone(), slot);
                });
                self.project_open_part = Some((fighter, Part::Moves));
            }
            PageAction::PickFolder(fighter, part, slot) => {
                let Some(folder) = rfd::FileDialog::new().set_title(format!("Folder for {}", part.title().to_lowercase())).pick_folder() else {
                    return;
                };
                if part == Part::Moves {
                    self.link_acmd_source(folder.clone());
                    if self.acmd_src.as_ref().map(|index| index.root.as_path()) != Some(folder.as_path()) {
                        return;
                    }
                }
                self.change_parts(|parts| parts.set_link(&fighter, part, slot, Some(folder)));
                if part == Part::Effects {
                    self.reopen_fighter_effects(&fighter);
                }
            }
            PageAction::ForgetFolder(fighter, part, slot) => {
                if part == Part::Moves {
                    self.unlink_acmd_source();
                }
                self.change_parts(|parts| parts.set_link(&fighter, part, slot, None));
                if part == Part::Effects {
                    self.reopen_fighter_effects(&fighter);
                }
            }
            PageAction::Reveal(dir) => {
                let _ = std::fs::create_dir_all(&dir);
                if let Err(error) = crate::roster::reveal::reveal(&dir) {
                    self.state.status = format!("Could not open {}: {error:#}", dir.display());
                }
            }
            PageAction::EditMoves(fighter, nodes) => {
                self.focus_card(&fighter, None);
                self.roster.open = false;
                self.eff_editor.open = false;
                self.node_workspace = nodes;
                self.primary_tab = if nodes { PrimaryEditorTab::Nodes } else { PrimaryEditorTab::Collisions };
                if nodes {
                    self.node_editor.refresh_character();
                }
            }
            PageAction::EditEffects(fighter) => {
                self.focus_card(&fighter, None);
                self.open_effects_page();
            }
            PageAction::EditOtherEffect(rel) => {
                let path = self.resolve_eff_source(&rel);
                self.eff_editor.load_as(&path, rel);
                self.open_effects_page();
            }
            PageAction::OpenEffectFile => {
                let start = self.eff_editor.export_root().join("effect");
                if let Some(path) = rfd::FileDialog::new().add_filter("Effect file", &["eff"]).set_directory(start).pick_file() {
                    self.open_external_eff(path);
                    self.open_effects_page();
                }
            }
            PageAction::EditValues(fighter) => self.roster.focus_traits_for(&fighter),
            PageAction::Made(action) => self.roster.handle_new_character(action),
            PageAction::EditGimmicks(fighter) => {
                self.focus_card(&fighter, None);
                self.roster.open = false;
                self.eff_editor.open = false;
                self.node_workspace = false;
                self.primary_tab = PrimaryEditorTab::Gimmicks;
            }
            PageAction::EditSelectScreen(key) => self.roster.select_css(key),
            PageAction::Files(fighter, slot, what) => self.run_file_action(&fighter, slot, what),
        }
    }

    fn run_file_action(&mut self, fighter: &str, slot: u8, what: FileAction) {
        let Some(workspace) = self.workspace() else {
            self.state.status = "Save the project first.".into();
            return;
        };
        let parts = self.project_parts();
        let result: anyhow::Result<Vec<PathBuf>> = match what {
            FileAction::CopyGameModel | FileAction::CopyGameCharacter => {
                let Some(root) = self.state.data_root.clone() else {
                    self.state.status = "Set the game folder in File first.".into();
                    return;
                };
                let sources = if matches!(what, FileAction::CopyGameCharacter) {
                    crate::project_hub::collect_base_character_files(&root, fighter, slot)
                } else {
                    let folder = root.join("fighter").join(fighter).join("model/body").join(format!("c{slot:02}"));
                    std::fs::read_dir(&folder)
                        .map(|entries| entries.flatten().map(|entry| entry.path()).filter(|path| path.is_file()).collect())
                        .map_err(anyhow::Error::from)
                };
                let sources = match sources {
                    Ok(files) => files,
                    Err(error) => {
                        self.state.status = format!("Could not read the game's files: {error:#}");
                        return;
                    }
                };
                // Copies go where the model and animations are kept, which may be a picked folder.
                let picked: Vec<(PathBuf, PathBuf)> = [Part::Model, Part::Animations]
                    .into_iter()
                    .filter_map(|part| Some((part.game_dir(fighter, slot)?, parts.link(fighter, part, slot)?.to_path_buf())))
                    .collect();
                let (sender, receiver) = std::sync::mpsc::channel();
                self.workspace_copy_rx = Some(receiver);
                self.state.status = "Copying the game's files.".into();
                std::thread::spawn(move || {
                    let result = crate::project_hub::copy_base_files(&workspace, &root, &sources, &picked)
                        .map_err(|error| format!("{error:#}"));
                    let _ = sender.send(result);
                });
                return;
            }
            FileAction::AddModelFolder => {
                let Some(source) = rfd::FileDialog::new().set_title("Choose model folder").pick_folder() else { return };
                crate::project_hub::import_model_folder(&workspace, fighter, slot, &source)
            }
            FileAction::AddModelFiles => {
                let Some(sources) = rfd::FileDialog::new().set_title("Choose model files").add_filter("Model files", crate::project_hub::MODEL_ASSET_EXTENSIONS).pick_files() else { return };
                crate::project_hub::import_model_files(&workspace, fighter, slot, &sources)
            }
            FileAction::AddAnimations => {
                let Some(sources) = rfd::FileDialog::new().set_title("Choose animations").add_filter("Animation", &["nuanmb", "bin"]).pick_files() else { return };
                crate::project_hub::import_animation_files(&workspace, fighter, slot, &sources)
            }
            FileAction::AddSwing => {
                let Some(source) = rfd::FileDialog::new().set_title("Choose swing.prc").add_filter("Swing values", &["prc"]).pick_file() else { return };
                crate::project_hub::import_swing_prc(&workspace, fighter, slot, &source).map(|path| vec![path])
            }
            FileAction::AddEffects | FileAction::AddSounds => {
                let (part, filter): (Part, &[&str]) = if matches!(what, FileAction::AddEffects) { (Part::Effects, &["eff"]) } else { (Part::Sounds, &["nus3audio", "nus3bank"]) };
                let Some(sources) = rfd::FileDialog::new().add_filter(part.title(), filter).pick_files() else { return };
                let Some(dir) = parts.dir(&workspace, fighter, part, slot) else { return };
                let linked = parts.link(fighter, part, slot).is_some();
                sources
                    .iter()
                    .map(|source| copy_part_file(&dir, linked, part, source))
                    .collect()
            }
            FileAction::Send => {
                self.send_workspace_asset_bundle(&workspace, fighter, slot);
                return;
            }
            FileAction::Stop => {
                let generation = self.next_workspace_asset_generation();
                self.game_link.send_asset_bundle(&crate::game_link::AssetBundleWire { generation, target: String::new(), files: Vec::new() });
                self.state.status = "Stopped the model preview in the game.".into();
                return;
            }
        };
        self.state.status = match result {
            Ok(files) => format!("Added {} files to {fighter} c{slot:02}.", files.len()),
            Err(error) => format!("Could not add the files: {error:#}"),
        };
    }

    /// Open the fighter's effect file again from wherever it now comes from.
    fn reopen_fighter_effects(&mut self, fighter: &str) {
        let rel = if fighter == OTHER_EFFECTS {
            match self.eff_editor.loaded_rel() {
                Some(rel) if !rel.starts_with("effect/fighter/") => rel,
                _ => return,
            }
        } else {
            format!("effect/fighter/{fighter}/ef_{fighter}.eff")
        };
        let path = self.resolve_eff_source(&rel);
        if path.is_file() {
            self.current_eff_path = Some(path.clone());
            self.eff_editor.load_as(&path, rel);
        }
    }

    /// Reload the open effect file when it changes on disk in the project or a picked folder:
    /// here at once, and in the game from the next match.
    pub(super) fn reload_changed_effect_file(&mut self) {
        let Some(rel) = self.eff_editor.loaded_rel() else { return };
        let path = self.resolve_eff_source(&rel);
        let primary = self.eff_editor.export_root().join(&rel);
        if path == primary {
            self.effect_file_stamp = None;
            return;
        }
        let stamp = std::fs::metadata(&path).and_then(|meta| meta.modified()).ok();
        let before = self.effect_file_stamp.replace((path.clone(), stamp));
        if before.as_ref().is_none_or(|(was, _)| *was != path) || before.as_ref().map(|(_, at)| *at) == Some(stamp) {
            return;
        }
        self.eff_editor.load_as(&path, rel.clone());
        let fighter = crate::mod_project::fighter_from_source_rel(&rel);
        let staged = if self.eff_mods.get(&fighter).is_some_and(|eff| !eff.is_empty()) {
            self.deploy_live_eff(&fighter)
        } else {
            std::fs::read(&path).is_ok_and(|bytes| self.serve_live_eff(&fighter, &rel, &bytes))
        };
        self.state.status = if staged {
            format!("Reloaded {}. The game uses it from the next match.", folder_name(&path))
        } else {
            format!("Reloaded {}.", folder_name(&path))
        };
    }
}

fn count_files(dir: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if entry.path().is_dir() {
                stack.push(entry.path());
            } else {
                count += 1;
            }
        }
    }
    count
}

/// Copy an effect or sound file into its part's folder. In the project's own copy of the game
/// folders a sound goes into its bank; a picked folder keeps whatever layout it has.
fn copy_part_file(dir: &Path, picked: bool, part: Part, source: &Path) -> anyhow::Result<PathBuf> {
    let name = source.file_name().ok_or_else(|| anyhow::anyhow!("not a file"))?;
    let dir = if part == Part::Sounds && !picked {
        dir.join(crate::project_parts::sound_target(&name.to_string_lossy()))
    } else {
        dir.to_path_buf()
    };
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(name);
    std::fs::copy(source, &out)?;
    Ok(out)
}
