//! The Project page: one card per character the project touches, each with a button per
//! part (moves, gimmicks, effects, sounds, model, animations, values, select screen). A part's
//! button is solid when the project changes that part of that character and dark while it is
//! still the game's; opening it shows the part's numbers, where its files are, and its tools.
//!
//! A card is one character: a base fighter, a group of fighters the game shows as one, or a
//! character made in the project. Its open panel, its costume and every page it opens are its
//! own, never another card's that shares its fighter. Every page a card opens goes through
//! [`VisionaryApp::go`], every count through [`VisionaryApp::card_counts`], and removing a made
//! character through [`VisionaryApp::remove_character`].

use super::*;
use crate::project_parts::{Part, ProjectParts, OTHER_EFFECTS};
use crate::roster::RosterKey;
use crate::ui::{act, chip, look, Look};

/// The narrowest a card is laid out; wider windows fit more columns.
const CARD_WIDTH: f32 = 560.0;

/// Characters the game shows as one that are several fighters underneath. Each gets one card,
/// with its fighters to switch between: every part button then acts on the one picked.
const GROUPS: [(&str, &[(&str, &str)]); 3] = [
    (
        "Pokémon Trainer",
        &[
            ("pzenigame", "Squirtle"),
            ("pfushigisou", "Ivysaur"),
            ("plizardon", "Charizard"),
            ("ptrainer", "Trainer"),
        ],
    ),
    ("Pyra / Mythra", &[("eflame", "Pyra"), ("elight", "Mythra")]),
    ("Ice Climbers", &[("popo", "Popo"), ("nana", "Nana")]),
];

fn group_of(fighter: &str) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
    GROUPS
        .iter()
        .find(|(_, members)| {
            members
                .iter()
                .any(|(member, _)| member.eq_ignore_ascii_case(fighter))
        })
        .copied()
}

/// The page a card action opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CardPage {
    ClassicCollisions,
    ClassicEffects,
    ClassicAudio,
    MoveNodes,
    Gimmicks,
    Effects,
    Values,
    SelectScreen,
}

/// One card on the page.
#[derive(Clone)]
pub(super) struct Card {
    /// The character: `"mario"` for a fighter, `"mario#c08"` for one made in the project. For
    /// a group, the fighter it shows.
    key: RosterKey,
    /// What the card's open panel is stored under: its key, or a group's title.
    id: String,
    /// The fighter whose files the card is about: the base fighter, for a made character.
    fighter: String,
    /// This character's costumes alone.
    slots: Vec<u8>,
    slot: u8,
    name: String,
    /// The line under the name: "c00–c07", "Mario c08–c15", "Mod fighter c00–c11".
    line: String,
    made: bool,
    /// The fighter a made character plays as, by name.
    base_name: String,
    /// The fighters this card stands for, with their names, when it is more than one.
    members: &'static [(&'static str, &'static str)],
    /// Whether the project edits this character, so it cannot be hidden.
    has_edits: bool,
}

/// Something a card asked for, done once the page is drawn.
enum PageAction {
    Go(RosterKey, u8, CardPage),
    Open(String, Part),
    /// The costume a card shows: (card key, fighter, costume).
    PickSlot(String, String, u8),
    Hide(String),
    Add(String),
    PickFolder(String, Part, u8),
    ForgetFolder(String, Part, u8),
    LinkMoveCode,
    UnlinkMoveCode,
    Reveal(PathBuf),
    EditOtherEffect(String),
    OpenEffectFile,
    /// Show another fighter of a card that stands for several: (group title, fighter).
    Member(&'static str, String),
    Remove(RosterKey),
    Files(Card, FileAction),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileAction {
    CopyBase(Part),
    AddModelFolder,
    AddModelFiles,
    AddAnimations,
    AddSwing,
    AddEffects,
    AddSounds,
    Send,
    Stop,
}

/// How much of each kind of edit the project makes to one character.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CardCounts {
    /// Moves edited, in any way.
    pub moves: usize,
    /// Moves built or changed with nodes.
    pub nodes: usize,
    pub gimmicks: usize,
    pub meters: usize,
    /// Effects whose look is changed, and effects borrowed from another file.
    pub effects_changed: usize,
    pub effects_borrowed: usize,
    /// Effect textures replaced, added or removed.
    pub textures: usize,
    /// Moves whose effects (which effects play, where) are changed.
    pub effect_moves: usize,
    /// Moves whose sounds are changed.
    pub sound_moves: usize,
    /// Fighter values changed, as the Fighter values page counts them.
    pub values: usize,
    /// Values of the fighter's gimmick files changed.
    pub gimmick_values: usize,
}

impl CardCounts {
    fn any(&self) -> bool {
        *self != Self::default()
    }
}

/// A part's files: how many the project has, whether in a folder the author picked, and
/// where the game's come from otherwise.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FolderFacts {
    files: usize,
    picked: bool,
    /// The mod the costume's files come from, when not the project's or the game's.
    from_mod: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SelectFacts {
    renamed: bool,
    portraits: usize,
    moved: bool,
    hidden: bool,
}

/// Everything a card's buttons and panels say, gathered once per frame.
#[derive(Debug, Clone, Default)]
struct CardFacts {
    counts: CardCounts,
    made: bool,
    /// The fighter a made character plays as.
    base_name: String,
    /// Characters made on this fighter, which share its values.
    sharing: Vec<String>,
    /// Who has the fighter's one gimmick, when another character on it does.
    gimmick_holder: Option<String>,
    /// The folder of the move code the project uses, by name.
    move_code: Option<String>,
    /// The project has the character's own effect file.
    effect_file: bool,
    effect_folder: bool,
    sound_files: usize,
    sound_folder: bool,
    model: FolderFacts,
    animations: FolderFacts,
    select: SelectFacts,
}

/// What a part's button and panel say.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PartSummary {
    /// The project changes this part of this character.
    changed: bool,
    chips: Vec<String>,
}

/// The fighters each kind of edit touches, by engine fighter name. Every kind is a field, so a
/// new kind of edit has to be named here to give a fighter its card.
#[derive(Debug, Default)]
struct EditFamilies {
    moves: Vec<String>,
    graphs: Vec<String>,
    expressions: Vec<String>,
    effect_calls: Vec<String>,
    node_effects: Vec<String>,
    sounds: Vec<String>,
    effects: Vec<String>,
    values: Vec<String>,
    gimmicks: Vec<String>,
    controllers: Vec<String>,
    meters: Vec<String>,
    live_rules: Vec<String>,
    folders: Vec<String>,
}

/// Every fighter any kind of edit touches.
fn fighters_with_edits(families: &EditFamilies) -> BTreeSet<String> {
    // Listed in full rather than with `..`: a kind added to the struct and not here fails to
    // build instead of silently giving its fighters no card.
    let EditFamilies {
        moves,
        graphs,
        expressions,
        effect_calls,
        node_effects,
        sounds,
        effects,
        values,
        gimmicks,
        controllers,
        meters,
        live_rules,
        folders,
    } = families;
    [
        moves,
        graphs,
        expressions,
        effect_calls,
        node_effects,
        sounds,
        effects,
        values,
        gimmicks,
        controllers,
        meters,
        live_rules,
        folders,
    ]
    .into_iter()
    .flatten()
    .map(|fighter| fighter.to_ascii_lowercase())
    .collect()
}

/// The character a per-move store key belongs to, as the store spells it: `"mario"` for a
/// fighter, `"mario#c08"` for a character made in the project. Live rule keys carry a `#` part
/// after the move, which is not the character's.
fn owner_of_move_key(key: &str) -> Option<String> {
    let (owner, _) = crate::character::split_move_key(key)?;
    (!owner.is_empty()).then(|| owner.to_owned())
}

/// Costumes as ranges: "c00–c07", "c08–c15, c20".
fn slot_ranges(slots: &[u8]) -> String {
    let mut sorted = slots.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut runs: Vec<(u8, u8)> = Vec::new();
    for slot in sorted {
        match runs.last_mut() {
            Some((_, end)) if end.checked_add(1) == Some(slot) => *end = slot,
            _ => runs.push((slot, slot)),
        }
    }
    runs.iter()
        .map(|(start, end)| {
            if start == end {
                format!("c{start:02}")
            } else {
                format!("c{start:02}–c{end:02}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a part's button and panel say about one character.
fn part_summary(part: Part, facts: &CardFacts) -> PartSummary {
    let counts = &facts.counts;
    let mut chips = Vec::new();
    fn add(chips: &mut Vec<String>, n: usize, text: String) {
        if n > 0 {
            chips.push(text);
        }
    }
    let changed = match part {
        Part::Moves => {
            add(&mut chips, counts.moves, format!("{} edited", counts.moves));
            add(
                &mut chips,
                counts.nodes,
                format!("{} with nodes", counts.nodes),
            );
            if let Some(code) = &facts.move_code {
                chips.push(format!("Move code: {code}"));
            }
            counts.moves + counts.nodes > 0
        }
        Part::Gimmicks => {
            add(
                &mut chips,
                counts.gimmicks,
                counted(counts.gimmicks, "gimmick"),
            );
            add(&mut chips, counts.meters, counted(counts.meters, "meter"));
            add(
                &mut chips,
                counts.gimmick_values,
                counted(counts.gimmick_values, "value"),
            );
            // The game runs one gimmick per fighter: say whose it is when it is another's.
            if let Some(holder) = &facts.gimmick_holder {
                chips.push(format!("{holder} has the gimmick"));
            }
            counts.gimmicks + counts.meters + counts.gimmick_values > 0
        }
        Part::Effects => {
            add(
                &mut chips,
                counts.effects_changed,
                format!("{} changed", counts.effects_changed),
            );
            add(
                &mut chips,
                counts.effects_borrowed,
                format!("{} borrowed", counts.effects_borrowed),
            );
            add(
                &mut chips,
                counts.textures,
                counted(counts.textures, "texture"),
            );
            add(
                &mut chips,
                counts.effect_moves,
                counted(counts.effect_moves, "move"),
            );
            if facts.effect_folder {
                chips.push("Your folder".into());
            } else if facts.effect_file {
                chips.push("Own file".into());
            }
            counts.effects_changed + counts.effects_borrowed + counts.textures + counts.effect_moves
                > 0
                || facts.effect_file
                || facts.effect_folder
        }
        Part::Sounds => {
            add(
                &mut chips,
                counts.sound_moves,
                counted(counts.sound_moves, "move"),
            );
            add(
                &mut chips,
                facts.sound_files,
                counted(facts.sound_files, "file"),
            );
            if facts.sound_folder {
                chips.push("Your folder".into());
            }
            counts.sound_moves + facts.sound_files > 0
        }
        Part::Model | Part::Animations => {
            let folder = if part == Part::Model {
                &facts.model
            } else {
                &facts.animations
            };
            add(&mut chips, folder.files, counted(folder.files, "file"));
            if folder.picked {
                chips.push("Your folder".into());
            } else if folder.files == 0 {
                chips.push(match &folder.from_mod {
                    Some(name) => format!("Mod: {name}"),
                    None => "Game".into(),
                });
            }
            folder.files > 0
        }
        Part::Values => {
            if facts.made {
                chips.push(format!("Shared with {}", facts.base_name));
                false
            } else {
                add(
                    &mut chips,
                    counts.values,
                    format!("{} changed", counts.values),
                );
                if !facts.sharing.is_empty() {
                    chips.push(format!("Also used by {}", facts.sharing.join(", ")));
                }
                counts.values > 0
            }
        }
        Part::SelectScreen => {
            let select = &facts.select;
            if facts.made {
                chips.push("Own cell".into());
            }
            if select.renamed {
                chips.push("Renamed".into());
            }
            add(
                &mut chips,
                select.portraits,
                counted(select.portraits, "portrait"),
            );
            if select.moved {
                chips.push("Moved".into());
            }
            if select.hidden {
                chips.push("Hidden".into());
            }
            facts.made || select.renamed || select.portraits > 0 || select.moved || select.hidden
        }
    };
    PartSummary { changed, chips }
}

/// A part button's hover: "Moves: 4 edited, 2 with nodes".
fn part_hover(part: Part, summary: &PartSummary) -> String {
    if summary.chips.is_empty() {
        part.title().to_owned()
    } else {
        format!("{}: {}", part.title(), summary.chips.join(", "))
    }
}

/// The name an effect or sound file gets in a made character's folder: the base fighter's
/// files are per fighter (`ef_mario.eff`), the character's per costume (`ef_mario_c08.eff`),
/// or the file would replace the base fighter's for every costume.
fn made_file_name(name: &str, fighter: &str, slot: u8) -> String {
    let lower = name.to_ascii_lowercase();
    let fighter = fighter.to_ascii_lowercase();
    let own = format!("_c{slot:02}");
    let Some((stem, ext)) = lower.rsplit_once('.') else {
        return lower;
    };
    let tag = format!("_{fighter}");
    // Already a costume's file: `…_mario_c03.ext` becomes `…_mario_c08.ext`.
    if let Some(at) = stem.rfind(&format!("{tag}_c")) {
        let digits = &stem[at + tag.len() + 2..];
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return format!("{}{tag}{own}.{ext}", &stem[..at]);
        }
    }
    if stem.ends_with(&tag) {
        return format!("{stem}{own}.{ext}");
    }
    lower
}

/// A disk look-up redone at most every two seconds: the page is drawn every frame, and
/// reading folders that often is slow on some systems.
fn cached<T: Clone + Send + Sync + 'static>(
    ctx: &egui::Context,
    key: impl std::hash::Hash + std::fmt::Debug,
    read: impl FnOnce() -> T,
) -> T {
    let id = egui::Id::unique(("project_page_disk", key));
    let now = ctx.input(|input| input.time);
    if let Some((at, value)) = ctx.data(|data| data.get_temp::<(f64, T)>(id)) {
        if now - at < 2.0 && now >= at {
            return value;
        }
    }
    let value = read();
    ctx.data_mut(|data| data.insert_temp(id, (now, value.clone())));
    value
}

/// Forget every cached disk look-up, after the page changed files itself.
fn forget_cached(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.remove_by_type::<(f64, usize)>();
        data.remove_by_type::<(f64, bool)>();
        data.remove_by_type::<(f64, Option<String>)>();
    });
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
            painter.add(egui::Shape::convex_polygon(
                star(p(-0.15, 0.1), s * 0.7),
                colour,
                egui::Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                star(p(0.55, -0.5), s * 0.3),
                colour,
                egui::Stroke::NONE,
            ));
        }
        Part::Sounds => {
            // A speaker with sound waves.
            painter.add(egui::Shape::convex_polygon(
                vec![
                    p(-0.8, -0.2),
                    p(-0.5, -0.2),
                    p(-0.15, -0.55),
                    p(-0.15, 0.55),
                    p(-0.5, 0.2),
                    p(-0.8, 0.2),
                ],
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
            painter.rect_filled(
                egui::Rect::from_min_max(p(-0.8, 0.2), p(0.15, 0.65)),
                s * 0.12,
                colour,
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    p(0.1, -0.85),
                    p(-0.35, -0.2),
                    p(0.0, -0.2),
                    p(-0.1, 0.05),
                    p(0.4, -0.5),
                    p(0.05, -0.5),
                ],
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
fn part_button(
    ui: &mut Ui,
    look: Look,
    part: Part,
    changed: bool,
    picked_folder: bool,
    open: bool,
    hover: &str,
) -> egui::Response {
    let size = egui::vec2(46.0, 46.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let colour = part_colour(part);
    let painter = ui.painter();
    let (fill, icon) = if changed {
        (colour, Color32::from_rgb(18, 18, 22))
    } else {
        (look.sunk, look.dim)
    };
    painter.rect_filled(rect, 9.0, fill);
    if !changed {
        painter.rect_stroke(
            rect,
            9.0,
            egui::Stroke::new(1.5, look.edge),
            egui::StrokeKind::Inside,
        );
    }
    if open || response.hovered() {
        painter.rect_stroke(
            rect.expand(3.0),
            11.0,
            egui::Stroke::new(2.0, if open { look.text } else { colour }),
            egui::StrokeKind::Inside,
        );
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
#[allow(clippy::too_many_arguments)]
fn game_tile(
    ui: &mut Ui,
    width: f32,
    look: Look,
    icon: Tile,
    title: &str,
    line: &str,
    changed: bool,
    open: bool,
) -> egui::Response {
    let colour = match icon {
        Tile::Part(part) => part_colour(part),
        Tile::Mods => Color32::from_rgb(190, 150, 255),
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 64.0), egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 12.0, look.card);
    let edge = if open {
        look.text
    } else if response.hovered() {
        colour
    } else {
        look.edge
    };
    painter.rect_stroke(
        rect,
        12.0,
        egui::Stroke::new(2.0, edge),
        egui::StrokeKind::Inside,
    );
    let badge =
        egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 10.0), egui::vec2(44.0, 44.0));
    let (fill, ink) = if changed {
        (colour, Color32::from_rgb(18, 18, 22))
    } else {
        (look.sunk, colour)
    };
    painter.rect_filled(badge, 9.0, fill);
    match icon {
        Tile::Part(part) => paint_part_icon(painter, badge.shrink(10.0), part, ink),
        Tile::Mods => {
            // Three stacked boxes.
            let stroke = egui::Stroke::new(2.0, ink);
            for i in 0..3 {
                let y = badge.top() + 12.0 + i as f32 * 7.0;
                painter.rect_stroke(
                    egui::Rect::from_min_size(
                        egui::pos2(badge.left() + 11.0, y),
                        egui::vec2(22.0, 5.0),
                    ),
                    1.5,
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
        }
    }
    let x = badge.right() + 12.0;
    // The title sits alone until the project changes something here.
    if line.is_empty() {
        painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(18.0),
            look.text,
        );
    } else {
        painter.text(
            egui::pos2(x, rect.top() + 12.0),
            egui::Align2::LEFT_TOP,
            title,
            egui::FontId::proportional(18.0),
            look.text,
        );
        painter.text(
            egui::pos2(x, rect.top() + 36.0),
            egui::Align2::LEFT_TOP,
            line,
            egui::FontId::proportional(14.0),
            look.dim,
        );
    }
    response
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl VisionaryApp {
    pub(super) fn workspace(&self) -> Option<PathBuf> {
        self.project_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf)
    }

    /// The open project's characters and folders, read from beside the project when it
    /// changes. Its made characters' folders are kept in step with the project here, so code
    /// that reads `parts.json` itself finds their files too.
    pub(super) fn project_parts(&mut self) -> ProjectParts {
        let workspace = self.workspace();
        if self.project_parts_cache.as_ref().map(|(at, _)| at) != workspace.as_ref() {
            let parts = workspace
                .as_deref()
                .map(ProjectParts::load)
                .unwrap_or_default();
            // A project that names its own move code links it on opening.
            if let Some(folder) = parts.link("", Part::Moves, 0) {
                if self.acmd_src.as_ref().map(|index| index.root.as_path()) != Some(folder) {
                    self.link_acmd_source(folder.to_path_buf());
                }
            }
            self.project_parts_cache = workspace.map(|at| (at, parts));
        }
        if let Some(status) = self.roster.take_settled_status() {
            self.state.status = status;
        }
        let made = self.roster.made_folders();
        if let Some((workspace, parts)) = &mut self.project_parts_cache {
            if parts.set_made(made) {
                if let Err(error) = parts.save(workspace) {
                    self.state.status = format!("Could not save parts.json: {error:#}");
                }
                self.preview_skin = None;
            }
        }
        self.project_parts_cache
            .as_ref()
            .map(|(_, parts)| parts.clone())
            .unwrap_or_default()
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

    /// Costumes of each fighter that characters made in the project own.
    fn made_slots(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for made in &self.roster.project.authored {
            out.entry(made.donor.to_ascii_lowercase())
                .or_default()
                .extend(made.all_slots());
        }
        out
    }

    /// The characters each kind of edit touches, as each store spells them: `"mario"` for a
    /// fighter, `"mario#c08"` for a character made in the project. Every store keeps a made
    /// character's edits under its own key, so none of them counts on its base fighter.
    fn edit_families(&self, parts: &ProjectParts) -> EditFamilies {
        let made = self.made_slots();
        let of_keys = |keys: &mut dyn Iterator<Item = &String>| -> Vec<String> {
            keys.filter_map(|key| owner_of_move_key(key)).collect()
        };
        let acts = |graph: &crate::gimmicks::behavior::BehaviorGraph| {
            graph
                .transitions
                .iter()
                .any(|step| !step.actions.is_empty())
        };
        // The game keeps one gimmick per fighter: it is the made character's when limited to
        // its costumes, else the fighter's.
        let gimmick_of = |fighter: &String| {
            self.gimmick_owner(fighter)
                .map(|owner| owner.as_str().to_owned())
        };
        let roster = &self.roster;
        EditFamilies {
            moves: self
                .state
                .edit_log
                .entries
                .iter()
                .filter(|(_, log)| !log.is_empty())
                .map(|(owner, _)| owner.clone())
                .collect(),
            graphs: self
                .move_graphs
                .iter()
                .filter(|(_, graphs)| !graphs.is_empty())
                .map(|(owner, _)| owner.clone())
                .collect(),
            expressions: of_keys(&mut self.state.expression_script_edits.keys()),
            effect_calls: of_keys(
                &mut self
                    .state
                    .effect_call_edits
                    .iter()
                    .filter(|(_, edits)| !edits.is_empty())
                    .map(|(key, _)| key),
            ),
            node_effects: of_keys(&mut self.node_effect_scripts.keys()),
            sounds: of_keys(&mut self.state.sound_script_edits.keys()),
            effects: self
                .eff_mods
                .iter()
                .filter(|(_, eff)| !eff.is_empty())
                .map(|(owner, _)| owner.clone())
                .collect(),
            values: roster
                .params
                .iter()
                .filter(|(_, params)| !params.is_empty())
                .map(|(fighter, _)| fighter.clone())
                .collect(),
            gimmicks: roster
                .behaviors
                .iter()
                .filter(|(_, graph)| acts(graph))
                .filter_map(|(fighter, _)| gimmick_of(fighter))
                .collect(),
            controllers: roster
                .controller_charts
                .iter()
                .filter(|(_, charts)| charts.values().any(|chart| acts(&chart.graph)))
                .filter_map(|(fighter, _)| gimmick_of(fighter))
                .collect(),
            meters: roster
                .meters
                .iter()
                .filter(|(_, meters)| !meters.is_empty())
                .filter_map(|(fighter, _)| gimmick_of(fighter))
                .collect(),
            live_rules: of_keys(
                &mut self
                    .hitbox_rules_store
                    .iter()
                    .filter(|(_, rules)| !rules.is_empty())
                    .map(|(key, _)| key)
                    .chain(
                        self.effect_rules_store
                            .iter()
                            .filter(|(_, rules)| !rules.is_empty())
                            .map(|(key, _)| key),
                    )
                    .chain(
                        self.effect_control_rules_store
                            .iter()
                            .filter(|(_, rules)| !rules.is_empty())
                            .map(|(key, _)| key),
                    ),
            ),
            // Folders are picked per fighter and costume: one picked for a made character's
            // costume is that character's.
            folders: parts
                .links
                .iter()
                .filter(|link| link.part != Part::Moves)
                .filter(|link| {
                    !link.slot.is_some_and(|slot| {
                        made.get(&link.fighter.to_ascii_lowercase())
                            .is_some_and(|slots| slots.contains(&slot))
                    })
                })
                .map(|link| link.fighter.clone())
                .collect(),
        }
    }

    /// Every character the project touches: picked onto the page, edited anywhere, given a
    /// folder, or made in the project.
    fn project_cards(&self, parts: &ProjectParts) -> Vec<Card> {
        let made = self.made_slots();
        let edited = fighters_with_edits(&self.edit_families(parts));
        let mut names: BTreeSet<String> = parts
            .characters
            .keys()
            .filter(|key| !key.contains('#'))
            .cloned()
            .collect();
        // A made character's edits are on its own card, which every made character has.
        names.extend(edited.iter().filter(|owner| !owner.contains('#')).cloned());
        let mut cards: Vec<Card> = Vec::new();
        for name in names {
            // One card for all the fighters of a character, showing the one picked on it.
            let group = group_of(&name);
            let name = match group {
                Some((title, members)) => {
                    if cards.iter().any(|card| card.id == title) {
                        continue;
                    }
                    let picked = self.project_member.get(title).cloned();
                    picked
                        .filter(|picked| members.iter().any(|(member, _)| member == picked))
                        .unwrap_or(name)
                }
                None => name,
            };
            let Some(entry) = self
                .state
                .fighters
                .iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(&name))
            else {
                continue;
            };
            // Costumes a made character owns are on its own card, not this one.
            let taken = made.get(&entry.name).cloned().unwrap_or_default();
            let mut slots: Vec<u8> = entry
                .slots
                .iter()
                .copied()
                .filter(|slot| !taken.contains(slot))
                .collect();
            if slots.is_empty() {
                slots = entry.slots.clone();
            }
            let slot = parts
                .characters
                .get(&entry.name)
                .copied()
                .filter(|slot| slots.contains(slot))
                .unwrap_or(slots.first().copied().unwrap_or(0));
            let key = RosterKey::fighter(&entry.name);
            let ranges = slot_ranges(&slots);
            cards.push(Card {
                id: group.map_or_else(|| key.to_string(), |(title, _)| title.to_owned()),
                fighter: entry.name.clone(),
                slot,
                // "Ice Climbers · Popo" is the Ice Climbers on a card of their own.
                name: match group {
                    Some((title, _)) => title.to_owned(),
                    None => self.roster.shown_name(&key).unwrap_or_else(|| {
                        entry
                            .display_name
                            .split(" · ")
                            .next()
                            .unwrap_or(&entry.display_name)
                            .to_owned()
                    }),
                },
                line: if entry.is_modded() {
                    format!("Mod fighter {ranges}")
                } else {
                    ranges
                },
                slots,
                key,
                made: false,
                base_name: String::new(),
                members: group.map_or(&[], |(_, members)| members),
                has_edits: match group {
                    Some((_, members)) => {
                        members.iter().any(|(member, _)| edited.contains(*member))
                    }
                    None => edited.contains(&entry.name),
                },
            });
        }
        for authored in &self.roster.project.authored {
            let base_name = self
                .state
                .fighters
                .iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(&authored.donor))
                .map_or_else(
                    || crate::data::fighter_display_name(&authored.donor),
                    |entry| entry.display_name.clone(),
                );
            let slots = authored.all_slots();
            let slot = parts
                .characters
                .get(authored.key.as_str())
                .copied()
                .filter(|slot| slots.contains(slot))
                .unwrap_or(slots.first().copied().unwrap_or(authored.slot));
            cards.push(Card {
                id: authored.key.to_string(),
                fighter: authored.donor.clone(),
                slot,
                name: self
                    .roster
                    .shown_name(&authored.key)
                    .unwrap_or_else(|| authored.display_name.clone()),
                line: format!("{base_name} {}", slot_ranges(&slots)),
                slots,
                key: authored.key.clone(),
                made: true,
                base_name,
                members: &[],
                has_edits: true,
            });
        }
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        cards
    }

    /// The character a card stands for: its made character, or the fighter it shows.
    fn card_character(card: &Card) -> crate::character::CharacterKey {
        if card.made {
            crate::character::CharacterKey::made(card.key.clone())
        } else {
            crate::character::CharacterKey::base(&card.fighter)
        }
    }

    /// The moves `owner` has edited in any way the Moves part shows: hitboxes and other game
    /// script calls, node graphs, expressions, and moves the game is changed in live.
    fn character_edited_moves(&self, owner: &crate::character::CharacterKey) -> BTreeSet<String> {
        let mine = |key: &&String| owner_of_move_key(key).as_deref() == Some(owner.as_str());
        let mut moves: BTreeSet<String> = self
            .state
            .edit_log
            .entries
            .get(owner.as_str())
            .into_iter()
            .flat_map(|log| log.keys().cloned())
            .collect();
        moves.extend(
            self.move_graphs
                .get(owner.as_str())
                .into_iter()
                .flat_map(|graphs| graphs.keys().cloned()),
        );
        let named = |key: &String| {
            crate::character::split_move_key(key)
                .map(|(_, name)| name.split('#').next().unwrap_or(name).to_owned())
        };
        moves.extend(
            self.state
                .expression_script_edits
                .keys()
                .filter(mine)
                .filter_map(named),
        );
        // A live rule exists only for a changed move.
        let live = self
            .hitbox_rules_store
            .iter()
            .filter(|(_, rules)| !rules.is_empty())
            .map(|(key, _)| key)
            .chain(
                self.effect_rules_store
                    .iter()
                    .filter(|(_, rules)| !rules.is_empty())
                    .map(|(key, _)| key),
            )
            .chain(
                self.effect_control_rules_store
                    .iter()
                    .filter(|(_, rules)| !rules.is_empty())
                    .map(|(key, _)| key),
            );
        moves.extend(live.filter(mine).filter_map(named));
        moves
    }

    /// How much of each kind of edit the project makes to `card`'s character: its own edits
    /// only, as [`VisionaryApp::character_edit_counts`] counts them. Fighter values are kept per
    /// fighter, so a made character's card counts none of its own.
    fn card_counts(&self, card: &Card) -> CardCounts {
        let owner = Self::card_character(card);
        let counts = self.character_edit_counts(&owner);
        let fighter = card.fighter.to_ascii_lowercase();
        let roster = &self.roster;
        let (values, gimmick_values) = if card.made {
            (0, 0)
        } else {
            let values = roster.fighter_value_count(&fighter);
            (
                values,
                roster
                    .params
                    .get(&fighter)
                    .map_or(0, |params| params.field_count().saturating_sub(values)),
            )
        };
        CardCounts {
            moves: self.character_edited_moves(&owner).len(),
            nodes: counts.node_graphs,
            gimmicks: counts.gimmicks,
            meters: counts.meters,
            effects_changed: counts.authored_effects,
            effects_borrowed: counts.borrowed_effects,
            textures: counts.textures,
            effect_moves: counts.effect_call_moves,
            sound_moves: counts.sound_moves,
            values,
            gimmick_values,
        }
    }

    /// The select screen cell a card stands for.
    fn card_cell(&self, card: &Card) -> RosterKey {
        if card.made {
            return card.key.clone();
        }
        let members: Vec<&str> = if card.members.is_empty() {
            vec![card.fighter.as_str()]
        } else {
            card.members.iter().map(|(member, _)| *member).collect()
        };
        self.roster
            .select_screen_cell(&members)
            .unwrap_or_else(|| card.key.clone())
    }

    /// Where a costume's model or animations come from when the project has none of its own:
    /// the mod that last provides them, or `None` for the game.
    fn costume_source(
        &self,
        ctx: &egui::Context,
        fighter: &str,
        kind: &str,
        slot: u8,
    ) -> Option<String> {
        cached(
            ctx,
            ("source", fighter, kind, slot, self.all_roots()),
            || {
                let (_, from) = self.skin_folder(fighter, kind, slot)?;
                from.strip_prefix("the mod ").map(str::to_owned)
            },
        )
    }

    /// Everything `card`'s buttons and panels say.
    fn card_facts(&self, ctx: &egui::Context, card: &Card, parts: &ProjectParts) -> CardFacts {
        let workspace = self.workspace();
        let dir = |part: Part| {
            workspace
                .as_deref()
                .and_then(|workspace| parts.dir(workspace, &card.fighter, part, card.slot))
        };
        let picked = |part: Part| parts.link(&card.fighter, part, card.slot).is_some();
        let files = |part: Part| {
            dir(part).map_or(0, |dir| {
                cached(ctx, ("files", dir.clone()), || count_files(&dir))
            })
        };
        let folder = |part: Part, kind: &str| {
            let files = files(part);
            FolderFacts {
                files,
                picked: picked(part),
                from_mod: if files == 0 {
                    self.costume_source(ctx, &card.fighter, kind, card.slot)
                } else {
                    None
                },
            }
        };
        let fighter = card.fighter.to_ascii_lowercase();
        // The base fighter's effect file, or a made character's own one for this costume.
        let effect_name = if card.made {
            format!("ef_{fighter}_c{:02}.eff", card.slot)
        } else {
            format!("ef_{fighter}.eff")
        };
        let effect_file = dir(Part::Effects).is_some_and(|dir| {
            let file = dir.join(&effect_name);
            cached(ctx, ("file", file.clone()), || file.is_file())
        });
        // A base fighter's sound files sit with every other fighter's in the project's bank.
        let sound_files = dir(Part::Sounds).map_or(0, |dir| {
            let tag = format!("_{fighter}");
            let only_own = !card.made && !picked(Part::Sounds);
            cached(ctx, ("sounds", dir.clone(), only_own), || {
                count_matching(&dir, |name| !only_own || name.contains(&tag))
            })
        });
        let cell = self.card_cell(card);
        let project = &self.roster.project;
        let own_name = project
            .authored
            .iter()
            .find(|entry| entry.key == cell)
            .map(|entry| entry.display_name.clone());
        let character = Self::card_character(card);
        let gimmick_holder = self
            .gimmick_owner(&card.fighter)
            .filter(|owner| *owner != character)
            .map(|owner| self.character_name(&owner));
        CardFacts {
            gimmick_holder,
            counts: self.card_counts(card),
            made: card.made,
            base_name: card.base_name.clone(),
            sharing: if card.made {
                Vec::new()
            } else {
                project
                    .authored
                    .iter()
                    .filter(|entry| entry.donor.eq_ignore_ascii_case(&fighter))
                    .map(|entry| {
                        self.roster
                            .shown_name(&entry.key)
                            .unwrap_or_else(|| entry.display_name.clone())
                    })
                    .collect()
            },
            move_code: self.acmd_src.as_ref().map(|index| folder_name(&index.root)),
            effect_file,
            effect_folder: picked(Part::Effects),
            sound_files,
            sound_folder: picked(Part::Sounds),
            model: folder(Part::Model, "model"),
            animations: folder(Part::Animations, "motion"),
            select: SelectFacts {
                // A made character's own name is not a rename.
                renamed: project
                    .names
                    .get(&cell)
                    .is_some_and(|name| Some(name) != own_name.as_ref()),
                portraits: project
                    .ui_images
                    .get(&cell)
                    .map_or(0, |images| images.len()),
                moved: project.order.contains_key(&cell),
                hidden: project.hidden.contains(&cell),
            },
        }
    }

    /// Whether `card`'s character is the one the editors have open.
    fn card_is_active(&self, card: &Card) -> bool {
        self.active_key().as_ref() == Some(&Self::card_character(card))
    }

    /// Project > Characters.
    pub(super) fn draw_project_page(&mut self, ui: &mut Ui) {
        let look = look(ui);
        ui.painter().rect_filled(ui.max_rect(), 0.0, look.page);
        let workspace = self.workspace();
        let mut actions = Vec::new();
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let title = if self.project_name.is_empty() {
                "Untitled project"
            } else {
                &self.project_name
            };
            ui.label(RichText::new(title).size(30.0).strong().color(look.text));
            ui.add_space(12.0);
            if let Some(dir) = &workspace {
                if act(ui, look, "Open folder", None)
                    .on_hover_text(dir.display().to_string())
                    .clicked()
                {
                    actions.push(PageAction::Reveal(dir.clone()));
                }
            }
            if act(ui, look, "Projects", None).clicked() {
                self.project_hub.show = true;
            }
            if act(ui, look, "Export mod", Some(look.accent)).clicked() {
                self.export_full_mod();
            }
            let code = self.acmd_src.as_ref().map(|index| index.root.clone());
            ui.menu_button(RichText::new("⋯").size(20.0).strong(), |ui| match &code {
                None => {
                    if ui
                        .button("Use my Smashline project")
                        .on_hover_text(
                            "Edit the move code of a Smashline plugin you write yourself",
                        )
                        .clicked()
                    {
                        actions.push(PageAction::LinkMoveCode);
                        ui.close();
                    }
                }
                Some(_) => {
                    if ui.button("Stop using it").clicked() {
                        actions.push(PageAction::UnlinkMoveCode);
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Project options");
            if let Some(root) = &code {
                ui.label(
                    RichText::new(format!("Move code: {}", folder_name(root)))
                        .color(look.accent)
                        .strong(),
                )
                .on_hover_text(root.display().to_string());
            }
        });
        if workspace.is_none() {
            ui.label(RichText::new("Not saved yet").color(look.dim));
        }
        ui.add_space(10.0);
        let parts = self.project_parts();
        let cards = self.project_cards(&parts);
        let width = ui.available_width();
        let columns = ((width / CARD_WIDTH) as usize).clamp(1, 3);
        // The cards share the whole row, however many fit.
        let card_width = (width - 10.0 * (columns as f32 - 1.0)) / columns as f32 - 1.0;

        section_title(ui, look, "Across the game");
        self.draw_game_tiles(ui, look, &parts, &mut actions);
        if self
            .project_open_part
            .as_ref()
            .is_some_and(|(key, _)| key == OTHER_EFFECTS)
        {
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
            // Fighters with a card of their own; a made character's base fighter is not one.
            let shown: BTreeSet<&str> = cards
                .iter()
                .filter(|card| !card.made)
                .flat_map(|card| {
                    std::iter::once(card.fighter.as_str())
                        .chain(card.members.iter().map(|(member, _)| *member))
                })
                .collect();
            egui::ComboBox::from_id_salt("project_add_character")
                .selected_text(RichText::new("Add character").strong().color(look.text))
                .width(200.0)
                .height(420.0)
                .show_ui(ui, |ui| {
                    let mut fighters: Vec<_> = self
                        .state
                        .fighters
                        .iter()
                        .filter(|entry| !shown.contains(entry.name.as_str()))
                        .collect();
                    fighters.sort_by(|a, b| a.display_name.cmp(&b.display_name));
                    for entry in fighters {
                        if ui.selectable_label(false, &entry.display_name).clicked() {
                            actions.push(PageAction::Add(entry.name.clone()));
                        }
                    }
                });
            if act(ui, look, "New character", None)
                .on_hover_text("Make a fighter of your own on top of a base one")
                .clicked()
            {
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

        let dropped: Vec<PathBuf> = ui.ctx().input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            // Dropped files go to the card whose files are open, and nowhere else.
            let target = self.project_open_part.as_ref().and_then(|(id, part)| {
                part.has_folder()
                    .then(|| cards.iter().find(|card| &card.id == id))
                    .flatten()
            });
            match (&workspace, target) {
                (None, _) => self.state.status = "Save the project before adding files.".into(),
                (_, None) => {
                    self.state.status = "Open a card's Model or Animations first.".into();
                }
                (Some(workspace), Some(card)) => {
                    // The status names the costume's owner: the made character, for its own.
                    self.import_dropped_files(workspace, &card.fighter, card.slot, dropped);
                    forget_cached(ui.ctx());
                }
            }
        }
        let ctx = ui.ctx().clone();
        for action in actions {
            self.run_page_action(&ctx, action);
        }
    }

    fn card_frame(look: Look, selected: bool) -> egui::Frame {
        egui::Frame::new()
            .fill(look.card)
            .corner_radius(14.0)
            .inner_margin(egui::Margin::same(14))
            .stroke(egui::Stroke::new(
                2.0,
                if selected { look.accent } else { look.edge },
            ))
    }

    fn draw_character_card(
        &mut self,
        ui: &mut Ui,
        look: Look,
        card: &Card,
        parts: &ProjectParts,
        actions: &mut Vec<PageAction>,
    ) {
        let selected = self.card_is_active(card);
        let open = self
            .project_open_part
            .as_ref()
            .filter(|(id, _)| *id == card.id)
            .map(|(_, part)| *part);
        let facts = self.card_facts(ui.ctx(), card, parts);
        let confirm_id = egui::Id::unique(("project_remove_confirm", card.key.as_str()));
        let confirming = ui.ctx().data(|data| data.get_temp::<bool>(confirm_id)) == Some(true);
        Self::card_frame(look, selected).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let portrait = self
                    .roster
                    .portrait(ui.ctx(), &card.key, &card.fighter, card.slot);
                let size = egui::vec2(112.0, 112.0);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                ui.painter().rect_filled(rect, 10.0, look.sunk);
                match portrait {
                    Some(texture) => {
                        egui::Image::new(&texture)
                            .corner_radius(10.0)
                            .paint_at(ui, rect);
                    }
                    None => paint_part_icon(ui.painter(), rect.shrink(32.0), Part::Model, look.dim),
                }
                let edge = if response.hovered() {
                    look.accent
                } else {
                    look.edge
                };
                ui.painter().rect_stroke(
                    rect,
                    10.0,
                    egui::Stroke::new(1.5, edge),
                    egui::StrokeKind::Inside,
                );
                if response
                    .on_hover_text("Open in the editor")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    actions.push(PageAction::Go(
                        card.key.clone(),
                        card.slot,
                        CardPage::ClassicCollisions,
                    ));
                }
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&card.name)
                                    .size(28.0)
                                    .strong()
                                    .color(look.text),
                            )
                            .truncate(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let has_edits = card.has_edits || facts.counts.any();
                            self.draw_card_menu(ui, card, has_edits, confirm_id, actions);
                            egui::ComboBox::from_id_salt(("card_slot", card.id.as_str()))
                                .selected_text(
                                    RichText::new(format!("c{:02}", card.slot))
                                        .strong()
                                        .color(look.text),
                                )
                                .width(64.0)
                                .show_ui(ui, |ui| {
                                    for slot in &card.slots {
                                        if ui
                                            .selectable_label(
                                                *slot == card.slot,
                                                format!("c{slot:02}"),
                                            )
                                            .clicked()
                                        {
                                            actions.push(PageAction::PickSlot(
                                                card.key.to_string(),
                                                card.fighter.clone(),
                                                *slot,
                                            ));
                                        }
                                    }
                                })
                                .response
                                .on_hover_text("Costume");
                        });
                    });
                    ui.horizontal_wrapped(|ui| {
                        for (member, label) in card.members {
                            if self.fighter_index(member).is_none() {
                                continue;
                            }
                            let shown = member.eq_ignore_ascii_case(&card.fighter);
                            if act(ui, look, label, shown.then_some(look.accent)).clicked()
                                && !shown
                            {
                                if let Some((title, _)) = group_of(member) {
                                    actions.push(PageAction::Member(title, (*member).to_owned()));
                                }
                            }
                        }
                        ui.label(RichText::new(&card.line).size(15.0).color(look.dim));
                    });
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for part in Part::ALL {
                            let summary = part_summary(part, &facts);
                            let picked = part.has_folder()
                                && parts.link(&card.fighter, part, card.slot).is_some();
                            let hover = part_hover(part, &summary);
                            if part_button(
                                ui,
                                look,
                                part,
                                summary.changed,
                                picked,
                                open == Some(part),
                                &hover,
                            )
                            .clicked()
                            {
                                actions.push(PageAction::Open(card.id.clone(), part));
                            }
                        }
                    });
                });
            });
            if confirming {
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    let danger = Color32::from_rgb(235, 95, 95);
                    if act(
                        ui,
                        look,
                        &format!("Remove {} and its edits", card.name),
                        Some(danger),
                    )
                    .on_hover_text("Its files stay in its folder")
                    .clicked()
                    {
                        ui.ctx().data_mut(|data| data.remove::<bool>(confirm_id));
                        actions.push(PageAction::Remove(card.key.clone()));
                    }
                    if act(ui, look, "Cancel", None).clicked() {
                        ui.ctx().data_mut(|data| data.remove::<bool>(confirm_id));
                    }
                });
            }
            if let Some(part) = open {
                ui.add_space(10.0);
                self.draw_part_detail(ui, look, card, part, &facts, parts, actions);
            }
        });
    }

    /// The card's "⋯" menu: hide a fighter's card, or remove a made character.
    fn draw_card_menu(
        &self,
        ui: &mut Ui,
        card: &Card,
        has_edits: bool,
        confirm_id: egui::Id,
        actions: &mut Vec<PageAction>,
    ) {
        ui.menu_button(RichText::new("⋯").size(20.0).strong(), |ui| {
            if card.made {
                if ui.button(format!("Remove {}", card.name)).clicked() {
                    ui.ctx().data_mut(|data| data.insert_temp(confirm_id, true));
                    ui.close();
                }
            } else {
                let hide = ui
                    .add_enabled(!has_edits, egui::Button::new("Hide from page"))
                    .on_disabled_hover_text("Has edits");
                if hide.clicked() {
                    actions.push(PageAction::Hide(card.fighter.clone()));
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text(format!("{} options", card.name));
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_part_detail(
        &mut self,
        ui: &mut Ui,
        look: Look,
        card: &Card,
        part: Part,
        facts: &CardFacts,
        parts: &ProjectParts,
        actions: &mut Vec<PageAction>,
    ) {
        let summary = part_summary(part, facts);
        let fighter = &card.fighter;
        let slot = card.slot;
        let colour = part_colour(part);
        let name = &card.name;
        egui::Frame::new()
            .fill(look.sunk)
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.5, colour))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                    paint_part_icon(ui.painter(), rect, part, colour);
                    ui.label(RichText::new(part.title()).size(19.0).strong().color(look.text));
                    for text in &summary.chips {
                        chip(ui, text, if summary.changed { colour } else { look.dim });
                    }
                });
                // Where the part's files are, for the parts that have files.
                if part.has_folder() && part != Part::Moves {
                    let workspace = self.workspace();
                    let folder = workspace
                        .as_deref()
                        .and_then(|workspace| parts.dir(workspace, fighter, part, slot));
                    let picked = parts.link(fighter, part, slot).is_some();
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        let shown = folder.as_ref().map_or_else(
                            || "Save the project first".to_owned(),
                            |dir| dir.display().to_string(),
                        );
                        ui.label(RichText::new(shown).monospace().color(look.text));
                    });
                    ui.horizontal_wrapped(|ui| {
                        if let Some(dir) = folder.clone() {
                            if act(ui, look, "Open folder", None).clicked() {
                                actions.push(PageAction::Reveal(dir));
                            }
                        }
                        // A made character's effects and sounds live in its own folder.
                        let own_only = card.made && !part.per_costume();
                        if picked {
                            if act(ui, look, "Use project folder", None).clicked() {
                                actions.push(PageAction::ForgetFolder(fighter.clone(), part, slot));
                            }
                        } else if !own_only
                            && act(ui, look, "Use another folder", None)
                                .on_hover_text(if part.per_costume() {
                                    format!("Keep c{slot:02}'s files in a folder of yours")
                                } else {
                                    format!("Keep {name}'s files in a folder of yours")
                                })
                                .clicked()
                        {
                            actions.push(PageAction::PickFolder(fighter.clone(), part, slot));
                        }
                    });
                }
                ui.add_space(6.0);
                let go = |page| PageAction::Go(card.key.clone(), slot, page);
                let files = |what| PageAction::Files(card.clone(), what);
                let main = Some(colour);
                ui.horizontal_wrapped(|ui| match part {
                    Part::Moves => {
                        if act(ui, look, "Edit", main)
                            .on_hover_text(format!("{name} c{slot:02} in the Classic editor"))
                            .clicked()
                        {
                            actions.push(go(CardPage::ClassicCollisions));
                        }
                        if act(ui, look, "Edit as nodes", None)
                            .on_hover_text(format!("{name} c{slot:02} in Move nodes"))
                            .clicked()
                        {
                            actions.push(go(CardPage::MoveNodes));
                        }
                    }
                    Part::Gimmicks => {
                        if act(ui, look, "Edit", main)
                            .on_hover_text(format!("{name} in Gimmicks"))
                            .clicked()
                        {
                            actions.push(go(CardPage::Gimmicks));
                        }
                    }
                    Part::Effects => {
                        if act(ui, look, "Edit effects", main)
                            .on_hover_text(format!("{name}'s effect file on the Effects page"))
                            .clicked()
                        {
                            actions.push(go(CardPage::Effects));
                        }
                        if act(ui, look, "Edit in moves", None)
                            .on_hover_text("Which effects a move plays, and where")
                            .clicked()
                        {
                            actions.push(go(CardPage::ClassicEffects));
                        }
                        let add = act(ui, look, "Add effect file", None);
                        let add = if card.made {
                            add.on_hover_text(format!(
                                "Saved as ef_{fighter}_c{slot:02}.eff, {name} c{slot:02}'s own"
                            ))
                        } else {
                            add
                        };
                        if add.clicked() {
                            actions.push(files(FileAction::AddEffects));
                        }
                    }
                    Part::Sounds => {
                        if act(ui, look, "Edit move sounds", main)
                            .on_hover_text(format!("{name} c{slot:02} on the Classic editor's sound tab"))
                            .clicked()
                        {
                            actions.push(go(CardPage::ClassicAudio));
                        }
                        let add = act(ui, look, "Add sound files", None);
                        let add = if card.made {
                            add.on_hover_text(format!(
                                "Named for {name} c{slot:02}, as se_{fighter}_c{slot:02}.nus3audio"
                            ))
                        } else {
                            add
                        };
                        if add.clicked() {
                            actions.push(files(FileAction::AddSounds));
                        }
                    }
                    Part::Model | Part::Animations => {
                        let base = if card.made {
                            format!(
                                "{}'s c{:02}; files already here are kept",
                                card.base_name,
                                card.slots.iter().position(|own| *own == slot).unwrap_or(0) % 8
                            )
                        } else {
                            "The game's files; files already here are kept".to_owned()
                        };
                        let busy = self.workspace_copy_rx.is_some()
                            || (!card.made && self.state.data_root.is_none());
                        if ui
                            .add_enabled_ui(!busy, |ui| act(ui, look, "Copy base costume", main))
                            .inner
                            .on_hover_text(base)
                            .clicked()
                        {
                            actions.push(files(FileAction::CopyBase(part)));
                        }
                        if part == Part::Model {
                            if act(ui, look, "Add files", None).clicked() {
                                actions.push(files(FileAction::AddModelFiles));
                            }
                            if act(ui, look, "Add folder", None)
                                .on_hover_text("Every model file in a folder")
                                .clicked()
                            {
                                actions.push(files(FileAction::AddModelFolder));
                            }
                        } else {
                            if act(ui, look, "Add animations", None).clicked() {
                                actions.push(files(FileAction::AddAnimations));
                            }
                            if act(ui, look, "Add swing.prc", None).clicked() {
                                actions.push(files(FileAction::AddSwing));
                            }
                        }
                    }
                    Part::Values => {
                        if card.made {
                            if act(ui, look, &format!("Edit {}'s values", card.base_name), main)
                                .on_hover_text(
                                    "The game keeps one set of values per fighter; every costume uses them",
                                )
                                .clicked()
                            {
                                actions.push(go(CardPage::Values));
                            }
                        } else if act(ui, look, "Edit", main).clicked() {
                            actions.push(go(CardPage::Values));
                        }
                    }
                    Part::SelectScreen => {
                        if act(ui, look, "Edit", main)
                            .on_hover_text("Name and portraits")
                            .clicked()
                        {
                            actions.push(go(CardPage::SelectScreen));
                        }
                    }
                });
                if matches!(part, Part::Model | Part::Animations) {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        let online = self.game_link.status()
                            == crate::game_link::LinkStatus::Connected;
                        if ui
                            .add_enabled_ui(online, |ui| act(ui, look, "Show in game", None))
                            .inner
                            .on_hover_text("The fighter must not hold an item while it loads")
                            .on_disabled_hover_text("Start the game with the plugin first")
                            .clicked()
                        {
                            actions.push(files(FileAction::Send));
                        }
                        let status = self.game_link.asset_bundle_status();
                        let sent = status.target == fighter.to_ascii_lowercase()
                            && self.workspace_asset_sent_slot == Some(slot);
                        if sent
                            && act(ui, look, "Stop", None)
                                .on_hover_text("Back to the game's own files")
                                .clicked()
                        {
                            actions.push(files(FileAction::Stop));
                        }
                        if sent && status.generation == self.workspace_asset_generation {
                            let state = if status.ready {
                                "In game"
                            } else if status.phase == "ready" {
                                "Waiting for the game"
                            } else {
                                status.phase.as_str()
                            };
                            chip(ui, &format!("{state} {}/{}", status.served, status.staged), look.accent);
                        }
                    });
                }
            });
    }

    /// Effects that belong to no fighter: common, item and stage effects.
    /// Effect files of no fighter the project edits, by where they live in the game.
    fn other_effect_edits(&self) -> Vec<(&'static str, String)> {
        let fighters: BTreeSet<&str> = self
            .state
            .fighters
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        let mut edited: Vec<(&'static str, String)> = self
            .eff_mods
            .iter()
            // A made character's effect file is on its own card.
            .filter(|(key, eff)| {
                !fighters.contains(key.as_str()) && !key.contains('#') && !eff.is_empty()
            })
            .map(|(_, eff)| (effect_group(&eff.source_rel), eff.source_rel.clone()))
            .collect();
        edited.sort();
        edited
    }

    /// One tile per game-wide part of the project: effects of no fighter, the select screen,
    /// fighter values and the installed mods, each saying what the project changes there.
    fn draw_game_tiles(
        &mut self,
        ui: &mut Ui,
        look: Look,
        parts: &ProjectParts,
        actions: &mut Vec<PageAction>,
    ) {
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
        // Counted as the Fighter values page counts them.
        let value_count: usize = self
            .roster
            .params
            .keys()
            .map(|fighter| self.roster.fighter_value_count(fighter))
            .sum();
        let values = match value_count {
            0 => String::new(),
            n => format!("{} changed", counted(n, "value")),
        };
        // Installed mods only: a made character's folder is the project's own.
        let mods = match self.roster.library.enabled_roots().len() {
            0 => String::new(),
            1 => "1 turned on".to_owned(),
            n => format!("{n} turned on"),
        };
        let open = self
            .project_open_part
            .as_ref()
            .is_some_and(|(key, _)| key == OTHER_EFFECTS);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
            let tiles: [(Tile, &str, &str, bool); 4] = [
                (
                    Tile::Part(Part::Effects),
                    "Effects",
                    &effects,
                    !edited.is_empty() || picked,
                ),
                (
                    Tile::Part(Part::SelectScreen),
                    "Select screen",
                    &screen,
                    summary.order + summary.hidden + summary.names > 0,
                ),
                (
                    Tile::Part(Part::Values),
                    "Fighter values",
                    &values,
                    value_count > 0,
                ),
                (Tile::Mods, "Installed mods", &mods, false),
            ];
            // Four to a row when they fit at a readable width, else two, else one; together they
            // always fill the row.
            let gap = 10.0;
            let full = ui.available_width();
            let per_row = [4.0f32, 2.0, 1.0]
                .into_iter()
                .find(|n| (full - gap * (n - 1.0)) / n >= 230.0)
                .unwrap_or(1.0);
            let width = ((full - gap * (per_row - 1.0)) / per_row).floor() - 1.0;
            for (index, (icon, title, line, changed)) in tiles.into_iter().enumerate() {
                if game_tile(
                    ui,
                    width,
                    look,
                    icon,
                    title,
                    line,
                    changed,
                    index == 0 && open,
                )
                .clicked()
                {
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

    fn draw_other_effects(
        &mut self,
        ui: &mut Ui,
        look: Look,
        parts: &ProjectParts,
        actions: &mut Vec<PageAction>,
    ) {
        let edited = self.other_effect_edits();
        let picked = parts
            .link(OTHER_EFFECTS, Part::Effects, 0)
            .map(Path::to_path_buf);
        let colour = part_colour(Part::Effects);
        egui::Frame::new()
            .fill(look.card)
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.5, colour))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Effects of no fighter")
                        .size(19.0)
                        .strong()
                        .color(look.text),
                );
                ui.add_space(6.0);
                for group in EFFECT_GROUPS {
                    let files: Vec<&String> = edited
                        .iter()
                        .filter(|(g, _)| *g == group)
                        .map(|(_, rel)| rel)
                        .collect();
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
                        ui.label(
                            RichText::new(dir.display().to_string())
                                .monospace()
                                .color(look.text),
                        );
                    });
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if act(ui, look, "Open .eff", Some(colour)).clicked() {
                        actions.push(PageAction::OpenEffectFile);
                    }
                    if act(ui, look, "Choose folder", None)
                        .on_hover_text("Laid out like the game's effect folder")
                        .clicked()
                    {
                        actions.push(PageAction::PickFolder(
                            OTHER_EFFECTS.into(),
                            Part::Effects,
                            0,
                        ));
                    }
                    if picked.is_some() && act(ui, look, "Use game files", None).clicked() {
                        actions.push(PageAction::ForgetFolder(
                            OTHER_EFFECTS.into(),
                            Part::Effects,
                            0,
                        ));
                    }
                });
            });
    }

    fn fighter_index(&self, fighter: &str) -> Option<usize> {
        self.state
            .fighters
            .iter()
            .position(|entry| entry.name.eq_ignore_ascii_case(fighter))
    }

    /// Open `page` with `key`'s character active and its costume `costume` shown. Every page a
    /// card opens goes through here.
    fn go(&mut self, key: &RosterKey, costume: u8, page: CardPage) {
        use crate::character::{CharacterKey, CharacterPage};
        let character = CharacterKey::parse(key.as_str());
        let fighter = character.engine_fighter().to_owned();
        let page = match page {
            CardPage::Values => {
                self.roster.focus_traits_for(&fighter);
                return;
            }
            CardPage::SelectScreen => {
                let cell = if character.is_made() {
                    key.clone()
                } else {
                    let members: Vec<&str> = group_of(&fighter).map_or_else(
                        || vec![fighter.as_str()],
                        |(_, members)| members.iter().map(|(member, _)| *member).collect(),
                    );
                    self.roster
                        .select_screen_cell(&members)
                        .unwrap_or_else(|| key.clone())
                };
                self.roster.select_css(cell);
                return;
            }
            CardPage::ClassicCollisions => CharacterPage::ClassicCollisions,
            CardPage::ClassicEffects => CharacterPage::ClassicEffects,
            CardPage::ClassicAudio => CharacterPage::ClassicAudio,
            CardPage::MoveNodes => CharacterPage::MoveNodes,
            CardPage::Gimmicks => CharacterPage::Gimmicks,
            CardPage::Effects => CharacterPage::Effects,
        };
        self.open_character(character, Some(costume), page);
    }

    /// Take a made character out of the project, with every edit it owns.
    fn remove_character(&mut self, key: &RosterKey) {
        let Some(made) = self
            .roster
            .project
            .authored
            .iter()
            .find(|made| &made.key == key)
            .cloned()
        else {
            return;
        };
        let slots = made.all_slots();
        self.remove_character_edits(&crate::character::CharacterKey::made(key.clone()));
        let status = self.roster.handle_new_character(
            crate::roster::new_character::NewCharacterAction::Remove(key.clone()),
        );
        let donor = made.donor.clone();
        self.change_parts(|parts| {
            parts.characters.remove(key.as_str());
            parts.links.retain(|link| {
                !(link.fighter.eq_ignore_ascii_case(&donor)
                    && link.slot.is_some_and(|slot| slots.contains(&slot)))
            });
        });
        if self
            .project_open_part
            .as_ref()
            .is_some_and(|(id, _)| id == key.as_str())
        {
            self.project_open_part = None;
        }
        if let Some(status) = status {
            self.state.status = status;
        }
    }

    fn run_page_action(&mut self, ctx: &egui::Context, action: PageAction) {
        match action {
            PageAction::Go(key, slot, page) => self.go(&key, slot, page),
            PageAction::Open(id, part) => {
                let again = self.project_open_part.as_ref() == Some(&(id.clone(), part));
                self.project_open_part = (!again).then_some((id, part));
            }
            PageAction::PickSlot(key, fighter, slot) => {
                self.change_parts(|parts| {
                    parts.characters.insert(key, slot);
                });
                if self.fighter_index(&fighter) == self.state.selected_fighter {
                    self.workspace_asset_slot = slot;
                    self.show_skin(&fighter, slot);
                }
            }
            PageAction::Hide(fighter) => {
                let members: Vec<&str> = group_of(&fighter)
                    .map_or(vec![fighter.as_str()], |(_, members)| {
                        members.iter().map(|(member, _)| *member).collect()
                    });
                self.change_parts(|parts| {
                    for member in &members {
                        parts.characters.remove(*member);
                    }
                });
            }
            PageAction::Member(title, fighter) => {
                self.project_member.insert(title, fighter.clone());
                // The card stays on the page whichever of its fighters is shown.
                let slot = self
                    .fighter_index(&fighter)
                    .map_or(0, |index| self.state.fighters[index].base_slot());
                self.change_parts(|parts| {
                    parts.characters.entry(fighter.clone()).or_insert(slot);
                });
            }
            PageAction::Add(fighter) => {
                let slot = self
                    .fighter_index(&fighter)
                    .map_or(0, |index| self.state.fighters[index].base_slot());
                self.change_parts(|parts| {
                    parts.characters.insert(fighter.clone(), slot);
                });
                let id = group_of(&fighter).map_or(fighter, |(title, _)| title.to_owned());
                self.project_open_part = Some((id, Part::Moves));
            }
            PageAction::LinkMoveCode => {
                let Some(folder) = rfd::FileDialog::new()
                    .set_title("Your Smashline project")
                    .pick_folder()
                else {
                    return;
                };
                self.link_acmd_source(folder.clone());
                if self.acmd_src.as_ref().map(|index| index.root.as_path())
                    == Some(folder.as_path())
                {
                    self.change_parts(|parts| parts.set_link("", Part::Moves, 0, Some(folder)));
                }
            }
            PageAction::UnlinkMoveCode => {
                self.unlink_acmd_source();
                self.change_parts(|parts| parts.set_link("", Part::Moves, 0, None));
            }
            PageAction::PickFolder(fighter, part, slot) => {
                let Some(folder) = rfd::FileDialog::new()
                    .set_title(format!("Folder for {}", part.title().to_lowercase()))
                    .pick_folder()
                else {
                    return;
                };
                self.change_parts(|parts| parts.set_link(&fighter, part, slot, Some(folder)));
                if part == Part::Effects {
                    self.reopen_fighter_effects(&fighter);
                }
                forget_cached(ctx);
            }
            PageAction::ForgetFolder(fighter, part, slot) => {
                self.change_parts(|parts| parts.set_link(&fighter, part, slot, None));
                if part == Part::Effects {
                    self.reopen_fighter_effects(&fighter);
                }
                forget_cached(ctx);
            }
            PageAction::Reveal(dir) => {
                let _ = std::fs::create_dir_all(&dir);
                if let Err(error) = crate::roster::reveal::reveal(&dir) {
                    self.state.status = format!("Could not open {}: {error:#}", dir.display());
                }
            }
            PageAction::EditOtherEffect(rel) => {
                let path = self.resolve_eff_source(&rel);
                self.eff_editor.load_as(&path, rel);
                self.open_effects_page();
            }
            PageAction::OpenEffectFile => {
                let start = self.eff_editor.export_root().join("effect");
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Effect file", &["eff"])
                    .set_directory(start)
                    .pick_file()
                {
                    self.open_external_eff(path);
                    self.open_effects_page();
                }
            }
            PageAction::Remove(key) => self.remove_character(&key),
            PageAction::Files(card, what) => {
                self.run_file_action(&card, what);
                forget_cached(ctx);
            }
        }
    }

    /// Copy the base costume into `card`'s costume: for a made character, its base fighter's
    /// costume of the same colour; for a fighter, the game's own files into the project.
    /// Files already there are kept. Returns (copied, kept).
    fn copy_base_costume(
        &self,
        workspace: &Path,
        parts: &ProjectParts,
        card: &Card,
        part: Part,
    ) -> anyhow::Result<(usize, usize)> {
        let kind = if part == Part::Model {
            "model"
        } else {
            "motion"
        };
        let picked = parts
            .link(&card.fighter, part, card.slot)
            .map(Path::to_path_buf);
        if card.made {
            let root = self
                .roster
                .made_root(&card.key)
                .ok_or_else(|| anyhow::anyhow!("{} has no folder", card.name))?;
            // Colour k of the character started as the base fighter's costume k.
            let colour = card
                .slots
                .iter()
                .position(|own| *own == card.slot)
                .unwrap_or(0)
                % 8;
            let roots: Vec<PathBuf> = self
                .all_roots()
                .into_iter()
                .filter(|known| *known != root)
                .collect();
            return crate::project_hub::copy_base_costume(
                &roots,
                &card.fighter,
                kind,
                colour as u8,
                card.slot,
                &root,
                picked.as_deref(),
            );
        }
        let root = self
            .state
            .data_root
            .clone()
            .ok_or_else(|| anyhow::anyhow!("set the game folder in File first"))?;
        let only = root.join("fighter").join(&card.fighter).join(kind);
        let sources: Vec<PathBuf> =
            crate::project_hub::collect_base_character_files(&root, &card.fighter, card.slot)?
                .into_iter()
                .filter(|source| source.starts_with(&only))
                .collect();
        anyhow::ensure!(
            !sources.is_empty(),
            "the game has no {kind} files for {} c{:02}",
            card.name,
            card.slot
        );
        let picked: Vec<(PathBuf, PathBuf)> = picked
            .into_iter()
            .filter_map(|folder| Some((part.game_dir(&card.fighter, card.slot)?, folder)))
            .collect();
        crate::project_hub::copy_base_files(workspace, &root, &sources, &picked)
    }

    fn run_file_action(&mut self, card: &Card, what: FileAction) {
        let Some(workspace) = self.workspace() else {
            self.state.status = "Save the project first.".into();
            return;
        };
        let parts = self.project_parts();
        let (fighter, slot) = (card.fighter.as_str(), card.slot);
        // A group card names the fighter it shows: the files are that fighter's.
        let name = card
            .members
            .iter()
            .find(|(member, _)| member.eq_ignore_ascii_case(fighter))
            .map_or(card.name.as_str(), |(_, label)| *label);
        let shown = format!("{name} c{slot:02}");
        let result: anyhow::Result<Vec<PathBuf>> = match what {
            FileAction::CopyBase(part) => {
                self.state.status = match self.copy_base_costume(&workspace, &parts, card, part) {
                    Ok((0, kept)) => format!("{shown} already has all {kept} files."),
                    Ok((copied, 0)) => format!("Copied {} to {shown}.", counted(copied, "file")),
                    Ok((copied, kept)) => format!(
                        "Copied {} to {shown}. Kept {kept} already there.",
                        counted(copied, "file")
                    ),
                    Err(error) => format!("Could not copy the base costume: {error:#}"),
                };
                self.preview_skin = None;
                self.show_skin(fighter, slot);
                return;
            }
            FileAction::AddModelFolder => {
                let Some(source) = rfd::FileDialog::new()
                    .set_title("Choose model folder")
                    .pick_folder()
                else {
                    return;
                };
                crate::project_hub::import_model_folder(&workspace, fighter, slot, &source)
            }
            FileAction::AddModelFiles => {
                let Some(sources) = rfd::FileDialog::new()
                    .set_title("Choose model files")
                    .add_filter("Model files", crate::project_hub::MODEL_ASSET_EXTENSIONS)
                    .pick_files()
                else {
                    return;
                };
                crate::project_hub::import_model_files(&workspace, fighter, slot, &sources)
            }
            FileAction::AddAnimations => {
                let Some(sources) = rfd::FileDialog::new()
                    .set_title("Choose animations")
                    .add_filter("Animation", &["nuanmb", "bin"])
                    .pick_files()
                else {
                    return;
                };
                crate::project_hub::import_animation_files(&workspace, fighter, slot, &sources)
            }
            FileAction::AddSwing => {
                let Some(source) = rfd::FileDialog::new()
                    .set_title("Choose swing.prc")
                    .add_filter("Swing values", &["prc"])
                    .pick_file()
                else {
                    return;
                };
                crate::project_hub::import_swing_prc(&workspace, fighter, slot, &source)
                    .map(|path| vec![path])
            }
            FileAction::AddEffects | FileAction::AddSounds => {
                let (part, filter): (Part, &[&str]) = if what == FileAction::AddEffects {
                    (Part::Effects, &["eff"])
                } else {
                    (Part::Sounds, &["nus3audio", "nus3bank"])
                };
                let Some(sources) = rfd::FileDialog::new()
                    .add_filter(part.title(), filter)
                    .pick_files()
                else {
                    return;
                };
                let Some(dir) = parts.dir(&workspace, fighter, part, slot) else {
                    return;
                };
                let linked = parts.link(fighter, part, slot).is_some();
                let made = card.made.then_some((fighter, slot));
                sources
                    .iter()
                    .map(|source| copy_part_file(&dir, linked, part, source, made))
                    .collect()
            }
            FileAction::Send => {
                self.send_workspace_asset_bundle(&workspace, fighter, slot);
                return;
            }
            FileAction::Stop => {
                let generation = self.next_workspace_asset_generation();
                self.game_link
                    .send_asset_bundle(&crate::game_link::AssetBundleWire {
                        generation,
                        target: String::new(),
                        files: Vec::new(),
                    });
                self.state.status = "Stopped the model preview in the game.".into();
                return;
            }
        };
        self.state.status = match result {
            Ok(files) => format!("Added {} to {shown}.", counted(files.len(), "file")),
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
        let Some(rel) = self.eff_editor.loaded_rel() else {
            return;
        };
        let path = self.resolve_eff_source(&rel);
        let primary = self.eff_editor.export_root().join(&rel);
        if path == primary {
            self.effect_file_stamp = None;
            return;
        }
        let stamp = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok();
        let before = self.effect_file_stamp.replace((path.clone(), stamp));
        if before.as_ref().is_none_or(|(was, _)| *was != path)
            || before.as_ref().map(|(_, at)| *at) == Some(stamp)
        {
            return;
        }
        self.eff_editor.load_as(&path, rel.clone());
        // Whose file it is: a made character's costume file is its own.
        let owner = self.eff_owner(&rel);
        let staged = if self.eff_mods.get(&owner).is_some_and(|eff| !eff.is_empty()) {
            self.deploy_live_eff(&owner)
        } else {
            std::fs::read(&path).is_ok_and(|bytes| self.serve_live_eff(&owner, &rel, &bytes))
        };
        self.state.status = if staged {
            format!(
                "Reloaded {}. The game uses it from the next match.",
                folder_name(&path)
            )
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

/// How many files under `dir` have a name `wanted` accepts.
fn count_matching(dir: &Path, wanted: impl Fn(&str) -> bool) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if entry.path().is_dir() {
                stack.push(entry.path());
            } else if wanted(&entry.file_name().to_string_lossy().to_ascii_lowercase()) {
                count += 1;
            }
        }
    }
    count
}

/// Copy an effect or sound file into its part's folder. In the project's own copy of the game
/// folders a sound goes into its bank; a picked folder keeps whatever layout it has. A made
/// character's copy is named for its costume (`made` is its fighter and costume).
fn copy_part_file(
    dir: &Path,
    picked: bool,
    part: Part,
    source: &Path,
    made: Option<(&str, u8)>,
) -> anyhow::Result<PathBuf> {
    let name = source
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("not a file"))?
        .to_string_lossy()
        .into_owned();
    let name = match made {
        Some((fighter, slot)) => made_file_name(&name, fighter, slot),
        None => name,
    };
    let dir = if part == Part::Sounds && !picked {
        dir.join(crate::project_parts::sound_target(&name))
    } else {
        dir.to_path_buf()
    };
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(name);
    std::fs::copy(source, &out)?;
    Ok(out)
}

fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costumes_read_as_ranges() {
        assert_eq!(slot_ranges(&[0, 1, 2, 3, 4, 5, 6, 7]), "c00–c07");
        assert_eq!(slot_ranges(&[15, 8, 9, 10, 11, 12, 13, 14]), "c08–c15");
        assert_eq!(slot_ranges(&[0, 1, 2, 5, 9, 10]), "c00–c02, c05, c09–c10");
        assert_eq!(slot_ranges(&[3]), "c03");
        assert_eq!(slot_ranges(&[]), "");
        assert_eq!(slot_ranges(&[254, 255]), "c254–c255");
    }

    #[test]
    fn every_kind_of_edit_gives_its_fighter_a_card() {
        let one = |name: &str| vec![name.to_owned()];
        let families = EditFamilies {
            moves: one("mario"),
            graphs: one("luigi"),
            expressions: one("peach"),
            effect_calls: one("daisy"),
            node_effects: one("bowser"),
            sounds: one("yoshi"),
            effects: one("dk"),
            values: one("link"),
            gimmicks: one("samus"),
            controllers: one("kirby"),
            meters: one("fox"),
            live_rules: one("ness"),
            folders: one("pickel"),
        };
        let named = fighters_with_edits(&families);
        for fighter in [
            "mario", "luigi", "peach", "daisy", "bowser", "yoshi", "dk", "link", "samus", "kirby",
            "fox", "ness", "pickel",
        ] {
            assert!(named.contains(fighter), "{fighter} got no card");
        }
        assert_eq!(named.len(), 13);
        assert!(fighters_with_edits(&EditFamilies::default()).is_empty());
    }

    #[test]
    fn move_keys_name_their_character() {
        assert_eq!(
            owner_of_move_key("mario/attack11").as_deref(),
            Some("mario")
        );
        assert_eq!(
            owner_of_move_key("mario/attack11#hurt").as_deref(),
            Some("mario")
        );
        // A made character's edits are its own, never its base fighter's.
        assert_eq!(
            owner_of_move_key("mario#c08/attack11").as_deref(),
            Some("mario#c08")
        );
        assert_eq!(owner_of_move_key("attack11"), None);
    }

    fn facts() -> CardFacts {
        CardFacts {
            base_name: "Mario".into(),
            ..CardFacts::default()
        }
    }

    #[test]
    fn a_part_is_solid_only_when_the_project_changes_it_for_this_character() {
        let mut base = facts();
        let untouched = part_summary(Part::Moves, &base);
        assert!(!untouched.changed);
        assert!(untouched.chips.is_empty());
        assert_eq!(part_hover(Part::Moves, &untouched), "Moves");

        base.counts.moves = 4;
        base.counts.nodes = 2;
        let moves = part_summary(Part::Moves, &base);
        assert!(moves.changed);
        assert_eq!(
            part_hover(Part::Moves, &moves),
            "Moves: 4 edited, 2 with nodes"
        );

        // A costume another mod provides is not the project's change.
        base.model.from_mod = Some("cape".into());
        let model = part_summary(Part::Model, &base);
        assert!(!model.changed);
        assert_eq!(model.chips, vec!["Mod: cape".to_owned()]);
        base.model = FolderFacts {
            files: 12,
            picked: false,
            from_mod: None,
        };
        let model = part_summary(Part::Model, &base);
        assert!(model.changed);
        assert_eq!(model.chips, vec!["12 files".to_owned()]);
        let animations = part_summary(Part::Animations, &base);
        assert_eq!(animations.chips, vec!["Game".to_owned()]);
    }

    #[test]
    fn gimmicks_say_who_has_the_fighters_one_gimmick() {
        let mut vision = facts();
        vision.made = true;
        let free = part_summary(Part::Gimmicks, &vision);
        assert!(free.chips.is_empty() && !free.changed);
        vision.gimmick_holder = Some("Mario".into());
        let held = part_summary(Part::Gimmicks, &vision);
        assert_eq!(held.chips, ["Mario has the gimmick"]);
        // Another character's gimmick is not a change to this one.
        assert!(!held.changed);
    }

    #[test]
    fn values_say_who_shares_them() {
        let mut base = facts();
        base.counts.values = 12;
        base.sharing = vec!["Vision".into()];
        let values = part_summary(Part::Values, &base);
        assert!(values.changed);
        assert_eq!(
            values.chips,
            vec!["12 changed".to_owned(), "Also used by Vision".to_owned()]
        );
        // A made character's values are its base fighter's: never its own change.
        let mut made = facts();
        made.made = true;
        made.counts.values = 12;
        let values = part_summary(Part::Values, &made);
        assert!(!values.changed);
        assert_eq!(values.chips, vec!["Shared with Mario".to_owned()]);
    }

    #[test]
    fn effects_count_looks_borrowed_effects_and_moves() {
        let mut base = facts();
        base.counts.effects_changed = 3;
        base.counts.effects_borrowed = 1;
        base.counts.effect_moves = 5;
        let effects = part_summary(Part::Effects, &base);
        assert!(effects.changed);
        assert_eq!(effects.chips, vec!["3 changed", "1 borrowed", "5 moves"]);
        let mut sounds = facts();
        sounds.counts.sound_moves = 2;
        sounds.sound_files = 3;
        assert_eq!(
            part_summary(Part::Sounds, &sounds).chips,
            vec!["2 moves", "3 files"]
        );
        let mut made = facts();
        made.made = true;
        made.effect_file = true;
        let effects = part_summary(Part::Effects, &made);
        assert!(effects.changed);
        assert_eq!(effects.chips, vec!["Own file"]);
    }

    /// A card that exists because of an edit lights the part the edit is in: effect textures
    /// and a gimmick's own values gave a card with every button dark.
    #[test]
    fn textures_and_gimmick_values_light_their_part() {
        let mut base = facts();
        base.counts.textures = 2;
        let effects = part_summary(Part::Effects, &base);
        assert!(effects.changed);
        assert_eq!(effects.chips, vec!["2 textures"]);
        base.counts.gimmick_values = 3;
        let gimmicks = part_summary(Part::Gimmicks, &base);
        assert!(gimmicks.changed);
        assert_eq!(gimmicks.chips, vec!["3 values"]);
        // The fighter values file is counted on its own button, not here.
        assert!(!part_summary(Part::Values, &base).changed);
    }

    #[test]
    fn the_select_screen_says_what_changed() {
        let mut base = facts();
        assert!(!part_summary(Part::SelectScreen, &base).changed);
        base.select = SelectFacts {
            renamed: true,
            portraits: 2,
            moved: true,
            hidden: false,
        };
        let select = part_summary(Part::SelectScreen, &base);
        assert!(select.changed);
        assert_eq!(select.chips, vec!["Renamed", "2 portraits", "Moved"]);
        let mut made = facts();
        made.made = true;
        assert_eq!(
            part_summary(Part::SelectScreen, &made).chips,
            vec!["Own cell"]
        );
    }

    #[test]
    fn a_made_characters_files_are_named_for_its_costume() {
        assert_eq!(
            made_file_name("ef_mario.eff", "mario", 8),
            "ef_mario_c08.eff"
        );
        assert_eq!(
            made_file_name("ef_mario_c03.eff", "mario", 9),
            "ef_mario_c09.eff"
        );
        assert_eq!(
            made_file_name("se_mario_c00.nus3audio", "mario", 12),
            "se_mario_c12.nus3audio"
        );
        assert_eq!(
            made_file_name("vc_mario.nus3bank", "mario", 8),
            "vc_mario_c08.nus3bank"
        );
        // A file of no fighter keeps its name.
        assert_eq!(
            made_file_name("se_custom.nus3audio", "mario", 8),
            "se_custom.nus3audio"
        );
        // `_mario_cape` is not a costume number.
        assert_eq!(
            made_file_name("se_mario_cape.nus3audio", "mario", 8),
            "se_mario_cape.nus3audio"
        );
    }

    #[test]
    fn counts_with_nothing_are_no_edits() {
        assert!(!CardCounts::default().any());
        assert!(CardCounts {
            meters: 1,
            ..CardCounts::default()
        }
        .any());
    }
}
