// Eff editor: browse ef_*.eff game dumps, edit authored emitter values against a pristine
// snapshot, and preview the edit live in game through the slight_replica plugin.
//
// HOW AUTHORED EDITS REACH THE GAME
// Authored PTCL values are PER EMITTER. The runtime modifier protocol is not: the plugin's
// `rainbow.color` / `scale` are pinned on an `EffectData` for a whole effect KIND, so there
// is no wire message that can say "tint only emitter 3". Deriving a multiplier from the
// authored delta therefore cannot be made correct — the old code averaged every emitter's
// colour ratio into one kind-level value, which recoloured the ENTIRE effect whenever you
// edited a single emitter (and silently ignored color1 edits entirely).
//
// So authored edits do NOT go on the modifier wire at all. They are baked into the LIVE
// CARRIER: the edited effect is cloned into a borrowed assist eff, the edits are written into
// that clone (per-emitter exact, the same writer the exporter uses), and the original kind is
// aliased onto it. The fighter's own eff is not a candidate — `reparse_game_path` rebuilds the
// parsed structs from the resident buffer and never re-requests the file, so edited bytes went
// unread mid-match. The editor only raises a request flag; the app owns the rebuild.
//
// Transplants, per-emitter texture swaps and whole-texture PNG imports all ride that same
// rebuild, so one Send ships everything the project says.
//
// The kind-level multipliers are still exposed, but only as their own honest feature: the
// "Kind look — color × / speed ×" panel, which the user drives directly and which is
// documented as applying to every spawn of the effect.

use std::path::{Path, PathBuf};
use std::time::Instant;

use egui::Ui;

use crate::eff_attrs::{AttrKind, AttrValue};
use crate::effects::{
    load_effect, ColorKey, EffEntryInfo, EffModelInfo, EffVariantInfo, EmitterDef, PtclFile,
    TextureInfo,
};
use crate::game_link::GameLink;
use crate::mod_project::{AuthoredEdit, EmitterFieldEdits, TransplantOp};

/// A move that plays an effect, as the Effects page lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct MoveUse {
    /// The move in plain words, with the object that plays the effect for it if one does:
    /// "Neutral special (fireball)".
    pub label: String,
    /// The move's motion name, to open it; `None` when it is not in the fighter's move list.
    pub motion: Option<String>,
    /// The bones the move spawns the effect on, as its scripts name them.
    pub bones: Vec<String>,
}

/// How the Effects page's view plays the selected effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectView {
    /// By itself at the centre of the floor, looping, with the fighter hidden.
    Alone,
    /// In the open move, as the game plays it.
    InMove,
}

/// Everything that decides what the texture thumbnail should show: the eff, the pool index, the
/// replacement PNG's path and modification time, and whether raw form is selected.
///
/// Two parts of this are easy to miss. The mtime: exporting to `star.png`, painting it, and
/// importing `star.png` again leaves the path identical, so without it the panel would keep
/// showing the previous import. And the pool generation, because adding or removing a texture
/// changes what an INDEX means without changing the eff or the file on disk.
type TexturePreviewKey = (PathBuf, usize, String, bool, u128, u64);

/// Pristine copy of the editable authored fields of one emitter.
#[derive(Clone)]
struct EmitterSnapshot {
    /// Every attribute as loaded, aligned to [`crate::eff_attrs::table`].
    attrs: Vec<Option<AttrValue>>,
    subsections: Vec<crate::effects::EmitterSubsectionDef>,
    /// Name and nesting as loaded. Not restored by [`Self::restore`], which resets an emitter's
    /// VALUES — where an emitter sits in the list is the emitter-list editor's business, and a
    /// duplicate keeps its own name through a value reset.
    name: String,
    depth: u8,
    color0: Vec<ColorKey>,
    color1: Vec<ColorKey>,
    alpha0_keys: Vec<ColorKey>,
    texture_index: Option<u32>,
}

impl EmitterSnapshot {
    fn of(e: &EmitterDef) -> Self {
        Self {
            attrs: e.attrs.clone(),
            subsections: e.subsections.clone(),
            name: e.name.clone(),
            depth: e.depth,
            color0: e.color0.clone(),
            color1: e.color1.clone(),
            alpha0_keys: e.alpha0_keys.clone(),
            texture_index: e.texture_index,
        }
    }

    fn restore(&self, e: &mut EmitterDef) {
        e.attrs = self.attrs.clone();
        e.subsections = self.subsections.clone();
        e.color0 = self.color0.clone();
        e.color1 = self.color1.clone();
        e.alpha0_keys = self.alpha0_keys.clone();
        e.texture_index = self.texture_index;
    }

    /// The emitter this snapshot was taken of, as the file has it.
    ///
    /// Lets an emitter list be rebuilt from the pristine snapshots rather than from whatever is
    /// currently on screen, which is what makes re-applying a saved list idempotent — applying it
    /// twice would otherwise duplicate the duplicates.
    fn to_def(&self, source_idx: usize) -> EmitterDef {
        EmitterDef {
            name: self.name.clone(),
            attrs: self.attrs.clone(),
            subsections: self.subsections.clone(),
            depth: self.depth,
            source_idx,
            color0: self.color0.clone(),
            color1: self.color1.clone(),
            alpha0_keys: self.alpha0_keys.clone(),
            texture_index: self.texture_index,
        }
    }
}

/// Read one attribute off an emitter by id, for the handful of places that want a specific
/// attribute rather than the whole table (the Basics rows, the spawn summary).
fn attr(e: &EmitterDef, id: &str) -> Option<AttrValue> {
    crate::eff_attrs::index_of(id).and_then(|i| e.attrs.get(i).copied().flatten())
}

fn attr_f32(e: &EmitterDef, id: &str) -> f32 {
    attr(e, id).map(|v| v.as_f32()).unwrap_or(0.0)
}

/// Write one attribute by id, leaving an emitter that has no such block untouched.
fn set_attr(e: &mut EmitterDef, id: &str, value: AttrValue) {
    if let Some(i) = crate::eff_attrs::index_of(id) {
        if let Some(slot) = e.attrs.get_mut(i) {
            if slot.is_some() {
                *slot = Some(value);
            }
        }
    }
}

fn set_key_component(
    emitter: &mut EmitterDef,
    table: &str,
    index: usize,
    component: &str,
    value: f32,
) {
    set_attr(
        emitter,
        &format!("emitter_static.{table}.keys[{index}].{component}"),
        AttrValue::Float(value),
    );
}

fn key_component(emitter: &EmitterDef, table: &str, index: usize, component: &str) -> Option<f32> {
    attr(
        emitter,
        &format!("emitter_static.{table}.keys[{index}].{component}"),
    )
    .map(AttrValue::as_f32)
}

/// Keep the colour/keyframe convenience controls and the complete attribute table on the same
/// underlying values. Which scalar is effective follows the same precedence as the exporter.
fn effective_keys_to_attrs(emitter: &mut EmitterDef) {
    let color0_animated = attr(emitter, "emitter_static.num_color0_keys")
        .is_some_and(|v| v.as_i64() > 0)
        && attr(emitter, "particle_color.color0_type").is_some_and(|v| v.as_i64() != 0);
    let color1_animated = attr(emitter, "emitter_static.num_color1_keys")
        .is_some_and(|v| v.as_i64() > 0)
        && attr(emitter, "particle_color.color1_type").is_some_and(|v| v.as_i64() != 0);
    let alpha0_animated =
        attr(emitter, "emitter_static.num_alpha0_keys").is_some_and(|v| v.as_i64() > 0);

    let color0 = emitter.color0.clone();
    for (i, key) in color0.iter().enumerate() {
        if color0_animated {
            for (component, value) in [
                ("x", key.r),
                ("y", key.g),
                ("z", key.b),
                ("time", key.frame),
            ] {
                set_key_component(emitter, "color0", i, component, value);
            }
        } else if i == 0 {
            let prefix =
                if attr(emitter, "particle_color.color0_type").is_some_and(|v| v.as_i64() == 0) {
                    "particle_color.color0_"
                } else {
                    "emitter_info.color0_"
                };
            for (suffix, value) in [("r", key.r), ("g", key.g), ("b", key.b)] {
                set_attr(
                    emitter,
                    &format!("{prefix}{suffix}"),
                    AttrValue::Float(value),
                );
            }
        }
    }

    let color1 = emitter.color1.clone();
    for (i, key) in color1.iter().enumerate() {
        if color1_animated {
            for (component, value) in [
                ("x", key.r),
                ("y", key.g),
                ("z", key.b),
                ("time", key.frame),
            ] {
                set_key_component(emitter, "color1", i, component, value);
            }
        } else if i == 0 {
            let prefix =
                if attr(emitter, "particle_color.color1_type").is_some_and(|v| v.as_i64() == 0) {
                    "particle_color.color1_"
                } else {
                    "emitter_info.color1_"
                };
            for (suffix, value) in [("r", key.r), ("g", key.g), ("b", key.b)] {
                set_attr(
                    emitter,
                    &format!("{prefix}{suffix}"),
                    AttrValue::Float(value),
                );
            }
        }
    }

    let alpha0 = emitter.alpha0_keys.clone();
    for (i, key) in alpha0.iter().enumerate() {
        if alpha0_animated {
            set_key_component(emitter, "alpha0", i, "x", key.r);
            set_key_component(emitter, "alpha0", i, "time", key.frame);
        } else if i == 0 {
            let id = if attr(emitter, "particle_color.alpha0_type").is_some_and(|v| v.as_i64() == 0)
            {
                "particle_color.alpha0"
            } else {
                "emitter_info.color0_a"
            };
            set_attr(emitter, id, AttrValue::Float(key.r));
        }
    }
}

fn attrs_to_effective_keys(emitter: &mut EmitterDef) {
    let color0_animated = attr(emitter, "emitter_static.num_color0_keys")
        .is_some_and(|v| v.as_i64() > 0)
        && attr(emitter, "particle_color.color0_type").is_some_and(|v| v.as_i64() != 0);
    let color1_animated = attr(emitter, "emitter_static.num_color1_keys")
        .is_some_and(|v| v.as_i64() > 0)
        && attr(emitter, "particle_color.color1_type").is_some_and(|v| v.as_i64() != 0);
    let alpha0_animated =
        attr(emitter, "emitter_static.num_alpha0_keys").is_some_and(|v| v.as_i64() > 0);

    if color0_animated {
        for i in 0..emitter.color0.len() {
            if let Some(value) = key_component(emitter, "color0", i, "x") {
                emitter.color0[i].r = value;
            }
            if let Some(value) = key_component(emitter, "color0", i, "y") {
                emitter.color0[i].g = value;
            }
            if let Some(value) = key_component(emitter, "color0", i, "z") {
                emitter.color0[i].b = value;
            }
            if let Some(value) = key_component(emitter, "color0", i, "time") {
                emitter.color0[i].frame = value;
            }
        }
    } else {
        let prefix = if attr(emitter, "particle_color.color0_type").is_some_and(|v| v.as_i64() == 0)
        {
            "particle_color.color0_"
        } else {
            "emitter_info.color0_"
        };
        let rgb = [
            attr_f32(emitter, &format!("{prefix}r")),
            attr_f32(emitter, &format!("{prefix}g")),
            attr_f32(emitter, &format!("{prefix}b")),
        ];
        if let Some(key) = emitter.color0.first_mut() {
            [key.r, key.g, key.b] = rgb;
        }
    }
    if color1_animated {
        for i in 0..emitter.color1.len() {
            if let Some(value) = key_component(emitter, "color1", i, "x") {
                emitter.color1[i].r = value;
            }
            if let Some(value) = key_component(emitter, "color1", i, "y") {
                emitter.color1[i].g = value;
            }
            if let Some(value) = key_component(emitter, "color1", i, "z") {
                emitter.color1[i].b = value;
            }
            if let Some(value) = key_component(emitter, "color1", i, "time") {
                emitter.color1[i].frame = value;
            }
        }
    } else {
        let prefix = if attr(emitter, "particle_color.color1_type").is_some_and(|v| v.as_i64() == 0)
        {
            "particle_color.color1_"
        } else {
            "emitter_info.color1_"
        };
        let rgb = [
            attr_f32(emitter, &format!("{prefix}r")),
            attr_f32(emitter, &format!("{prefix}g")),
            attr_f32(emitter, &format!("{prefix}b")),
        ];
        if let Some(key) = emitter.color1.first_mut() {
            [key.r, key.g, key.b] = rgb;
        }
    }
    if alpha0_animated {
        for i in 0..emitter.alpha0_keys.len() {
            if let Some(value) = key_component(emitter, "alpha0", i, "x") {
                emitter.alpha0_keys[i].r = value;
            }
            if let Some(value) = key_component(emitter, "alpha0", i, "time") {
                emitter.alpha0_keys[i].frame = value;
            }
        }
    } else {
        let id = if attr(emitter, "particle_color.alpha0_type").is_some_and(|v| v.as_i64() == 0) {
            "particle_color.alpha0"
        } else {
            "emitter_info.color0_a"
        };
        let value = attr_f32(emitter, id);
        if let Some(key) = emitter.alpha0_keys.first_mut() {
            key.r = value;
        }
    }
}

/// Which view the emitter column is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EmitterTab {
    /// The selected emitter's own values.
    Attributes,
    /// What this effect brings on screen at all: which emitters play, which extra parts start
    /// when, and which model comes with it.
    Spawning,
}

/// One named effect entry inside the loaded .eff (name → emitter set).
struct EffEntry {
    name: String,
    /// hash40 of the name — the plugin's kind-tab id.
    hash: u64,
    set_idx: usize,
}

/// One edited fighter (from the app's project state) — the primary unit the editor's
/// selector is organized around. `merged` is the transplants-applied build to parse when
/// present; `alt_base` covers the data-root path when it differs from the export root.
#[derive(Clone, PartialEq)]
pub struct EditSource {
    /// The key the project keeps these edits under: a fighter, or a made character's key.
    pub fighter: String,
    /// The name it is shown by.
    pub label: String,
    pub base: PathBuf,
    pub alt_base: Option<PathBuf>,
    pub merged: Option<PathBuf>,
    pub transplant_count: usize,
    /// How many effects differ from the file, by [`crate::mod_project::EffMod::changed_effects`].
    pub changed_effects: usize,
    /// How many of this eff's pool textures the user has replaced with their own image.
    pub textures: usize,
    pub transplants: Vec<EffTransplant>,
}

/// Project metadata for one entry shown in the merged EFF view.
#[derive(Clone, PartialEq)]
pub struct EffTransplant {
    pub op_index: usize,
    /// The entry visible in the editor (the new name, or replacement target).
    pub entry_name: String,
    pub donor_name: String,
    pub donor_file: String,
    pub one_slot_slots: Vec<u8>,
}

#[derive(Clone)]
pub struct EffTransplantRemoval {
    pub fighter: String,
    pub op_index: usize,
    pub entry_name: String,
    pub donor_name: String,
}

/// One step of the Effects page's history: one effect's parts, and every effect's timing, as
/// they were before an edit.
#[derive(Clone)]
struct EffHistoryStep {
    /// What the step changed, for the Undo and Redo hovers: "Brightness in Afterburner line".
    label: String,
    set_idx: usize,
    selected_entry: Option<usize>,
    selected_emitter: usize,
    emitters: Vec<EmitterDef>,
    entry_info: Vec<EffEntryInfo>,
}

/// Undo and redo for the open file's effect edits.
///
/// An edit is drawn before anyone can know it is coming, so the state before it is kept as
/// `baseline`: the selected effect as of the last step. Edits made close together (one drag,
/// a few keystrokes) become one step, closed once the pointer is up and the edits stop.
#[derive(Default)]
struct EffHistory {
    baseline: Option<EffHistoryStep>,
    /// When the last edit of the open step was made; None when no step is open.
    open_since: Option<Instant>,
    /// `edit_rev` as last seen by the history, so changes made elsewhere (a load, a project
    /// applying its edits) refresh the baseline instead of becoming a step.
    seen_rev: u64,
    undo: Vec<EffHistoryStep>,
    redo: Vec<EffHistoryStep>,
    /// The name the open step gets when it closes, for edits better named by their button than
    /// by the first value they changed (a reset changes them all).
    next_label: Option<String>,
    /// The row the open step's edits were made on, when it drives values under other names
    /// (Speed scales two speeds): the step is named after it, on the part it changed.
    touched: Option<String>,
    /// Why the steps were dropped, until the next step: the Undo hover says so.
    cleared: Option<&'static str>,
    /// The part open as this frame began: a step shows the part its first edit was made on,
    /// not the one open when the step before it closed.
    frame_part: usize,
}

/// The most files whose history is kept while another is open.
const HISTORY_FILES: usize = 8;

/// How long the edits of one step may pause before the step closes.
const HISTORY_STEP_GAP: std::time::Duration = std::time::Duration::from_millis(500);
/// The most steps kept.
const HISTORY_LIMIT: usize = 100;

pub struct EffEditor {
    pub open: bool,
    pub transplant_requested: bool,
    /// Eff path queued by the main editor (fighter selection) — loaded when the window is
    /// (or becomes) open, so closed-window fighter browsing stays cheap.
    pending_load: Option<PathBuf>,
    /// Entry name (lowercase) to select after the next load — transplant hand-off.
    pending_select: Option<String>,
    /// Authored edits applied once the queued eff loads.
    pending_edits: Option<Vec<AuthoredEdit>>,
    /// The open file's working copy is from before the project in the app now: a project was
    /// opened or closed since it loaded. Its diff is not the project's until a load gives it
    /// the project's edits, so nothing may be read back from it until then.
    working_stale: bool,
    /// Emitter lists and spawn structures to apply once the queued eff loads. Applied BEFORE
    /// [`Self::pending_edits`], for the same reason the exporter applies them first: they decide
    /// which emitters an authored edit has to land on.
    pending_structure: Option<(
        Vec<crate::mod_project::EmitterRoster>,
        Vec<crate::mod_project::EntryEdit>,
    )>,
    /// Whether applying [`Self::pending_edits`] should also push them to the running game.
    ///
    /// True when a project was just opened — the point is to get the game showing it. False
    /// when the reload is one we caused ourselves (a transplant rebuild): the edits are being
    /// carried across, not newly requested, and the user has not asked to send anything.
    pending_edits_push_live: bool,
    export_root: PathBuf,
    eff_files: Vec<PathBuf>,
    file_filter: String,
    scan_error: Option<String>,

    loaded_path: Option<PathBuf>,
    /// Game paths of files opened from outside the game folder. See [`Self::load_as`].
    game_paths: std::collections::HashMap<PathBuf, String>,
    /// Base eff path → merged (transplants applied) file to ACTUALLY parse when that base
    /// is opened. Makes the merged view character-centric: picking ef_kirby.eff shows
    /// kirby WITH its transplants, wherever it's opened from.
    merged_overlays: std::collections::HashMap<PathBuf, PathBuf>,
    /// The currently loaded view came from a merged overlay (shown in the header).
    loaded_is_merged: bool,
    /// The project's edited fighters — the PRIMARY things this window edits. Fed by the
    /// app every frame; drives the "Editing:" selector and the overlay map. Base game
    /// files are only a reference source below it.
    edit_sources: Vec<EditSource>,
    load_error: Option<String>,
    ptcl: Option<PtclFile>,
    /// The loaded eff's BNTX pool, verbatim — what "Export PNG" decodes from. `ptcl` carries
    /// only what each texture IS (name, size, format), never its pixels.
    texture_pool: Option<Vec<u8>>,
    /// Texture replacements the user picked, drained by the app into the project store.
    pending_texture_imports: Vec<crate::mod_project::TextureImport>,
    /// Pool textures the user added, drained by the app into the project store.
    pending_texture_additions: Vec<crate::mod_project::TextureAddition>,
    /// Names of pool textures the user removed, drained by the app.
    pending_texture_removals: Vec<String>,
    /// Bumped whenever `texture_pool` is rebuilt in place. Part of the preview cache key: adding
    /// or removing a texture changes what an index means without changing the path or the file.
    texture_pool_gen: u64,
    /// The project's texture replacements for the loaded eff, fed by the app each frame so
    /// the texture panel can show what is already replaced.
    texture_imports: Vec<crate::mod_project::TextureImport>,
    /// Result of the last export/import, shown beside the buttons. Errors here are the whole
    /// point: an unconvertible format or an unreadable PNG has to say so, not do nothing.
    texture_note: Option<(String, bool)>,
    /// Pool index + info of the texture the selected emitter samples, published by the emitter
    /// column for the texture panel on the right.
    selected_texture: Option<(usize, TextureInfo)>,
    /// The texture slots the selected part samples a pool texture through, as (slot, pool
    /// index), and the slot the Texture section shows.
    texture_slots: Vec<(usize, usize)>,
    texture_slot: usize,
    /// Decoded thumbnail for the selected texture. `None` in the second slot means "tried and
    /// could not" — kept so a failing texture is not re-decoded every frame.
    texture_preview: Option<(TexturePreviewKey, Option<egui::TextureHandle>)>,
    /// The replacement PNG's modified time, with the path and when it was read: read again at
    /// most once a second, not on every frame the Look tab is open.
    replacement_stamp: Option<(String, Instant, u128)>,
    /// Small pictures of the file's textures for the texture picker.
    texture_thumbs: TextureThumbs,
    /// Show textures as the stored channels rather than in editable form.
    show_raw_textures: bool,
    /// Plain-English note about the selected texture's layout, refreshed with the thumbnail.
    /// Cached because working it out means decoding the texture, which must not happen per frame.
    texture_form_note: String,
    /// Pristine snapshots per emitter set, parallel to `ptcl.emitter_sets`.
    pristine: Vec<Vec<EmitterSnapshot>>,
    entries: Vec<EffEntry>,
    /// The moves that spawn each effect (hash40 of its name), and whether every move has been
    /// read yet.
    effect_uses: std::collections::HashMap<u64, Vec<MoveUse>>,
    effect_uses_ready: bool,
    /// The character whose moves were read, when the open file is not that character's own
    /// (the shared file): only its moves were, so "no move" would be a claim about all of them.
    effect_uses_of: Option<String>,
    /// How the view plays the selected effect.
    pub view: EffectView,
    /// The part last picked in each effect, by effect name.
    parts_picked: std::collections::HashMap<String, usize>,
    /// The effect last open in each file, to open again when the file is.
    entry_picked: std::collections::HashMap<PathBuf, String>,
    /// A move picked in the view's switch, for the app to open.
    pub open_move_request: Option<String>,
    /// The characters the header's picker offers, as (folder name, display name).
    pub fighter_choices: Vec<(String, String)>,
    /// The open character's folder name.
    pub open_fighter: Option<String>,
    /// The open character's moves, by motion name.
    pub move_choices: Vec<String>,
    /// A character picked in the header, for the app to open.
    pub fighter_request: Option<String>,
    fighter_filter: String,
    /// The texture picker's search.
    texture_filter: String,
    move_filter: String,
    /// List only the effects the open move plays.
    only_open_move: bool,
    /// The user turned that off: picking a move leaves it off until they turn it on.
    only_open_move_declined: bool,
    /// A move just picked in the header: once it is open, show one of its effects in it.
    show_in_picked_move: Option<String>,
    /// A file just loaded with nothing to select: open the open move's effect once.
    open_first_effect: bool,
    /// A game has been connected in this session. Before one has, every edit is unsent and
    /// saying so tells the user nothing.
    game_seen: bool,
    /// The open move's motion name, to know whether it plays the selected effect.
    open_motion: Option<String>,
    entry_filter: String,
    selected_entry: Option<usize>,
    selected_emitter: usize,
    /// Draw only the selected emitter in the view, to see which part of the look it is: the
    /// effect it was turned on in. Opening another effect turns it off, as a view showing one
    /// part of an effect just opened looked empty.
    solo_selected: Option<String>,
    /// The whole-effect controls of the open effect.
    whole_effect: Option<WholeEffect>,
    /// The open move's effects, by name (lowercase), each with the first frame it spawns on.
    open_move_effects: Vec<(String, u32)>,
    /// An effect picked in the list that lives in another file, for the app to open.
    pub open_effect_request: Option<String>,
    /// The move open in the main window, by its plain name, for "Use in <move>".
    open_move: Option<String>,
    /// Set when "Use in <move>" is clicked; the app adds the effect to the move.
    pub use_in_move_requested: bool,
    /// What the last "Use in <move>" added, shown beside the button with an Undo, until another
    /// effect or move opens.
    pub use_in_move_note: Option<String>,
    /// Set when that Undo is clicked; the app takes the added call out again.
    pub use_in_move_undo_requested: bool,
    /// Set when "Open in Classic editor" is clicked beside that note.
    pub open_classic_requested: bool,
    /// Substring filter over the attribute tree in the emitter panel.
    attr_filter: String,
    /// Which half of the emitter column is showing: the selected emitter's attributes, or the
    /// entry's spawn structure (its emitter list, its parts, its model).
    emitter_tab: EmitterTab,
    /// Spawn structure of the loaded eff's entries, as edited. Parallel to `entries` by name.
    entry_info: Vec<EffEntryInfo>,
    /// The same, as loaded — what an edit is diffed against.
    entry_pristine: Vec<EffEntryInfo>,

    // Transplant state
    /// The main editor's selected fighter — target for transplant ops (set by the app).
    target_fighter: Option<String>,
    /// Recorded transplant ops, drained by the app into the project store.
    pending_transplants: Vec<TransplantOp>,
    /// Entry-level removals requested from the EFF editor, drained by the app so it can
    /// rebuild project state, live aliases, and the runtime carrier atomically.
    pending_transplant_removals: Vec<EffTransplantRemoval>,

    // Live-preview state
    eff_dirty_at: Option<Instant>,
    /// Bumped on every edit and every load, so the 3D preview knows to rebuild what it draws.
    edit_rev: u64,
    /// Bring the selected effect into view in the list (it was picked from elsewhere).
    scroll_to_selected: bool,
    /// Raised when the authored edits need to reach the running game. The app drains it
    /// (`take_live_deploy_request`) and services it by rebuilding this fighter's eff and
    /// hot-reloading it — the only per-emitter-exact path there is.
    live_deploy_request: bool,
    /// When the app last serviced a deploy request — drives `REDEPLOY_MIN_GAP_MS`.
    live_deploy_at: Option<Instant>,
    /// True from clicking Send until the app has finished building + sending the carrier.
    /// Drives the spinner beside the button.
    pub sending: bool,
    /// Carrier generation that was live in game when the current send started. The send is
    /// complete only once the game reports a NEWER one — see `awaiting_game`.
    pub gen_at_send: u64,
    /// Whether the snapshot being sent contains anything that needs a live carrier object.
    /// An empty snapshot is a teardown request, whose successful terminal state is the exact
    /// opposite: no kinds and no object.
    pub carrier_expected: bool,
    /// Carrier-status sequence at send time. Empty snapshots do not perform a new disk read,
    /// so their generation intentionally does not advance; a fresh idle report acknowledges
    /// that the teardown reached the game.
    pub carrier_reports_at_send: u64,
    /// Set once the snapshot has left the app: we are now waiting for the GAME to take it
    /// (bytes decompress, resource service settles, carrier object comes up). Cleared when
    /// the plugin reports the carrier live, or on timeout.
    pub awaiting_game: Option<Instant>,
    /// Last carrier report from the plugin, shown while waiting: (state, kinds, spawned).
    /// Surfaced rather than hidden because "the spinner cleared too early" has been
    /// diagnosed by guesswork twice now, and the raw signal settles it.
    pub carrier_report: (u8, usize, bool),
    /// The carrier state last seen, and when it was first seen. A swap that is progressing moves
    /// through its states; one that is wedged sits in a single state forever. Without this the
    /// only distinction available was "reported anything at all", which cannot tell a teardown
    /// still running from one that has stopped moving — so a stuck retire waited indefinitely.
    pub carrier_state_since: Option<(u8, Instant)>,
    /// Carrier generation the game last reported. Compared against `gen_at_send` to tell a
    /// freshly-taken carrier from the previous one still sitting there reporting ready.
    pub carrier_gen_now: u64,
    /// Final carrier reading from the last send, kept on screen after the spinner clears.
    pub last_carrier_result: Option<String>,
    /// The game's own numbers behind that reading, for its hover.
    pub last_carrier_detail: Option<String>,
    /// Whether the send in flight must wait for the game to read new bytes. A send of the
    /// same bytes leaves the live edit in place, and the game never reads them again.
    pub carrier_needs_new_bytes: bool,
    /// Whether that final reading was a success — colours the persisted readout so a failed
    /// send is not a grey line that reads like a status message.
    pub carrier_ok: bool,
    /// The carrier rebuild is synchronous on the UI thread, so servicing the request in the
    /// same frame as the click would block before the spinner ever painted. Hold the request
    /// for one frame so the "sending" state is visible.
    deploy_defer: bool,
    last_sent_note: Option<String>,
    /// Undo and redo for effect edits on this page.
    history: EffHistory,
    /// The history of each file opened before, with the file's fingerprint when it was put
    /// aside (see [`Self::file_fingerprint`]).
    history_store: Vec<(PathBuf, u64, EffHistory)>,
}

impl Default for EffEditor {
    fn default() -> Self {
        Self {
            open: false,
            transplant_requested: false,
            pending_load: None,
            pending_select: None,
            pending_edits: None,
            working_stale: false,
            pending_structure: None,
            pending_edits_push_live: false,
            export_root: std::env::current_dir().unwrap_or_default(),
            eff_files: Vec::new(),
            file_filter: String::new(),
            scan_error: None,
            loaded_path: None,
            game_paths: Default::default(),
            merged_overlays: std::collections::HashMap::new(),
            loaded_is_merged: false,
            edit_sources: Vec::new(),
            load_error: None,
            ptcl: None,
            texture_pool: None,
            pending_texture_imports: Vec::new(),
            pending_texture_additions: Vec::new(),
            pending_texture_removals: Vec::new(),
            texture_pool_gen: 0,
            texture_imports: Vec::new(),
            texture_note: None,
            selected_texture: None,
            texture_slots: Vec::new(),
            texture_slot: 0,
            texture_preview: None,
            replacement_stamp: None,
            texture_thumbs: TextureThumbs::default(),
            show_raw_textures: false,
            texture_form_note: String::new(),
            pristine: Vec::new(),
            entries: Vec::new(),
            effect_uses: Default::default(),
            effect_uses_ready: false,
            effect_uses_of: None,
            view: EffectView::Alone,
            parts_picked: Default::default(),
            entry_picked: Default::default(),
            open_move_request: None,
            fighter_choices: Vec::new(),
            open_fighter: None,
            move_choices: Vec::new(),
            fighter_request: None,
            fighter_filter: String::new(),
            texture_filter: String::new(),
            move_filter: String::new(),
            only_open_move: false,
            only_open_move_declined: false,
            show_in_picked_move: None,
            open_first_effect: false,
            game_seen: false,
            open_motion: None,
            entry_filter: String::new(),
            selected_entry: None,
            selected_emitter: 0,
            solo_selected: None,
            whole_effect: None,
            open_move_effects: Vec::new(),
            open_effect_request: None,
            open_move: None,
            use_in_move_requested: false,
            use_in_move_note: None,
            use_in_move_undo_requested: false,
            open_classic_requested: false,
            attr_filter: String::new(),
            emitter_tab: EmitterTab::Attributes,
            entry_info: Vec::new(),
            entry_pristine: Vec::new(),
            target_fighter: None,
            pending_transplants: Vec::new(),
            pending_transplant_removals: Vec::new(),
            eff_dirty_at: None,
            edit_rev: 0,
            scroll_to_selected: false,
            live_deploy_request: false,
            live_deploy_at: None,
            sending: false,
            gen_at_send: 0,
            carrier_expected: false,
            carrier_reports_at_send: 0,
            awaiting_game: None,
            carrier_report: (0, 0, false),
            carrier_state_since: None,
            carrier_gen_now: 0,
            last_carrier_result: None,
            last_carrier_detail: None,
            carrier_needs_new_bytes: true,
            carrier_ok: false,
            deploy_defer: false,
            last_sent_note: None,
            history: EffHistory::default(),
            history_store: Vec::new(),
        }
    }
}

fn scan_eff_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_eff_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("eff") {
            // Transient transplant preview files aren't real sources — hide them.
            let is_preview = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(crate::scratch_dirs::is_transplant_preview_name)
                .unwrap_or(false);
            if !is_preview {
                out.push(path);
            }
        }
    }
}

fn diff(a: f32, b: f32) -> bool {
    (a - b).abs() > 1e-3
}

/// Whether a set's part list is the file's: the same parts, in the same order and nesting, under
/// the same names. A set that fails this is what a roster records.
fn part_list_untouched(emitters: &[EmitterDef], pristine: &[EmitterSnapshot]) -> bool {
    emitters.len() == pristine.len()
        && emitters.iter().enumerate().all(|(i, em)| {
            em.source_idx == i
                && pristine
                    .get(i)
                    .is_some_and(|p| p.depth == em.depth && p.name == em.name)
        })
}

/// How wide a row of small buttons with these labels is.
fn small_buttons_width(ui: &Ui, labels: &[&str]) -> f32 {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let spacing = ui.spacing();
    labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(label.to_string(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
                + 2.0 * spacing.button_padding.x
        })
        .sum::<f32>()
        + spacing.item_spacing.x * (labels.len() + 1) as f32
}

/// The line a part's hover gets when it was copied in or moved.
fn part_place_note(moved: bool, source_idx: usize) -> String {
    if moved {
        format!("\nCopied or moved from part {} in the game", source_idx + 1)
    } else {
        String::new()
    }
}

/// Which parts of a list were added or moved, from where each came from in the file and
/// whether it is a copy (a copy is always given a name of its own).
///
/// A copy is always marked. Of the rest, the parts left in place are the longest run whose
/// file positions still rise; every other part was moved. On a tie the later part counts as in
/// place, so the part that was dragged up above another is the one marked, not the one it
/// passed.
fn added_or_moved(parts: &[(usize, bool)]) -> Vec<bool> {
    let n = parts.len();
    let mut run = vec![0usize; n];
    let mut prev: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        if parts[i].1 {
            continue;
        }
        run[i] = 1;
        for j in 0..i {
            if run[j] > 0 && parts[j].0 < parts[i].0 && run[j] + 1 >= run[i] {
                run[i] = run[j] + 1;
                prev[i] = Some(j);
            }
        }
    }
    let mut moved = vec![true; n];
    // `max_by_key` keeps the last of equal keys: the latest part ending a longest run.
    let mut at = (0..n).filter(|&i| run[i] > 0).max_by_key(|&i| run[i]);
    while let Some(i) = at {
        moved[i] = false;
        at = prev[i];
    }
    moved
}

/// Which authored fields of ONE emitter differ from its pristine snapshot, as the
/// absolute-value edit record the exporter/rebuilder consumes. Single source of truth for
/// "is this emitter edited?" (`is_empty`) and for what to write (`collect_authored_edits`),
/// so the UI's edited-emitter list can never disagree with what actually ships.
fn field_edits(em: &EmitterDef, pr: &EmitterSnapshot, tex_names: &[String]) -> EmitterFieldEdits {
    let mut f = EmitterFieldEdits::default();
    // Swaps are recorded by texture NAME. The picker works in pool indices, but the index is
    // only meaningful against the file loaded right now — see `EmitterFieldEdits::texture_name`.
    if em.texture_index != pr.texture_index {
        f.texture_name = em
            .texture_index
            .and_then(|i| tex_names.get(i as usize))
            .cloned();
    }
    // Every attribute that now reads differently from the file. Recorded by id rather than by
    // position: the table's order is stable within a run but the id is what a project file has
    // to survive on.
    for (i, attr) in crate::eff_attrs::table().iter().enumerate() {
        let (Some(now), Some(was)) = (
            em.attrs.get(i).copied().flatten(),
            pr.attrs.get(i).copied().flatten(),
        ) else {
            continue;
        };
        // A non-finite value cannot be written to the project file at all — serde_json refuses
        // NaN and infinity — so it is dropped here rather than failing the whole save.
        if now.same(was) || !now.is_storable() {
            continue;
        }
        f.attrs.insert(attr.id.to_string(), now);
    }
    for (index, now) in em.subsections.iter().enumerate() {
        let Some(was) = pr.subsections.get(index) else {
            continue;
        };
        if now.magic != was.magic {
            continue;
        }
        let bytes: std::collections::BTreeMap<usize, u8> = now
            .data
            .iter()
            .zip(&was.data)
            .enumerate()
            .filter_map(|(offset, (now, was))| (now != was).then_some((offset, *now)))
            .collect();
        if !bytes.is_empty() {
            f.subsections.push(crate::mod_project::SubsectionEdit {
                index,
                magic: now.magic.clone(),
                bytes,
            });
        }
    }
    let keys_differ = |a: &[ColorKey], b: &[ColorKey]| {
        a.len() != b.len()
            || a.iter().zip(b).any(|(x, y)| {
                diff(x.r, y.r) || diff(x.g, y.g) || diff(x.b, y.b) || diff(x.frame, y.frame)
            })
    };
    if keys_differ(&em.color0, &pr.color0) {
        f.color0 = Some(em.color0.iter().map(|k| [k.r, k.g, k.b, k.frame]).collect());
    }
    if keys_differ(&em.color1, &pr.color1) {
        f.color1 = Some(em.color1.iter().map(|k| [k.r, k.g, k.b, k.frame]).collect());
    }
    if em.alpha0_keys.len() != pr.alpha0_keys.len()
        || em
            .alpha0_keys
            .iter()
            .zip(&pr.alpha0_keys)
            .any(|(x, y)| diff(x.r, y.r) || diff(x.frame, y.frame))
    {
        f.alpha0 = Some(em.alpha0_keys.iter().map(|k| [k.r, k.frame]).collect());
    }
    f
}

/// The half-open range covering an emitter and everything nested under it.
///
/// The emitter list is flat and parent-first, so a subtree is the emitter plus every following
/// row deeper than it, up to the next row at its own depth or shallower.
fn subtree(emitters: &[EmitterDef], i: usize) -> std::ops::Range<usize> {
    let Some(root) = emitters.get(i) else {
        return i..i;
    };
    let end = emitters[i + 1..]
        .iter()
        .position(|e| e.depth <= root.depth)
        .map(|offset| i + 1 + offset)
        .unwrap_or(emitters.len());
    i..end
}

/// Move an emitter and every descendant by the same nesting-depth delta.
fn shift_subtree_depth(emitters: &mut [EmitterDef], i: usize, delta: i16) -> bool {
    let range = subtree(emitters, i);
    if range.is_empty() || delta == 0 {
        return false;
    }
    for emitter in &mut emitters[range] {
        emitter.depth = (emitter.depth as i16 + delta).max(0) as u8;
    }
    true
}

/// Whether copying row `i` (and what is made from it) keeps the set inside what the game loads:
/// at most [`MAX_CHILDREN`] parts made from one part.
///
/// [`MAX_CHILDREN`]: crate::eff_validate::MAX_CHILDREN
fn room_to_copy(depths: &[u8], i: usize) -> bool {
    match depths.get(i) {
        None => false,
        Some(0) => true,
        Some(_) => {
            let parent = (0..i).rev().find(|&k| depths[k] == 0).unwrap_or(0);
            children_of(depths, parent) < crate::eff_validate::MAX_CHILDREN
        }
    }
}

/// Whether row `i` can be made from the part above it. The game plays one level of parts made
/// from parts, so only a part on its own with nothing made from it qualifies, and the part it
/// would be made from must have room for another.
fn room_to_nest(depths: &[u8], i: usize) -> bool {
    if i == 0 || depths.get(i) != Some(&0) || depths.get(i + 1).is_some_and(|d| *d > 0) {
        return false;
    }
    let parent = (0..i).rev().find(|&k| depths[k] == 0).unwrap_or(0);
    children_of(depths, parent) < crate::eff_validate::MAX_CHILDREN
}

/// How many parts are made directly from row `parent`.
fn children_of(depths: &[u8], parent: usize) -> usize {
    depths[parent + 1..]
        .iter()
        .take_while(|d| **d > 0)
        .filter(|d| **d == 1)
        .count()
}

/// A name for a duplicated emitter that no sibling already has.
///
/// Emitters are addressed by name by every path that ships an edit — the exporter resolves an
/// authored edit and a roster slot both by (name, index) — so two emitters called the same thing
/// would make one of them unaddressable. The `_copy` / `_copy2` shape keeps the original name
/// readable in the middle of a twenty-emitter set.
fn unique_emitter_name(base: &str, existing: &[EmitterDef]) -> String {
    // Only a `_copy` suffix this function itself added is stripped, so copying `arc2` yields
    // `arc2_copy` rather than `arc_copy` — which would collide with a copy of `arc`.
    let stem = match base.rsplit_once("_copy") {
        Some((head, digits)) if digits.chars().all(|c| c.is_ascii_digit()) => head,
        _ => base,
    };
    let taken = |name: &str| existing.iter().any(|e| e.name == name);
    let first = format!("{stem}_copy");
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{stem}_copy{n}"))
        .find(|name| !taken(name))
        .unwrap_or(first)
}

/// How far the whole-effect controls have moved: a hue turn in degrees, and three factors.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WholeFactors {
    hue: f32,
    size: f32,
    life: f32,
    speed: f32,
}

impl Default for WholeFactors {
    fn default() -> Self {
        Self {
            hue: 0.0,
            size: 1.0,
            life: 1.0,
            speed: 1.0,
        }
    }
}

/// The values of one part the whole-effect controls change.
#[derive(Clone, Debug, PartialEq)]
struct WholeFields {
    color0: Vec<[f32; 3]>,
    color1: Vec<[f32; 3]>,
    scale: [Option<f32>; 3],
    life: Option<i64>,
    speeds: [Option<f32>; 2],
}

const PARTICLE_SCALE_ATTRS: [&str; 3] = [
    "particle_scale.scale_x",
    "particle_scale.scale_y",
    "particle_scale.scale_z",
];

/// A particle life at or past this is the game's "stays forever", which Life × leaves alone.
const FOREVER_LIFE: i64 = 100_000;

fn whole_fields(em: &EmitterDef) -> WholeFields {
    let rgb = |keys: &[ColorKey]| keys.iter().map(|k| [k.r, k.g, k.b]).collect();
    WholeFields {
        color0: rgb(&em.color0),
        color1: rgb(&em.color1),
        scale: PARTICLE_SCALE_ATTRS.map(|id| attr(em, id).map(AttrValue::as_f32)),
        life: attr(em, "particle_data.life").map(AttrValue::as_i64),
        speeds: SPEED_ATTRS.map(|id| attr(em, id).map(AttrValue::as_f32)),
    }
}

/// `base` with the controls at `k`.
fn scaled_whole_fields(base: &WholeFields, k: WholeFactors) -> WholeFields {
    let turn = |keys: &[[f32; 3]]| keys.iter().map(|rgb| rotate_hue(*rgb, k.hue)).collect();
    WholeFields {
        color0: turn(&base.color0),
        color1: turn(&base.color1),
        scale: base.scale.map(|v| v.map(|v| v * k.size)),
        life: base.life.map(|life| {
            if life <= 0 || life >= FOREVER_LIFE {
                life
            } else {
                ((life as f32 * k.life).round() as i64).max(1)
            }
        }),
        speeds: base.speeds.map(|v| v.map(|v| v * k.speed)),
    }
}

/// Write `fields` into the part, and its colour keys into the attribute table, as an edit made
/// on the part itself would be.
fn write_whole_fields(em: &mut EmitterDef, fields: &WholeFields) {
    for (key, rgb) in em.color0.iter_mut().zip(&fields.color0) {
        [key.r, key.g, key.b] = *rgb;
    }
    for (key, rgb) in em.color1.iter_mut().zip(&fields.color1) {
        [key.r, key.g, key.b] = *rgb;
    }
    for (id, value) in PARTICLE_SCALE_ATTRS.iter().zip(fields.scale) {
        if let Some(value) = value {
            set_attr(em, id, AttrValue::Float(value));
        }
    }
    if let Some(life) = fields.life {
        set_attr(em, "particle_data.life", AttrValue::Int(life));
    }
    for (id, value) in SPEED_ATTRS.iter().zip(fields.speeds) {
        if let Some(value) = value {
            set_attr(em, id, AttrValue::Float(value));
        }
    }
    effective_keys_to_attrs(em);
}

/// A colour's hue in degrees, `None` for a grey.
fn hue_of([r, g, b]: [f32; 3]) -> Option<f32> {
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    if delta <= 1e-3 {
        return None;
    }
    let sextant = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    Some(sextant * 60.0)
}

/// The whole-effect controls that turn each part's `pristine` values into its `now` values, when
/// one setting of them does for every part. Read back from the parts, so the controls show what
/// was done after a project reload instead of starting again from neutral.
fn read_whole_factors(pristine: &[WholeFields], now: &[WholeFields]) -> Option<WholeFactors> {
    if pristine.len() != now.len() {
        return None;
    }
    let pairs = || pristine.iter().zip(now);
    // Each factor from the pair that pins it most precisely, rounded as its control shows it.
    let factor = |pairs: Vec<(f32, f32)>| -> f32 {
        pairs
            .into_iter()
            .filter(|(from, _)| from.abs() > 1e-6)
            .max_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
            .map_or(1.0, |(from, to)| (to / from * 100.0).round() / 100.0)
    };
    let size = factor(
        pairs()
            .flat_map(|(p, n)| p.scale.iter().zip(&n.scale))
            .filter_map(|(p, n)| Some(((*p)?, (*n)?)))
            .collect(),
    );
    let speed = factor(
        pairs()
            .flat_map(|(p, n)| p.speeds.iter().zip(&n.speeds))
            .filter_map(|(p, n)| Some(((*p)?, (*n)?)))
            .collect(),
    );
    let life = factor(
        pairs()
            .filter_map(|(p, n)| Some((p.life?, n.life?)))
            .filter(|(p, _)| *p > 0 && *p < FOREVER_LIFE)
            .map(|(p, n)| (p as f32, n as f32))
            .collect(),
    );
    // The turn of the most colourful key, to the nearest degree as the control shows it.
    let hue = pairs()
        .flat_map(|(p, n)| {
            p.color0
                .iter()
                .zip(&n.color0)
                .chain(p.color1.iter().zip(&n.color1))
        })
        .filter_map(|(p, n)| {
            let chroma = p.iter().copied().fold(f32::MIN, f32::max)
                - p.iter().copied().fold(f32::MAX, f32::min);
            Some((chroma, hue_of(*n)? - hue_of(*p)?))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(0.0, |(_, turn)| {
            let turn = (turn + 180.0).rem_euclid(360.0) - 180.0;
            turn.round()
        });
    let factors = WholeFactors {
        hue,
        size,
        life,
        speed,
    };
    let close = |a: f32, b: f32| (a - b).abs() <= 1e-3_f32.max(b.abs() * 1e-2);
    let close_keys = |a: &[[f32; 3]], b: &[[f32; 3]]| {
        a.len() == b.len()
            && a.iter().zip(b).all(|(a, b)| {
                a.iter()
                    .zip(b)
                    .all(|(a, b)| (a - b).abs() <= 2e-2 * b.abs().max(1.0))
            })
    };
    let close_opt = |a: &[Option<f32>], b: &[Option<f32>]| {
        a.iter().zip(b).all(|(a, b)| match (a, b) {
            (Some(a), Some(b)) => close(*a, *b),
            (a, b) => a == b,
        })
    };
    pairs()
        .all(|(p, n)| {
            let want = scaled_whole_fields(p, factors);
            close_keys(&want.color0, &n.color0)
                && close_keys(&want.color1, &n.color1)
                && close_opt(&want.scale, &n.scale)
                && close_opt(&want.speeds, &n.speeds)
                && match (want.life, n.life) {
                    (Some(a), Some(b)) => (a - b).abs() <= 1,
                    (a, b) => a == b,
                }
        })
        .then_some(factors)
}

/// Turn a colour's hue by `degrees`, keeping its brightest and darkest channel. Works on the
/// game's over-bright values (above 1) as on any other.
fn rotate_hue(rgb: [f32; 3], degrees: f32) -> [f32; 3] {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    if delta <= 1e-6 || degrees == 0.0 {
        return rgb;
    }
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    let hue = (hue + degrees / 60.0).rem_euclid(6.0);
    let x = delta * (1.0 - (hue.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (delta, x, 0.0),
        1 => (x, delta, 0.0),
        2 => (0.0, delta, x),
        3 => (0.0, x, delta),
        4 => (x, 0.0, delta),
        _ => (delta, 0.0, x),
    };
    [r + min, g + min, b + min]
}

/// The whole-effect controls of the open effect: each part's values as they were when the
/// controls last read neutral, how far they have moved since, and what that wrote. Built again
/// whenever the parts are unlike what was written (a reload, an undo, a part's own rows): from the
/// game's values when one setting of the controls explains every part, else from neutral on what
/// is there.
struct WholeEffect {
    set_idx: usize,
    base: Vec<WholeFields>,
    factors: WholeFactors,
    written: Vec<WholeFields>,
}

/// The attributes the emitter panel puts above the group tree, in this order. These are the ones
/// an effect edit almost always starts from; every one of them is also in its own group below.
const BASIC_ATTRS: &[&str] = &[
    "emitter_static.color_scale",
    "render_state.blend_type",
    "emission.rate",
    "particle_data.life",
];

/// The two speeds a particle starts with, which the Look tab's Speed row scales together: out
/// from where it starts, and along the part's direction.
const SPEED_ATTRS: [&str; 2] = [
    "particle_velocity.all_direction",
    "particle_velocity.designated_dir_scale",
];

/// The speed the Speed row shows: the larger of the two, as that is the one you see.
fn shown_speed(speeds: [f32; 2]) -> f32 {
    if speeds[1].abs() > speeds[0].abs() {
        speeds[1]
    } else {
        speeds[0]
    }
}

/// Both speeds set so the shown one is `to`, keeping their ratio. From zero, the outward speed
/// takes the new value: a part that did not move gets particles flying out.
fn scaled_speeds(speeds: [f32; 2], to: f32) -> [f32; 2] {
    let from = shown_speed(speeds);
    if from.abs() < 1e-6 {
        [to, speeds[1]]
    } else {
        let ratio = to / from;
        [speeds[0] * ratio, speeds[1] * ratio]
    }
}

/// The area a part's particles start in, X Y Z.
const SPREAD_ATTRS: [&str; 3] = [
    "emitter_info.scale_x",
    "emitter_info.scale_y",
    "emitter_info.scale_z",
];

/// Why texture slot `slot` of a part showing `current` cannot be pointed at `t`, if it cannot.
/// Another size or format only changes the look; another number of stacked pictures is read
/// through another kind of texture view, and texture 2 and 3 name a texture by an id that one
/// added in the editor has only once the file is written.
fn texture_swap_blocked(t: &TextureInfo, current: &TextureInfo, slot: usize) -> Option<String> {
    if t.layers != current.layers {
        Some(format!(
            "Stacks {} picture{}; this part's texture stacks {}. The game reads a stacked \
             texture through another kind of view, which can break the part.",
            t.layers,
            if t.layers == 1 { "" } else { "s" },
            current.layers
        ))
    } else if slot != 0 && t.id.is_none() {
        Some(format!(
            "Added here: texture {} can point at it once the file is written.",
            slot + 1
        ))
    } else {
        None
    }
}

/// Below this width the right panel's colour rows drop their R G B numbers.
const NARROW_PANEL: f32 = 380.0;

/// Where a part sits from the effect's spawn point, X Y Z.
const POSITION_ATTRS: [&str; 3] = [
    "emitter_info.trans_x",
    "emitter_info.trans_y",
    "emitter_info.trans_z",
];

/// Whether any of two lists of values differ.
fn values_differ(a: &[f32], b: &[f32]) -> bool {
    a.iter().zip(b).any(|(a, b)| (a - b).abs() > 1e-6)
}

/// The label of a Look row, tinted once the row differs from the game, as the attribute rows
/// are.
fn quick_label(ui: &mut Ui, label: &str, edited: bool) -> egui::Response {
    let text = egui::RichText::new(label);
    column_label(
        ui,
        if edited {
            text.color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60))
        } else {
            text
        },
    )
}

/// How wide the label column of the Look rows and the settings tree is, from the left of the
/// part's settings, so their values line up in one column.
const LABEL_COLUMN: f32 = 170.0;

/// Below this width a row's "was …" and reset go on a line of their own under its value: in the
/// third column they ran past the panel's edge after the X Y Z boxes.
const WAS_BELOW_PANEL: f32 = 520.0;

/// The label column for a panel `width` wide: narrower in a narrow panel, so the values keep
/// their room, X Y Z boxes included (about 200 px with their gaps).
fn label_column(width: f32) -> f32 {
    LABEL_COLUMN
        .min(0.35 * width)
        .min((width - 200.0).max(60.0))
}

/// The least room a label gets, however deep its group sits in a narrow panel.
const MIN_LABEL_WIDTH: f32 = 72.0;

fn label_column_id() -> egui::Id {
    egui::Id::unique("eff_label_column_end")
}

fn was_below_id() -> egui::Id {
    egui::Id::unique("eff_was_below")
}

/// In a narrow panel, ends the grid row so what follows goes under the row's value instead of
/// beside it. Call it before a row's "was …" once the row is edited.
fn was_line(ui: &mut Ui) {
    if ui
        .data(|d| d.get_temp::<bool>(was_below_id()))
        .unwrap_or(false)
    {
        ui.end_row();
        ui.label("");
    }
}

/// A settings row's label, padded to the shared label column when one is set, however deep the
/// row's group is.
fn column_label(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    let end = ui.data(|d| d.get_temp::<f32>(label_column_id()));
    // Never so thin that a deep row's label wraps word by word in a narrow panel.
    let Some(width) = end.map(|end| (end - ui.cursor().left()).max(MIN_LABEL_WIDTH)) else {
        return ui.label(text);
    };
    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            // A long label wraps inside the column rather than widening it.
            ui.set_max_width(width);
            ui.add(egui::Label::new(text).wrap())
        },
    )
    .inner
}

/// Decimals a float shows on this page: two, or as many as a small value needs not to read as
/// zero.
fn float_decimals(value: f32) -> usize {
    let size = value.abs();
    if size == 0.0 || size >= 0.01 || !size.is_finite() {
        2
    } else {
        ((-size.log10()).ceil() as usize + 1).clamp(3, 6)
    }
}

/// A float field as every one on this page reads: the same decimals, its unit after it, and a
/// drag that suits the unit.
fn float_drag(value: &mut f32, speed: f64, unit: crate::eff_attrs::Unit) -> egui::DragValue<'_> {
    use crate::eff_attrs::Unit;
    let (suffix, speed, decimals) = match unit {
        Unit::Percent => ("%", 1.0, 1),
        Unit::Degrees => ("°", 1.0, 1),
        Unit::None => ("", speed, float_decimals(*value)),
    };
    let drag = egui::DragValue::new(value)
        .speed(speed)
        .max_decimals(decimals)
        .suffix(suffix);
    if unit == Unit::None {
        drag.min_decimals(decimals)
    } else {
        drag
    }
}

/// A label and its box in a wrapping row, moved to the next line together when the rest of the
/// line is narrower than they were last frame: a plain `horizontal` there never wraps, so its
/// box ran past the panel's edge.
fn wrapping_pair(ui: &mut Ui, key: &str, add: impl FnOnce(&mut Ui)) {
    let id = ui.scope_id().with(("wrapping_pair", key));
    let width = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
    // What is left of this line: `available_width` is the whole line's in a wrapping row.
    let left = ui.max_rect().right() - ui.cursor().left();
    if ui.cursor().left() > ui.max_rect().left() + 1.0 && left < width {
        ui.end_row();
    }
    let used = ui.horizontal(add).response.rect.width();
    ui.data_mut(|d| d.insert_temp(id, used));
}

/// The last column of a Look row: once edited, what the game had and a reset. Returns whether
/// the reset was clicked.
fn quick_was(ui: &mut Ui, edited: bool, was: String) -> bool {
    if edited {
        was_line(ui);
    }
    ui.horizontal(|ui| {
        if !edited {
            return false;
        }
        ui.label(egui::RichText::new(was).small().color(egui::Color32::GRAY));
        ui.small_button("↺")
            .on_hover_text("Reset to the game's value")
            .clicked()
    })
    .inner
}

/// Draw one attribute's editor. Returns whether the value changed.
///
/// The widget follows the attribute's kind rather than its storage: a flag is a checkbox even
/// though the file holds a byte, and a named type is a dropdown even though the file holds an
/// index. Values outside the names a type declares are shown as the raw number instead of being
/// clamped into range — vanilla data does carry a few, and silently rewriting one to "the nearest
/// thing we have a name for" would be an edit the user never made.
fn attr_widget(ui: &mut Ui, attr: &crate::eff_attrs::Attr, value: &mut AttrValue) -> bool {
    attr_control(ui, attr, value).0
}

/// [`attr_widget`], with the control's response for a hover or a menu.
fn attr_control(
    ui: &mut Ui,
    attr: &crate::eff_attrs::Attr,
    value: &mut AttrValue,
) -> (bool, egui::Response) {
    match attr.kind {
        AttrKind::Float { speed } => {
            let mut v = value.as_f32();
            let response = ui.add(float_drag(
                &mut v,
                speed as f64,
                crate::eff_attrs::unit_of(attr.id),
            ));
            let changed = response.changed();
            if changed {
                *value = AttrValue::Float(v);
            }
            (changed, response)
        }
        AttrKind::Int => {
            let mut v = value.as_i64();
            let mut drag = egui::DragValue::new(&mut v).speed(1.0);
            if crate::eff_attrs::is_key_count(attr.id) {
                drag = drag.range(0..=crate::eff_attrs::KEY_SLOTS as i64);
            }
            let response = ui.add(drag);
            let changed = response.changed();
            if changed {
                *value = AttrValue::Int(v);
            }
            (changed, response)
        }
        AttrKind::UInt if crate::eff_attrs::is_key_count(attr.id) => {
            let mut v = value.as_u64();
            let drag = egui::DragValue::new(&mut v)
                .speed(1.0)
                .range(0..=crate::eff_attrs::KEY_SLOTS as u64);
            let response = ui.add(drag);
            let changed = response.changed();
            if changed {
                *value = AttrValue::UInt(v);
            }
            (changed, response)
        }
        // Ids, seeds and flag words: typed, because a drag goes through f64 and cannot hold a
        // 64-bit id (u64::MAX, "none", showed as 18446744073709551616).
        AttrKind::UInt => {
            let mut v = value.as_u64();
            let (changed, response) = uint_text_edit(ui, attr.id, &mut v);
            if changed {
                *value = AttrValue::UInt(v);
            }
            (changed, response)
        }
        AttrKind::Flag => {
            let mut on = value.as_bool();
            let response = ui.checkbox(&mut on, "");
            let changed = response.changed();
            if changed {
                *value = AttrValue::Int(i64::from(on));
            }
            (changed, response)
        }
        AttrKind::Enum(names) => {
            let current = value.as_i64();
            let label = enum_label(names, current);
            let mut changed = false;
            let response = egui::ComboBox::from_id_salt(attr.id)
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        if ui.selectable_label(current == i as i64, *name).clicked() {
                            *value = AttrValue::Int(i as i64);
                            changed = true;
                        }
                    }
                })
                .response
                .on_hover_text(format!("Stored as {current}"));
            (changed, response)
        }
    }
}

/// A colour or opacity key's time, stored as a fraction of the particle's life (0 to 1) and
/// shown as a percentage of it. The stored value changes only when the field is edited.
fn life_time_field(ui: &mut Ui, fraction: &mut f32) -> bool {
    let mut percent = *fraction * 100.0;
    let response = ui.add(
        egui::DragValue::new(&mut percent)
            .speed(1.0)
            .range(0.0..=100.0)
            .clamp_existing_to_range(false)
            .max_decimals(1)
            .suffix("%"),
    );
    let changed = response.changed();
    if changed {
        *fraction = life_fraction(percent);
    }
    changed
}

/// A percentage of a particle's life as the stored fraction.
fn life_fraction(percent: f32) -> f32 {
    percent / 100.0
}

/// An opacity key drawn as a small bar, filled as far as the key is solid, as a colour key has
/// its swatch.
fn opacity_bar(ui: &mut Ui, opacity: f32) {
    let size = egui::vec2(28.0, ui.spacing().interact_size.y * 0.5);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(40));
    let mut filled = rect;
    filled.set_width(rect.width() * opacity.clamp(0.0, 1.0));
    painter.rect_filled(filled, 2.0, egui::Color32::from_gray(200));
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
        egui::StrokeKind::Inside,
    );
}

/// Small pictures of the file's textures for the texture picker: each decoded once, when its row
/// first shows, a few per frame, and all dropped when the file or its textures change. Nothing is
/// read or decoded again once a picture is kept.
#[derive(Default)]
struct TextureThumbs {
    /// What the pictures were made from: the file, its texture pool's generation and the
    /// replaced textures' pictures.
    made_from: Option<ThumbSource>,
    /// By texture index; `None` for one that cannot be decoded.
    thumbs: std::collections::HashMap<usize, Option<egui::TextureHandle>>,
    /// Decoded this frame, against the per-frame limit.
    decoded: (u64, usize),
}

/// What texture picker pictures are made from: the file, its texture pool's generation, and each
/// replaced texture's name and picture.
type ThumbSource = (Option<PathBuf>, u64, Vec<(String, String)>);

/// The side of a texture picker picture.
const THUMB: u32 = 32;
/// How many pictures the picker decodes in one frame.
const THUMBS_PER_FRAME: usize = 2;

impl TextureThumbs {
    /// Forget every picture unless they were made from `made_from`.
    fn keep_for(&mut self, made_from: ThumbSource) {
        if self.made_from.as_ref() != Some(&made_from) {
            self.thumbs.clear();
            self.made_from = Some(made_from);
        }
    }

    /// Draw texture `index`'s picture in a `THUMB` square, decoding it with `decode` when it is
    /// on screen, not kept yet, and this frame's limit allows.
    fn draw(
        &mut self,
        ui: &mut Ui,
        index: usize,
        decode: impl FnOnce() -> Option<image::RgbaImage>,
    ) {
        let side = THUMB as f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
        let frame = ui.ctx().cumulative_frame_nr();
        if self.decoded.0 != frame {
            self.decoded = (frame, 0);
        }
        if !self.thumbs.contains_key(&index) && ui.is_rect_visible(rect) {
            if self.decoded.1 < THUMBS_PER_FRAME {
                self.decoded.1 += 1;
                let handle = decode().map(|image| {
                    let size = [image.width() as usize, image.height() as usize];
                    ui.ctx().load_texture(
                        format!("eff_thumb_{index}"),
                        egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                        egui::TextureOptions::LINEAR,
                    )
                });
                self.thumbs.insert(index, handle);
            } else {
                ui.ctx().request_repaint();
            }
        }
        // A mid-grey checker behind it, so a dark or clear texture does not read as an empty
        // square.
        let painter = ui.painter();
        let cell = side / 4.0;
        for (x, y) in (0..4).flat_map(|x| (0..4).map(move |y| (x, y))) {
            let grey = if (x + y) % 2 == 0 { 96 } else { 128 };
            let min = rect.min + egui::vec2(x as f32 * cell, y as f32 * cell);
            painter.rect_filled(
                egui::Rect::from_min_size(min, egui::vec2(cell, cell)),
                0.0,
                egui::Color32::from_gray(grey),
            );
        }
        if let Some(Some(handle)) = self.thumbs.get(&index) {
            let size = handle.size_vec2();
            let scale = side / size.x.max(size.y).max(1.0);
            let shown = egui::Rect::from_center_size(rect.center(), size * scale);
            painter.image(
                handle.id(),
                shown,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }
}

/// A named choice's label: its name, or "Other (N)" for a value with no name.
fn enum_label(names: &[&str], value: i64) -> String {
    usize::try_from(value)
        .ok()
        .and_then(|i| names.get(i))
        .map(|n| (*n).to_string())
        .unwrap_or_else(|| format!("Other ({value})"))
}

/// The right panel's bottom split, holding the game's view while one is connected: its smallest,
/// starting and largest heights in a right panel this tall.
fn game_split_sizes(panel_height: f32) -> (f32, f32, f32) {
    // The settings above keep at least half the panel.
    let max = (panel_height * 0.5).max(80.0);
    let min = 80.0_f32.min(max);
    let default = (panel_height * 0.3).clamp(min, max);
    (min, default, max)
}

/// Why Send edits cannot be clicked without a game: one plain line, not the socket's error.
fn send_blocked_text(view: &crate::game_link::LinkView) -> String {
    let state = match (view.status, view.dialled) {
        (crate::game_link::LinkStatus::Connecting, Some(addr)) => {
            format!("Looking for the game at {addr}")
        }
        (_, Some(addr)) => format!("Game not running at {addr}"),
        (status, None) => status.label().to_owned(),
    };
    format!("Connect to the game first.\n\n{state}")
}

/// What each effect is listed as: its name in words, as the timeline shows it, or its file name
/// where two effects would read the same.
fn entry_list_names(entries: &[EffEntry]) -> Vec<String> {
    let words: Vec<String> = entries
        .iter()
        .map(|entry| crate::app::effect_words(&entry.name))
        .collect();
    let mut seen = std::collections::HashMap::<String, usize>::new();
    for word in &words {
        *seen.entry(word.to_lowercase()).or_default() += 1;
    }
    words
        .into_iter()
        .zip(entries)
        .map(|(word, entry)| {
            if seen[&word.to_lowercase()] > 1 {
                entry.name.clone()
            } else {
                word
            }
        })
        .collect()
}

/// How well a search matches a choice shown under any of `names`: 0 is an exact name, 1 a name
/// starting with it, 2 a name containing it, and None no match. An empty search matches all
/// alike. Case and surrounding spaces are ignored.
fn search_rank(search: &str, names: &[&str]) -> Option<u8> {
    let search = search.trim().to_lowercase();
    if search.is_empty() {
        return Some(0);
    }
    names
        .iter()
        .filter_map(|name| {
            let name = name.to_lowercase();
            if name == search {
                Some(0)
            } else if name.starts_with(&search) {
                Some(1)
            } else if name.contains(&search) {
                Some(2)
            } else {
                None
            }
        })
        .min()
}

/// A whole number as the attribute rows show it: the all-ones value means no id is set.
fn uint_text(value: u64) -> String {
    if value == u64::MAX {
        "None".to_owned()
    } else {
        value.to_string()
    }
}

/// The number typed into a whole-number row: decimal, `0x` hex, or `None` for the all-ones "no
/// id" value. Anything else, an empty box included, is not a number yet.
fn parse_uint_text(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("none") {
        return Some(u64::MAX);
    }
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// A text box for a whole number. While it has focus the typed text is kept as typed, so a
/// half-typed number is not snapped back; the value changes when the text reads as a number.
fn uint_text_edit(ui: &mut Ui, id_salt: &str, value: &mut u64) -> (bool, egui::Response) {
    let id = ui.make_persistent_id(("uint_text", id_salt));
    let mut text = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| uint_text(*value));
    let response = ui
        .add(egui::TextEdit::singleline(&mut text).desired_width(150.0))
        .on_hover_text("A whole number, 0x for hex, or None for no id");
    let mut changed = false;
    if response.changed() {
        if let Some(parsed) = parse_uint_text(&text) {
            changed = parsed != *value;
            *value = parsed;
        }
    }
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    (changed, response)
}

/// The attribute holding each texture slot's texture id. Slot 1's is read into
/// `EmitterDef::texture_index` at load.
const TEXTURE_SLOT_IDS: [&str; 3] = [
    "sampler0.texture_id",
    "sampler1.texture_id",
    "sampler2.texture_id",
];

/// The texture slots `em` samples a pool texture through, as (slot, pool index). A slot whose id
/// is the all-ones "none" value, or names no texture in this file, is left out.
fn texture_slots(em: &EmitterDef, textures: &[TextureInfo]) -> Vec<(usize, usize)> {
    let mut slots = Vec::new();
    if let Some(at) = em
        .texture_index
        .map(|i| i as usize)
        .filter(|i| *i < textures.len())
    {
        slots.push((0, at));
    }
    for (slot, id) in TEXTURE_SLOT_IDS.iter().enumerate().skip(1) {
        let Some(guid) = attr(em, id)
            .map(|v| v.as_u64())
            .filter(|guid| *guid != u64::MAX)
        else {
            continue;
        };
        if let Some(at) = textures.iter().position(|t| t.id == Some(guid)) {
            slots.push((slot, at));
        }
    }
    slots
}

/// A part's name in words, as the effect list names effects: "flare1" reads "Flare 1".
/// Whether an effect is one of the shared ones in `ef_common.eff`, which every fighter plays:
/// the game names them `sys_`.
pub(crate) fn is_shared_effect(name: &str) -> bool {
    name.get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("sys_"))
}

/// Whether `path` is the shared effect file.
pub(crate) fn is_shared_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("ef_common.eff"))
}

/// The colour shared effects are marked in.
pub(crate) const SHARED_TINT: egui::Color32 = egui::Color32::from_rgb(110, 170, 235);
/// The colour borrowed effects are marked in.
const BORROWED_TINT: egui::Color32 = egui::Color32::from_rgb(190, 140, 255);

/// The hover line a shared effect carries wherever it is named.
pub(crate) const SHARED_EFFECT_NOTE: &str =
    "In the shared effects file: an edit changes it for every fighter that plays it. To change it \
     for one fighter, borrow it into that fighter's file and point the move at the copy.";

fn part_words(name: &str) -> String {
    let mut words = String::new();
    let mut last: Option<char> = None;
    for c in name.chars() {
        let c = if c == '_' { ' ' } else { c };
        if let Some(prev) = last {
            // "flare1" and "FlareBase" both split, the second where a capital follows a small
            // letter.
            let boundary = (prev.is_ascii_alphabetic() && c.is_ascii_digit())
                || (prev.is_ascii_digit() && c.is_ascii_alphabetic())
                || (prev.is_ascii_lowercase() && c.is_ascii_uppercase());
            if boundary {
                words.push(' ');
            }
        }
        words.push(c);
        last = Some(c);
    }
    // A sentence: a capital to start, small letters after.
    let words = words
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => words,
    }
}

/// What each part is shown as: its name in words, or its own name where two would read the
/// same, or "Part N" when it has none.
fn part_list_names(names: &[&str]) -> Vec<String> {
    let words: Vec<String> = names.iter().map(|name| part_words(name)).collect();
    words
        .iter()
        .zip(names)
        .enumerate()
        .map(|(i, (word, name))| {
            if name.is_empty() {
                format!("Part {}", i + 1)
            } else if words.iter().filter(|other| *other == word).count() > 1 {
                (*name).to_owned()
            } else {
                word.clone()
            }
        })
        .collect()
}

/// The selected part, and the file's own values for it.
fn part_mut<'a>(
    ptcl: &'a mut Option<PtclFile>,
    pristine: &'a [Vec<EmitterSnapshot>],
    set_idx: usize,
    part: usize,
) -> Option<(&'a mut EmitterDef, &'a EmitterSnapshot)> {
    let em = ptcl
        .as_mut()?
        .emitter_sets
        .get_mut(set_idx)?
        .emitters
        .get_mut(part)?;
    let pr = pristine.get(set_idx)?.get(em.source_idx)?;
    Some((em, pr))
}

/// How an attribute's value reads in the "orig" column.
fn attr_text(attr: &crate::eff_attrs::Attr, value: AttrValue) -> String {
    match attr.kind {
        AttrKind::Float { .. } => {
            let v = value.as_f32();
            match crate::eff_attrs::unit_of(attr.id) {
                crate::eff_attrs::Unit::Percent => format!("{v:.1}%"),
                crate::eff_attrs::Unit::Degrees => format!("{v:.1}°"),
                crate::eff_attrs::Unit::None => format!("{v:.*}", float_decimals(v)),
            }
        }
        AttrKind::Int => value.as_i64().to_string(),
        AttrKind::UInt => uint_text(value.as_u64()),
        AttrKind::Flag => if value.as_bool() { "on" } else { "off" }.to_string(),
        AttrKind::Enum(names) => enum_label(names, value.as_i64()),
    }
}

/// Draw a set of attribute rows as a three-column grid: label, editor, original value.
///
/// Rows the emitter does not carry are skipped, not greyed: an emitter with no sampler 2 has no
/// sampler-2 wrap mode to show, and a disabled control implies there is a value behind it.
fn attr_rows_by_index(
    ui: &mut Ui,
    salt: &str,
    em: &mut EmitterDef,
    pr: &EmitterSnapshot,
    indices: &[usize],
    textures: &[TextureInfo],
) -> bool {
    let mut changed = false;
    egui::Grid::new(salt)
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            changed = attr_grid_rows(ui, em, pr, indices, textures);
        });
    changed
}

/// Attribute rows inside a three-column grid the caller opens: label, editor, the file's value
/// once it differs.
fn attr_grid_rows(
    ui: &mut Ui,
    em: &mut EmitterDef,
    pr: &EmitterSnapshot,
    indices: &[usize],
    textures: &[TextureInfo],
) -> bool {
    let table = crate::eff_attrs::table();
    let mut changed = false;
    for &i in indices {
        let (Some(attr), Some(Some(mut value))) = (table.get(i), em.attrs.get(i).copied()) else {
            continue;
        };
        let orig = pr.attrs.get(i).copied().flatten();
        let edited = orig.is_some_and(|o| !o.same(value));

        let label = if edited {
            egui::RichText::new(attr.label).color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60))
        } else {
            egui::RichText::new(attr.label)
        };
        column_label(ui, label).on_hover_text(format!("{}\n\n{}", attr.doc, attr.id));

        // A texture id that names a texture in this file reads as that texture; the Texture
        // section is where it is changed.
        let texture_name = |id: u64| {
            textures
                .iter()
                .find(|t| t.id == Some(id))
                .map(|t| t.tex_name.clone())
        };
        let named = attr
            .id
            .ends_with(".texture_id")
            .then(|| texture_name(value.as_u64()))
            .flatten();
        if let Some(name) = named {
            ui.label(name).on_hover_text(format!(
                "Texture id {}. Pick another in the Texture section.",
                value.as_u64()
            ));
        } else if attr_widget(ui, attr, &mut value) {
            em.attrs[i] = Some(value);
            changed = true;
        }

        if edited {
            was_line(ui);
        }
        ui.horizontal(|ui| {
            if let Some(orig) = orig {
                // The file's value only matters once it differs; on every row it was
                // a column of noise repeating the field beside it.
                if edited {
                    let was = attr
                        .id
                        .ends_with(".texture_id")
                        .then(|| texture_name(orig.as_u64()))
                        .flatten()
                        .unwrap_or_else(|| attr_text(attr, orig));
                    ui.label(
                        egui::RichText::new(format!("was {was}"))
                            .small()
                            .color(egui::Color32::GRAY),
                    );
                }
                // Reset is per attribute rather than per group: with three hundred rows,
                // "put this one back" is the operation you actually want, and the
                // emitter-wide reset is still one button away.
                if edited
                    && ui
                        .small_button("↺")
                        .on_hover_text("Reset to the game's value")
                        .clicked()
                {
                    em.attrs[i] = Some(orig);
                    changed = true;
                }
            }
        });
        ui.end_row();
    }
    changed
}

/// The attribute groups of texture slots 2 and 3 when the part samples no texture through them:
/// the slot's texture id is the all-ones "none" value. Slot 1 is never counted as unused.
fn unused_texture_groups(em: &EmitterDef) -> Vec<&'static str> {
    use crate::eff_attrs::*;
    let slots = [
        (
            "sampler1.texture_id",
            [G_SAMPLER1, G_TEXANIM1, G_TEXPAT1, G_TEXUV1],
        ),
        (
            "sampler2.texture_id",
            [G_SAMPLER2, G_TEXANIM2, G_TEXPAT2, G_TEXUV2],
        ),
    ];
    slots
        .into_iter()
        .filter(|(id, _)| {
            index_of(id)
                .and_then(|at| em.attrs.get(at).copied().flatten())
                .is_some_and(|value| value.as_u64() == u64::MAX)
        })
        .flat_map(|(_, groups)| groups)
        .collect()
}

/// A settings group's header text, wrapped to the panel: a header never wraps its own text, so a
/// long one ("Position, turn, area and colour  (26, 1 edited)") ran past a narrow panel's edge.
fn wrapped_header(ui: &Ui, text: egui::RichText) -> egui::WidgetText {
    let width = ui.available_width() - ui.spacing().indent;
    egui::WidgetText::from(text)
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Wrap),
            width,
            egui::TextStyle::Button,
        )
        .into()
}

/// Words a settings search also looks for: people search by what they want ("speed", "count"),
/// and the rows are named by what they are ("Outward speed", "Particles per frame").
const FILTER_SYNONYMS: &[&[&str]] = &[
    &["speed", "velocity", "fast", "slow"],
    &["count", "amount", "number", "particles per frame"],
    &["size", "scale", "bigger", "smaller"],
    &["colour", "color", "tint"],
    &["opacity", "alpha", "transparen", "fade"],
    &["glow", "blend", "additive"],
    &["life", "lifetime", "how long"],
    &["spin", "rotat"],
];

/// The search's own text, and the words it stands for.
fn filter_terms(needle: &str) -> Vec<String> {
    let mut terms = vec![needle.to_owned()];
    for group in FILTER_SYNONYMS {
        if group.contains(&needle) {
            terms.extend(group.iter().map(|w| w.to_string()));
        }
    }
    terms.dedup();
    terms
}

/// The whole emitter, one collapsible group per section of the format.
///
/// Three hundred rows is more than anyone wants to scroll, so the groups start collapsed and the
/// filter is the primary way in: typing `gravity` opens every group holding a match and hides
/// everything else. Groups with an edit in them are marked and open on their own, because the
/// question "what did I change on this emitter?" must not require opening thirty headers.
fn draw_attribute_groups(
    ui: &mut Ui,
    em: &mut EmitterDef,
    pr: &EmitterSnapshot,
    filter: &mut String,
    textures: &[TextureInfo],
    part: egui::Id,
) -> bool {
    let table = crate::eff_attrs::table();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("All attributes").strong());
        // Room left for Clear after it, in a narrow panel.
        let width = (ui.available_width() - 56.0).clamp(80.0, 160.0);
        ui.add(
            egui::TextEdit::singleline(filter)
                .hint_text("filter, e.g. gravity")
                .desired_width(width),
        );
        if !filter.is_empty() && ui.small_button("Clear").clicked() {
            filter.clear();
        }
    });

    let needle = filter.trim().to_lowercase();
    let terms = filter_terms(&needle);
    let matches = |a: &crate::eff_attrs::Attr| {
        needle.is_empty()
            || terms.iter().any(|term| {
                a.label.to_lowercase().contains(term)
                    || a.id.to_lowercase().contains(term)
                    || a.doc.to_lowercase().contains(term)
                    || a.group.to_lowercase().contains(term)
            })
    };

    // Each group's rows and how many of them are edited. An attribute the emitter does not carry
    // is not listed at all, so a group that is entirely absent (no sampler 2 on this emitter)
    // does not draw an empty header either.
    let groups: Vec<(&str, Vec<usize>, usize)> = crate::eff_attrs::GROUPS
        .iter()
        .filter_map(|group| {
            let rows: Vec<usize> = table
                .iter()
                .enumerate()
                .filter(|(i, a)| {
                    a.group == *group && matches(a) && em.attrs.get(*i).copied().flatten().is_some()
                })
                .map(|(i, _)| i)
                .collect();
            if rows.is_empty() {
                return None;
            }
            let edited = rows
                .iter()
                .filter(
                    |&&i| match (em.attrs[i], pr.attrs.get(i).copied().flatten()) {
                        (Some(now), Some(was)) => !now.same(was),
                        _ => false,
                    },
                )
                .count();
            Some((*group, rows, edited))
        })
        .collect();
    // Texture slots 2 and 3 sample nothing on most parts; their settings are kept, folded under
    // one header instead of eight.
    let unused = unused_texture_groups(em);

    let mut changed = false;
    let filtering = !needle.is_empty();
    let heading = |name: &str, rows: usize, edited: usize| {
        if edited > 0 {
            egui::RichText::new(format!("{name}  ({rows}, {edited} edited)"))
                .color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60))
        } else {
            egui::RichText::new(format!("{name}  ({rows})"))
        }
    };
    // A header is open or closed for every part alike, so one opened stays open while parts are
    // compared. A header with an edit in it opens by itself, and is this part's own: shared, the
    // first edited part opened it for every part after it.
    let salt = |edited: usize| (edited > 0).then_some(part);
    let totals = |list: &[&(&str, Vec<usize>, usize)]| {
        list.iter().fold((0, 0), |(rows, edited), (_, r, e)| {
            (rows + r.len(), edited + e)
        })
    };
    let mut draw_group = |ui: &mut Ui, group: &str, shown: &str, rows: &[usize], edited: usize| {
        // A filter opens every group holding a match: `default_open` alone only applied the
        // first time a group was seen, so groups closed before stayed closed.
        egui::CollapsingHeader::new(wrapped_header(ui, heading(shown, rows.len(), edited)))
            .id_salt((group, salt(edited)))
            .default_open(edited > 0)
            .open(filtering.then_some(true))
            .show(ui, |ui| {
                changed |= attr_rows_by_index(ui, group, em, pr, rows, textures);
            });
    };
    // Six sections of groups, rather than thirty headers in a row. A group no section names
    // goes under "Other", so a group added to the table is never out of reach.
    let filed: Vec<&str> = crate::eff_attrs::SECTIONS
        .iter()
        .flat_map(|(_, members)| members.iter().copied())
        .collect();
    let other: Vec<&str> = crate::eff_attrs::GROUPS
        .iter()
        .copied()
        .filter(|group| !filed.contains(group))
        .collect();
    let sections = crate::eff_attrs::SECTIONS
        .iter()
        .map(|(name, members)| (*name, members.to_vec()))
        .chain((!other.is_empty()).then_some(("Other", other)));
    // The filter opens every header holding a match. Under its own ids, so clearing the filter
    // leaves each header as it was before the search, not open for every part from then on.
    ui.push_id(("attr_tree", filtering), |ui| {
        for (section, members) in sections {
            // In the section's own order: where the table lists them put a colour group eight
            // headers from its "over life" partner.
            let present: Vec<&(&str, Vec<usize>, usize)> = members
                .iter()
                .filter_map(|member| groups.iter().find(|(group, _, _)| group == member))
                .collect();
            if present.is_empty() {
                continue;
            }
            let (rows, edited) = totals(&present);
            egui::CollapsingHeader::new(wrapped_header(
                ui,
                heading(section, rows, edited).strong(),
            ))
            .id_salt(("attr_section", section, salt(edited)))
            .default_open(edited > 0)
            .open(filtering.then_some(true))
            .show(ui, |ui| {
                let by_slot = present
                    .iter()
                    .any(|(group, _, _)| crate::eff_attrs::texture_slot_of(group).is_some());
                if !by_slot {
                    for (group, rows, edited) in &present {
                        draw_group(ui, group, group, rows, *edited);
                    }
                    return;
                }
                // One header per texture slot, each holding that slot's groups.
                for slot in 0..3 {
                    let slot_groups: Vec<&(&str, Vec<usize>, usize)> = present
                        .iter()
                        .copied()
                        .filter(|(group, _, _)| {
                            crate::eff_attrs::texture_slot_of(group) == Some(slot)
                                && !unused.contains(group)
                        })
                        .collect();
                    if slot_groups.is_empty() {
                        continue;
                    }
                    let (rows, edited) = totals(&slot_groups);
                    let title = format!("Texture {}", slot + 1);
                    egui::CollapsingHeader::new(wrapped_header(ui, heading(&title, rows, edited)))
                        .id_salt(("attr_texture_slot", slot, salt(edited)))
                        .default_open(edited > 0)
                        .open(filtering.then_some(true))
                        .show(ui, |ui| {
                            for (group, rows, edited) in &slot_groups {
                                // "Texture 2 sampling" reads "Sampling" under "Texture 2".
                                let shown = group
                                    .strip_prefix(title.as_str())
                                    .map(str::trim)
                                    .map(|rest| {
                                        let mut chars = rest.chars();
                                        chars
                                            .next()
                                            .map(|c| c.to_uppercase().chain(chars).collect())
                                            .unwrap_or_default()
                                    })
                                    .filter(|rest: &String| !rest.is_empty())
                                    .unwrap_or_else(|| group.to_string());
                                draw_group(ui, group, &shown, rows, *edited);
                            }
                        });
                }
                let spare: Vec<&(&str, Vec<usize>, usize)> = present
                    .iter()
                    .copied()
                    .filter(|(group, _, _)| unused.contains(group))
                    .collect();
                if spare.is_empty() {
                    return;
                }
                let (rows, edited) = totals(&spare);
                egui::CollapsingHeader::new(wrapped_header(
                    ui,
                    heading("Unused texture slots", rows, edited),
                ))
                .id_salt(("unused_texture_slots", salt(edited)))
                .default_open(edited > 0)
                .open(filtering.then_some(true))
                .show(ui, |ui| {
                    for (group, rows, edited) in &spare {
                        draw_group(ui, group, group, rows, *edited);
                    }
                })
                .header_response
                .on_hover_text(
                    "The settings of texture slots this part samples no texture through. \
                             They are kept as the file has them.",
                );
            });
        }
    });
    if !needle.is_empty() && !changed {
        // A filter that matches nothing is otherwise indistinguishable from every group being
        // collapsed.
        let any = table
            .iter()
            .enumerate()
            .any(|(i, a)| matches(a) && em.attrs.get(i).copied().flatten().is_some());
        if !any {
            ui.colored_label(
                egui::Color32::GRAY,
                format!("no setting of this part matches '{filter}'"),
            );
        }
    }
    changed
}

impl EffEditor {
    // ── Loading ───────────────────────────────────────────────────────────────

    pub fn rescan(&mut self) {
        self.eff_files.clear();
        self.scan_error = None;
        let effect_dir = self.export_root.join("effect");
        let root = if effect_dir.is_dir() {
            effect_dir
        } else {
            self.export_root.clone()
        };
        scan_eff_files(&root, &mut self.eff_files);
        self.eff_files.sort();
        if self.eff_files.is_empty() {
            self.scan_error = Some(format!("no .eff files under {}", root.display()));
        }
    }

    fn load_eff(&mut self, path: &Path) {
        self.remember_picks();
        self.edit_rev += 1;
        // A step holds emitters of the file it was taken in, by position: it is kept with that
        // file and comes back when the file does.
        self.history_close_step();
        self.stash_history();
        self.load_error = None;
        self.ptcl = None;
        self.pristine.clear();
        self.entries.clear();
        self.selected_entry = None;
        self.selected_emitter = 0;
        self.open_first_effect = true;
        // `eff_dirty_at` deliberately SURVIVES a load: it tracks project-vs-game divergence,
        // not the working copy. Clearing it here silently undid the `mark_unsent` a transplant
        // had just done, because recording one reloads the eff from the merged preview — so the
        // Send button went back to grey the frame after a transplant lit it. It is cleared where
        // the divergence actually ends, in `request_live_apply`.

        // Character-centric view: the fighter's base eff transparently resolves to its
        // MERGED (transplants applied) file when one exists — the new/replaced entries
        // show up and are editable no matter how the file was opened.
        let overlay = self
            .merged_overlays
            .get(path)
            .filter(|m| m.exists())
            .cloned();
        self.loaded_is_merged = overlay.is_some();
        let actual: PathBuf = overlay.unwrap_or_else(|| path.to_path_buf());

        let loaded = match load_effect(&actual) {
            Ok(effect) => effect,
            Err(e) => {
                self.load_error = Some(e.to_string());
                return;
            }
        };
        let index = loaded.index;
        let ptcl = loaded.ptcl;

        self.pristine = ptcl
            .emitter_sets
            .iter()
            .map(|set| set.emitters.iter().map(EmitterSnapshot::of).collect())
            .collect();

        // Handles carry an original-case and a lowercase duplicate — keep one per lowercase
        // name. hash40 in ACMD is over the lowercase name, which is what the plugin reports.
        let mut seen = std::collections::HashSet::new();
        let mut entries: Vec<EffEntry> = Vec::new();
        for (name, set_idx) in &index.handles {
            let lower = name.to_lowercase();
            if !seen.insert(lower.clone()) {
                continue;
            }
            let idx = *set_idx;
            if idx < 0 || idx as usize >= ptcl.emitter_sets.len() {
                continue;
            }
            entries.push(EffEntry {
                hash: hash40::hash40(&lower).0,
                name: lower,
                set_idx: idx as usize,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        self.entries = entries;
        self.entry_pristine = loaded.entries.clone();
        self.entry_info = loaded.entries;
        self.ptcl = Some(ptcl);
        self.texture_pool = loaded.texture_pool;
        self.texture_note = None;
        self.loaded_path = Some(path.to_path_buf());
        self.restore_history(path);
        // The queued edits for it, if any, are applied right after this.
        self.working_stale = false;
        // Back on a file: the effect and part that were open in it are again.
        if let Some(name) = self.entry_picked.get(path).cloned() {
            if let Some(i) = self.entries.iter().position(|e| e.name == name) {
                self.selected_entry = Some(i);
                self.selected_emitter = self.parts_picked.get(&name).copied().unwrap_or(0);
                self.scroll_to_selected = true;
                self.view = if self.open_move_plays_selected() {
                    EffectView::InMove
                } else {
                    EffectView::Alone
                };
            }
        }
    }

    /// Note the open effect and part, to reopen them when this file or effect is.
    fn remember_picks(&mut self) {
        let Some(entry) = self.selected_entry.and_then(|e| self.entries.get(e)) else {
            return;
        };
        self.parts_picked
            .insert(entry.name.clone(), self.selected_emitter);
        if let Some(path) = &self.loaded_path {
            self.entry_picked.insert(path.clone(), entry.name.clone());
        }
    }

    // ── Project integration ───────────────────────────────────────────────────

    /// Loaded eff path relative to the export root (project `source_rel`).
    pub fn loaded_rel(&self) -> Option<String> {
        Some(self.game_path_of(self.loaded_path.as_ref()?))
    }

    /// The game path a file on disk stands for: its name inside the game folder, or the one
    /// it was given when opened from a project or a folder the author picked.
    fn game_path_of(&self, path: &Path) -> String {
        if let Some(rel) = self.game_paths.get(path) {
            return rel.clone();
        }
        let rel = path.strip_prefix(&self.export_root).unwrap_or(path);
        rel.to_string_lossy().replace('\\', "/")
    }

    /// Open `path` as the game's `rel` (e.g. `effect/fighter/pickel/ef_pickel.eff`), so edits
    /// to an effect file kept outside the game folder are keyed and exported as that file.
    /// Opens it again even when it is already open, as a changed file needs.
    pub fn load_as(&mut self, path: &Path, rel: String) {
        self.game_paths.insert(path.to_path_buf(), rel);
        self.replace_pending_load(path);
    }

    /// Queue `path` to load next. Edits queued for another file that has not loaded yet were
    /// that file's: they are dropped rather than applied to this one, and the app queues this
    /// file's own (see [`Self::load_needing_edits`]).
    fn replace_pending_load(&mut self, path: &Path) {
        if self
            .pending_load
            .as_deref()
            .is_some_and(|pending| pending != path)
        {
            self.pending_edits = None;
            self.pending_structure = None;
            self.pending_edits_push_live = false;
        }
        self.pending_load = Some(path.to_path_buf());
    }

    /// Whether the open file's working copy holds the project's edits, so its diff may be read
    /// back into the project. Not while a project's edits for it wait to be applied, nor after
    /// another project opened until a file loads for it.
    pub fn working_is_current(&self) -> bool {
        !self.working_stale
            && !(self.pending_load.is_some()
                && self.pending_load == self.loaded_path
                && (self.pending_edits.is_some() || self.pending_structure.is_some()))
    }

    pub fn export_root(&self) -> &Path {
        &self.export_root
    }

    pub fn set_export_root(&mut self, root: PathBuf) {
        if self.export_root != root {
            self.export_root = root;
            self.rescan();
        }
    }

    /// Queue authored edits to apply after the next queued eff loads (project load path).
    pub fn queue_edits(&mut self, edits: Vec<AuthoredEdit>) {
        self.pending_edits = Some(edits);
        self.pending_edits_push_live = true;
    }

    /// The file a queued load is about to open (relative to the export root) when nothing has
    /// queued its edits yet. The app answers with [`Self::queue_carried_edits`], so a file
    /// switched away from and back to opens with its edits instead of pristine.
    pub fn load_needing_edits(&self) -> Option<String> {
        if self.pending_edits.is_some() || self.pending_structure.is_some() {
            return None;
        }
        Some(self.game_path_of(self.pending_load.as_ref()?))
    }

    /// The project's edits for the file a queued load opens. Applied without sending anything
    /// to the game: the game already has them.
    pub fn queue_carried_edits(
        &mut self,
        authored: Vec<AuthoredEdit>,
        rosters: Vec<crate::mod_project::EmitterRoster>,
        entry_edits: Vec<crate::mod_project::EntryEdit>,
    ) {
        self.pending_edits = Some(authored);
        self.pending_edits_push_live = false;
        self.queue_structure_edits(rosters, entry_edits);
    }

    /// Whether a file is open and parsed, so its edits can be read from it.
    pub fn has_loaded_effects(&self) -> bool {
        self.ptcl.is_some()
    }

    /// Queue the project's emitter lists and spawn structures for the next load.
    pub fn queue_structure_edits(
        &mut self,
        rosters: Vec<crate::mod_project::EmitterRoster>,
        entry_edits: Vec<crate::mod_project::EntryEdit>,
    ) {
        if rosters.is_empty() && entry_edits.is_empty() {
            return;
        }
        self.pending_structure = Some((rosters, entry_edits));
    }

    pub fn set_target_fighter(&mut self, fighter: Option<String>) {
        self.target_fighter = fighter;
    }

    /// Transplant ops recorded since the last drain (the app owns the project store).
    pub fn take_transplants(&mut self) -> Vec<TransplantOp> {
        std::mem::take(&mut self.pending_transplants)
    }

    pub fn take_transplant_removals(&mut self) -> Vec<EffTransplantRemoval> {
        std::mem::take(&mut self.pending_transplant_removals)
    }

    /// Texture replacements picked since the last drain (the app owns the project store).
    pub fn take_texture_imports(&mut self) -> Vec<crate::mod_project::TextureImport> {
        std::mem::take(&mut self.pending_texture_imports)
    }

    /// The project's texture replacements for the loaded eff, so the panel can show them.
    /// The moves that spawn each effect, by hash40 of its name; `ready` once every move has
    /// been read (until then an effect with no moves may just not have been reached).
    pub fn set_effect_uses(
        &mut self,
        uses: std::collections::HashMap<u64, Vec<MoveUse>>,
        ready: bool,
    ) {
        // The list moves the effects no move spawns into their fold once every move is read,
        // which shifts the rows: the open one is revealed again where it now is.
        if ready && !self.effect_uses_ready && self.selected_entry.is_some() {
            self.scroll_to_selected = true;
        }
        self.effect_uses = uses;
        self.effect_uses_ready = ready;
    }

    /// The character whose moves [`Self::set_effect_uses`] reads, when the open file is not
    /// that character's own; `None` on a character's own file.
    pub fn set_effect_uses_of(&mut self, of: Option<String>) {
        if self.effect_uses_of != of {
            self.effect_uses_of = of;
        }
    }

    /// What a row of the fold of effects no move spawns says of itself, and the fold's name.
    fn not_used_words(&self) -> (String, String, String) {
        match &self.effect_uses_of {
            Some(who) => (
                format!("Not in {who}'s moves"),
                format!(
                    "None of {who}'s move scripts spawns these. Only {who}'s moves were read: \
                     other characters' moves may."
                ),
                format!(
                    "None of {who}'s move scripts spawns it. Only {who}'s moves were read: \
                     other characters' moves may."
                ),
            ),
            None => (
                "Not used by any move".to_owned(),
                "No move script spawns these. The game may spawn them from code the editor \
                 can't read."
                    .to_owned(),
                "No move script spawns it. The game may spawn it from code the editor can't \
                 read."
                    .to_owned(),
            ),
        }
    }

    pub fn set_texture_imports(&mut self, imports: Vec<crate::mod_project::TextureImport>) {
        self.texture_imports = imports;
    }

    /// Drop project-owned overlays and queued operations before another project replaces it.
    /// Reloading the base file immediately afterwards resets the current authored diff too.
    pub fn reset_project_state(&mut self) {
        // What is open was loaded for the project before: its diff is not this one's. It
        // loads again, and the app hands it the new project's edits for it.
        self.working_stale = true;
        // Another project's steps would put its edits into this one.
        self.history = EffHistory::default();
        self.history_store.clear();
        if self.pending_load.is_none() {
            self.pending_load = self.loaded_path.clone();
        }
        self.pending_edits = None;
        self.pending_structure = None;
        self.pending_edits_push_live = false;
        self.pending_texture_imports.clear();
        self.pending_texture_additions.clear();
        self.pending_texture_removals.clear();
        self.pending_transplants.clear();
        self.pending_transplant_removals.clear();
        self.texture_imports.clear();
        self.merged_overlays.clear();
        self.edit_sources.clear();
    }

    /// Pool textures added since the last drain (the app owns the project store).
    pub fn take_texture_additions(&mut self) -> Vec<crate::mod_project::TextureAddition> {
        std::mem::take(&mut self.pending_texture_additions)
    }

    /// Pool textures removed since the last drain.
    pub fn take_texture_removals(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_texture_removals)
    }

    /// How many of the loaded eff's emitters sample pool texture `index`.
    ///
    /// Counts sampler0 ONLY, because that is all the editor's model carries — `convert_emitter`
    /// resolves `sampler0` and discards the other five. So this is a lower bound: a texture used
    /// only as a distortion or mask input reads as unused here. The export-side removal pass
    /// checks all six and refuses with a `texture-in-use` warning, which is the backstop.
    fn texture_users(&self, index: usize) -> usize {
        self.ptcl
            .as_ref()
            .map(|p| {
                p.emitter_sets
                    .iter()
                    .flat_map(|set| set.emitters.iter())
                    .filter(|em| {
                        texture_slots(em, &p.bntx_textures)
                            .iter()
                            .any(|(_, at)| *at == index)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Point texture slot `slot` of the selected part at pool texture `index`: texture 1 by its
    /// place in the pool, as the export names it, and 2 and 3 by the texture's id.
    fn point_slot_at(&mut self, slot: usize, index: usize) {
        if slot == 0 {
            self.point_selected_emitter_at(index);
            return;
        }
        let (Some(entry_idx), Some(ptcl)) = (self.selected_entry, self.ptcl.as_mut()) else {
            return;
        };
        let Some(guid) = ptcl.bntx_textures.get(index).and_then(|t| t.id) else {
            return;
        };
        let set_idx = self.entries[entry_idx].set_idx;
        if let Some(em) = ptcl
            .emitter_sets
            .get_mut(set_idx)
            .and_then(|s| s.emitters.get_mut(self.selected_emitter))
        {
            if attr(em, TEXTURE_SLOT_IDS[slot]).is_some() {
                set_attr(em, TEXTURE_SLOT_IDS[slot], AttrValue::UInt(guid));
                touch(&mut self.eff_dirty_at, &mut self.edit_rev);
            }
        }
    }

    /// Point the selected emitter at pool texture `index`.
    fn point_selected_emitter_at(&mut self, index: usize) {
        if let (Some(entry_idx), Some(ptcl)) = (self.selected_entry, self.ptcl.as_mut()) {
            let set_idx = self.entries[entry_idx].set_idx;
            if let Some(em) = ptcl
                .emitter_sets
                .get_mut(set_idx)
                .and_then(|s| s.emitters.get_mut(self.selected_emitter))
            {
                em.texture_index = Some(index as u32);
                touch(&mut self.eff_dirty_at, &mut self.edit_rev);
            }
        }
    }

    /// Adopt a rebuilt pool that has one texture appended, and point the emitter at it.
    ///
    /// The editor's own view has to move with the pool or the panel lies: the label list, the
    /// preview and the index a swap edit records all read from `bntx_textures`.
    fn adopt_appended_texture(&mut self, pool: Vec<u8>, info: TextureInfo) {
        self.texture_pool = Some(pool);
        self.texture_pool_gen += 1;
        // An appended texture leaves every texture number the steps hold where it was.
        let Some(ptcl) = self.ptcl.as_mut() else {
            return;
        };
        ptcl.bntx_textures.push(info.clone());
        let new_index = ptcl.bntx_textures.len() - 1;
        self.selected_texture = Some((new_index, info));
        self.point_selected_emitter_at(new_index);
    }

    fn current_edit_source(&self) -> Option<&EditSource> {
        let loaded = self.loaded_path.as_ref()?;
        self.edit_sources.iter().find(|source| {
            loaded == &source.base
                || source.alt_base.as_ref() == Some(loaded)
                || source.merged.as_ref() == Some(loaded)
        })
    }

    fn current_transplant_for_entry(&self, entry_name: &str) -> Option<(String, EffTransplant)> {
        let source = self.current_edit_source()?;
        source
            .transplants
            .iter()
            .find(|item| item.entry_name.eq_ignore_ascii_case(entry_name))
            .cloned()
            .map(|item| (source.fighter.clone(), item))
    }

    /// Which form the texture panel is showing and importing in.
    fn texture_form(&self) -> crate::texture_import::Form {
        if self.show_raw_textures {
            crate::texture_import::Form::Raw
        } else {
            crate::texture_import::Form::Editable
        }
    }

    /// Names of the loaded eff's pool textures, in pool order.
    fn texture_names(&self) -> Vec<String> {
        self.ptcl
            .as_ref()
            .map(|p| p.bntx_textures.iter().map(|t| t.tex_name.clone()).collect())
            .unwrap_or_default()
    }

    /// Diff the working PTCL against the pristine snapshots → absolute-value edit records.
    pub fn collect_authored_edits(&self) -> Vec<AuthoredEdit> {
        let Some(ptcl) = self.ptcl.as_ref() else {
            return Vec::new();
        };
        let tex_names = self.texture_names();
        let mut out = Vec::new();
        for (set_idx, (set, pset)) in ptcl.emitter_sets.iter().zip(&self.pristine).enumerate() {
            for (em_idx, em) in set.emitters.iter().enumerate() {
                // An emitter the roster added has no pristine of its own — it is a copy of one,
                // and `source_idx` names it. Diffing against that is what keeps a duplicate's
                // edits down to the fields the user actually changed on the copy.
                let Some(pr) = pset.get(em.source_idx) else {
                    continue;
                };
                let f = field_edits(em, pr, &tex_names);
                if !f.is_empty() {
                    // The entry (kind) name is a separate namespace from the emitter-set
                    // name and cannot be derived from it — resolve it via the entry list,
                    // which is the only place the set_idx → kind mapping exists.
                    let entry_name = self
                        .entries
                        .iter()
                        .find(|e| e.set_idx == set_idx)
                        .map(|e| e.name.clone())
                        .unwrap_or_default();
                    out.push(AuthoredEdit {
                        set_name: set.name.clone(),
                        entry_name,
                        set_idx,
                        emitter_name: em.name.clone(),
                        emitter_idx: em_idx,
                        fields: f,
                    });
                }
            }
        }
        out
    }

    /// Emitter lists that differ from the file's, as the roster records the exporter consumes.
    ///
    /// A set whose emitters are untouched produces nothing: the roster rebuilds the set from
    /// scratch, so recording one for every set would make every export depend on this code
    /// reproducing the file exactly, for no gain.
    pub fn collect_rosters(&self) -> Vec<crate::mod_project::EmitterRoster> {
        let Some(ptcl) = self.ptcl.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (set_idx, (set, pristine)) in ptcl.emitter_sets.iter().zip(&self.pristine).enumerate() {
            if part_list_untouched(&set.emitters, pristine) {
                continue;
            }
            out.push(crate::mod_project::EmitterRoster {
                set_name: set.name.clone(),
                entry_name: self
                    .entries
                    .iter()
                    .find(|e| e.set_idx == set_idx)
                    .map(|e| e.name.clone())
                    .unwrap_or_default(),
                set_idx,
                slots: set
                    .emitters
                    .iter()
                    .map(|em| crate::mod_project::EmitterSlot {
                        source_idx: em.source_idx,
                        // The name the SOURCE emitter has in the file, which is what the
                        // exporter looks it up by — not the name of the copy.
                        source_name: pristine
                            .get(em.source_idx)
                            .map(|p| p.name.clone())
                            .unwrap_or_default(),
                        name: em.name.clone(),
                        depth: em.depth,
                    })
                    .collect(),
            });
        }
        out
    }

    /// Entry spawn structures that differ from the file's.
    pub fn collect_entry_edits(&self) -> Vec<crate::mod_project::EntryEdit> {
        let set_name = |idx: Option<usize>| {
            idx.and_then(|i| {
                self.ptcl
                    .as_ref()?
                    .emitter_sets
                    .get(i)
                    .map(|s| s.name.clone())
            })
            .unwrap_or_default()
        };
        self.entry_info
            .iter()
            .zip(&self.entry_pristine)
            .filter_map(|(now, was)| {
                let emitter_set = (now.set_idx != was.set_idx).then(|| set_name(now.set_idx));
                let variants = (now.variants != was.variants).then(|| {
                    now.variants
                        .iter()
                        .map(|v| crate::mod_project::VariantEdit {
                            start_frame: v.start_frame,
                            set_name: set_name(v.set_idx),
                            bone: v.bone.clone(),
                        })
                        .collect()
                });
                let model = (now.model != was.model).then(|| match &now.model {
                    Some(m) => crate::mod_project::ModelEdit {
                        name: m.name.clone(),
                        flag: m.flag,
                    },
                    // An empty name is how "this effect no longer spawns a model" is expressed;
                    // there is no other way to say it in the format, where 0 means "none".
                    None => crate::mod_project::ModelEdit {
                        name: String::new(),
                        flag: 0,
                    },
                });
                (emitter_set.is_some() || variants.is_some() || model.is_some()).then(|| {
                    crate::mod_project::EntryEdit {
                        entry_name: now.name.clone(),
                        emitter_set,
                        variants,
                        model,
                    }
                })
            })
            .collect()
    }

    /// Re-apply saved emitter lists and spawn structures onto the loaded eff.
    pub fn apply_structure_edits(
        &mut self,
        rosters: &[crate::mod_project::EmitterRoster],
        entry_edits: &[crate::mod_project::EntryEdit],
    ) {
        for roster in rosters {
            let Some(ptcl) = self.ptcl.as_mut() else {
                return;
            };
            let Some(set_idx) = ptcl
                .emitter_sets
                .iter()
                .position(|s| s.name == roster.set_name)
                .or(Some(roster.set_idx).filter(|i| *i < ptcl.emitter_sets.len()))
            else {
                eprintln!(
                    "[EFF-PROJECT] emitter list for set '{}' does not resolve in this eff",
                    roster.set_name
                );
                continue;
            };
            // Built from the PRISTINE snapshots rather than from the list currently on screen.
            // A roster is a statement about the emitters the FILE has, so re-applying one — which
            // happens when a second project is opened over an eff this one already rostered —
            // must start from the same place every time, or the duplicates duplicate.
            let Some(source) = self.pristine.get(set_idx).map(|snapshots| {
                snapshots
                    .iter()
                    .enumerate()
                    .map(|(i, s)| s.to_def(i))
                    .collect::<Vec<_>>()
            }) else {
                continue;
            };
            let rebuilt: Vec<EmitterDef> = roster
                .slots
                .iter()
                .filter_map(|slot| {
                    // Stored name at the stored index wins, then the name anywhere, then the
                    // bare index — the same rule the exporter resolves a slot by.
                    let named = |i: usize| {
                        !slot.source_name.is_empty()
                            && source.get(i).map(|e| &e.name) == Some(&slot.source_name)
                    };
                    let idx = if named(slot.source_idx) {
                        slot.source_idx
                    } else {
                        (0..source.len())
                            .find(|i| named(*i))
                            .unwrap_or(slot.source_idx)
                    };
                    let mut em = source.get(idx)?.clone();
                    em.source_idx = idx;
                    em.depth = slot.depth;
                    if !slot.name.is_empty() {
                        em.name = slot.name.clone();
                    }
                    Some(em)
                })
                .collect();
            if rebuilt.is_empty() && !roster.slots.is_empty() {
                eprintln!(
                    "[EFF-PROJECT] emitter list for set '{}' resolved to nothing — left as the \
                     file has it",
                    roster.set_name
                );
                continue;
            }
            ptcl.emitter_sets[set_idx].emitters = rebuilt;
        }

        for edit in entry_edits {
            let set_idx = |name: &str| {
                self.ptcl
                    .as_ref()
                    .and_then(|p| p.emitter_sets.iter().position(|s| s.name == name))
            };
            let variants: Option<Vec<EffVariantInfo>> = edit.variants.as_ref().map(|list| {
                list.iter()
                    .map(|v| EffVariantInfo {
                        start_frame: v.start_frame,
                        set_idx: set_idx(&v.set_name),
                        bone: v.bone.clone(),
                    })
                    .collect()
            });
            let Some(entry) = self
                .entry_info
                .iter_mut()
                .find(|e| e.name.eq_ignore_ascii_case(&edit.entry_name))
            else {
                eprintln!(
                    "[EFF-PROJECT] spawn edit names entry '{}', which this eff does not have",
                    edit.entry_name
                );
                continue;
            };
            if let Some(name) = &edit.emitter_set {
                entry.set_idx = if name.is_empty() { None } else { set_idx(name) };
            }
            if let Some(variants) = variants {
                entry.variants = variants;
            }
            if let Some(model) = &edit.model {
                entry.model = (!model.name.is_empty()).then(|| EffModelInfo {
                    name: model.name.clone(),
                    flag: model.flag,
                });
            }
        }
    }

    /// Apply saved authored edits onto the loaded eff. Prefers name matches; falls back to
    /// stored indices with a warning (source dump may have changed between sessions).
    pub fn apply_authored_edits(&mut self, edits: &[AuthoredEdit]) {
        let tex_names = self.texture_names();
        let Some(ptcl) = self.ptcl.as_mut() else {
            return;
        };
        for edit in edits {
            let set_idx = ptcl
                .emitter_sets
                .iter()
                .position(|s| !edit.set_name.is_empty() && s.name == edit.set_name)
                .unwrap_or_else(|| {
                    eprintln!(
                        "[EFF-PROJECT] set '{}' not found by name; using stored index {}",
                        edit.set_name, edit.set_idx
                    );
                    edit.set_idx
                });
            let Some(set) = ptcl.emitter_sets.get_mut(set_idx) else {
                continue;
            };
            let em_idx = set
                .emitters
                .iter()
                .position(|e| !edit.emitter_name.is_empty() && e.name == edit.emitter_name)
                .unwrap_or(edit.emitter_idx);
            let Some(em) = set.emitters.get_mut(em_idx) else {
                continue;
            };

            let f = &edit.fields;
            // The five named scalars predate the attribute table. Fold them in first, so a
            // project saved by an older build lands on the same fields the table now owns and is
            // re-recorded as `attrs` the next time the editor collects. `attrs` is applied after
            // them and therefore wins where a project carries both.
            if let Some(v) = f.emission_rate {
                set_attr(em, "emission.rate", AttrValue::Float(v));
            }
            if let Some(v) = f.lifetime {
                set_attr(em, "particle_data.life", AttrValue::Int(v.round() as i64));
            }
            if let Some(v) = f.scale {
                // The old scalar tracked scale_x and applied its RATIO to the other two axes,
                // which is what preserved authored anisotropy. Reproduce that here rather than
                // writing v to all three, or reopening an old project would quietly square up
                // every deliberately-flattened particle.
                let base = attr_f32(em, "particle_scale.scale_x");
                let (y, z) = (
                    attr_f32(em, "particle_scale.scale_y"),
                    attr_f32(em, "particle_scale.scale_z"),
                );
                let r = if base.abs() > 1e-6 { v / base } else { 1.0 };
                set_attr(em, "particle_scale.scale_x", AttrValue::Float(v));
                set_attr(
                    em,
                    "particle_scale.scale_y",
                    AttrValue::Float(if base.abs() > 1e-6 { y * r } else { v }),
                );
                set_attr(
                    em,
                    "particle_scale.scale_z",
                    AttrValue::Float(if base.abs() > 1e-6 { z * r } else { v }),
                );
            }
            if let Some(v) = f.color_scale {
                set_attr(em, "emitter_static.color_scale", AttrValue::Float(v));
            }
            if let Some(v) = f.emitter_scale {
                set_attr(em, "emitter_info.scale_x", AttrValue::Float(v[0]));
                set_attr(em, "emitter_info.scale_y", AttrValue::Float(v[1]));
                set_attr(em, "emitter_info.scale_z", AttrValue::Float(v[2]));
            }
            for (id, value) in &f.attrs {
                set_attr(em, id, *value);
            }
            for edit in &f.subsections {
                let Some(section) = em.subsections.get_mut(edit.index) else {
                    continue;
                };
                if section.magic != edit.magic {
                    continue;
                }
                for (&offset, &value) in &edit.bytes {
                    if let Some(byte) = section.data.get_mut(offset) {
                        *byte = value;
                    }
                }
            }
            let apply_keys = |dst: &mut Vec<ColorKey>, rows: &Vec<[f32; 4]>| {
                for (k, row) in dst.iter_mut().zip(rows) {
                    k.r = row[0];
                    k.g = row[1];
                    k.b = row[2];
                    k.frame = row[3];
                }
            };
            if let Some(rows) = &f.color0 {
                apply_keys(&mut em.color0, rows);
            }
            if let Some(rows) = &f.color1 {
                apply_keys(&mut em.color1, rows);
            }
            if let Some(rows) = &f.alpha0 {
                for (k, row) in em.alpha0_keys.iter_mut().zip(rows) {
                    k.r = row[0];
                    k.frame = row[1];
                }
            }
            if let Some(wanted) = &f.texture_name {
                // A swap to a texture this file does not hold leaves the emitter showing what
                // it actually samples, which is the truth. The edit itself is untouched — it
                // may still resolve in the carrier, whose pool is assembled differently.
                if let Some(i) = tex_names.iter().position(|n| n == wanted) {
                    em.texture_index = Some(i as u32);
                }
            }
            // Older projects stored the friendly colour/key fields separately from the
            // general attribute map. Fold them into the complete table so both editor views
            // and the exporter observe the same final values.
            effective_keys_to_attrs(em);
        }
    }

    /// Note that the project now differs from what the running game was last given.
    ///
    /// Lights the Send button's unsent indicator. Used by changes that deliberately do NOT
    /// deploy — recording a transplant, for one — so the divergence is visible rather than
    /// silent, without taking the decision to send away from the user.
    pub fn mark_unsent(&mut self) {
        self.touch();
    }

    /// The project holds edits the running game was not given.
    pub fn is_unsent(&self) -> bool {
        self.eff_dirty_at.is_some()
    }

    /// An edit happened: there is something unsent, and the preview is out of date.
    fn touch(&mut self) {
        touch(&mut self.eff_dirty_at, &mut self.edit_rev);
    }

    /// Changes whenever what the editor holds changes (an edit or another file loaded).
    pub fn edit_rev(&self) -> u64 {
        self.edit_rev
    }

    /// The file the editor has open, as loaded (the base file, even when a merged build with
    /// transplants is what was parsed).
    pub fn loaded_path(&self) -> Option<&Path> {
        self.loaded_path.as_deref()
    }

    /// The selected effect's name, as the game spawns it.
    /// The move open in the main window (its plain name), which "Use in" adds effects to.
    pub fn set_open_move(&mut self, open_move: Option<String>, motion: Option<String>) {
        self.open_move = open_move;
        self.open_motion = motion;
    }

    /// The open move's effects, by name, with the first frame each spawns on.
    pub fn set_open_move_effects(&mut self, effects: Vec<(String, u32)>) {
        self.open_move_effects = effects;
    }

    /// Whether the open file is the shared one every fighter plays effects from.
    pub fn loaded_is_shared(&self) -> bool {
        self.loaded_path.as_deref().is_some_and(is_shared_file)
    }

    /// The open move's effects that are in the shared file, when another file is open, in the
    /// order they spawn.
    fn open_move_shared_effects(&self) -> Vec<String> {
        if self.loaded_is_shared() || self.open_motion.is_none() {
            return Vec::new();
        }
        let mut shared: Vec<&(String, u32)> = self
            .open_move_effects
            .iter()
            .filter(|(name, _)| is_shared_effect(name))
            .collect();
        shared.sort_by_key(|(_, start)| *start);
        let mut names: Vec<String> = Vec::new();
        for (name, _) in shared {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    /// The first frame the open move spawns `name` on, if its script does.
    fn open_move_start(&self, name: &str) -> Option<u32> {
        self.open_move_effects
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, start)| *start)
            .min()
    }

    /// Whether "Use in <move>" can add `entry_name` to the open move, and its hover. A move
    /// that already plays the effect would get a second copy of it.
    fn use_in_move_state(&self, entry_name: &str) -> (bool, String) {
        match (&self.open_move, self.open_move_start(entry_name)) {
            (None, _) => (false, "Open a move in the Classic editor first.".to_owned()),
            (Some(name), Some(frame)) => {
                (false, format!("{name} already plays it from frame {frame}"))
            }
            (Some(_), None) => (
                true,
                "Add this effect to the open move at the current frame, on the fighter's root"
                    .to_owned(),
            ),
        }
    }

    /// The moves that play the selected effect.
    fn selected_uses(&self) -> &[MoveUse] {
        self.selected_entry
            .and_then(|i| self.entries.get(i))
            .and_then(|entry| self.effect_uses.get(&entry.hash))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The bones the moves that play the selected effect spawn it on, once every move has been
    /// read.
    pub fn selected_spawn_bones(&self) -> Option<Vec<&str>> {
        self.effect_uses_ready.then(|| {
            self.selected_uses()
                .iter()
                .flat_map(|found| found.bones.iter().map(String::as_str))
                .collect()
        })
    }

    /// Whether the open move plays the selected effect, by its script or an object it makes.
    pub fn open_move_plays_selected(&self) -> bool {
        self.open_motion.as_deref().is_some_and(|motion| {
            self.selected_uses()
                .iter()
                .any(|found| found.motion.as_deref() == Some(motion))
        })
    }

    /// The view's switch between the effect alone and a move that plays it. Drawn over the
    /// view by the app.
    pub fn draw_view_switch(&mut self, ui: &mut Ui, in_move: bool) {
        if self.selected_entry.is_none() {
            return;
        }
        let uses: Vec<MoveUse> = self
            .selected_uses()
            .iter()
            .filter(|found| found.motion.is_some())
            .cloned()
            .collect();
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.view == EffectView::Alone || !in_move, "Alone")
                .on_hover_text("Play the effect by itself, over and over")
                .clicked()
            {
                self.view = EffectView::Alone;
            }
            let showing_move = self.view == EffectView::InMove && in_move;
            let current = if showing_move {
                self.open_move.clone().unwrap_or_default()
            } else {
                "In a move".to_owned()
            };
            ui.add_enabled_ui(!uses.is_empty(), |ui| {
                let combo = egui::ComboBox::from_id_salt("effect_view_move")
                    .selected_text(egui::RichText::new(current).color(if showing_move {
                        ui.visuals().strong_text_color()
                    } else {
                        ui.visuals().text_color()
                    }))
                    .height(360.0)
                    .show_ui(ui, |ui| {
                        for found in &uses {
                            let here = showing_move
                                && found.motion.as_deref() == self.open_motion.as_deref();
                            if ui.selectable_label(here, &found.label).clicked() {
                                self.view = EffectView::InMove;
                                if self.open_motion != found.motion {
                                    self.open_move_request = found.motion.clone();
                                }
                            }
                        }
                    });
                combo
                    .response
                    .on_hover_text("Play a move that spawns this effect, as it plays in the game");
            })
            .response
            // Short lines: one long line ran off the view's edge, under the right panel.
            .on_disabled_hover_text(match (&self.effect_uses_of, self.effect_uses_ready) {
                (_, false) => "Still reading the moves".to_owned(),
                (Some(who), true) => format!("None of {who}'s\nmove scripts spawns it"),
                (None, true) => {
                    "No move script spawns this effect.\nThe game may spawn it\nfrom its own code."
                        .to_owned()
                }
            });
        });
    }

    /// The emitter (index in the selected effect's set) the view draws alone, if any.
    pub fn solo_emitter(&self) -> Option<usize> {
        let name = self.selected_entry_name()?;
        (self.solo_selected.as_deref() == Some(name)).then_some(self.selected_emitter)
    }

    pub fn selected_entry_name(&self) -> Option<&str> {
        self.entries
            .get(self.selected_entry?)
            .map(|entry| entry.name.as_str())
    }

    /// Ask the app to rebuild the live carrier and hand it to the running game.
    ///
    /// The fighter's OWN eff is not the target and never can be: `reparse_game_path` rebuilds
    /// the parsed emitter structs from the resident buffer and never re-requests the file, so
    /// edited bytes went unread mid-match (`cb_game=0`) and only appeared after a reboot. The
    /// carrier's eff IS reloadable, so an edited effect is cloned into it with its edits baked
    /// in and the original kind is aliased onto the clone. Transplants, texture imports and
    /// texture swaps all ride that same rebuild.
    pub fn request_live_apply(&mut self) {
        // The game is about to be given everything we hold, so the unsent marker goes out here —
        // the one place that is true of, whichever button or project load asked for the send.
        self.mark_sent();
        self.live_deploy_request = true;
        self.sending = true;
        self.deploy_defer = true;
    }

    /// Everything the editor holds was just handed to the game (Send edits, or the send of
    /// everything when the game connects): nothing is unsent any more.
    pub fn mark_sent(&mut self) {
        self.eff_dirty_at = None;
    }

    /// Drain the deploy request (app-side, once per frame). Returns true when the app
    /// should rebuild + hot-reload the loaded fighter's eff.
    pub fn take_live_deploy_request(&mut self) -> bool {
        if !self.live_deploy_request {
            return false;
        }
        // Let one frame paint with the spinner up before the blocking rebuild starts.
        if self.deploy_defer {
            self.deploy_defer = false;
            return false;
        }
        self.live_deploy_request = false;
        self.live_deploy_at = Some(Instant::now());
        true
    }

    /// Report the outcome of a serviced deploy back into the editor's status line, so the
    /// user still sees exactly what was sent (this replaces the old "queued eff-derived
    /// modifiers for …" note).
    pub fn set_sent_note(&mut self, note: impl Into<String>) {
        self.last_sent_note = Some(note.into());
    }

    /// Drop the outcome of the last send and stop waiting on it.
    pub fn forget_send_result(&mut self) {
        self.awaiting_game = None;
        self.last_carrier_result = None;
        self.last_carrier_detail = None;
        self.last_sent_note = None;
        self.carrier_ok = false;
    }

    // ── Edit scope ────────────────────────────────────────────────────────────

    /// Indices of the emitters in this set that differ from pristine. Purely informational
    /// (the panel shows WHAT is edited); the edits themselves travel as absolute values
    /// through `collect_authored_edits`, per emitter, never as an aggregate.
    /// Keep the emitter selection inside the set being shown.
    ///
    /// Clamping used to happen into a LOCAL index, leaving `selected_emitter` out of range —
    /// so moving from a ten-emitter entry to a three-emitter one drew emitter 2's fields with
    /// no tab highlighted, and the texture panel's swap (which writes through
    /// `selected_emitter`) silently landed on nothing at all.
    fn clamp_selected_emitter(&mut self, set_idx: usize) {
        let count = self
            .ptcl
            .as_ref()
            .and_then(|p| p.emitter_sets.get(set_idx))
            .map(|s| s.emitters.len())
            .unwrap_or(0);
        if self.selected_emitter >= count {
            self.selected_emitter = 0;
        }
    }

    /// Emitter-set indices holding at least one authored edit.
    ///
    /// One pass over the file rather than `edited_emitters` per entry: the entry list draws
    /// every frame, and re-deriving the texture-name table for each of sixty entries is the
    /// difference between free and noticeable.
    fn edited_sets(&self) -> std::collections::HashSet<usize> {
        let Some(ptcl) = self.ptcl.as_ref() else {
            return std::collections::HashSet::new();
        };
        let tex_names = self.texture_names();
        ptcl.emitter_sets
            .iter()
            .zip(&self.pristine)
            .enumerate()
            .filter(|(_, (set, pristine))| {
                !part_list_untouched(&set.emitters, pristine)
                    || set.emitters.iter().any(|e| {
                        pristine
                            .get(e.source_idx)
                            .is_none_or(|p| !field_edits(e, p, &tex_names).is_empty())
                    })
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn edited_emitters(&self, set_idx: usize) -> Vec<usize> {
        let (Some(ptcl), Some(pristine)) = (self.ptcl.as_ref(), self.pristine.get(set_idx)) else {
            return Vec::new();
        };
        let Some(set) = ptcl.emitter_sets.get(set_idx) else {
            return Vec::new();
        };
        let tex_names = self.texture_names();
        // Paired by `source_idx`, not by position: once the emitter LIST has been edited the two
        // are different lengths, and zipping them would diff each emitter against whichever
        // pristine emitter happened to land at the same index.
        set.emitters
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                pristine
                    .get(e.source_idx)
                    .is_none_or(|p| !field_edits(e, p, &tex_names).is_empty())
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The parts of a set that were copied in or moved, by position in the list.
    fn moved_parts(&self, set_idx: usize) -> Vec<bool> {
        let Some(set) = self.ptcl.as_ref().and_then(|p| p.emitter_sets.get(set_idx)) else {
            return Vec::new();
        };
        let pristine = self.pristine.get(set_idx).map_or(&[][..], |p| p);
        let parts: Vec<(usize, bool)> = set
            .emitters
            .iter()
            .map(|e| {
                let copy = pristine.get(e.source_idx).is_none_or(|p| p.name != e.name);
                (e.source_idx, copy)
            })
            .collect();
        added_or_moved(&parts)
    }

    /// Whether an effect's timing (its extra parts, their frames, its model) differs from the
    /// file's.
    fn entry_timing_edited(&self, entry_name: &str) -> bool {
        let now = self.entry_info.iter().find(|e| e.name == entry_name);
        let was = self.entry_pristine.iter().find(|e| e.name == entry_name);
        now != was
    }

    /// The effects in the list that differ from the game, by position: a part's values, the
    /// part list, or the effect's timing.
    fn edited_entries(&self) -> Vec<bool> {
        let edited_sets = self.edited_sets();
        self.entries
            .iter()
            .map(|e| edited_sets.contains(&e.set_idx) || self.entry_timing_edited(&e.name))
            .collect()
    }

    // ── History ─────────────────────────────────────────────────────────────────

    /// What a file's steps depend on: its effects, their parts and its textures, as loaded. A
    /// step holds parts by position, so a file that loads with another list (a borrow, a
    /// texture deleted) must not get its old steps back.
    fn file_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.loaded_is_merged.hash(&mut h);
        if let Some(ptcl) = &self.ptcl {
            for set in &ptcl.emitter_sets {
                set.name.hash(&mut h);
            }
            for texture in &ptcl.bntx_textures {
                texture.tex_name.hash(&mut h);
            }
        }
        for set in &self.pristine {
            set.len().hash(&mut h);
            for part in set {
                part.name.hash(&mut h);
            }
        }
        for entry in &self.entry_pristine {
            entry.name.hash(&mut h);
        }
        h.finish()
    }

    /// Put the open file's history aside, to come back when the file does.
    fn stash_history(&mut self) {
        let mut history = std::mem::take(&mut self.history);
        let (Some(path), Some(_)) = (self.loaded_path.clone(), self.ptcl.as_ref()) else {
            return;
        };
        if history.undo.is_empty() && history.redo.is_empty() {
            return;
        }
        let key = self.file_fingerprint();
        self.history_store.retain(|(p, _, _)| *p != path);
        history.baseline = None;
        history.open_since = None;
        history.cleared = None;
        self.history_store.push((path, key, history));
        if self.history_store.len() > HISTORY_FILES {
            self.history_store.remove(0);
        }
    }

    /// Take back the history `path` had, when the file is still the one it was taken in.
    fn restore_history(&mut self, path: &Path) {
        let key = self.file_fingerprint();
        if let Some(at) = self.history_store.iter().position(|(p, _, _)| p == path) {
            let (_, was, history) = self.history_store.remove(at);
            if was == key {
                self.history = history;
            } else {
                self.history.cleared = Some("the file's effects or textures changed");
            }
        }
    }

    /// Drop every step of the open file, saying why in the Undo hover.
    fn clear_history(&mut self, why: &'static str) {
        let had = !self.history.undo.is_empty() || !self.history.redo.is_empty();
        self.history = EffHistory {
            cleared: had.then_some(why),
            ..Default::default()
        };
    }

    /// What changed between two states of one set, in words, with the part and the effect it is
    /// in: "Colour key 2 on Fire 1 in Fb shoot". `touched` names the row the edit was made on.
    fn step_label(
        &self,
        before: &EffHistoryStep,
        after: &EffHistoryStep,
        touched: Option<&str>,
    ) -> String {
        let table = crate::eff_attrs::table();
        let parts_same = before.emitters.len() == after.emitters.len()
            && before.emitters.iter().zip(&after.emitters).all(|(a, b)| {
                a.source_idx == b.source_idx && a.name == b.name && a.depth == b.depth
            });
        let changed_part = before
            .emitters
            .iter()
            .zip(&after.emitters)
            .position(|(a, b)| a != b);
        let key_named = |name: &str, a: &[ColorKey], b: &[ColorKey]| {
            let at = (0..a.len().max(b.len())).find(|&i| a.get(i) != b.get(i))?;
            Some(format!("{name} {}", at + 1))
        };
        let (what, part) = if !parts_same {
            ("the part list".to_owned(), None)
        } else if let Some(i) = changed_part {
            let (a, b) = (&before.emitters[i], &after.emitters[i]);
            // The colour keys first: they are mirrored into the attribute table, where a colour
            // picked with the swatch read as "Second colour key 1 X".
            let what = touched
                .map(str::to_owned)
                .or_else(|| key_named("Colour key", &a.color0, &b.color0))
                .or_else(|| key_named("Second colour key", &a.color1, &b.color1))
                .or_else(|| key_named("Opacity key", &a.alpha0_keys, &b.alpha0_keys))
                .or_else(|| {
                    let at = (0..a.attrs.len().min(b.attrs.len()))
                        .find(|&i| a.attrs[i] != b.attrs[i])?;
                    table.get(at).map(|t| t.label.to_owned())
                })
                .unwrap_or_else(|| {
                    if a.texture_index != b.texture_index {
                        "Texture".to_owned()
                    } else {
                        "Animation".to_owned()
                    }
                });
            (what, Some(i))
        } else if before.entry_info != after.entry_info {
            ("timing".to_owned(), None)
        } else {
            ("an edit".to_owned(), None)
        };
        let part = part.and_then(|i| {
            let names: Vec<&str> = after.emitters.iter().map(|e| e.name.as_str()).collect();
            part_list_names(&names).get(i).cloned()
        });
        let effect = before
            .selected_entry
            .filter(|i| {
                self.entries
                    .get(*i)
                    .is_some_and(|e| e.set_idx == before.set_idx)
            })
            .or_else(|| {
                self.entries
                    .iter()
                    .position(|e| e.set_idx == before.set_idx)
            })
            .and_then(|i| entry_list_names(&self.entries).get(i).cloned());
        match (part, effect) {
            (Some(part), Some(effect)) => format!("{what} on {part} in {effect}"),
            (None, Some(effect)) => format!("{what} in {effect}"),
            (Some(part), None) => format!("{what} on {part}"),
            (None, None) => what,
        }
    }

    /// The name the open step will get, if one is open.
    fn open_step_label(&self) -> Option<String> {
        self.history.open_since?;
        let before = self.history.baseline.as_ref()?;
        let now = self.history_capture_set(before.set_idx)?;
        Some(
            self.history
                .next_label
                .clone()
                .unwrap_or_else(|| self.step_label(before, &now, self.history.touched.as_deref())),
        )
    }

    /// The steps Undo can go back through, the next one first, and those Redo can go forward
    /// through, the next one first.
    fn history_steps(&self) -> (Vec<String>, Vec<String>) {
        // Each line starts with a capital, as a list does; the hovers read "Undo the part list".
        let line = |label: &str| -> String {
            let mut chars = label.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        };
        let mut undo: Vec<String> = self.open_step_label().iter().map(|l| line(l)).collect();
        undo.extend(self.history.undo.iter().rev().map(|step| line(&step.label)));
        let redo = if self.history.open_since.is_some() {
            Vec::new()
        } else {
            self.history
                .redo
                .iter()
                .rev()
                .map(|step| line(&step.label))
                .collect()
        };
        (undo, redo)
    }

    /// What Undo would undo now, in words.
    fn undo_label(&self) -> Option<String> {
        if self.history.open_since.is_some() {
            return self.open_step_label();
        }
        self.history.undo.last().map(|step| step.label.clone())
    }

    fn redo_label(&self) -> Option<String> {
        if self.history.open_since.is_some() {
            return None;
        }
        self.history.redo.last().map(|step| step.label.clone())
    }

    /// The selected effect's set as it is now, to keep as a step.
    fn history_capture(&self) -> Option<EffHistoryStep> {
        let set_idx = self.entries.get(self.selected_entry?)?.set_idx;
        self.history_capture_set(set_idx)
    }

    fn history_capture_set(&self, set_idx: usize) -> Option<EffHistoryStep> {
        let set = self.ptcl.as_ref()?.emitter_sets.get(set_idx)?;
        Some(EffHistoryStep {
            label: String::new(),
            set_idx,
            selected_entry: self.selected_entry,
            selected_emitter: self.selected_emitter,
            emitters: set.emitters.clone(),
            entry_info: self.entry_info.clone(),
        })
    }

    /// Close the open step, if there is one: what was there before it goes on the undo list.
    fn history_close_step(&mut self) {
        if self.history.open_since.take().is_none() {
            return;
        }
        let now = self.history_capture();
        if let Some(mut before) = self.history.baseline.take() {
            // An edit that changed nothing (a value typed back to what it was) is no step.
            let same = now.as_ref().is_some_and(|now| {
                now.set_idx == before.set_idx
                    && now.emitters == before.emitters
                    && now.entry_info == before.entry_info
            });
            if !same {
                let touched = self.history.touched.clone();
                before.label = self
                    .history
                    .next_label
                    .take()
                    .or_else(|| {
                        now.as_ref()
                            .map(|now| self.step_label(&before, now, touched.as_deref()))
                    })
                    .unwrap_or_default();
                self.history.undo.push(before);
                if self.history.undo.len() > HISTORY_LIMIT {
                    self.history.undo.remove(0);
                }
                self.history.redo.clear();
                self.history.cleared = None;
            }
        }
        self.history.next_label = None;
        self.history.touched = None;
        self.history.baseline = now;
    }

    /// Runs before the page draws: closes a finished step, and keeps the baseline on the
    /// effect on screen.
    fn history_begin_frame(&mut self, pointer_down: bool, now: Instant) {
        if self
            .history
            .open_since
            .is_some_and(|at| !pointer_down && now.duration_since(at) >= HISTORY_STEP_GAP)
        {
            self.history_close_step();
        }
        let set_idx = self
            .selected_entry
            .and_then(|i| self.entries.get(i))
            .map(|e| e.set_idx);
        let other_set = self.history.baseline.as_ref().map(|b| b.set_idx) != set_idx;
        if other_set {
            self.history_close_step();
        }
        if other_set
            || (self.history.open_since.is_none() && self.edit_rev != self.history.seen_rev)
        {
            self.history.baseline = self.history_capture();
        }
        self.history.seen_rev = self.edit_rev;
        self.history.frame_part = self.selected_emitter;
    }

    /// An edit was made on the page this frame.
    fn history_note_edit(&mut self, now: Instant) {
        if let Some(baseline) = self.history.baseline.as_mut() {
            if self.history.open_since.is_none() {
                baseline.selected_emitter = self.history.frame_part;
            }
            self.history.open_since = Some(now);
        }
        self.history.seen_rev = self.edit_rev;
    }

    pub fn can_undo(&self) -> bool {
        self.history.open_since.is_some() || !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        self.history.open_since.is_none() && !self.history.redo.is_empty()
    }

    /// Undo the last effect edit, and show the effect and part it was made in.
    pub fn undo(&mut self) {
        self.history_close_step();
        if let Some(step) = self.history.undo.pop() {
            if let Some(now) = self.history_restore(step) {
                self.history.redo.push(now);
            }
        }
    }

    /// Undo `steps` steps at once, as picked in the history list.
    fn undo_steps(&mut self, steps: usize) {
        for _ in 0..steps {
            self.undo();
        }
    }

    fn redo_steps(&mut self, steps: usize) {
        for _ in 0..steps {
            self.redo();
        }
    }

    pub fn redo(&mut self) {
        self.history_close_step();
        if let Some(step) = self.history.redo.pop() {
            if let Some(now) = self.history_restore(step) {
                self.history.undo.push(now);
            }
        }
    }

    /// Put `step` back, returning the state it replaced.
    fn history_restore(&mut self, step: EffHistoryStep) -> Option<EffHistoryStep> {
        let mut replaced = self.history_capture_set(step.set_idx)?;
        replaced.label = step.label.clone();
        // The step being undone was made with this selection; the redo step returns to it.
        replaced.selected_entry = step.selected_entry;
        replaced.selected_emitter = step.selected_emitter;
        let set = self.ptcl.as_mut()?.emitter_sets.get_mut(step.set_idx)?;
        set.emitters = step.emitters;
        self.entry_info = step.entry_info;
        if step.selected_entry != self.selected_entry {
            self.remember_picks();
            self.selected_entry = step.selected_entry;
            self.scroll_to_selected = true;
        }
        self.selected_emitter = step.selected_emitter;
        self.clamp_selected_emitter(step.set_idx);
        self.touch();
        self.history.seen_rev = self.edit_rev;
        self.history.baseline = self.history_capture();
        Some(replaced)
    }

    /// Put the effect `entry_name` playing set `set_idx` back as the game has it: every part's
    /// values, the part list (copies gone, removed parts back, the file's order) and the
    /// effect's timing.
    fn reset_effect(&mut self, set_idx: usize, entry_name: &str) {
        let (Some(ptcl), Some(pristine_set)) = (self.ptcl.as_mut(), self.pristine.get(set_idx))
        else {
            return;
        };
        let Some(set) = ptcl.emitter_sets.get_mut(set_idx) else {
            return;
        };
        set.emitters = pristine_set
            .iter()
            .enumerate()
            .map(|(i, p)| p.to_def(i))
            .collect();
        let was = self
            .entry_pristine
            .iter()
            .find(|e| e.name == entry_name)
            .cloned();
        if let (Some(now), Some(was)) = (
            self.entry_info.iter_mut().find(|e| e.name == entry_name),
            was,
        ) {
            *now = was;
        }
        self.clamp_selected_emitter(set_idx);
        touch(&mut self.eff_dirty_at, &mut self.edit_rev);
    }

    /// Whether the effect differs from the game in anything Reset effect puts back.
    fn effect_edited(&self, set_idx: usize, entry_name: &str) -> bool {
        self.edited_sets().contains(&set_idx) || self.entry_timing_edited(entry_name)
    }

    /// "Reset Flare 1 in Afterburner line blue", for the Undo hover.
    fn reset_part_label(&self, set_idx: usize, part: usize) -> String {
        let part_name = self
            .ptcl
            .as_ref()
            .and_then(|p| p.emitter_sets.get(set_idx))
            .and_then(|set| {
                let names: Vec<&str> = set.emitters.iter().map(|e| e.name.as_str()).collect();
                part_list_names(&names).get(part).cloned()
            })
            .unwrap_or_else(|| "part".to_owned());
        let effect = self
            .selected_entry
            .and_then(|i| entry_list_names(&self.entries).get(i).cloned());
        match effect {
            Some(effect) => format!("Reset {part_name} in {effect}"),
            None => format!("Reset {part_name}"),
        }
    }

    /// Put one part of the effect in `set_idx` back as the game has it.
    fn reset_part_fields(&mut self, set_idx: usize, part: usize) {
        let (Some(ptcl), Some(pristine_set)) = (self.ptcl.as_mut(), self.pristine.get(set_idx))
        else {
            return;
        };
        let Some(e) = ptcl
            .emitter_sets
            .get_mut(set_idx)
            .and_then(|set| set.emitters.get_mut(part))
        else {
            return;
        };
        if let Some(p) = pristine_set.get(e.source_idx) {
            p.restore(e);
            touch(&mut self.eff_dirty_at, &mut self.edit_rev);
        }
    }

    // ── UI ────────────────────────────────────────────────────────────────────

    /// Queue an eff to show in the editor (e.g. the selected fighter's ef_*.eff).
    /// Loads lazily on the next frame the window is open — cheap while it's closed.
    pub fn queue_load(&mut self, path: &Path) {
        // Already showing this file and no reload pending → nothing to do. (A pending
        // reload — e.g. a merged overlay just changed under the loaded file — survives.)
        // A working copy from before another project opened loads again, with this one's edits.
        if self.loaded_path.as_deref() == Some(path)
            && self.pending_load.is_none()
            && !self.working_stale
        {
            return;
        }
        self.replace_pending_load(path);
    }

    /// Replace the edit-source list (the app's per-fighter eff mods). This is the single
    /// source of truth for the "Editing:" selector AND the overlay map — every base path
    /// (export-root AND data-root variants) redirects to the fighter's merged build, so
    /// transplanted entries show no matter which path a load request used.
    pub fn set_edit_sources(&mut self, sources: Vec<EditSource>) {
        let mut overlays = std::collections::HashMap::new();
        for s in &sources {
            if let Some(m) = &s.merged {
                overlays.insert(s.base.clone(), m.clone());
                if let Some(alt) = &s.alt_base {
                    overlays.insert(alt.clone(), m.clone());
                }
            }
        }
        // If the overlay for the currently loaded base just appeared/changed, reload so
        // the merged entries show up without any user action.
        if let Some(loaded) = &self.loaded_path {
            if overlays.get(loaded) != self.merged_overlays.get(loaded)
                && self.pending_load.is_none()
            {
                self.pending_load = Some(loaded.clone());
            }
        }
        self.merged_overlays = overlays;
        self.edit_sources = sources;
    }

    /// Forget one fighter's editor-owned effect view without touching the source `.eff` files.
    ///
    /// The main app removes the fighter's project record and live deployment separately. This
    /// side clears the corresponding overlay, edit-source row, and any loaded/queued view so a
    /// forgotten fighter cannot remain visible in the standalone editor window.
    pub fn forget_fighter(&mut self, fighter: &str) {
        let belongs = |path: &Path| {
            let path = path.to_string_lossy().replace('\\', "/").to_lowercase();
            let prefix = format!("effect/fighter/{}/", fighter.to_lowercase());
            path.starts_with(&prefix) || path.contains(&format!("/{prefix}"))
        };
        let loaded_belongs = self.loaded_path.as_deref().is_some_and(belongs);
        let pending_belongs = self.pending_load.as_deref().is_some_and(belongs);

        self.merged_overlays.retain(|base, _| !belongs(base));
        self.edit_sources
            .retain(|source| !source.fighter.eq_ignore_ascii_case(fighter));
        if pending_belongs {
            self.pending_load = None;
        }
        if loaded_belongs {
            self.clear_loaded_view();
        }
    }

    /// Clear the parsed effect view after its source has been forgotten. The window itself stays
    /// open, but it now truthfully has no loaded effect instead of retaining stale entries.
    fn clear_loaded_view(&mut self) {
        self.pending_load = None;
        self.pending_select = None;
        self.pending_edits = None;
        self.pending_structure = None;
        self.pending_edits_push_live = false;
        self.loaded_path = None;
        self.loaded_is_merged = false;
        self.load_error = None;
        self.ptcl = None;
        self.texture_pool = None;
        self.pending_texture_imports.clear();
        self.pending_texture_additions.clear();
        self.pending_texture_removals.clear();
        self.texture_imports.clear();
        self.texture_note = None;
        self.selected_texture = None;
        self.texture_preview = None;
        self.texture_form_note.clear();
        self.pristine.clear();
        self.entries.clear();
        self.selected_entry = None;
        self.selected_emitter = 0;
        self.entry_info.clear();
        self.entry_pristine.clear();
        self.pending_transplants.clear();
        self.pending_transplant_removals.clear();
        self.eff_dirty_at = None;
        self.live_deploy_request = false;
        self.live_deploy_at = None;
        self.sending = false;
        self.awaiting_game = None;
        self.last_carrier_result = None;
        self.last_carrier_detail = None;
        self.carrier_ok = false;
        self.deploy_defer = false;
        self.last_sent_note = None;
    }

    /// Point `base` (a fighter's real eff path) at `merged` (its transplants-applied build).
    /// While set, ANY load of `base` — the file combo, fighter auto-follow, project load —
    /// parses the merged file instead. Pass None to drop the overlay. If `base` is the
    /// currently loaded file, it reloads so the view updates immediately.
    pub fn set_merged_overlay(&mut self, base: &Path, merged: Option<&Path>) {
        match merged {
            Some(m) => {
                self.merged_overlays
                    .insert(base.to_path_buf(), m.to_path_buf());
            }
            None => {
                self.merged_overlays.remove(base);
            }
        }
        if self.loaded_path.as_deref() == Some(base) {
            // Carry in-progress emitter edits across the reload.
            //
            // The merged baseline deliberately does NOT bake authored edits in (see
            // `build_merged_preview`), so re-parsing it resets every emitter to pristine and
            // the working-vs-pristine diff goes empty — the edits vanish from the panel, and
            // the next `sync_eff_mods_from_editor` then writes that empty diff back over the
            // project's `authored` list. Capture them here, while the old working copy is
            // still loaded, and re-apply after the load.
            //
            // Not `queue_edits`: this reload is ours, not a project open, so it must not also
            // deploy to the running game.
            if self.pending_edits.is_none() {
                let carried = self.collect_authored_edits();
                if !carried.is_empty() {
                    self.pending_edits = Some(carried);
                    self.pending_edits_push_live = false;
                }
            }
            // The emitter lists and spawn structures go the same way and for the same reason:
            // the merged baseline has neither baked in, so a reload would drop them.
            if self.pending_structure.is_none() {
                self.queue_structure_edits(self.collect_rosters(), self.collect_entry_edits());
            }
            self.pending_load = Some(base.to_path_buf());
        }
    }

    /// Select this entry (by name, case-insensitive) once the pending/current eff is
    /// loaded — used to land on a freshly transplanted or replaced entry.
    pub fn queue_select(&mut self, name: &str) {
        // Never leave the previous donor highlighted while a new merged preview is pending.
        // If the rebuild/load fails, an empty selection is truthful; showing the old entry
        // made a Bomberman request appear to have transplanted Alucard.
        self.remember_picks();
        self.selected_entry = None;
        self.selected_emitter = 0;
        self.pending_select = Some(name.to_lowercase());
        self.open = true; // surface the result
    }

    fn apply_pending_select(&mut self) {
        let Some(want) = self.pending_select.take() else {
            return;
        };
        self.selected_entry = self.entries.iter().position(|e| e.name == want);
        // Back on the part last picked in it.
        self.selected_emitter = self.parts_picked.get(&want).copied().unwrap_or(0);
        self.scroll_to_selected = true;
        // Opened from a move's effect row: show it in that move.
        self.view = EffectView::InMove;
    }

    /// Per-frame work while the Effects page is open: scan the files, and apply a queued load
    /// and the project's edits to it. Drawing is done by the `page_*` parts the app lays out.
    pub fn update(&mut self, ctx: &egui::Context, link: &GameLink) {
        // A result belongs to the connection it came from: after a drop it describes a match
        // the editor can no longer see, and a reconnect sends everything again.
        if !link.status().is_connected() {
            self.forget_send_result();
        } else {
            self.game_seen = true;
        }
        // A project load queues its edits here; they apply even while the page is closed, so
        // the project store and the preview see them without the page having been opened.
        let pending = self.pending_load.is_some()
            && (self.pending_edits.is_some() || self.pending_structure.is_some());
        if !self.open && !pending {
            return;
        }
        link.ensure_started();
        // Before a load: the app sets the open move from the loaded file after this runs, so a
        // file is looked at the frame after it loads.
        self.open_first_effect_once();
        if self.eff_files.is_empty() && self.scan_error.is_none() {
            self.rescan();
        }
        if let Some(path) = self.pending_load.take() {
            if path.exists() {
                self.load_eff(&path);
                if let Some((rosters, entry_edits)) = self.pending_structure.take() {
                    self.apply_structure_edits(&rosters, &entry_edits);
                }
                if let Some(edits) = self.pending_edits.take() {
                    let had_edits = !edits.is_empty();
                    let push_live = std::mem::take(&mut self.pending_edits_push_live);
                    self.apply_authored_edits(&edits);
                    // Project just loaded: get the game showing it. Rebuild-and-reload, so
                    // every emitter lands where the project says — not an averaged tint.
                    // Edits merely carried across a reload we caused push nothing.
                    if had_edits && push_live && link.status().is_connected() {
                        self.request_live_apply();
                    }
                }
                self.apply_pending_select();
            }
        }
        if self.pending_select.is_some() {
            self.apply_pending_select();
        }
        self.settle_picked_move();

        // No auto-apply: `eff_dirty_at` only marks "there are unsent changes" for the Send
        // button; nothing is uploaded to the game until the user asks. The 3D preview follows
        // every edit on its own (see `edit_rev`).

        // Live values change while the game runs: keep the page fresh.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// The strip across the top of the Effects page: game connection, Send, and which file.
    pub fn page_top(&mut self, ui: &mut Ui, link: &GameLink) {
        self.draw_header(ui, link);
    }

    /// The effects in the open file.
    pub fn page_left(&mut self, ui: &mut Ui, link: &GameLink) {
        self.draw_entry_list(ui, link);
    }

    /// The selected effect's emitters, with its texture and the game's live values below.
    pub fn page_right(&mut self, ui: &mut Ui, link: &GameLink) {
        let pointer_down = ui.ctx().input(|i| i.pointer.any_down());
        self.history_begin_frame(pointer_down, Instant::now());
        let rev = self.edit_rev;
        self.draw_page_right(ui, link);
        if self.edit_rev != rev {
            self.history_note_edit(Instant::now());
        }
    }

    fn draw_page_right(&mut self, ui: &mut Ui, link: &GameLink) {
        if self.selected_entry.is_none() {
            ui.weak("No effect selected")
                .on_hover_text("Pick an effect on the left to change how it looks.");
            return;
        }
        // The game's view of the effect while one is connected; without one there is nothing to
        // put there.
        if !link.status().is_connected() {
            egui::CentralPanel::default().show(ui, |ui| self.draw_emitter_editor(ui));
            return;
        }
        // One line while closed, so connecting does not take the settings' room away; open, it
        // holds the game's view of the effect in at most half the panel.
        let (_, _, max) = game_split_sizes(ui.available_height());
        let summary = self.game_panel_summary(link);
        egui::Panel::bottom("eff_game_panel_line")
            .resizable(false)
            .show(ui, |ui| {
                let header = egui::CollapsingHeader::new(summary.0)
                    .id_salt("eff_game_panel_open")
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("eff_game_panel")
                            .max_height(max)
                            .show(ui, |ui| self.draw_game_panel(ui, link));
                    });
                header.header_response.on_hover_text(summary.1);
            });
        egui::CentralPanel::default().show(ui, |ui| self.draw_emitter_editor(ui));
    }

    fn draw_header(&mut self, ui: &mut Ui, link: &GameLink) {
        // Which character and move, with Send at the row's right end, then which file: the
        // project's edited ones first, then any of the game's.
        ui.horizontal_wrapped(|ui| {
            self.draw_fighter_picker(ui);
            ui.add_space(8.0);
            self.draw_move_picker(ui);
            const SEND_ROOM: f32 = 260.0;
            if ui.available_size_before_wrap().x < SEND_ROOM {
                ui.end_row();
            }
            let size = egui::vec2(
                ui.available_size_before_wrap().x,
                ui.spacing().interact_size.y,
            );
            ui.allocate_ui_with_layout(
                size,
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| self.draw_send(ui, link),
            );
        });
        ui.horizontal_wrapped(|ui| {
            self.draw_file_picker(ui);
        });
    }

    /// The ONLY send control, in the header so it is reachable from every panel, with the
    /// send's state beside it. A second copy used to sit in the game panel reporting none of
    /// the phases below it, so whichever one you happened to be looking at decided how much you
    /// were told about a stalled send. Laid out right to left: the button is drawn first.
    fn draw_send(&mut self, ui: &mut Ui, link: &GameLink) {
        let connected = link.status().is_connected();
        let unsent = self.eff_dirty_at.is_some();
        let btn = egui::Button::new(egui::RichText::new("⬆ Send edits").strong());
        // Filled only when it can be clicked: the fill made a disabled button read as live.
        let btn = if unsent && connected {
            btn.fill(egui::Color32::from_rgb(0x7A, 0x5A, 0x10))
        } else {
            btn
        };
        if ui
            .add_enabled(connected && !self.sending, btn)
            .on_hover_text(
                "Send every effect edit, borrowed effect and texture to the running game. \
                 Do the move again to see it.",
            )
            .on_disabled_hover_text(if self.sending {
                "Sending…".to_owned()
            } else {
                send_blocked_text(&link.view())
            })
            .clicked()
        {
            self.request_live_apply();
        }
        // The connection state and its Connect button are the top bar's, shown once.
        if self.sending || self.awaiting_game.is_some() {
            ui.add(egui::Spinner::new().size(14.0));
            // The phase in words; the game's own numbers go in the hover.
            let (text, hover) = if self.sending {
                ("Sending to the game…".to_string(), None)
            } else {
                let secs = self
                    .awaiting_game
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(0);
                let (st, kinds, spawned) = self.carrier_report;
                // `took_ours` is the difference between "an edit is up" and "THIS edit is
                // up". Without it the previous send answers for this one.
                let took_ours =
                    !self.carrier_needs_new_bytes || self.carrier_gen_now > self.gen_at_send;
                let phase = send_phase(st, spawned, took_ours, secs);
                (
                    if secs >= 2 {
                        format!("{phase}… {secs}s")
                    } else {
                        format!("{phase}…")
                    },
                    Some(carrier_numbers(st, kinds, spawned)),
                )
            };
            let label = ui.label(
                egui::RichText::new(text)
                    .small()
                    .color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60)),
            );
            if let Some(hover) = hover {
                label.on_hover_text(hover);
            }
        } else if unsent {
            if self.game_seen {
                ui.label(
                    egui::RichText::new("unsent")
                        .small()
                        .color(egui::Color32::from_rgb(0xE0, 0xA0, 0x30)),
                )
                .on_hover_text("Edits the game has not been sent");
            }
        } else if let Some(r) = &self.last_carrier_result {
            let label = ui.label(egui::RichText::new(r).small().color(if self.carrier_ok {
                egui::Color32::from_rgb(0x70, 0xB0, 0x70)
            } else {
                egui::Color32::from_rgb(0xD0, 0x80, 0x60)
            }));
            if let Some(detail) = &self.last_carrier_detail {
                label.on_hover_text(detail);
            }
        }
    }

    fn draw_fighter_picker(&mut self, ui: &mut Ui) {
        ui.label(egui::RichText::new("Character").strong());
        let current = self
            .open_fighter
            .as_ref()
            .and_then(|open| self.fighter_choices.iter().find(|(name, _)| name == open))
            .map(|(_, shown)| shown.clone())
            .unwrap_or_else(|| "Pick one".to_owned());
        egui::ComboBox::from_id_salt("eff_fighter_combo")
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .selected_text(current)
            .width(170.0)
            .height(420.0)
            .show_ui(ui, |ui| {
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.fighter_filter)
                        .hint_text("Search characters")
                        .desired_width(200.0),
                );
                if !search.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                    search.request_focus();
                }
                // Best matches first (exact, then starting with, then containing), and Enter
                // picks the top one: "mario" is Mario, not Dr. Mario.
                let mut enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let mut matches: Vec<(u8, &String, &String)> = self
                    .fighter_choices
                    .iter()
                    .filter_map(|(name, shown)| {
                        search_rank(&self.fighter_filter, &[shown.as_str(), name.as_str()])
                            .map(|rank| (rank, name, shown))
                    })
                    .collect();
                matches.sort_by_key(|(rank, _, _)| *rank);
                let mut picked = None;
                let any = !matches.is_empty();
                for (_, name, shown) in matches {
                    let open = self.open_fighter.as_deref() == Some(name.as_str());
                    if ui.selectable_label(open, shown).clicked() || std::mem::take(&mut enter) {
                        if !open {
                            picked = Some(name.clone());
                        }
                        ui.close();
                    }
                }
                if !any {
                    ui.weak("No matching characters");
                }
                if let Some(name) = picked {
                    self.fighter_request = Some(name);
                    self.fighter_filter.clear();
                    self.only_open_move = false;
                }
            })
            .inner
            .is_none()
            // A search left behind when the list closed would greet the next opening.
            .then(|| self.fighter_filter.clear());
    }

    fn draw_move_picker(&mut self, ui: &mut Ui) {
        ui.label(egui::RichText::new("Move").strong());
        let current = self.open_move.clone().unwrap_or_else(|| "None".to_owned());
        let plays: std::collections::HashSet<String> = self
            .selected_uses()
            .iter()
            .filter_map(|found| found.motion.clone())
            .collect();
        let marked = egui::Color32::from_rgb(0x70, 0xC0, 0xF0);
        let response = ui.add_enabled_ui(!self.move_choices.is_empty(), |ui| {
            egui::ComboBox::from_id_salt("eff_move_combo")
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .selected_text(current)
                .width(190.0)
                .height(460.0)
                .show_ui(ui, |ui| {
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.move_filter)
                            .hint_text("Search moves")
                            .desired_width(220.0),
                    );
                    if !search.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                        search.request_focus();
                    }
                    let mut enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    use crate::move_kinds::MoveGroup;
                    let order: Vec<MoveGroup> = MoveGroup::ORDER
                        .iter()
                        .copied()
                        .chain(std::iter::once(MoveGroup::Other))
                        .collect();
                    let mut groups: Vec<Vec<(u8, String, String)>> = vec![Vec::new(); order.len()];
                    for motion in &self.move_choices {
                        let label = crate::move_kinds::plain_name(motion);
                        let Some(rank) =
                            search_rank(&self.move_filter, &[label.as_str(), motion.as_str()])
                        else {
                            continue;
                        };
                        let group = crate::move_kinds::group_of(motion);
                        let at = order
                            .iter()
                            .position(|g| *g == group)
                            .unwrap_or(order.len() - 1);
                        groups[at].push((rank, motion.clone(), label));
                    }
                    // Best matches first in each group, and Enter takes the best of all:
                    // "forward smash" is Forward smash, not Forward smash angled up.
                    for moves in &mut groups {
                        moves.sort_by_key(|(rank, _, _)| *rank);
                    }
                    let best = groups
                        .iter()
                        .flatten()
                        .min_by_key(|(rank, _, _)| *rank)
                        .map(|(_, motion, _)| motion.clone());
                    if groups.iter().all(Vec::is_empty) {
                        ui.weak("No matching moves");
                    }
                    let mut picked = None;
                    for (group, moves) in order.iter().zip(&groups) {
                        if moves.is_empty() {
                            continue;
                        }
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(group.label()).small().strong());
                        for (_, motion, label) in moves {
                            let open = self.open_motion.as_deref() == Some(motion.as_str());
                            let text = if plays.contains(motion) {
                                egui::RichText::new(format!("● {label}")).color(marked)
                            } else {
                                egui::RichText::new(label)
                            };
                            let enter_here = enter && best.as_deref() == Some(motion.as_str());
                            let row = ui.selectable_label(open, text);
                            let row = if plays.contains(motion) {
                                row.on_hover_text("Plays the selected effect")
                            } else {
                                row
                            };
                            if row.clicked() || enter_here {
                                enter = false;
                                picked = Some(motion.clone());
                                ui.close();
                            }
                        }
                    }
                    if let Some(motion) = picked {
                        self.move_filter.clear();
                        self.only_open_move = self.only_in_move_on_pick(&motion);
                        if self.open_motion.as_deref() != Some(motion.as_str()) {
                            self.open_move_request = Some(motion.clone());
                        }
                        self.show_in_picked_move = Some(motion);
                    }
                })
        });
        if response.inner.inner.is_none() {
            self.move_filter.clear();
        }
        let response = response
            .response
            .on_disabled_hover_text("Pick a character first");
        if !plays.is_empty() {
            response.on_hover_text("Moves marked ● play the selected effect");
        }
    }

    /// The effects of this file the open move plays, by its script or an object it makes.
    fn in_open_move(&self, entry: &EffEntry) -> bool {
        self.open_motion.as_deref().is_some_and(|motion| {
            self.effect_uses.get(&entry.hash).is_some_and(|moves| {
                moves
                    .iter()
                    .any(|found| found.motion.as_deref() == Some(motion))
            })
        })
    }

    /// Once a move picked in the header is open, show it with one of its effects: the selected
    /// one if the move plays it, or else the first it plays.
    fn settle_picked_move(&mut self) {
        let Some(motion) = self.show_in_picked_move.clone() else {
            return;
        };
        if self.open_motion.as_deref() != Some(motion.as_str()) || self.entries.is_empty() {
            return;
        }
        if self.open_move_plays_selected() {
            self.view = EffectView::InMove;
            self.show_in_picked_move = None;
            return;
        }
        match self.first_spawned_in_open_move() {
            Some(i) => {
                self.select_entry(i);
                self.scroll_to_selected = true;
                self.view = EffectView::InMove;
                self.show_in_picked_move = None;
            }
            // Wait for the uses to arrive; once they have, this move plays none of the file's.
            None if self.effect_uses_ready => self.show_in_picked_move = None,
            None => {}
        }
    }

    /// The effect of this file the open move spawns first, an edited one before the rest; list
    /// order breaks ties, and effects an object of the move plays come after those its script
    /// spawns.
    fn first_spawned_in_open_move(&self) -> Option<usize> {
        let edited = self.edited_entries();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.in_open_move(entry))
            .min_by_key(|(i, entry)| {
                (
                    !edited[*i],
                    self.open_move_start(&entry.name).unwrap_or(u32::MAX),
                    *i,
                )
            })
            .map(|(i, _)| i)
    }

    /// A file just opened with nothing selected shows something at once: the open move's
    /// first effect, or with no move open, the first edited effect. It waits for the moves to
    /// be read, by when the app has set the open move for the file.
    fn open_first_effect_once(&mut self) {
        if !self.open_first_effect
            || self.entries.is_empty()
            || self.pending_select.is_some()
            || !self.effect_uses_ready
        {
            return;
        }
        self.open_first_effect = false;
        if self.selected_entry.is_some() || self.show_in_picked_move.is_some() {
            return;
        }
        match self.open_motion.clone() {
            Some(motion) => self.show_in_picked_move = Some(motion),
            None => {
                if let Some(i) = self.edited_entries().iter().position(|edited| *edited) {
                    self.select_entry(i);
                    self.scroll_to_selected = true;
                }
            }
        }
    }

    fn draw_file_picker(&mut self, ui: &mut Ui) {
        ui.label(egui::RichText::new("File").strong());
        {
            let loaded_name = self
                .loaded_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| {
                    let n = n.to_string_lossy();
                    if self.loaded_is_merged {
                        format!("{n} (with borrowed effects)")
                    } else {
                        n.to_string()
                    }
                })
                .unwrap_or_else(|| "none".into());
            egui::ComboBox::from_id_salt("eff_file_combo")
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .selected_text(loaded_name)
                .width(260.0)
                .height(420.0)
                .show_ui(ui, |ui| {
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.file_filter)
                            .hint_text("Search, e.g. mario")
                            .desired_width(240.0),
                    );
                    // Typing goes straight to the search when the list opens.
                    if !search.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                        search.request_focus();
                    }
                    let filter = self.file_filter.to_lowercase();
                    let files: Vec<PathBuf> = self
                        .eff_files
                        .iter()
                        .filter(|p| {
                            filter.is_empty()
                                || p.to_string_lossy().to_lowercase().contains(&filter)
                        })
                        .take(400)
                        .cloned()
                        .collect();
                    for path in files {
                        let label = path
                            .strip_prefix(&self.export_root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .to_string();
                        let selected = self.loaded_path.as_deref() == Some(path.as_path());
                        if ui.selectable_label(selected, label).clicked() {
                            // Queued, not loaded here, so the app first keeps the open file's
                            // edits and hands this one its own.
                            self.pending_load = Some(path.clone());
                            self.file_filter.clear();
                            ui.close();
                        }
                    }
                })
                .inner
                .is_none()
                .then(|| self.file_filter.clear());
        }
        egui::containers::menu::MenuButton::new("Game files").ui(ui, |ui| {
            ui.label(egui::RichText::new(self.export_root.display().to_string()).small())
                .on_hover_text(
                    "Where effect files are read from. Follows the data folder opened in the \
                     main window until you pick one here.",
                );
            if ui.button("Change folder").clicked() {
                let mut dialog = rfd::FileDialog::new().set_title("Select ArcExplorer export root");
                // Start where we already are, not in the launch folder.
                if self.export_root.is_dir() {
                    dialog = dialog.set_directory(&self.export_root);
                }
                if let Some(dir) = dialog.pick_folder() {
                    self.set_export_root(dir.clone());
                    // Remembered across restarts, and it outranks the data root next launch —
                    // picking a folder here used to last only as long as the session.
                    crate::app::save_config_path(crate::app::EFF_ROOT_CONFIG_KEY, &dir);
                }
            }
            if ui.button("Look for files again").clicked() {
                self.rescan();
            }
        });
        if let Some(err) = &self.scan_error {
            ui.colored_label(egui::Color32::from_rgb(220, 120, 90), err);
        }
        if let Some(err) = &self.load_error {
            ui.colored_label(egui::Color32::from_rgb(220, 120, 90), err);
        }
        if !self.edit_sources.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new("Edited").weak());
        }
        {
            let sources = self.edit_sources.clone();
            for src in &sources {
                let selected = self.loaded_path.as_deref() == Some(src.base.as_path())
                    || (src.alt_base.is_some()
                        && self.loaded_path.as_deref() == src.alt_base.as_deref());
                let mut badges: Vec<String> = Vec::new();
                if src.transplant_count > 0 {
                    badges.push(format!("{} borrowed", src.transplant_count));
                }
                if src.changed_effects > 0 {
                    // Effects, as the list's "N edited" counts them.
                    badges.push(match src.changed_effects {
                        1 => "1 effect edited".to_owned(),
                        n => format!("{n} effects edited"),
                    });
                }
                if src.textures > 0 {
                    badges.push(format!(
                        "{} texture{}",
                        src.textures,
                        if src.textures == 1 { "" } else { "s" }
                    ));
                }
                let label = if badges.is_empty() {
                    src.label.clone()
                } else {
                    format!("{} ({})", src.label, badges.join(", "))
                };
                // A slotted fighter whose merged build is missing signals a failed merge —
                // surface it instead of silently showing the base file.
                let broken = src.transplant_count > 0 && src.merged.is_none();
                let text = if broken {
                    egui::RichText::new(format!("⚠ {label}"))
                        .color(egui::Color32::from_rgb(230, 190, 80))
                } else {
                    egui::RichText::new(label)
                };
                // A button on every page, so another character's reads as a way back; the
                // open one is highlighted.
                let chip = if selected {
                    ui.selectable_label(true, text)
                } else {
                    ui.button(text)
                };
                let resp = chip.on_hover_text(if broken {
                    "Its borrowed effects couldn't be put together. The status bar says why; borrow them again"
                        .to_string()
                } else {
                    if src.fighter == "common" {
                        format!("Open the shared effects with your edits. {SHARED_EFFECT_NOTE}")
                    } else {
                        format!("Open {}'s effects with your edits", src.label)
                    }
                });
                if resp.clicked() {
                    self.open_edit_source(src);
                }
            }
        }
    }

    /// Open an edited character's effects. Through the character, when it is not the open one:
    /// opening only its file left Character and Move on the old fighter, and the move picker then
    /// listed the old fighter's moves for the new file's effects.
    fn open_edit_source(&mut self, src: &EditSource) {
        let known = self
            .fighter_choices
            .iter()
            .any(|(name, _)| *name == src.fighter);
        if known && self.open_fighter.as_deref() != Some(src.fighter.as_str()) {
            self.fighter_request = Some(src.fighter.clone());
            self.only_open_move = false;
        } else {
            self.pending_load = Some(src.base.clone());
        }
    }

    /// Whether picking `motion` turns on "Only in <move>": when the move plays one of this
    /// file's effects, and the user has not turned the filter off.
    fn only_in_move_on_pick(&self, motion: &str) -> bool {
        !self.only_open_move_declined
            && self.entries.iter().any(|entry| {
                self.effect_uses
                    .get(&entry.hash)
                    .is_some_and(|moves| moves.iter().any(|m| m.motion.as_deref() == Some(motion)))
            })
    }

    fn draw_entry_list(&mut self, ui: &mut Ui, link: &GameLink) {
        let edited_flags = self.edited_entries();
        let edited_entries = edited_flags.iter().filter(|e| **e).count();
        ui.horizontal(|ui| {
            if self.loaded_is_shared() {
                ui.label(
                    egui::RichText::new(format!("Shared effects ({})", self.entries.len()))
                        .strong(),
                )
                .on_hover_text(
                    "Every fighter can play these. An edit changes the effect for every fighter \
                     that plays it.",
                );
            } else {
                ui.label(
                    egui::RichText::new(format!("Effects in this file ({})", self.entries.len()))
                        .strong(),
                );
            }
            if edited_entries > 0 {
                ui.label(
                    egui::RichText::new(format!("{edited_entries} edited"))
                        .small()
                        .color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60)),
                )
                .on_hover_text("Effects with unsent changes, highlighted in the list");
            }
        });
        crate::ui::search_field(
            ui,
            "eff_entry_search",
            &mut self.entry_filter,
            "Search effects or moves…",
        );
        if let Some(open_move) = self
            .open_move
            .clone()
            .filter(|_| self.open_motion.is_some())
        {
            let count = self.entries.iter().filter(|e| self.in_open_move(e)).count();
            let shared = self.open_move_shared_effects().len();
            let counts = if shared > 0 {
                format!("{count}, {shared} shared")
            } else {
                count.to_string()
            };
            ui.horizontal(|ui| {
                if ui
                    .toggle_value(
                        &mut self.only_open_move,
                        format!("Only in {open_move} ({counts})"),
                    )
                    .on_hover_text(if shared > 0 {
                        "List only the effects this move plays. The shared ones are listed last \
                         and open the shared effects file."
                    } else {
                        "List only the effects this move plays"
                    })
                    .changed()
                {
                    self.only_open_move_declined = !self.only_open_move;
                }
            });
        }
        let only_move = self.only_open_move && self.open_motion.is_some();
        let filter = self.entry_filter.trim().to_lowercase();
        let transplant_entries: std::collections::HashMap<String, EffTransplant> = self
            .current_edit_source()
            .map(|source| {
                source
                    .transplants
                    .iter()
                    .cloned()
                    .map(|item| (item.entry_name.to_lowercase(), item))
                    .collect()
            })
            .unwrap_or_default();
        // Rows read as the timeline names them; the file name stays in the hover.
        let shown_names = entry_list_names(&self.entries);
        // The move's effects from the shared file, listed after this file's so the list shows
        // everything the move plays.
        let shared_rows: Vec<String> = if only_move {
            self.open_move_shared_effects()
                .into_iter()
                .filter(|name| filter.is_empty() || name.contains(&filter))
                .collect()
        } else {
            Vec::new()
        };
        let (fold_name, fold_hover, row_not_used) = self.not_used_words();
        // Each file's fold is its own: one fold for all of them opened Mario's because
        // Bayonetta's held an edit.
        let fold_file = self.loaded_path.clone();
        let mut open_shared = None;
        let mut clicked = None;
        let mut scroll_to_selected = self.scroll_to_selected;
        // As wide as the panel, so the bar sits at its edge rather than after the longest name.
        egui::ScrollArea::vertical()
            .id_salt("eff_entries")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                // A search finds an effect by its name or by a move that spawns it ("forward
                // smash" lists that move's effects).
                let uses = &self.effect_uses;
                let open_motion = self.open_motion.as_deref();
                let in_move = |entry: &EffEntry| {
                    uses.get(&entry.hash).is_some_and(|moves| {
                        moves.iter().any(|m| m.motion.as_deref() == open_motion)
                    })
                };
                let strict = |(i, entry): (usize, &EffEntry)| {
                    (!only_move || in_move(entry))
                        && (filter.is_empty()
                            || entry.name.to_lowercase().contains(&filter)
                            || shown_names[i].to_lowercase().contains(&filter)
                            || uses.get(&entry.hash).is_some_and(|moves| {
                                moves
                                    .iter()
                                    .any(|m| m.label.to_lowercase().contains(&filter))
                            }))
                };
                // The open effect stays listed, so a search or a move never hides what is being
                // edited.
                let selected_entry = self.selected_entry;
                let matches = |(i, entry): (usize, &EffEntry)| {
                    selected_entry == Some(i) || strict((i, entry))
                };
                if !self.entries.is_empty()
                    && !self.entries.iter().enumerate().any(&strict)
                    && shared_rows.is_empty()
                {
                    ui.weak(if only_move && filter.is_empty() {
                        "This move plays none of this file's effects"
                    } else {
                        "No matching effects"
                    });
                }
                // Effects no move script spawns go in a fold of their own at the end: greyed in
                // place they read as disabled.
                let unused =
                    |entry: &EffEntry| self.effect_uses_ready && !uses.contains_key(&entry.hash);
                let reveal_selected = scroll_to_selected;
                let mut row = |ui: &mut Ui, i: usize, entry: &EffEntry| {
                    ui.horizontal(|ui| {
                        // Whether the game has spawned it, only while there is a game to ask.
                        if link.status().is_connected() {
                            let live = link.is_live(entry.hash);
                            let dot = if live {
                                egui::Color32::from_rgb(90, 220, 90)
                            } else {
                                egui::Color32::DARK_GRAY
                            };
                            ui.colored_label(dot, "●").on_hover_text(if live {
                                "Seen spawning in the game"
                            } else {
                                "Not seen in the game yet. Do the move in training mode."
                            });
                        }
                        let selected = self.selected_entry == Some(i);
                        // An edited entry is tinted rather than badged: the list is narrow, a
                        // word of text per row would push the names out of view, and this is
                        // the one thing you need to find again after scrolling away.
                        let text = egui::RichText::new(&shown_names[i]);
                        let text = if edited_flags[i] {
                            text.color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60))
                        } else {
                            text
                        };
                        let response = ui.selectable_label(selected, text).on_hover_text({
                            let plays = match uses.get(&entry.hash) {
                                Some(moves) => format!(
                                    "Plays in: {}",
                                    moves
                                        .iter()
                                        .map(|m| m.label.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                                None if unused(entry) => row_not_used.clone(),
                                None => "Reading the moves…".to_owned(),
                            };
                            let edited = if edited_flags[i] {
                                "\nHas unsent changes"
                            } else {
                                ""
                            };
                            format!(
                                "{}\n{plays}{edited}\nRight-click to copy the name",
                                entry.name
                            )
                        });
                        // The entry name is what goes into an ACMD `EFFECT` call, and the list
                        // is the only place it appears in full — the label is truncated once
                        // the panel is narrow, so retyping it from the screen is not an option.
                        if response.secondary_clicked() {
                            ui.ctx().copy_text(entry.name.clone());
                        }
                        if transplant_entries.contains_key(&entry.name) {
                            ui.label(egui::RichText::new("Borrowed").small().color(BORROWED_TINT))
                                .on_hover_text("Borrowed from another effect file");
                        }
                        if response.clicked() {
                            clicked = Some(i);
                        }
                        if selected && scroll_to_selected {
                            response.scroll_to_me(Some(egui::Align::Center));
                            scroll_to_selected = false;
                        }
                    });
                };
                let mut not_used = Vec::new();
                for (i, entry) in self.entries.iter().enumerate() {
                    if !matches((i, entry)) {
                        continue;
                    }
                    if unused(entry) {
                        not_used.push(i);
                    } else {
                        row(ui, i, entry);
                    }
                }
                if !not_used.is_empty() {
                    // Closed until it holds what is wanted: the open effect, an edited one, or
                    // a search's matches.
                    let open_effect = selected_entry.is_some_and(|i| not_used.contains(&i));
                    // An edited effect is the one thing to find again: never hidden.
                    let wanted = open_effect
                        || !filter.is_empty()
                        || not_used.iter().any(|&i| edited_flags[i]);
                    // It opens when it comes to hold one, and is otherwise left as the user left
                    // it: keyed on `wanted`, a reset of the open effect closed it under them.
                    let was_wanted_id =
                        ui.make_persistent_id(("eff_entries_not_used_wanted", &fold_file));
                    let was_wanted = ui.data_mut(|d| {
                        let was = d.get_temp::<bool>(was_wanted_id).unwrap_or(false);
                        d.insert_temp(was_wanted_id, wanted);
                        was
                    });
                    let fold =
                        egui::CollapsingHeader::new(format!("{fold_name} ({})", not_used.len()))
                            .id_salt(("eff_entries_not_used", &fold_file))
                            .default_open(wanted);
                    let fold = if (wanted && !was_wanted) || (open_effect && reveal_selected) {
                        fold.open(Some(true))
                    } else {
                        fold
                    };
                    fold.show(ui, |ui| {
                        for &i in &not_used {
                            row(ui, i, &self.entries[i]);
                        }
                    })
                    .header_response
                    .on_hover_text(fold_hover);
                }
                for name in &shared_rows {
                    ui.horizontal(|ui| {
                        let response = ui
                            .selectable_label(false, crate::app::effect_words(name))
                            .on_hover_text(format!(
                                "{name}\n{SHARED_EFFECT_NOTE}\nClick to open it in the shared file"
                            ));
                        ui.label(egui::RichText::new("Shared").small().color(SHARED_TINT))
                            .on_hover_text(SHARED_EFFECT_NOTE);
                        if response.clicked() {
                            open_shared = Some(name.clone());
                        }
                    });
                }
            });
        self.scroll_to_selected = scroll_to_selected;
        if let Some(i) = clicked {
            self.select_entry(i);
        }
        if open_shared.is_some() {
            self.open_effect_request = open_shared;
        }
    }

    /// Open an effect of the list, on the part last picked in it, in the open move when that
    /// move plays it.
    fn select_entry(&mut self, i: usize) {
        if self.selected_entry == Some(i) {
            return;
        }
        // Each effect keeps the part last picked in it for the session.
        self.remember_picks();
        self.selected_entry = Some(i);
        self.selected_emitter = self
            .entries
            .get(i)
            .and_then(|entry| self.parts_picked.get(&entry.name))
            .copied()
            .unwrap_or(0);
        self.view = if self.open_move_plays_selected() {
            EffectView::InMove
        } else {
            EffectView::Alone
        };
    }

    fn draw_emitter_editor(&mut self, ui: &mut Ui) {
        // The texture panel on the right renders from this and runs AFTER this column, so
        // clear it on every path that does not reach a live emitter — otherwise deselecting
        // leaves the previous emitter's texture on screen, with working Replace buttons.
        self.selected_texture = None;
        let Some(entry_idx) = self.selected_entry else {
            ui.colored_label(egui::Color32::GRAY, "No effect selected").on_hover_text(
                "Pick an effect on the left to change how it looks. It plays in the view as you edit.",
            );
            return;
        };
        let set_idx = self.entries[entry_idx].set_idx;
        let entry_name = self.entries[entry_idx].name.clone();

        // Which emitters of this set are edited, for the tab markers. Computed BEFORE the
        // mutable borrow below, which is what makes it usable while the fields are drawn.
        let edited: std::collections::HashSet<usize> =
            self.edited_emitters(set_idx).into_iter().collect();
        let moved = self.moved_parts(set_idx);

        self.clamp_selected_emitter(set_idx);

        let emitter_count = self
            .ptcl
            .as_ref()
            .and_then(|p| p.emitter_sets.get(set_idx))
            .map(|s| s.emitters.len())
            .unwrap_or(0);
        // In words, as the list shows it; the file name is in the hover.
        let shown_name = entry_list_names(&self.entries)
            .get(entry_idx)
            .cloned()
            .unwrap_or_else(|| entry_name.clone());
        let mut reset_entry = false;
        let shared_file = self.loaded_is_shared();
        let effect_edited = self.effect_edited(set_idx, &entry_name);
        // Some((true, n)) to undo n steps, Some((false, n)) to redo them.
        let mut history = None;
        // Wraps in a narrow panel: a long effect name widened the panel past its edge.
        ui.horizontal_wrapped(|ui| {
            let heading = ui
                .add(
                    egui::Label::new(egui::RichText::new(&shown_name).heading())
                        .sense(egui::Sense::click()),
                )
                .on_hover_text(if shared_file {
                    format!("{entry_name}\n{SHARED_EFFECT_NOTE}\nRight-click to copy the name")
                } else {
                    format!("{entry_name}\nRight-click to copy the name")
                });
            if heading.secondary_clicked() {
                ui.ctx().copy_text(entry_name.clone());
            }
            ui.label(
                egui::RichText::new(if emitter_count == 1 {
                    "1 part".to_owned()
                } else {
                    format!("{emitter_count} parts")
                })
                .color(egui::Color32::LIGHT_GRAY),
            );
            if shared_file {
                ui.label(egui::RichText::new("Shared").small().color(SHARED_TINT))
                    .on_hover_text(SHARED_EFFECT_NOTE);
            }
            reset_entry = ui
                .add_enabled(effect_edited, egui::Button::new("Reset effect").small())
                .on_hover_text(
                    "Put this effect back as the game has it: every part's values, the part \
                     list and its timing",
                )
                .on_disabled_hover_text("This effect is as the game has it")
                .clicked();
            // Kept together when the row wraps. A row nested in a wrapping one cannot wrap by
            // itself: it ran past a narrow panel's edge and widened everything under it.
            if ui.available_size_before_wrap().x < small_buttons_width(ui, &["Undo", "▾", "Redo"])
            {
                ui.end_row();
            }
            ui.horizontal(|ui| {
                let cleared = self
                    .history
                    .cleared
                    .map(|why| format!("Undo history cleared: {why}"));
                let undo = ui.add_enabled(self.can_undo(), egui::Button::new("Undo").small());
                let undo = match (undo.contains_pointer(), undo.enabled()) {
                    (true, true) => undo.on_hover_text(format!(
                        "Undo {} (Ctrl+Z)",
                        self.undo_label().unwrap_or_default()
                    )),
                    (true, false) => undo.on_disabled_hover_text(
                        cleared
                            .clone()
                            .unwrap_or_else(|| "Nothing to undo".to_owned()),
                    ),
                    _ => undo,
                };
                if undo.clicked() {
                    history = Some((true, 1));
                }
                // Every step, to go back several at once.
                let (undo_steps, redo_steps) = self.history_steps();
                ui.add_enabled_ui(!undo_steps.is_empty() || !redo_steps.is_empty(), |ui| {
                    ui.menu_button(egui::RichText::new("▾").small(), |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .show(ui, |ui| {
                                for (i, label) in redo_steps.iter().enumerate().rev() {
                                    if ui
                                        .add(egui::Button::new(egui::RichText::new(label).weak()))
                                        .on_hover_text("Undone. Click to redo up to this step.")
                                        .clicked()
                                    {
                                        history = Some((false, i + 1));
                                        ui.close();
                                    }
                                }
                                if !redo_steps.is_empty() && !undo_steps.is_empty() {
                                    ui.separator();
                                }
                                for (i, label) in undo_steps.iter().enumerate() {
                                    if ui
                                        .button(label)
                                        .on_hover_text("Click to undo back to before this step")
                                        .clicked()
                                    {
                                        history = Some((true, i + 1));
                                        ui.close();
                                    }
                                }
                            });
                    })
                    .response
                    .on_hover_text("Undo history")
                    .on_disabled_hover_text("Nothing to undo");
                });
                let redo = ui.add_enabled(self.can_redo(), egui::Button::new("Redo").small());
                let redo = match (redo.contains_pointer(), redo.enabled()) {
                    (true, true) => redo.on_hover_text(format!(
                        "Redo {} (Ctrl+Shift+Z)",
                        self.redo_label().unwrap_or_default()
                    )),
                    (true, false) => redo.on_disabled_hover_text("Nothing to redo"),
                    _ => redo,
                };
                if redo.clicked() {
                    history = Some((false, 1));
                }
            });
        });
        if let Some((undo, steps)) = history {
            // It may open another effect: draw that one next frame.
            if undo {
                self.undo_steps(steps);
            } else {
                self.redo_steps(steps);
            }
            ui.ctx().request_repaint();
            return;
        }
        // A borrowed effect says where it came from, and can be taken out again.
        if let Some((fighter, item)) = self.current_transplant_for_entry(&entry_name) {
            ui.horizontal_wrapped(|ui| {
                let file = Path::new(&item.donor_file)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| item.donor_file.clone());
                ui.label(
                    egui::RichText::new(format!("Borrowed from {} in {file}", item.donor_name))
                        .color(egui::Color32::from_rgb(190, 140, 255)),
                );
                if ui
                    .small_button("Remove")
                    .on_hover_text("Take this borrowed effect out of the project and the game")
                    .clicked()
                {
                    self.pending_transplant_removals.push(EffTransplantRemoval {
                        fighter,
                        op_index: item.op_index,
                        entry_name: item.entry_name,
                        donor_name: item.donor_name,
                    });
                }
            });
        }
        // The tabs, then putting the effect in the open move: a row of its own held one
        // control, greyed whenever the move already plays the effect.
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.emitter_tab, EmitterTab::Attributes, "Look")
                .on_hover_text("How the selected part looks and moves");
            ui.selectable_value(
                &mut self.emitter_tab,
                EmitterTab::Spawning,
                "Parts and timing",
            )
            .on_hover_text(
                "The parts this effect plays: each makes its own particles. When each starts, on \
                 the timeline's frames, how long it runs, and the model that comes with it.",
            );
            ui.add_space(8.0);
            let label = match &self.open_move {
                Some(name) => format!("Use in {name}"),
                None => "Use in a move".to_owned(),
            };
            let (enabled, hover) = self.use_in_move_state(&entry_name);
            let button = ui.add_enabled(enabled, egui::Button::new(label).small());
            let button = if enabled {
                button.on_hover_text(hover)
            } else {
                button.on_disabled_hover_text(hover)
            };
            if button.clicked() {
                self.use_in_move_requested = true;
            }
            if let Some(note) = &self.use_in_move_note {
                ui.label(egui::RichText::new(note).small().weak());
                if ui
                    .small_button("Undo")
                    .on_hover_text("Take it out of the move again")
                    .clicked()
                {
                    self.use_in_move_undo_requested = true;
                }
                if ui
                    .small_button("Open in Classic editor")
                    .on_hover_text(
                        "Place it in the move: where it sits, its size and when it stops",
                    )
                    .clicked()
                {
                    self.open_classic_requested = true;
                }
            }
        });
        // Before the tab branch: the button is on both tabs.
        if reset_entry {
            self.history_close_step();
            self.reset_effect(set_idx, &entry_name);
            self.history.next_label = Some(format!("Reset effect in {shown_name}"));
        }
        // Drawn before the borrows below, because it edits the emitter LIST and the entry's
        // header rather than one emitter's fields.
        if self.emitter_tab == EmitterTab::Spawning {
            self.draw_spawning_panel(ui, set_idx, &entry_name);
            return;
        }

        let Some(ptcl) = self.ptcl.as_mut() else {
            return;
        };
        // Texture pool of the loaded eff, cloned up front so the `set`/`em` mutable borrows
        // below don't conflict with reading it.
        let textures = ptcl.bntx_textures.clone();
        let Some(set) = ptcl.emitter_sets.get(set_idx) else {
            return;
        };
        if self.pristine.get(set_idx).is_none() {
            return;
        }
        let part_names: Vec<&str> = set.emitters.iter().map(|em| em.name.as_str()).collect();
        let part_names = part_list_names(&part_names);
        // Emitter tabs. Edited ones carry a dot and are tinted: a set can have twenty
        // emitters, and without a marker the only way to find the two you changed was to
        // click through all of them.
        let tab = |i: usize, em: &EmitterDef| {
            let name = &part_names[i];
            let moved = moved.get(i).copied().unwrap_or(false);
            let text = if edited.contains(&i) || moved {
                egui::RichText::new(format!("{name} •"))
                    .color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60))
            } else {
                egui::RichText::new(name)
            };
            let hover = format!(
                "{}{}\n\nOne part of the effect. Tick \"Show only this part\" to see which.",
                em.name,
                part_place_note(moved, em.source_idx),
            );
            (text, hover)
        };
        // At most two rows of tabs; more parts than that are picked from a list, so the settings
        // below keep their room in a small window.
        let font = egui::TextStyle::Button.resolve(ui.style());
        let tabs_width: f32 = part_names
            .iter()
            .map(|name| {
                ui.painter()
                    .layout_no_wrap(format!("{name} •"), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
                    + 2.0 * ui.spacing().button_padding.x
                    + ui.spacing().item_spacing.x
            })
            .sum();
        if tabs_width <= 2.0 * ui.available_width() {
            ui.horizontal_wrapped(|ui| {
                for (i, em) in set.emitters.iter().enumerate() {
                    let (text, hover) = tab(i, em);
                    if ui
                        .selectable_label(self.selected_emitter == i, text)
                        .on_hover_text(hover)
                        .clicked()
                    {
                        self.selected_emitter = i;
                    }
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.label("Part")
                    .on_hover_text(format!("{} parts", set.emitters.len()));
                let current = set
                    .emitters
                    .get(self.selected_emitter)
                    .map(|em| tab(self.selected_emitter, em).0)
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt(("part_picker", set_idx))
                    .selected_text(current)
                    .height(420.0)
                    .show_ui(ui, |ui| {
                        for (i, em) in set.emitters.iter().enumerate() {
                            let (text, hover) = tab(i, em);
                            if ui
                                .selectable_label(self.selected_emitter == i, text)
                                .on_hover_text(hover)
                                .clicked()
                            {
                                self.selected_emitter = i;
                            }
                        }
                    });
                let count = set.emitters.len();
                if ui
                    .add_enabled(self.selected_emitter > 0, egui::Button::new("◀").small())
                    .on_hover_text("The part before")
                    .clicked()
                {
                    self.selected_emitter -= 1;
                }
                if ui
                    .add_enabled(
                        self.selected_emitter + 1 < count,
                        egui::Button::new("▶").small(),
                    )
                    .on_hover_text("The next part")
                    .clicked()
                {
                    self.selected_emitter += 1;
                }
            });
        }
        if self.solo_selected.as_deref() != Some(entry_name.as_str()) {
            self.solo_selected = None;
        }
        let mut solo = self.solo_selected.is_some();
        let mut reset_part = false;
        ui.horizontal_wrapped(|ui| {
            if ui
                .checkbox(&mut solo, "Show only this part")
                .on_hover_text("Hide the effect's other parts in the view while you edit this one.")
                .changed()
            {
                self.solo_selected = solo.then(|| entry_name.clone());
            }
            // Beside the part it acts on, not under its last setting.
            if ui
                .add_enabled(
                    edited.contains(&self.selected_emitter),
                    egui::Button::new("Reset this part").small(),
                )
                .on_hover_text("Put this part's values back as the game has them")
                .on_disabled_hover_text("This part has the game's values")
                .clicked()
            {
                reset_part = true;
            }
        });
        ui.separator();

        let ei = self.selected_emitter;
        if part_mut(&mut self.ptcl, &self.pristine, set_idx, ei).is_none() {
            ui.colored_label(egui::Color32::GRAY, "This effect has no parts.");
            return;
        }

        let mut changed = false;
        // The Look row an edit was made on, to name its undo step.
        let mut touched: Option<&'static str> = None;
        let width = ui.available_width();
        let narrow = width < NARROW_PANEL;
        // Each part keeps its own place in the list: one scroll position for every effect left
        // the next one opened halfway down, with its colours out of sight.
        // Both ways: a row wider than a narrow panel scrolls inside it instead of running
        // under the view.
        egui::ScrollArea::both()
            // By file and effect name: by list position, effect 20 of one file opened where
            // effect 20 of another was left.
            .id_salt((
                "emitter_fields",
                &self.loaded_path,
                &entry_name,
                self.selected_emitter,
            ))
            .show(ui, |ui| {
                // A scroll both ways has no width of its own, so without this no row wrapped.
                ui.set_max_width(width - ui.spacing().scroll.allocated_width());
                // The Look rows and the settings tree put their values in one column.
                let label_end = ui.min_rect().left() + label_column(width);
                ui.data_mut(|d| {
                    d.insert_temp(label_column_id(), label_end);
                    d.insert_temp(was_below_id(), width < WAS_BELOW_PANEL);
                });
                // In the scroll, so it does not take the settings' room in a small window.
                self.draw_whole_effect(ui, set_idx, &shown_name);
                let Some((em, pr)) = part_mut(&mut self.ptcl, &self.pristine, set_idx, ei) else {
                    return;
                };
                // The handful of values that get changed on nearly every edit, kept at the top
                // and out of the group tree. Each one is the SAME attribute the tree below
                // holds, so editing it here or there is the same edit — there is no second
                // source of truth to fall out of step.
                let was_of = |id: &str| {
                    crate::eff_attrs::index_of(id)
                        .and_then(|at| pr.attrs.get(at).copied().flatten())
                        .map(|v| v.as_f32())
                        .unwrap_or(0.0)
                };
                egui::Grid::new("authored_fields")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        let basic: Vec<usize> = BASIC_ATTRS
                            .iter()
                            .filter_map(|id| crate::eff_attrs::index_of(id))
                            .collect();
                        changed |= attr_grid_rows(ui, em, pr, &basic, &textures);
                        // Uniform scale is the common intent, so it gets a control of its own:
                        // the per-axis rows are in the Particle scale group for anisotropy.
                        if PARTICLE_SCALE_ATTRS.iter().all(|id| attr(em, id).is_some()) {
                            let was = PARTICLE_SCALE_ATTRS.map(was_of);
                            let now = PARTICLE_SCALE_ATTRS.map(|id| attr_f32(em, id));
                            let edited = values_differ(&was, &now);
                            quick_label(ui, "Particle size", edited).on_hover_text(
                                "Scales X, Y and Z together, keeping the ratio the part was \
                                 made with.",
                            );
                            let base = now[0];
                            let mut v = base;
                            if ui
                                .add(
                                    float_drag(&mut v, 0.01, crate::eff_attrs::Unit::None)
                                        .range(0.0..=f32::MAX),
                                )
                                .changed()
                            {
                                let ratio = if base.abs() > 1e-6 { v / base } else { 0.0 };
                                for id in PARTICLE_SCALE_ATTRS {
                                    let now = attr_f32(em, id);
                                    let scaled = if base.abs() > 1e-6 { now * ratio } else { v };
                                    set_attr(em, id, AttrValue::Float(scaled));
                                }
                                changed = true;
                                touched = Some("Particle size");
                            }
                            if quick_was(ui, edited, format!("was {:.2}", was[0])) {
                                for (id, was) in PARTICLE_SCALE_ATTRS.iter().zip(was) {
                                    set_attr(em, id, AttrValue::Float(was));
                                }
                                changed = true;
                                touched = Some("Particle size");
                            }
                            ui.end_row();
                        }
                        // How fast the particles go: both starting speeds together, as the
                        // preview moves them.
                        if SPEED_ATTRS.iter().all(|id| attr(em, id).is_some()) {
                            let speeds = SPEED_ATTRS.map(|id| attr_f32(em, id));
                            let was = SPEED_ATTRS.map(was_of);
                            let edited = values_differ(&was, &speeds);
                            quick_label(ui, "Speed", edited).on_hover_text(
                                "How fast the particles fly. Scales Outward speed and Speed \
                                 along direction together; both are under Movement.",
                            );
                            let mut v = shown_speed(speeds);
                            if ui
                                .add(float_drag(&mut v, 0.01, crate::eff_attrs::Unit::None))
                                .changed()
                            {
                                for (id, value) in SPEED_ATTRS.iter().zip(scaled_speeds(speeds, v))
                                {
                                    set_attr(em, id, AttrValue::Float(value));
                                }
                                changed = true;
                                touched = Some("Speed");
                            }
                            if quick_was(ui, edited, format!("was {:.2}", shown_speed(was))) {
                                for (id, was) in SPEED_ATTRS.iter().zip(was) {
                                    set_attr(em, id, AttrValue::Float(was));
                                }
                                changed = true;
                                touched = Some("Speed");
                            }
                            ui.end_row();
                        }
                        // Where the particles start, and over what area: one row each, X Y Z.
                        for (attrs, name, hover) in [
                            (
                                SPREAD_ATTRS,
                                "Spread",
                                "Stretches the area the particles start in, and the way they \
                                 move, along X, Y and Z",
                            ),
                            (
                                POSITION_ATTRS,
                                "Position",
                                "Moves this part from where the effect is spawned, along X, Y \
                                 and Z",
                            ),
                        ] {
                            if !attrs.iter().all(|id| attr(em, id).is_some()) {
                                continue;
                            }
                            let was = attrs.map(was_of);
                            let now = attrs.map(|id| attr_f32(em, id));
                            let edited = values_differ(&was, &now);
                            quick_label(ui, name, edited).on_hover_text(hover);
                            ui.horizontal(|ui| {
                                for (id, axis) in attrs.iter().zip(["X ", "Y ", "Z "]) {
                                    let mut v = attr_f32(em, id);
                                    if ui
                                        .add(
                                            float_drag(&mut v, 0.01, crate::eff_attrs::Unit::None)
                                                .prefix(axis),
                                        )
                                        .changed()
                                    {
                                        set_attr(em, id, AttrValue::Float(v));
                                        changed = true;
                                        touched = Some(name);
                                    }
                                }
                            });
                            let was_text = format!("was {:.2} {:.2} {:.2}", was[0], was[1], was[2]);
                            if quick_was(ui, edited, was_text) {
                                for (id, was) in attrs.iter().zip(was) {
                                    set_attr(em, id, AttrValue::Float(was));
                                }
                                changed = true;
                                touched = Some(name);
                            }
                            ui.end_row();
                        }
                    });

                let mut key_table = |ui: &mut Ui,
                                     label: &str,
                                     hover: &str,
                                     keys: &mut Vec<ColorKey>,
                                     orig: &[ColorKey]| {
                    if keys.is_empty() {
                        return;
                    }
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(label).strong())
                        .on_hover_text(hover);
                    for (i, k) in keys.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            let mut rgb = [k.r, k.g, k.b];
                            // In a narrow panel the R G B numbers go to the code's hover,
                            // so the key's time stays in sight.
                            if crate::ui::colour_field_with(
                                ui,
                                ("colour_key", label, i),
                                &mut rgb,
                                !narrow,
                            ) {
                                k.r = rgb[0];
                                k.g = rgb[1];
                                k.b = rgb[2];
                                changed = true;
                            }
                            ui.label("at").on_hover_text(
                                "When in the particle's life this colour is reached: 0% when \
                                 it appears, 100% when it is gone",
                            );
                            changed |= life_time_field(ui, &mut k.frame);
                            if let Some(o) = orig.get(i) {
                                if (o.r, o.g, o.b) != (k.r, k.g, k.b) {
                                    // The colour before the edit, as a narrow swatch: with
                                    // a label beside it, the row outgrew the panel and
                                    // widened it, squeezing the view. A plain swatch, as
                                    // a disabled colour button is drawn dimmed.
                                    let size = egui::vec2(10.0, ui.spacing().interact_size.y);
                                    let before = egui::widgets::color_picker::show_color(
                                        ui,
                                        egui::Rgba::from_rgb(o.r, o.g, o.b),
                                        size,
                                    )
                                    .interact(egui::Sense::click())
                                    .on_hover_text(format!(
                                        "Before your edit: {}. Click to put it back.",
                                        crate::ui::colour_code([o.r, o.g, o.b])
                                    ));
                                    if before.clicked() {
                                        (k.r, k.g, k.b) = (o.r, o.g, o.b);
                                        changed = true;
                                    }
                                }
                            }
                        });
                    }
                };
                key_table(
                    ui,
                    "Colour over its life",
                    "The part's colour from the start of each particle's life to its end. How \
                     it combines with the second colour and the textures is under Colour \
                     mixing.",
                    &mut em.color0,
                    &pr.color0,
                );
                key_table(
                    ui,
                    "Second colour over its life",
                    "A second colour the part's shader can use, over each particle's life. \
                     Colour mixing says how it combines with the first colour and the \
                     textures.",
                    &mut em.color1,
                    &pr.color1,
                );

                if !em.alpha0_keys.is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Opacity over its life").strong())
                        .on_hover_text(
                            "How solid the part is from the start of each particle's life to \
                             its end: 0 is clear, 1 is solid",
                        );
                    for (i, k) in em.alpha0_keys.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            opacity_bar(ui, k.r);
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut k.r)
                                        .speed(0.01)
                                        .range(0.0..=4.0)
                                        .max_decimals(2),
                                )
                                .changed();
                            ui.label("at").on_hover_text(
                                "When in the particle's life this opacity is reached: 0% when \
                                 it appears, 100% when it is gone",
                            );
                            changed |= life_time_field(ui, &mut k.frame);
                            if let Some(o) = pr.alpha0_keys.get(i) {
                                if (o.r - k.r).abs() > 1e-6 {
                                    ui.label(
                                        egui::RichText::new(format!("was {:.2}", o.r))
                                            .small()
                                            .color(egui::Color32::GRAY),
                                    );
                                }
                            }
                        });
                    }
                }

                // The textures this part samples, in their own section of this scroll: as a
                // strip of their own below it, they took the settings' height in a short
                // window and clipped their buttons in a tall one.
                let slots = texture_slots(em, &textures);
                let shown = slots
                    .iter()
                    .find(|(slot, _)| *slot == self.texture_slot)
                    .or(slots.first())
                    .copied();
                if let Some((slot, _)) = shown {
                    self.texture_slot = slot;
                }
                self.texture_slots = slots;
                self.selected_texture = shown.map(|(_, at)| (at, textures[at].clone()));
                ui.add_space(6.0);
                egui::CollapsingHeader::new(egui::RichText::new("Texture").strong())
                    .id_salt("part_texture")
                    .default_open(true)
                    .show(ui, |ui| self.draw_texture_panel(ui));
                let Some((em, pr)) = part_mut(&mut self.ptcl, &self.pristine, set_idx, ei) else {
                    return;
                };

                // The friendly controls above are projections of the complete attribute
                // table below. Synchronize before drawing the table, then flow edits made in
                // the table back into those projections for the next frame.
                effective_keys_to_attrs(em);

                ui.add_space(10.0);
                ui.separator();
                let part = egui::Id::unique((self.loaded_path.clone(), set_idx, em.source_idx));
                let attributes_changed =
                    draw_attribute_groups(ui, em, pr, &mut self.attr_filter, &textures, part);
                if attributes_changed {
                    attrs_to_effective_keys(em);
                    changed = true;
                }
                // EffectResearch documents these separately from EmitterData. Keeping them in
                // the same authored emitter panel makes Send/export semantics identical.
                let pristine_def = pr.to_def(em.source_idx);
                // One part's open sections are its own, not every part's at that position.
                let part_id = (self.loaded_path.clone(), set_idx, em.source_idx);
                ui.push_id(part_id, |ui| {
                    changed |= crate::eff_subsections::draw(ui, em, &pristine_def);
                });
            });

        if changed {
            touch(&mut self.eff_dirty_at, &mut self.edit_rev);
        }
        if let Some(row) = touched {
            self.history.touched.get_or_insert_with(|| row.to_owned());
        }
        if reset_part {
            // A reset is a step of its own, never part of the edit just before it.
            self.history_close_step();
            self.reset_part_fields(set_idx, ei);
            self.history.next_label = Some(self.reset_part_label(set_idx, ei));
        }
    }

    /// The whole-effect controls' state for set `set_idx`, started again from neutral when the
    /// parts are no longer what the controls last wrote.
    fn whole_effect_for(&mut self, set_idx: usize) -> Option<&mut WholeEffect> {
        let set = self.ptcl.as_ref()?.emitter_sets.get(set_idx)?;
        let now: Vec<WholeFields> = set.emitters.iter().map(whole_fields).collect();
        let current = self
            .whole_effect
            .as_ref()
            .is_some_and(|w| w.set_idx == set_idx && w.written == now);
        if !current {
            // The game's values of each part, when every part has them.
            let pristine: Option<Vec<WholeFields>> = set
                .emitters
                .iter()
                .map(|em| {
                    let snapshot = self.pristine.get(set_idx)?.get(em.source_idx)?;
                    let mut game = em.clone();
                    snapshot.restore(&mut game);
                    Some(whole_fields(&game))
                })
                .collect();
            // Read back from the game's values when one setting explains every part; after an
            // edit on a part's own rows none does, and the controls start from neutral on what
            // is there.
            let whole = pristine
                .and_then(|base| Some((read_whole_factors(&base, &now)?, base)))
                .map_or_else(
                    || WholeEffect {
                        set_idx,
                        base: now.clone(),
                        factors: WholeFactors::default(),
                        written: now.clone(),
                    },
                    |(factors, base)| WholeEffect {
                        set_idx,
                        base,
                        factors,
                        written: now.clone(),
                    },
                );
            self.whole_effect = Some(whole);
        }
        self.whole_effect.as_mut()
    }

    /// Set the whole-effect controls of set `set_idx` to `factors`, writing every part.
    fn set_whole_effect(&mut self, set_idx: usize, factors: WholeFactors) {
        let Some(whole) = self.whole_effect_for(set_idx) else {
            return;
        };
        whole.factors = factors;
        whole.written = whole
            .base
            .iter()
            .map(|base| scaled_whole_fields(base, factors))
            .collect();
        let written = whole.written.clone();
        let Some(set) = self
            .ptcl
            .as_mut()
            .and_then(|p| p.emitter_sets.get_mut(set_idx))
        else {
            return;
        };
        for (em, fields) in set.emitters.iter_mut().zip(&written) {
            write_whole_fields(em, fields);
        }
        // What was written, read back, so the next frame sees the parts as the controls left them.
        let read_back: Vec<WholeFields> = set.emitters.iter().map(whole_fields).collect();
        if let Some(whole) = self.whole_effect.as_mut() {
            whole.written = read_back;
        }
        touch(&mut self.eff_dirty_at, &mut self.edit_rev);
    }

    /// Hue, size, life and speed for every part of the effect at once. Each is written as the
    /// parts' own values, so it exports and sends like any part edit.
    fn draw_whole_effect(&mut self, ui: &mut Ui, set_idx: usize, shown_name: &str) {
        let Some(whole) = self.whole_effect_for(set_idx) else {
            return;
        };
        let mut factors = whole.factors;
        let mut changed = None;
        let header = egui::CollapsingHeader::new(egui::RichText::new("Whole effect").strong())
            .id_salt("whole_effect")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    // Each label stays beside its box when the row wraps.
                    wrapping_pair(ui, "Hue", |ui| {
                        ui.label("Hue").on_hover_text(
                        "Turns the colour of every part, keeping how bright and how strong each \
                         colour is. 0 is the game's colours.",
                    );
                        if ui
                            .add(
                                egui::DragValue::new(&mut factors.hue)
                                    .speed(1.0)
                                    .range(-180.0..=180.0)
                                    .suffix("°")
                                    .max_decimals(0),
                            )
                            .changed()
                        {
                            changed = Some("Hue");
                        }
                    });
                    for (label, hover, value) in [
                        (
                            "Size ×",
                            "Multiplies the particle size of every part",
                            &mut factors.size,
                        ),
                        (
                            "Life ×",
                            "Multiplies how long every part's particles stay on screen",
                            &mut factors.life,
                        ),
                        (
                            "Speed ×",
                            "Multiplies how fast every part's particles fly",
                            &mut factors.speed,
                        ),
                    ] {
                        wrapping_pair(ui, label, |ui| {
                            ui.label(label).on_hover_text(hover);
                            if ui
                                .add(
                                    float_drag(value, 0.01, crate::eff_attrs::Unit::None)
                                        .range(0.0..=100.0),
                                )
                                .on_hover_text(hover)
                                .changed()
                            {
                                changed = Some(label);
                            }
                        });
                    }
                });
            });
        header
            .header_response
            .on_hover_text("Changes every part of this effect at once");
        if let Some(what) = changed {
            self.set_whole_effect(set_idx, factors);
            self.history.next_label = Some(format!("{what} for all of {shown_name}"));
        }
    }

    /// What this effect actually brings on screen: its emitter list, its extra parts, and its
    /// external model.
    ///
    /// Everything here is structure rather than values — it changes what exists, not what one
    /// emitter looks like — which is why it is a separate view from the attribute tree. The
    /// three sections answer three different questions:
    ///   EMITTERS  which emitters play at all, in what order, nested how
    ///   TIMING    when each of them starts and how long it keeps emitting
    ///   PARTS     the extra emitter sets a multi-part effect brings in on later frames, and
    ///             the model that comes with the whole thing
    fn draw_spawning_panel(&mut self, ui: &mut Ui, set_idx: usize, entry_name: &str) {
        egui::ScrollArea::both()
            .id_salt(("spawning_panel", entry_name))
            .show(ui, |ui| {
                self.draw_emitter_roster(ui, set_idx);
                ui.add_space(10.0);
                ui.separator();
                self.draw_entry_parts(ui, entry_name);
            });
    }

    /// The emitter list: which emitters this effect plays, and their nesting.
    ///
    /// Duplicating rather than creating from scratch is deliberate. An emitter carries shader
    /// indices, a texture GUID and a primitive id that only mean anything against the pools THIS
    /// eff ships; a blank emitter would reference none of them and draw nothing at best. Every
    /// emitter here is therefore a copy of one the effect already had, which is also what makes
    /// the edit expressible as a roster the exporter can re-apply.
    /// The timeline frame the effect of set `set_idx` spawns on, and that said in words: its
    /// first spawn in the open move while the view plays it there, else frame 1 of the effect
    /// played alone. Part starts are stored from the spawn and shown on the timeline's frames.
    fn timeline_spawn(&self, set_idx: usize) -> (i64, String) {
        let in_move = self
            .entries
            .iter()
            .filter(|entry| entry.set_idx == set_idx)
            .find_map(|entry| self.open_move_start(&entry.name))
            .filter(|_| self.view == EffectView::InMove);
        match (in_move, &self.open_move) {
            (Some(frame), Some(name)) => (
                i64::from(frame),
                format!("{name} spawns the effect on frame {frame}"),
            ),
            _ => (1, "The effect played alone spawns on frame 1".to_owned()),
        }
    }

    fn draw_emitter_roster(&mut self, ui: &mut Ui, set_idx: usize) {
        let edited: std::collections::HashSet<usize> =
            self.edited_emitters(set_idx).into_iter().collect();
        let moved = self.moved_parts(set_idx);
        let (spawn, spawn_hover) = self.timeline_spawn(set_idx);
        let pristine_set: &[EmitterSnapshot] = self.pristine.get(set_idx).map_or(&[], |p| p);
        let Some(set) = self
            .ptcl
            .as_mut()
            .and_then(|p| p.emitter_sets.get_mut(set_idx))
        else {
            return;
        };

        // Collected and applied after the loop: every one of these changes the list being
        // iterated.
        let mut duplicate: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut move_by: Option<(usize, isize)> = None;
        let mut nest: Option<(usize, i8)> = None;
        let mut reset: Option<usize> = None;
        let mut changed = false;

        let table = crate::eff_attrs::table();
        // The part's timing, one column each; a part without one of these shows a dash. Titles
        // name the values as the Look tab's rows do.
        const COLUMNS: &[(&str, &str, &str)] = &[
            (
                "emission.start",
                "Starts on\nframe",
                "The timeline frame the part starts making particles",
            ),
            (
                "emission.rate",
                "Particles\nper frame",
                "Particles it makes each frame while on. Fractions add up: 0.5 is one every \
                 other frame.",
            ),
            (
                "emission.interval",
                "Wait\nframes",
                "Frames it waits after each burst before the next",
            ),
            (
                "emission.duration",
                "Runs\nframes",
                "How many frames the part keeps making particles, or always: until the effect \
                 is removed. Right-click a value to switch.",
            ),
            (
                "particle_data.life",
                "Particle\nlife",
                "How many frames each particle stays on screen",
            ),
        ];
        let selected = self.selected_emitter;
        let mut select: Option<usize> = None;
        let part_names: Vec<&str> = set.emitters.iter().map(|em| em.name.as_str()).collect();
        let part_names = part_list_names(&part_names);
        egui::Grid::new(("parts_table", set_idx))
            .num_columns(COLUMNS.len() + 2)
            .striped(true)
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                // The menu first: at the end of the row it was past a narrow panel's edge. Every
                // title sits at the top of its cell, centred over its column.
                ui.label("");
                let title = |ui: &mut Ui, text: &str| {
                    // As wide as its own text and two lines tall, so a one-line title sits at
                    // the top like the others and no title widens its column.
                    let font = egui::TextStyle::Body.resolve(ui.style());
                    let width = text
                        .lines()
                        .map(|line| {
                            ui.painter()
                                .layout_no_wrap(line.to_owned(), font.clone(), egui::Color32::WHITE)
                                .size()
                                .x
                        })
                        .fold(0.0, f32::max);
                    let height = 2.0 * ui.text_style_height(&egui::TextStyle::Body);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width + 2.0, height),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(text).strong())
                                    .wrap_mode(egui::TextWrapMode::Extend),
                            )
                        },
                    )
                    .inner
                };
                title(ui, "Part");
                for (id, title_text, doc) in COLUMNS {
                    let doc = if *id == "emission.start" {
                        format!("{doc}. {spawn_hover}")
                    } else {
                        (*doc).to_owned()
                    };
                    title(ui, title_text).on_hover_text(doc);
                }
                ui.end_row();
                let count = set.emitters.len();
                let depths: Vec<u8> = set.emitters.iter().map(|e| e.depth).collect();
                let copy_room: Vec<bool> = (0..count).map(|i| room_to_copy(&depths, i)).collect();
                let nest_room: Vec<bool> = (0..count).map(|i| room_to_nest(&depths, i)).collect();
                for (i, em) in set.emitters.iter_mut().enumerate() {
                    ui.menu_button("⋯", |ui| {
                        if ui
                            .add_enabled(edited.contains(&i), egui::Button::new("Reset this part"))
                            .on_hover_text("Put this part back as the game has it")
                            .clicked()
                        {
                            reset = Some(i);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(copy_room[i], egui::Button::new("Copy"))
                            .on_hover_text("Add another part just like this one")
                            .on_disabled_hover_text(format!(
                                "The game holds at most {} parts made from one part",
                                crate::eff_validate::MAX_CHILDREN
                            ))
                            .clicked()
                        {
                            duplicate = Some(i);
                            ui.close();
                        }
                        if ui
                            .button("Remove")
                            .on_hover_text("Stop this part from playing")
                            .clicked()
                        {
                            remove = Some(i);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(i > 0, egui::Button::new("Draw earlier"))
                            .clicked()
                        {
                            move_by = Some((i, -1));
                            ui.close();
                        }
                        if ui
                            .add_enabled(i + 1 < count, egui::Button::new("Draw later"))
                            .clicked()
                        {
                            move_by = Some((i, 1));
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(
                                nest_room[i],
                                egui::Button::new("Make from the part above"),
                            )
                            .on_hover_text(
                                "Its particles come out of the particles of the part above",
                            )
                            .on_disabled_hover_text(
                                "Only a part on its own, with nothing made from it, can be made \
                                 from another, and one part makes at most 16",
                            )
                            .clicked()
                        {
                            nest = Some((i, 1));
                            ui.close();
                        }
                        if em.depth > 0 && ui.button("Make on its own").clicked() {
                            nest = Some((i, -1));
                            ui.close();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(em.depth as f32 * 12.0);
                        let name = &part_names[i];
                        // Marked as on the Look tab's part tabs.
                        let moved = moved.get(i).copied().unwrap_or(false);
                        let label = if edited.contains(&i) || moved {
                            egui::RichText::new(format!("{name} •"))
                                .color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60))
                        } else {
                            egui::RichText::new(name)
                        };
                        let place = part_place_note(moved, em.source_idx);
                        let hover = if em.depth > 0 {
                            format!(
                                "{}{place}\n\nMade from the particles of the part above it",
                                em.name
                            )
                        } else {
                            format!("{}{place}", em.name)
                        };
                        if ui
                            .selectable_label(selected == i, label)
                            .on_hover_text(hover)
                            .clicked()
                        {
                            select = Some(i);
                        }
                    });
                    let once = attr(em, "emission.is_one_time").is_some_and(|v| v.as_bool());
                    // Whether the part stops is part of its Runs cell, and edited with it.
                    let once_was = pristine_set.get(em.source_idx).and_then(|p| {
                        let at = crate::eff_attrs::index_of("emission.is_one_time")?;
                        Some(p.attrs.get(at).copied().flatten()?.as_bool())
                    });
                    let stop_edited = once_was.is_some_and(|was| was != once);
                    for (id, _, _) in COLUMNS {
                        let index = crate::eff_attrs::index_of(id);
                        let current = index.and_then(|at| em.attrs.get(at).copied().flatten());
                        match (index, current) {
                            (Some(at), Some(mut value)) if *id != "emission.duration" || once => {
                                // An edited cell is tinted, says what it was, and resets from
                                // its menu, as a row on the Look tab does.
                                let was = pristine_set
                                    .get(em.source_idx)
                                    .and_then(|p| p.attrs.get(at).copied().flatten())
                                    .filter(|was| !was.same(value));
                                let runs = *id == "emission.duration";
                                let start = *id == "emission.start";
                                ui.push_id((i, *id), |ui| {
                                    if was.is_some() || (runs && stop_edited) {
                                        let tint = egui::Color32::from_rgb(0x60, 0x50, 0x20);
                                        let widgets = &mut ui.visuals_mut().widgets;
                                        widgets.inactive.weak_bg_fill = tint;
                                        widgets.inactive.bg_fill = tint;
                                    }
                                    // The start is stored from the effect's spawn and shown on
                                    // the timeline's frames.
                                    let (edit, response) = if start {
                                        let mut shown = value.as_i64() + spawn;
                                        let response = ui.add(
                                            egui::DragValue::new(&mut shown)
                                                .speed(0.1)
                                                .range(spawn..=spawn + 100_000),
                                        );
                                        if response.changed() {
                                            value = AttrValue::Int(shown - spawn);
                                        }
                                        (response.changed(), response)
                                    } else {
                                        attr_control(ui, &table[at], &mut value)
                                    };
                                    if edit {
                                        em.attrs[at] = Some(value);
                                        changed = true;
                                    }
                                    let was_text = |was: AttrValue| {
                                        if start {
                                            (was.as_i64() + spawn).to_string()
                                        } else {
                                            attr_text(&table[at], was)
                                        }
                                    };
                                    let response = match (was, runs && stop_edited) {
                                        (_, true) => response
                                            .on_hover_text("was always. Right-click to reset."),
                                        (Some(was), _) => response.on_hover_text(format!(
                                            "was {}. Right-click to reset.",
                                            was_text(was)
                                        )),
                                        (None, _) if runs => {
                                            response.on_hover_text("Right-click to keep it running")
                                        }
                                        (None, _) => response,
                                    };
                                    if was.is_some() || runs {
                                        response.context_menu(|ui| {
                                            if (was.is_some() || stop_edited)
                                                && ui.button("Reset to the game's value").clicked()
                                            {
                                                if let Some(was) = was {
                                                    em.attrs[at] = Some(was);
                                                }
                                                if let Some(once) = once_was {
                                                    set_attr(
                                                        em,
                                                        "emission.is_one_time",
                                                        AttrValue::Int(i64::from(once)),
                                                    );
                                                }
                                                changed = true;
                                                ui.close();
                                            }
                                            if runs
                                                && ui
                                                    .button("Keep running")
                                                    .on_hover_text(
                                                        "Keep making particles until the effect \
                                                         is removed",
                                                    )
                                                    .clicked()
                                            {
                                                set_attr(
                                                    em,
                                                    "emission.is_one_time",
                                                    AttrValue::Int(0),
                                                );
                                                changed = true;
                                                ui.close();
                                            }
                                        });
                                    }
                                });
                            }
                            (Some(at), Some(duration)) => {
                                // Runs, for a part that does not stop.
                                ui.push_id((i, *id), |ui| {
                                    let mut text = egui::RichText::new("always").weak();
                                    if stop_edited {
                                        text =
                                            text.color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60));
                                    }
                                    let frames = duration.as_i64().max(1);
                                    let response = ui
                                        .add(egui::Label::new(text).sense(egui::Sense::click()))
                                        .on_hover_text(if stop_edited {
                                            format!(
                                                "was {}. Keeps making particles until the effect \
                                                 is removed. Right-click to reset or give it a \
                                                 length.",
                                                attr_text(&table[at], duration)
                                            )
                                        } else {
                                            "Keeps making particles until the effect is removed. \
                                             Right-click to give it a length."
                                                .to_owned()
                                        });
                                    response.context_menu(|ui| {
                                        if stop_edited
                                            && ui.button("Reset to the game's value").clicked()
                                        {
                                            if let Some(once) = once_was {
                                                set_attr(
                                                    em,
                                                    "emission.is_one_time",
                                                    AttrValue::Int(i64::from(once)),
                                                );
                                            }
                                            changed = true;
                                            ui.close();
                                        }
                                        if ui
                                            .button(match frames {
                                                1 => "Stop after 1 frame".to_owned(),
                                                n => format!("Stop after {n} frames"),
                                            })
                                            .clicked()
                                        {
                                            em.attrs[at] = Some(AttrValue::Int(frames));
                                            set_attr(em, "emission.is_one_time", AttrValue::Int(1));
                                            changed = true;
                                            ui.close();
                                        }
                                    });
                                });
                            }
                            _ => {
                                ui.weak("–");
                            }
                        }
                    }
                    ui.end_row();
                }
            });
        if let Some(i) = select {
            self.selected_emitter = i;
            self.emitter_tab = EmitterTab::Attributes;
        }

        // Every one of these operates on an emitter's whole SUBTREE, not on the one row. The
        // list is flat but the emitters are a tree, and the nesting is carried by `depth` and
        // position — so moving a parent without its children hands those children to whichever
        // emitter ends up above them.
        let mut select_copy = None;
        if let Some(i) = duplicate {
            let range = subtree(&set.emitters, i);
            let mut copies: Vec<EmitterDef> = set.emitters[range.clone()].to_vec();
            // Names are renamed against the list AS IT GROWS, so duplicating a parent with two
            // identically-shaped children does not produce two emitters called the same thing.
            let mut taken = set.emitters.clone();
            for copy in copies.iter_mut() {
                copy.name = unique_emitter_name(&copy.name, &taken);
                // `source_idx` keeps pointing at the emitter this was copied FROM, so the copy
                // is diffed against the same pristine values and the exporter knows what to
                // clone. It is not the copy's position in the list.
                taken.push(copy.clone());
            }
            if !copies.is_empty() {
                let at = range.end;
                set.emitters.splice(at..at, copies);
                changed = true;
                // The copy is what gets edited next.
                select_copy = Some(at);
            }
        }
        if let Some(i) = remove {
            let range = subtree(&set.emitters, i);
            if !range.is_empty() {
                set.emitters.drain(range);
                changed = true;
            }
        }
        if let Some((i, delta)) = move_by {
            let range = subtree(&set.emitters, i);
            if !range.is_empty() {
                let depth = set.emitters[i].depth;
                if delta > 0 {
                    // Swap places with the next sibling, taking both subtrees whole.
                    let next = range.end;
                    if set.emitters.get(next).is_some_and(|e| e.depth == depth) {
                        let next_end = subtree(&set.emitters, next).end;
                        set.emitters[range.start..next_end].rotate_left(range.len());
                        changed = true;
                    }
                } else if range.start > 0 {
                    // The row above is either the previous sibling's LAST descendant or this
                    // emitter's parent. Walk back to the previous sibling's root; a parent
                    // (shallower than us) means there is no earlier sibling to swap with.
                    let prev = (0..range.start)
                        .rev()
                        .find(|&k| set.emitters[k].depth <= depth);
                    if let Some(prev) = prev.filter(|&k| set.emitters[k].depth == depth) {
                        set.emitters[prev..range.end].rotate_left(range.start - prev);
                        changed = true;
                    }
                }
            }
        }
        if let Some((i, delta)) = nest {
            // At most one level deeper than the emitter above: any more and there is no emitter
            // at the depth being asked for, and the exporter would clamp it back anyway.
            let ceiling = if i == 0 {
                0
            } else {
                set.emitters
                    .get(i - 1)
                    .map(|p| p.depth.saturating_add(1).min(1))
                    .unwrap_or(0)
            };
            if let Some(root) = set.emitters.get(i) {
                let old_depth = root.depth;
                let new_depth = (old_depth as i16 + delta as i16).clamp(0, ceiling as i16) as u8;
                let applied = new_depth as i16 - old_depth as i16;
                // Nesting is a tree operation: descendants have to move by the same depth
                // delta or the root leaves its children behind as siblings.
                if shift_subtree_depth(&mut set.emitters, i, applied) {
                    changed = true;
                }
            }
        }
        if let Some(at) = select_copy {
            self.selected_emitter = at;
        }
        if changed {
            self.clamp_selected_emitter(set_idx);
            touch(&mut self.eff_dirty_at, &mut self.edit_rev);
        }
        if let Some(i) = reset {
            self.history_close_step();
            self.reset_part_fields(set_idx, i);
            self.history.next_label = Some(self.reset_part_label(set_idx, i));
        }
    }

    /// The entry's own spawn structure: the extra parts of a multi-part effect, and the external
    /// model.
    ///
    /// Both live in the eff's header rather than in the particle data, and the live carrier
    /// rides the same live clone as the selected emitter data. The carrier applies this header
    /// before cloning, so a changed part list also determines which sets and external model are
    /// transferred.
    fn draw_entry_parts(&mut self, ui: &mut Ui, entry_name: &str) {
        ui.label(egui::RichText::new("Linked effects and model").strong())
            .on_hover_text(
                "An effect can bring in more effects on later frames, and a model. Changes here \
                 show in the game after Send edits, the next time the effect plays.",
            );
        let Some(idx) = self
            .entry_info
            .iter()
            .position(|e| e.name.eq_ignore_ascii_case(entry_name))
        else {
            ui.colored_label(
                egui::Color32::GRAY,
                "This entry is not in the file's entry table, so it has no spawn structure to \
                 edit.",
            );
            return;
        };
        // Each set by the effect that plays it, in the list's words; a set no effect names
        // keeps its own name.
        let list_names = entry_list_names(&self.entries);
        let set_names: Vec<String> = self
            .ptcl
            .as_ref()
            .map(|p| {
                p.emitter_sets
                    .iter()
                    .enumerate()
                    .map(|(s, set)| {
                        self.entries
                            .iter()
                            .position(|e| e.set_idx == s)
                            .and_then(|e| list_names.get(e).cloned())
                            .unwrap_or_else(|| set.name.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let pristine = self.entry_pristine.get(idx).cloned().unwrap_or_default();
        let Some(entry) = self.entry_info.get_mut(idx) else {
            return;
        };
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.label("Main effect");
            let current = entry
                .set_idx
                .and_then(|s| set_names.get(s).cloned())
                .unwrap_or_else(|| "none".to_string());
            egui::ComboBox::from_id_salt("entry_primary_set")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(entry.set_idx.is_none(), "none")
                        .clicked()
                    {
                        entry.set_idx = None;
                        changed = true;
                    }
                    for (s, name) in set_names.iter().enumerate() {
                        if ui
                            .selectable_label(entry.set_idx == Some(s), name)
                            .clicked()
                        {
                            entry.set_idx = Some(s);
                            changed = true;
                        }
                    }
                });
            if entry.set_idx != pristine.set_idx && ui.small_button("Reset").clicked() {
                entry.set_idx = pristine.set_idx;
                changed = true;
            }
        });

        let mut remove_part: Option<usize> = None;
        for (i, variant) in entry.variants.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("linked {}", i + 1));
                ui.label("frame");
                let mut frame = variant.start_frame as i32;
                if ui
                    .add(egui::DragValue::new(&mut frame).speed(1.0).range(0..=65535))
                    .on_hover_text("Frames after the effect starts before this one comes in")
                    .changed()
                {
                    variant.start_frame = frame as u16;
                    changed = true;
                }
                let current = variant
                    .set_idx
                    .and_then(|s| set_names.get(s).cloned())
                    .unwrap_or_else(|| "none".to_string());
                egui::ComboBox::from_id_salt(("part_set", i))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(variant.set_idx.is_none(), "none")
                            .clicked()
                        {
                            variant.set_idx = None;
                            changed = true;
                        }
                        for (s, name) in set_names.iter().enumerate() {
                            if ui
                                .selectable_label(variant.set_idx == Some(s), name)
                                .clicked()
                            {
                                variant.set_idx = Some(s);
                                changed = true;
                            }
                        }
                    });
                ui.label("bone");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut variant.bone)
                            .hint_text("effect attachment")
                            .desired_width(120.0),
                    )
                    .on_hover_text("Bone it attaches to; empty uses the effect's own")
                    .changed();
                if ui.small_button("remove").clicked() {
                    remove_part = Some(i);
                }
            });
        }
        if let Some(i) = remove_part {
            entry.variants.remove(i);
            changed = true;
        }
        ui.horizontal(|ui| {
            if ui
                .small_button("Add linked effect")
                .on_hover_text("Play another effect as part of this one")
                .clicked()
            {
                // A new part copies the last one's set rather than defaulting to "none": a part
                // with no set is a part that does nothing, and every part added this way would
                // then need a second edit before it did anything at all.
                let set_idx = entry
                    .variants
                    .last()
                    .and_then(|v| v.set_idx)
                    .or(entry.set_idx)
                    .or(if set_names.is_empty() { None } else { Some(0) });
                let start_frame = entry
                    .variants
                    .last()
                    .map(|v| v.start_frame.saturating_add(1))
                    .unwrap_or(0);
                entry.variants.push(EffVariantInfo {
                    start_frame,
                    set_idx,
                    bone: String::new(),
                });
                changed = true;
            }
            if entry.variants != pristine.variants && ui.small_button("Reset parts").clicked() {
                entry.variants = pristine.variants.clone();
                changed = true;
            }
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Model");
            let mut has_model = entry.model.is_some();
            if ui
                .checkbox(&mut has_model, "")
                .on_hover_text(
                    "Whether this effect spawns an external model alongside its particles",
                )
                .changed()
            {
                entry.model = has_model.then(|| {
                    pristine.model.clone().unwrap_or(EffModelInfo {
                        name: String::new(),
                        flag: 0,
                    })
                });
                changed = true;
            }
            if let Some(model) = entry.model.as_mut() {
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut model.name)
                            .hint_text("model name")
                            .desired_width(160.0),
                    )
                    .changed();
                ui.label("spawn flag");
                let mut flag = model.flag as i32;
                if ui
                    .add(egui::DragValue::new(&mut flag).speed(1.0).range(0..=255))
                    .on_hover_text("The model's spawn condition byte, from the eff's model table")
                    .changed()
                {
                    model.flag = flag as u8;
                    changed = true;
                }
            }
            if entry.model != pristine.model && ui.small_button("Reset model").clicked() {
                entry.model = pristine.model.clone();
                changed = true;
            }
        });

        if changed {
            touch(&mut self.eff_dirty_at, &mut self.edit_rev);
        }
    }

    /// The texture panel: what the selected emitter samples, what it looks like, and the two
    /// things you can do to it.
    ///
    /// Reached through the emitter that uses it rather than from a flat list of the pool: 60-odd
    /// entries named `ef_cmn_line02` is guesswork, and going via the emitter means you are always
    /// looking at the one you are about to change.
    ///
    /// The two operations are deliberately labelled apart, because their blast radius differs:
    ///   SWAP   — point THIS emitter at another texture the eff already has. A per-emitter edit.
    ///   IMPORT — replace a pool texture's pixels. Every emitter sampling it changes.
    fn draw_texture_panel(&mut self, ui: &mut Ui) {
        // One switch for the slots the part samples, rather than a texture id typed into a
        // settings group for the second and third.
        if self.texture_slots.len() > 1 {
            ui.horizontal(|ui| {
                for (slot, _) in self.texture_slots.clone() {
                    ui.selectable_value(
                        &mut self.texture_slot,
                        slot,
                        format!("Texture {}", slot + 1),
                    )
                    .on_hover_text(format!("Texture slot {} of this part", slot + 1));
                }
            });
        }
        let slot = self.texture_slot;
        let Some((index, texture)) = self.selected_texture.clone() else {
            ui.label(
                egui::RichText::new(if self.ptcl.is_none() {
                    "no eff loaded"
                } else {
                    "this part uses no texture"
                })
                .small()
                .color(egui::Color32::GRAY),
            );
            return;
        };

        // Swap picker. Labels are rebuilt here rather than shared with the emitter column so
        // this panel does not depend on that column having drawn first.
        let labels: Vec<String> = self
            .ptcl
            .as_ref()
            .map(|p| {
                p.bntx_textures
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let name = if t.tex_name.is_empty() {
                            format!("texture {i}")
                        } else {
                            t.tex_name.clone()
                        };
                        if t.width > 0 && t.height > 0 {
                            format!("{name}  ({}×{})", t.width, t.height)
                        } else {
                            name
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Every texture of the file is offered but those that would break the part. A part
        // reads its texture by id and draws it over the same 0..1 coordinates whatever its size:
        // its frame grid, scrolling and repeats are counts and fractions, never pixels, and the
        // export and the send only change the id. So another size or format only changes the
        // look (another shape stretches; a sheet laid out otherwise shows other frames).
        // What does break it: a texture that stacks another number of pictures is read through
        // another kind of texture view, which the game's shader for this part does not take.
        // And texture 2 and 3 name a texture by its id, which one added here has only once the
        // file is written.
        let blocked: Vec<Option<String>> = self
            .ptcl
            .as_ref()
            .map(|p| {
                p.bntx_textures
                    .iter()
                    .map(|t| texture_swap_blocked(t, &texture, slot))
                    .collect()
            })
            .unwrap_or_default();
        let other_size: Vec<bool> = self
            .ptcl
            .as_ref()
            .map(|p| {
                p.bntx_textures
                    .iter()
                    .map(|t| (t.width, t.height) != (texture.width, texture.height))
                    .collect()
            })
            .unwrap_or_default();
        let mut swap_to = None;
        let texture_filter = &mut self.texture_filter;
        // Each row shows its texture: a replaced one as its new picture.
        let replaced: Vec<(String, String)> = self
            .texture_imports
            .iter()
            .filter(|t| !t.png_path.is_empty())
            .map(|t| (t.texture_name.clone(), t.png_path.clone()))
            .collect();
        let thumbs = &mut self.texture_thumbs;
        thumbs.keep_for((
            self.loaded_path.clone(),
            self.texture_pool_gen,
            replaced.clone(),
        ));
        let pool = self.texture_pool.as_deref();
        let pool_names: Vec<String> = self
            .ptcl
            .as_ref()
            .map(|p| p.bntx_textures.iter().map(|t| t.tex_name.clone()).collect())
            .unwrap_or_default();
        let picker = egui::ComboBox::from_id_salt(("emitter_texture_swap", slot))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .selected_text(
                labels
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| "(not in this file)".to_string()),
            )
            // No wider than the panel: at 280 it widened a narrow panel's settings, and the
            // texture buttons under it ran past the edge instead of wrapping.
            .width(ui.available_width().min(280.0))
            .height(420.0)
            .show_ui(ui, |ui| {
                let search = ui.add(
                    egui::TextEdit::singleline(texture_filter)
                        .hint_text("Search textures")
                        .desired_width(260.0),
                );
                let opened = !search.has_focus() && !ui.memory(|m| m.focused().is_some());
                if opened {
                    search.request_focus();
                }
                let mut any = false;
                for (i, label) in labels.iter().enumerate() {
                    if search_rank(texture_filter, &[label.as_str()]).is_none() {
                        continue;
                    }
                    any = true;
                    let name = pool_names.get(i).map(String::as_str).unwrap_or_default();
                    let picture = || match replaced.iter().find(|(texture, _)| texture == name) {
                        Some((_, png)) => std::fs::read(png).ok().and_then(|png| {
                            crate::texture_import::decode_png_rgba(&png, Some(THUMB)).ok()
                        }),
                        None => crate::texture_import::decode_preview(
                            pool?,
                            i,
                            name,
                            crate::texture_import::Form::Editable,
                            Some(THUMB),
                        )
                        .ok(),
                    };
                    // One selectable row, picture and name: only the name was, beside the
                    // picture.
                    let background = ui.painter().add(egui::Shape::Noop);
                    let selected = index == i;
                    let why = blocked.get(i).cloned().flatten();
                    let drawn = ui.horizontal(|ui| {
                        thumbs.draw(ui, i, picture);
                        let text = egui::RichText::new(label.as_str());
                        let text = if why.is_some() {
                            text.weak()
                        } else if selected {
                            text.color(ui.visuals().selection.stroke.color)
                        } else {
                            text
                        };
                        ui.add(egui::Label::new(text).selectable(false));
                    });
                    // The whole width of the list, as a selectable label's highlight is.
                    let rect = drawn.response.rect;
                    let right = ui.max_rect().right();
                    let rect = if right.is_finite() {
                        rect.with_max_x(right.max(rect.right()))
                    } else {
                        rect
                    };
                    let row = if let Some(why) = why {
                        ui.interact(
                            rect,
                            ui.scope_id().with(("texture_row", i)),
                            egui::Sense::hover(),
                        )
                        .on_hover_text(why);
                        None
                    } else {
                        let row = ui.interact(
                            rect,
                            ui.scope_id().with(("texture_row", i)),
                            egui::Sense::click(),
                        );
                        if selected || row.hovered() {
                            let look = ui.style().interact_selectable(&row, selected);
                            ui.painter().set(
                                background,
                                egui::epaint::RectShape::filled(
                                    rect.expand(1.0),
                                    look.corner_radius,
                                    look.weak_bg_fill,
                                ),
                            );
                        }
                        Some(row)
                    };
                    if let Some(mut row) = row {
                        if other_size.get(i).copied().unwrap_or(false) {
                            row = row.on_hover_text(format!(
                                "Another size than this part's texture ({}×{}). It fills the \
                                 part the same way: a picture of another shape stretches, and \
                                 a sheet of frames laid out otherwise shows other frames.",
                                texture.width, texture.height
                            ));
                        }
                        // Opens on the current texture rather than at the top of the list.
                        if opened && index == i {
                            row.scroll_to_me(Some(egui::Align::Center));
                        }
                        if row.clicked() {
                            swap_to = Some(i as u32);
                            ui.close();
                        }
                    }
                }
                if !any {
                    ui.weak("No matching textures");
                }
            });
        // A search left behind when the list closed would greet the next opening.
        if picker.inner.is_none() {
            self.texture_filter.clear();
        }
        picker.response.on_hover_text(
            "Point this part at another of the file's textures. Goes to the game with the next \
             Send, like any other edit.",
        );
        // Every other control in this window shows what it started as; a swap is an edit like
        // any other and gets the same treatment, so a changed texture is visible without
        // opening the picker to compare.
        let original = self
            .selected_entry
            .map(|entry_idx| self.entries[entry_idx].set_idx)
            .and_then(|set_idx| {
                let source = self
                    .ptcl
                    .as_ref()?
                    .emitter_sets
                    .get(set_idx)?
                    .emitters
                    .get(self.selected_emitter)?
                    .source_idx;
                self.pristine.get(set_idx)?.get(source)
            })
            .and_then(|snapshot| {
                if slot == 0 {
                    return snapshot.texture_index;
                }
                let at = crate::eff_attrs::index_of(TEXTURE_SLOT_IDS[slot])?;
                let guid = snapshot.attrs.get(at).copied().flatten()?.as_u64();
                let textures = &self.ptcl.as_ref()?.bntx_textures;
                textures
                    .iter()
                    .position(|t| t.id == Some(guid))
                    .map(|i| i as u32)
            });
        if original.map(|o| o as usize) != Some(index) {
            let was = original
                .and_then(|o| labels.get(o as usize).cloned())
                .unwrap_or_else(|| "nothing".to_string());
            ui.label(
                egui::RichText::new(format!("swapped, was {was}"))
                    .small()
                    .color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60)),
            );
        }
        if let Some(i) = swap_to {
            self.point_slot_at(slot, i as usize);
        }

        self.draw_texture_preview(ui, index, &texture);
        self.draw_texture_import(ui, index, &texture);
    }

    /// When the replacement PNG at `path` was last changed, read from the disk at most once a
    /// second: a file system call on every frame is free here and slow on Windows (antivirus,
    /// synced folders). No path, no read.
    fn replacement_stamp(&mut self, path: &str, now: Instant) -> u128 {
        if path.is_empty() {
            return 0;
        }
        if let Some((kept, at, stamp)) = &self.replacement_stamp {
            if kept == path && now.duration_since(*at) < std::time::Duration::from_secs(1) {
                return *stamp;
            }
        }
        let stamp = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis())
            .unwrap_or(0);
        self.replacement_stamp = Some((path.to_owned(), now, stamp));
        stamp
    }

    /// Draw the texture itself.
    ///
    /// Decoding is BCn → RGBA over up to a megapixel, so it is done ONCE per texture and the
    /// result kept as a GPU handle. Doing it per frame at 60fps would spend more time
    /// decompressing this thumbnail than rendering the rest of the app.
    fn draw_texture_preview(&mut self, ui: &mut Ui, index: usize, texture: &TextureInfo) {
        const PREVIEW: u32 = 112;
        // A replaced texture previews from the PNG the user picked. The pool still holds the
        // game's original — it is only rebuilt when the carrier is built — so decoding the pool
        // here would show the texture that was just replaced and read as "the import did nothing".
        let replacement = self
            .texture_imports
            .iter()
            .find(|t| t.texture_name == texture.tex_name && !t.png_path.is_empty())
            .map(|t| t.png_path.clone())
            .unwrap_or_default();
        let form = self.texture_form();
        // The form is part of the key: flipping the checkbox has to re-decode, and so does
        // re-importing the SAME path after painting it again — hence the file's mtime.
        let stamp = self.replacement_stamp(&replacement, Instant::now());
        let key = (
            self.loaded_path.clone().unwrap_or_default(),
            index,
            replacement.clone(),
            form == crate::texture_import::Form::Raw,
            stamp,
            self.texture_pool_gen,
        );
        if self.texture_preview.as_ref().map(|(k, _)| k) != Some(&key) {
            let decoded = if replacement.is_empty() {
                self.texture_pool.as_ref().and_then(|pool| {
                    crate::texture_import::decode_preview(
                        pool,
                        index,
                        &texture.tex_name,
                        form,
                        Some(PREVIEW),
                    )
                    .ok()
                })
            } else {
                // Previewed through the same conversion the import runs, so an edited mask shows
                // as the shape the game will draw rather than as a black-and-white square.
                std::fs::read(&replacement)
                    .ok()
                    .and_then(|png| match self.texture_pool.as_ref() {
                        Some(pool) => crate::texture_import::decode_import_preview(
                            pool,
                            index,
                            &texture.tex_name,
                            &png,
                            form,
                            Some(PREVIEW),
                        )
                        .ok(),
                        None => crate::texture_import::decode_png_rgba(&png, Some(PREVIEW)).ok(),
                    })
            };
            let handle = decoded.map(|image| {
                let size = [image.width() as usize, image.height() as usize];
                ui.ctx().load_texture(
                    format!("eff_tex_{index}"),
                    egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                    egui::TextureOptions::LINEAR,
                )
            });
            self.texture_form_note = match self.texture_pool.as_ref() {
                Some(pool) => describe_form(pool, index, &texture.tex_name, form),
                None => String::new(),
            };
            self.texture_preview = Some((key, handle));
        }

        match self.texture_preview.as_ref().and_then(|(_, h)| h.as_ref()) {
            Some(handle) => {
                let size = handle.size_vec2();
                let scale = PREVIEW as f32 / size.x.max(size.y).max(1.0);
                // Checkerboard behind it, for the textures that still carry alpha in editable
                // form: on a flat background a soft-edged puff reads as "nothing decoded". Masks
                // come through opaque, so it simply never shows for those.
                let (rect, _) = ui.allocate_exact_size(size * scale, egui::Sense::hover());
                let painter = ui.painter();
                let cell = 8.0;
                let cols = (rect.width() / cell).ceil() as i32;
                let rows = (rect.height() / cell).ceil() as i32;
                for row in 0..rows {
                    for col in 0..cols {
                        let shade = if (row + col) % 2 == 0 { 60 } else { 78 };
                        let cell_rect = egui::Rect::from_min_size(
                            rect.min + egui::vec2(col as f32 * cell, row as f32 * cell),
                            egui::vec2(cell, cell),
                        )
                        .intersect(rect);
                        painter.rect_filled(cell_rect, 0.0, egui::Color32::from_gray(shade));
                    }
                }
                painter.image(
                    handle.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                ui.label(
                    egui::RichText::new(if texture.convertible {
                        "preview unavailable"
                    } else {
                        "no preview: Visionary cannot read this format"
                    })
                    .small()
                    .color(egui::Color32::DARK_GRAY),
                );
            }
        }

        // One switch for the whole panel: preview, export, and import all follow it. Keeping
        // them together is deliberate — they are the same question, and letting them disagree is
        // how an image gets interpreted in the form it is not in.
        // What the texture holds is said in the checkbox's hover, not as a line beside it.
        let mut hover = String::new();
        if !self.texture_form_note.is_empty() {
            hover = format!("Showing {}.\n\n", self.texture_form_note);
        }
        hover.push_str(
            "Switches all three at once: what the preview shows, what 'Export PNG' writes, and \
             how an imported PNG is read.\n\nOff: as the game samples it, with the channel \
             swizzle resolved, transparency against the checkerboard, and exports you can paint \
             on.\n\nOn: the stored channels as the file holds them. Leave it ticked to import a \
             raw PNG back.",
        );
        ui.checkbox(
            &mut self.show_raw_textures,
            egui::RichText::new("Show channels as stored").small(),
        )
        .on_hover_text(hover);
    }

    /// Pool-shape operations: give this emitter a texture of its own, or drop one.
    ///
    /// These exist because a pool texture is SHARED. Every emitter that samples it changes when
    /// it changes, and the `ef_cmn_*` names are shared by dozens of effects inside a single eff,
    /// so "edit the texture" and "edit this effect's texture" are different jobs. Duplicating and
    /// repointing is the only way to do the second one.
    fn draw_texture_pool_ops(&mut self, ui: &mut Ui, index: usize, texture: &TextureInfo) {
        let pool = self.texture_pool.clone();
        let names = self.texture_names();
        let can_convert = texture.convertible && pool.is_some();
        // A texture added here has no id until the file is written, and only texture 1 is
        // pointed at a texture by name.
        let first_slot = self.texture_slot == 0;
        let has_emitter = self.selected_entry.is_some() && first_slot;
        let users = self.texture_users(index);

        ui.horizontal_wrapped(|ui| {
            // A duplicate is a rename, not a re-encode, so it works for ANY format Visionary can
            // slice — including the ones it cannot decode to a PNG.
            if ui
                .add_enabled(
                    pool.is_some() && has_emitter,
                    egui::Button::new("Copy for this part").small(),
                )
                .on_hover_text(
                    "Add a private copy of this texture and point this part at it. Edit the \
                     copy freely — every other effect sampling the original is left alone. The \
                     copy is byte-identical, not re-encoded.",
                )
                .on_disabled_hover_text(if pool.is_none() {
                    "This file has no textures"
                } else if !first_slot {
                    "Works on texture 1 only"
                } else {
                    "Select a part first: the copy is given to it"
                })
                .clicked()
            {
                let pool_bytes = pool.clone().unwrap_or_default();
                let new_name =
                    crate::texture_import::unique_texture_name(&names, &texture.tex_name);
                match crate::texture_import::duplicate_texture(
                    &pool_bytes,
                    &names,
                    index,
                    &new_name,
                ) {
                    Ok(rebuilt) => {
                        let mut info = texture.clone();
                        info.id = None;
                        info.tex_name = new_name.clone();
                        self.adopt_appended_texture(rebuilt, info);
                        self.pending_texture_additions
                            .push(crate::mod_project::TextureAddition {
                                texture_name: new_name.clone(),
                                template_name: texture.tex_name.clone(),
                                png_path: String::new(),
                                raw: false,
                            });
                        self.texture_note =
                            Some((format!("added {new_name}: this part now uses it"), true));
                    }
                    Err(e) => self.texture_note = Some((format!("duplicate failed: {e}"), false)),
                }
            }

            if ui
                .add_enabled(
                    can_convert && has_emitter,
                    egui::Button::new("Import as new texture").small(),
                )
                .on_hover_text(format!(
                    "Add your picture as a new texture for this part only; the old one stays for \
                     the other parts. It must be {}×{}.",
                    texture.width, texture.height
                ))
                .on_disabled_hover_text(if pool.is_none() {
                    "This file has no textures".to_string()
                } else if !first_slot {
                    "Works on texture 1 only".to_string()
                } else if !has_emitter {
                    "Select a part first: the new texture is given to it".to_string()
                } else {
                    format!("Visionary cannot convert {}", texture.format)
                })
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose a PNG for the new texture")
                    .add_filter("PNG", &["png"])
                    .pick_file()
                {
                    let pool_bytes = pool.clone().unwrap_or_default();
                    let new_name =
                        crate::texture_import::unique_texture_name(&names, &texture.tex_name);
                    let form = self.texture_form();
                    // Encoded NOW so a wrong size or a bad image is rejected here, where the user
                    // is looking at it — not silently, mid carrier build, minutes later.
                    match std::fs::read(&path)
                        .map_err(anyhow::Error::from)
                        .and_then(|png| {
                            crate::texture_import::add_texture_from_png(
                                &pool_bytes,
                                &names,
                                index,
                                &new_name,
                                &png,
                                form,
                            )
                        }) {
                        Ok((rebuilt, report)) => {
                            let mut info = texture.clone();
                            info.id = None;
                            info.tex_name = new_name.clone();
                            self.adopt_appended_texture(rebuilt, info);
                            self.pending_texture_additions.push(
                                crate::mod_project::TextureAddition {
                                    texture_name: new_name.clone(),
                                    template_name: texture.tex_name.clone(),
                                    png_path: path.to_string_lossy().to_string(),
                                    raw: form == crate::texture_import::Form::Raw,
                                },
                            );
                            self.texture_note = Some((
                                format!(
                                    "added {new_name}: {}×{} {}, this part now uses it",
                                    report.width, report.height, report.format
                                ),
                                true,
                            ));
                        }
                        Err(e) => self.texture_note = Some((format!("import failed: {e}"), false)),
                    }
                }
            }

            // Deleting is gated on nothing sampling it, because a pool without a texture some
            // emitter still addresses leaves that emitter pointing at a GUID no descriptor holds
            // — which draws nothing at all. Repoint the users first (the swap picker only offers
            // same-size textures, so the replacement always fits).
            let deletable = pool.is_some() && users == 0 && names.len() > 1;
            if ui
                .add_enabled(deletable, egui::Button::new("Delete texture").small())
                .on_hover_text(
                    "Remove this texture from the file. Only possible once no part uses it.",
                )
                .on_disabled_hover_text(if pool.is_none() {
                    "This file has no textures".to_string()
                } else if names.len() <= 1 {
                    "This is the file's only texture".to_string()
                } else {
                    match users {
                        1 => "1 part uses this texture. Give it another texture first.".to_owned(),
                        n => {
                            format!("{n} parts use this texture. Give them another texture first.")
                        }
                    }
                })
                .clicked()
            {
                let pool_bytes = pool.clone().unwrap_or_default();
                match crate::texture_import::remove_texture(&pool_bytes, &names, index) {
                    Ok(rebuilt) => {
                        let gone = texture.tex_name.clone();
                        self.drop_texture_at(index, rebuilt);
                        self.pending_texture_removals.push(gone.clone());
                        self.texture_note = Some((format!("removed {gone}"), true));
                    }
                    Err(e) => self.texture_note = Some((format!("delete failed: {e}"), false)),
                }
            }
        });

        if users > 1 {
            ui.label(
                egui::RichText::new(format!("{users} parts share this texture"))
                    .small()
                    .color(egui::Color32::from_rgb(0xE0, 0xC0, 0x60)),
            )
            .on_hover_text("Editing it changes all of them.");
        }
    }

    /// Drop pool texture `index` from the editor's view, keeping emitter indices valid.
    ///
    /// Removing a texture shifts every LATER index down by one, and the pristine snapshots have
    /// to shift with the working copy — otherwise the next diff reports a swap on every emitter
    /// above the hole, and those phantom edits would ship.
    fn drop_texture_at(&mut self, index: usize, pool: Vec<u8>) {
        self.texture_pool = Some(pool);
        self.texture_pool_gen += 1;
        // A step's texture numbers point into the pool as it was.
        self.clear_history("a texture was deleted");
        let shift = |slot: &mut Option<u32>| {
            if let Some(i) = slot {
                if *i as usize > index {
                    *i -= 1;
                }
            }
        };
        if let Some(ptcl) = self.ptcl.as_mut() {
            ptcl.bntx_textures.remove(index);
            for set in ptcl.emitter_sets.iter_mut() {
                for em in set.emitters.iter_mut() {
                    shift(&mut em.texture_index);
                }
            }
        }
        for set in self.pristine.iter_mut() {
            for snapshot in set.iter_mut() {
                shift(&mut snapshot.texture_index);
            }
        }
        self.selected_texture = None;
        touch(&mut self.eff_dirty_at, &mut self.edit_rev);
    }

    fn draw_texture_import(&mut self, ui: &mut Ui, index: usize, texture: &TextureInfo) {
        let name = texture.tex_name.clone();
        let replaced = self
            .texture_imports
            .iter()
            .find(|t| t.texture_name == name)
            .cloned();

        ui.horizontal_wrapped(|ui| {
            let size = ui.label(
                egui::RichText::new(format!("{}×{}", texture.width, texture.height))
                    .small()
                    .color(egui::Color32::GRAY),
            );
            if !texture.format.is_empty() {
                size.on_hover_text(format!("Stored as {}", texture.format));
            }
            let pool = self.texture_pool.clone();
            let can_convert = texture.convertible && pool.is_some();

            // ONE export button, whose form the "Show channels as stored" box decides — the same box that
            // decides what the preview shows and how an imported PNG is read. Two buttons made
            // the form settable in two places that could disagree with the preview; with one,
            // what you see is what you get and there is nothing to keep in sync.
            let form = self.texture_form();
            if ui
                .add_enabled(can_convert, egui::Button::new("Export PNG").small())
                .on_hover_text(match form {
                    crate::texture_import::Form::Editable => {
                        "Save what the preview is showing as something you can paint on: black \
                         where the effect is empty, white where it is solid, and it keeps its \
                         transparency. Either edit works, so painting black erases and so does \
                         erasing."
                    }
                    crate::texture_import::Form::Raw => {
                        "Save the texture's stored channels as the file holds them. Only useful \
                         if you know the packing, and 'Show channels as stored' has to stay \
                         ticked to import it back."
                    }
                })
                .on_disabled_hover_text(if pool.is_none() {
                    "This file has no textures".to_string()
                } else {
                    format!("Visionary cannot convert {}", texture.format)
                })
                .clicked()
            {
                // The suffix keeps the two forms apart on disk. Importing the wrong one puts the
                // shape in the wrong channel, and the filename is the only warning you get.
                let suffix = if form == crate::texture_import::Form::Raw {
                    "_raw"
                } else {
                    ""
                };
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Export texture as PNG")
                    .set_file_name(format!("{name}{suffix}.png"))
                    .add_filter("PNG", &["png"])
                    .save_file()
                {
                    let pool = pool.clone().unwrap_or_default();
                    self.texture_note = Some(
                        match crate::texture_import::export_png(&pool, index, &name, form)
                            .and_then(|png| Ok(std::fs::write(&path, png)?))
                        {
                            Ok(()) => (
                                format!(
                                    "exported {name}{suffix}.png ({})",
                                    describe_form(&pool, index, &name, form)
                                ),
                                true,
                            ),
                            Err(e) => (format!("export failed: {e}"), false),
                        },
                    );
                }
            }

            let users = match self.texture_users(index) {
                1 => "1 part".to_owned(),
                n => format!("{n} parts"),
            };
            if ui
                .add_enabled(can_convert, egui::Button::new("Replace with PNG").small())
                .on_hover_text(match form {
                    crate::texture_import::Form::Editable => format!(
                        "Use your own picture for this texture, read the way Export PNG wrote \
                         it: black is empty, white is solid. It must be {}×{}.\n\nChanges this \
                         texture for every part that uses it ({users}).",
                        texture.width, texture.height
                    ),
                    crate::texture_import::Form::Raw => format!(
                        "Use your own picture, read as the texture's stored channels: right only \
                         for a PNG exported with Show channels as stored ticked. It must be \
                         {}×{}.\n\nChanges this texture for every part that uses it ({users}).",
                        texture.width, texture.height
                    ),
                })
                .on_disabled_hover_text(if pool.is_none() {
                    "This file has no textures".to_string()
                } else {
                    format!("Visionary cannot convert {}", texture.format)
                })
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose a PNG for this texture")
                    .add_filter("PNG", &["png"])
                    .pick_file()
                {
                    // Encode once NOW so a bad image is rejected here, where the user is
                    // looking at it — not silently, mid carrier build, minutes later.
                    let pool = pool.clone().unwrap_or_default();
                    let names: Vec<String> = self.texture_names();
                    match std::fs::read(&path)
                        .map_err(anyhow::Error::from)
                        .and_then(|png| {
                            crate::texture_import::replace_with_png(
                                &pool, &names, index, &png, form,
                            )
                        }) {
                        Ok((_, report)) => {
                            self.pending_texture_imports
                                .push(crate::mod_project::TextureImport {
                                    texture_name: name.clone(),
                                    png_path: path.to_string_lossy().to_string(),
                                    raw: form == crate::texture_import::Form::Raw,
                                });
                            let shape = match report.layout {
                                Some(layout) => format!(": {}", layout_note(layout)),
                                None => String::new(),
                            };
                            let note = match &report.format_substituted_from {
                                Some(original) => format!(
                                    "{name}: {}×{}, {original} cannot be written, saved as {}{shape}",
                                    report.width, report.height, report.format
                                ),
                                None => format!(
                                    "{name}: {}×{} {}{shape}",
                                    report.width, report.height, report.format
                                ),
                            };
                            self.texture_note = Some((note, true));
                            touch(&mut self.eff_dirty_at, &mut self.edit_rev);
                        }
                        Err(e) => self.texture_note = Some((format!("import failed: {e}"), false)),
                    }
                }
            }

            if let Some(existing) = &replaced {
                if ui
                    .small_button("Restore original")
                    .on_hover_text(format!("Stop using {}", existing.png_path))
                    .clicked()
                {
                    // Recorded as an import with no image: the app reads this as "drop the
                    // entry for this texture". Removing it from `texture_imports` here would
                    // only change the display — the project store is the app's.
                    self.pending_texture_imports
                        .push(crate::mod_project::TextureImport {
                            texture_name: name.clone(),
                            png_path: String::new(),
                            raw: false,
                        });
                    touch(&mut self.eff_dirty_at, &mut self.edit_rev);
                }
            }
        });

        self.draw_texture_pool_ops(ui, index, texture);

        if let Some(existing) = &replaced {
            ui.label(
                egui::RichText::new(format!("replaced by {}", existing.png_path))
                    .small()
                    .color(egui::Color32::from_rgb(140, 200, 255)),
            );
        }
        if let Some((note, ok)) = &self.texture_note {
            ui.label(egui::RichText::new(note).small().color(if *ok {
                egui::Color32::from_rgb(0x70, 0xB0, 0x70)
            } else {
                egui::Color32::from_rgb(0xD0, 0x80, 0x60)
            }));
        }
    }

    /// The In game fold's one line and its hover: whether the game has played the open effect,
    /// and how many of this fighter's effects it has played since connecting.
    fn game_panel_summary(&self, link: &GameLink) -> (String, String) {
        let seen = link.kinds().len();
        let entry = self.selected_entry.and_then(|i| self.entries.get(i));
        let line = match entry {
            Some(entry) => {
                let name = crate::app::effect_words(&entry.name);
                match game_sighting(seen, link.kind(entry.hash).is_some()) {
                    Some(true) => format!("In game: {name} played"),
                    Some(false) => format!("In game: {name} not played yet"),
                    None => "In game".to_string(),
                }
            }
            None => "In game".to_string(),
        };
        (
            line,
            format!(
                "{seen} of this fighter's effects played since connecting. Send edits, then do \
                 the move again in game to see them there."
            ),
        )
    }

    fn draw_game_panel(&mut self, ui: &mut Ui, link: &GameLink) {
        // About the running game: what it has spawned and what has been sent to it. Without a
        // connection none of it says anything.
        if !link.status().is_connected() {
            return;
        }

        let Some(entry_idx) = self.selected_entry else {
            ui.colored_label(egui::Color32::GRAY, "Select an entry.");
            return;
        };
        let (hash, set_idx, entry_name) = {
            let e = &self.entries[entry_idx];
            (e.hash, e.set_idx, e.name.clone())
        };
        let transplant = self.current_transplant_for_entry(&entry_name);

        let kind = link.kind(hash);
        let sighting = game_sighting(link.kinds().len(), kind.is_some());
        ui.horizontal(|ui| {
            ui.label(crate::app::effect_words(&entry_name))
                .on_hover_text(&entry_name);
            if transplant.is_some() {
                ui.label(egui::RichText::new("Borrowed").small().color(BORROWED_TINT));
            }
            // Nothing is said before the game has reported any effect: "not played" then
            // means only that nothing has come back yet.
            match sighting {
                Some(true) => {
                    ui.colored_label(egui::Color32::from_rgb(90, 220, 90), "played");
                }
                Some(false) => {
                    ui.colored_label(egui::Color32::GRAY, "not played yet");
                }
                None => {}
            }
        });
        if let Some((fighter, item)) = transplant {
            let scope = if item.one_slot_slots.len() == 1 {
                format!("skin c{:02}", item.one_slot_slots[0])
            } else if item.one_slot_slots.is_empty() {
                "all skins".to_string()
            } else {
                format!(
                    "skins {}",
                    item.one_slot_slots
                        .iter()
                        .map(|slot| format!("c{slot:02}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            ui.label(
                egui::RichText::new(format!(
                    "From {} ({}) · {scope}",
                    item.donor_name, item.donor_file
                ))
                .small()
                .color(egui::Color32::from_rgb(190, 140, 255)),
            );
            if ui
                .button("Remove")
                .on_hover_text("Take this borrowed effect out of the project and the game")
                .clicked()
            {
                self.pending_transplant_removals.push(EffTransplantRemoval {
                    fighter,
                    op_index: item.op_index,
                    entry_name: item.entry_name,
                    donor_name: item.donor_name,
                });
            }
            ui.separator();
        }

        // Authored edits → live game. These are baked into the CARRIER's copy of the effect,
        // so each emitter gets exactly its own edited values — which is why the kind-level
        // colour multiplier that used to sit under this panel is gone: it was whole-effect by
        // construction, tinting emitters you never touched, and could not carry a per-key or
        // color1 edit at all.
        let edited = self.edited_emitters(set_idx);
        let total = self
            .ptcl
            .as_ref()
            .and_then(|p| p.emitter_sets.get(set_idx))
            .map(|s| s.emitters.len())
            .unwrap_or(0);
        let edited_names = self
            .ptcl
            .as_ref()
            .and_then(|p| p.emitter_sets.get(set_idx))
            .map(|s| {
                edited
                    .iter()
                    .map(|i| match s.emitters.get(*i) {
                        Some(e) if !e.name.is_empty() => format!("{i}:{}", e.name),
                        _ => format!("{i}"),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        ui.add_space(4.0);
        if edited.is_empty() {
            ui.label(
                egui::RichText::new("No changes")
                    .small()
                    .color(egui::Color32::DARK_GRAY),
            );
        } else {
            ui.label(
                egui::RichText::new(format!("{} of {total} parts changed", edited.len())).small(),
            )
            .on_hover_text(format!(
                "{edited_names}\nSend edits, then trigger the move again."
            ));
        }
        // Texture replacements are file-wide, not per-entry, so they are listed here in full
        // rather than filtered to the selected effect — a replaced texture changes every
        // emitter that samples it, including ones in entries you are not looking at.
        if !self.texture_imports.is_empty() {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!(
                    "{} replaced texture(s) in the whole file",
                    self.texture_imports.len()
                ))
                .small()
                .color(egui::Color32::from_rgb(140, 200, 255)),
            );
            for import in &self.texture_imports {
                ui.label(
                    egui::RichText::new(format!("  {}", import.texture_name))
                        .small()
                        .monospace()
                        .color(egui::Color32::GRAY),
                );
            }
        }
        // There is exactly ONE send control, and it lives in the header where it is reachable
        // from every panel. A second copy used to sit here with a plain "waiting for game…"
        // spinner, which reported none of the phases the header's does — so whichever one you
        // happened to be looking at decided how much you were told about a stalled send.
        //
        // Feedback for BOTH senders (the header's rebuild and the kind color×/speed "Send now"
        // below) lives here, above the live-kind guard: a rebuild works whether or not the
        // effect has been seen in game yet, so its result must stay visible either way.
        if let Some(note) = &self.last_sent_note {
            ui.label(
                egui::RichText::new(note)
                    .small()
                    .color(egui::Color32::LIGHT_GRAY),
            );
        }

        let Some(kind) = kind else {
            ui.separator();
            self.draw_transplant_section(ui, &entry_name);
            return;
        };

        ui.separator();
        ui.collapsing("Live values", |ui| {
            let d = &kind.data;
            ui.label(
                egui::RichText::new(format!(
                    "scale {:.3} · speed {:.2} · visible {} · updates {}",
                    d.scale, d.speed, d.visible, kind.updates
                ))
                .small()
                .monospace(),
            );
            ui.label(
                egui::RichText::new(format!(
                    "pos [{:.2} {:.2} {:.2}] · rot X {:.2} · Y {:.2} · Z {:.2} · bone {}",
                    d.pos.x, d.pos.y, d.pos.z, d.rot.x, d.rot.y, d.rot.z, d.bone_name
                ))
                .small()
                .monospace(),
            );
            ui.label(
                egui::RichText::new(format!(
                    "spawn baseline: scale {:.3} · pos [{:.2} {:.2} {:.2}]",
                    kind.first.scale, kind.first.pos.x, kind.first.pos.y, kind.first.pos.z
                ))
                .small()
                .color(egui::Color32::GRAY),
            );
        });
        // The kind-level "color × / speed ×" multipliers used to sit here. They predate the
        // carrier: they were once the only way to change how an effect looked at runtime, and
        // they are whole-effect by construction — one multiplier tints every emitter of every
        // spawn and cannot express a per-key or color1 edit at all. Authored edits now do that
        // exactly, so offering a second, cruder way to recolour the same effect only invited
        // editing it two ways and wondering which won.
        //
        // The multipliers are not gone from the toolkit: `LiveOverrides` still restores them
        // from a project's saved tweaks and can still import or clear the ones already pinned
        // in the running game. What has gone is the form that let you type a new one HERE.

        ui.separator();
        self.draw_transplant_section(ui, &entry_name);
    }

    fn draw_transplant_section(&mut self, ui: &mut Ui, entry_name: &str) {
        if ui
            .button("Borrow effects")
            .on_hover_text(format!(
                "Copy {entry_name} or another effect into a fighter"
            ))
            .clicked()
        {
            self.transplant_requested = true;
        }
    }
}

/// Whether the game has played an effect, once it has reported any: `None` before the first
/// report, when "not played" would only mean nothing has come back yet.
pub(crate) fn game_sighting(reported: usize, this_one: bool) -> Option<bool> {
    (reported > 0 || this_one).then_some(this_one)
}

/// What a send is waiting on, in words, from the game's carrier report.
pub(crate) fn send_phase(state: u8, spawned: bool, took_ours: bool, secs: u64) -> &'static str {
    match (state, spawned) {
        _ if secs < 2 => "Sending to the game",
        (2, true) if took_ours => "In the game",
        (2, true) | (3, _) | (4, _) | (5, _) => "Waiting for the game to let go of the last edit",
        (2, false) => "Starting in the game",
        _ => "Sending to the game",
    }
}

/// The carrier report behind a phase or an outcome, for its hover.
pub(crate) fn carrier_numbers(state: u8, kinds: usize, spawned: bool) -> String {
    format!(
        "Game report: state {state}, {kinds} effect names, object {}",
        if spawned { "up" } else { "down" }
    )
}

/// How a send ended, in the words the Send row, the In game panel and the status line share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SendOutcome {
    /// The game took the edit and it plays now.
    InGame,
    /// The game dropped the last edit and plays its own effects.
    Restored,
    /// The game could not read the file it was pointed at.
    Unreadable,
    /// No match is reporting; the staged files load when one starts.
    NoMatch,
    /// The match did not take it; the staged files load at the next match.
    NextMatch,
}

impl SendOutcome {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::InGame => "In the game",
            Self::Restored => "The game plays its own effects again",
            Self::Unreadable => "The game could not read the edit",
            Self::NoMatch => "Saved for the next match",
            Self::NextMatch => "Not in this match: it loads at the next match",
        }
    }

    /// The status line's opening words, before the count of what was sent.
    pub(crate) fn status_prefix(self) -> &'static str {
        match self {
            Self::InGame | Self::Restored => "In the game",
            Self::Unreadable => "Not sent",
            Self::NoMatch | Self::NextMatch => "Saved for the next match",
        }
    }

    pub(crate) fn ok(self) -> bool {
        matches!(self, Self::InGame | Self::Restored)
    }
}

/// Plain-English name for what a texture carries, for the panel and the import note.
fn layout_note(layout: crate::texture_import::Layout) -> &'static str {
    use crate::texture_import::Layout;
    match layout {
        Layout::Mask => "a black-and-white mask: black is empty, white is solid",
        Layout::Matte { .. } => "a shape on a flat colour: black is empty, white is solid",
        Layout::Opaque => "a fully opaque image: every pixel draws",
        Layout::ColorAlpha => "colour plus its own transparency: keep the alpha channel",
    }
}

/// What the panel is currently showing, so the form in use is never a guess.
fn describe_form(
    pool: &[u8],
    index: usize,
    name: &str,
    form: crate::texture_import::Form,
) -> String {
    if form == crate::texture_import::Form::Raw {
        return "raw stored channels".to_string();
    }
    match crate::texture_import::layout_of_public(pool, index, name) {
        Ok(layout) => layout_note(layout).to_string(),
        Err(_) => "editable form".to_string(),
    }
}

/// An edit happened: there is something unsent, and the preview is out of date. A free
/// function over the two fields so it can run while an emitter is mutably borrowed.
fn touch(dirty_at: &mut Option<Instant>, edit_rev: &mut u64) {
    *dirty_at = Some(Instant::now());
    *edit_rev += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{EmitterSet, PtclFile};
    #[allow(unused_imports)]
    use crate::ui::TestFrame;

    /// The send row names the phase in words. The game's numbers are kept for the hover.
    #[test]
    fn a_send_names_its_phase_in_plain_words() {
        assert_eq!(send_phase(4, false, false, 1), "Sending to the game");
        assert_eq!(send_phase(2, true, true, 9), "In the game");
        assert_eq!(
            send_phase(2, true, false, 9),
            "Waiting for the game to let go of the last edit"
        );
        assert_eq!(
            send_phase(4, false, false, 9),
            "Waiting for the game to let go of the last edit"
        );
        assert_eq!(send_phase(2, false, true, 9), "Starting in the game");
        assert_eq!(send_phase(1, false, false, 9), "Sending to the game");
        assert!(carrier_numbers(4, 5, false).contains("state 4"));
        let outcomes = [
            SendOutcome::InGame,
            SendOutcome::Restored,
            SendOutcome::Unreadable,
            SendOutcome::NoMatch,
            SendOutcome::NextMatch,
        ];
        for phase in [
            send_phase(0, false, false, 0),
            send_phase(2, true, true, 9),
            send_phase(2, true, false, 9),
            send_phase(2, false, true, 9),
        ]
        .into_iter()
        .chain(outcomes.iter().map(|o| o.text()))
        {
            for word in ["carrier", "state=", "kinds", ".nro", "gen=", "object="] {
                assert!(!phase.contains(word), "{phase:?} says {word}");
            }
        }
        assert!(SendOutcome::InGame.ok() && SendOutcome::Restored.ok());
        assert!(!SendOutcome::NextMatch.ok() && !SendOutcome::NoMatch.ok());
    }

    /// "Not played yet" waits for the game's first report: before it, nothing has come back,
    /// which says nothing about the effect.
    #[test]
    fn an_effect_is_called_unplayed_only_after_the_game_reports() {
        assert_eq!(game_sighting(0, false), None);
        assert_eq!(game_sighting(5, false), Some(false));
        assert_eq!(game_sighting(5, true), Some(true));
        assert_eq!(game_sighting(0, true), Some(true));
    }

    /// A send's result describes one connection. Once the link is down it is cleared, and the
    /// wait for the game stops: the next connection sends everything again.
    #[test]
    fn a_dropped_link_clears_the_last_send_result() {
        let link = GameLink::default();
        assert!(!link.status().is_connected());
        let mut editor = EffEditor {
            last_carrier_result: Some(SendOutcome::NextMatch.text().to_string()),
            last_carrier_detail: Some("Game report: state 4".to_string()),
            awaiting_game: Some(Instant::now()),
            ..EffEditor::default()
        };
        editor.set_sent_note(SendOutcome::NextMatch.text());
        editor.update(&egui::Context::default(), &link);
        assert_eq!(editor.last_carrier_result, None);
        assert_eq!(editor.last_carrier_detail, None);
        assert_eq!(editor.awaiting_game, None);
        assert_eq!(editor.last_sent_note, None);
    }

    #[test]
    fn texture_slots_with_no_texture_fold_and_used_ones_stay() {
        use crate::eff_attrs::{G_SAMPLER1, G_SAMPLER2, G_TEXANIM1, G_TEXUV2};
        let mut em = EmitterDef {
            name: "em0".into(),
            attrs: crate::eff_attrs::read_all(&test_emitter_data()),
            subsections: Vec::new(),
            depth: 0,
            source_idx: 0,
            color0: Vec::new(),
            color1: Vec::new(),
            alpha0_keys: Vec::new(),
            texture_index: None,
        };
        // Slot 2 samples nothing (the all-ones id), slot 3 samples a texture.
        set_attr(&mut em, "sampler1.texture_id", AttrValue::UInt(u64::MAX));
        set_attr(
            &mut em,
            "sampler2.texture_id",
            AttrValue::UInt(0x1234_5678_9ABC),
        );
        let unused = unused_texture_groups(&em);
        assert!(unused.contains(&G_SAMPLER1) && unused.contains(&G_TEXANIM1));
        assert!(!unused.contains(&G_SAMPLER2) && !unused.contains(&G_TEXUV2));
        assert_eq!(unused.len(), 4);
        // And the other way round.
        set_attr(&mut em, "sampler1.texture_id", AttrValue::UInt(7));
        set_attr(&mut em, "sampler2.texture_id", AttrValue::UInt(u64::MAX));
        let unused = unused_texture_groups(&em);
        assert!(!unused.contains(&G_SAMPLER1) && unused.contains(&G_TEXUV2));
    }

    /// In the game's files a texture slot with no texture holds the all-ones id, which is what
    /// the fold above keys on: both kinds occur, and nothing else stands for "none".
    #[test]
    fn game_files_mark_empty_texture_slots_with_the_all_ones_id() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "eff")
                    && !path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('_'))
                {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&root.join("effect"), &mut files);
        assert!(files.len() > 300, "only {} corpus files", files.len());
        let id_at = crate::eff_attrs::index_of("sampler1.texture_id").unwrap();
        let (mut folded, mut kept, mut zero) = (0usize, 0usize, 0usize);
        for path in &files {
            let Ok(effect) = crate::effects::load_effect(path) else {
                continue;
            };
            for em in effect.ptcl.emitter_sets.iter().flat_map(|s| &s.emitters) {
                let Some(id) = em.attrs[id_at].map(|v| v.as_u64()) else {
                    continue;
                };
                if id == 0 {
                    zero += 1;
                }
                if unused_texture_groups(em).contains(&crate::eff_attrs::G_SAMPLER1) {
                    folded += 1;
                    assert_eq!(id, u64::MAX);
                } else {
                    kept += 1;
                }
            }
        }
        eprintln!("slot 2: {folded} folded, {kept} kept");
        assert!(folded > 1000 && kept > 1000, "{folded} folded, {kept} kept");
        assert_eq!(zero, 0, "a zero id would be a second way to say none");
    }

    /// Every blend and shape value in the game's files has a name, and the counts of the values
    /// left as numbers are printed for the record.
    #[test]
    fn game_files_use_only_named_blends_and_shapes() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "eff")
                    && !path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('_'))
                {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&root.join("effect"), &mut files);
        assert!(files.len() > 300, "only {} corpus files", files.len());
        let table = crate::eff_attrs::table();
        let named: Vec<usize> = ["render_state.blend_type", "shape_info.volume_type"]
            .iter()
            .map(|id| crate::eff_attrs::index_of(id).unwrap())
            .collect();
        let counted = [
            "emitter_info.follow_type",
            "particle_data.billboard_type",
            "sampler0.filter",
        ]
        .map(crate::eff_attrs::index_of);
        let mut seen: std::collections::BTreeMap<(&str, i64), usize> = Default::default();
        let mut emitters = 0usize;
        for path in &files {
            let Ok(effect) = crate::effects::load_effect(path) else {
                continue;
            };
            for em in effect.ptcl.emitter_sets.iter().flat_map(|s| &s.emitters) {
                emitters += 1;
                for &at in named.iter().chain(counted.iter().flatten()) {
                    if let Some(value) = em.attrs[at] {
                        *seen.entry((table[at].id, value.as_i64())).or_default() += 1;
                    }
                }
            }
        }
        eprintln!("{emitters} emitters: {seen:?}");
        assert!(emitters > 10_000);
        // Every blend is named. Of the shapes, only 15 is not: eff_wgpu places none of its
        // particles by a shape of its own, so it reads "Other (15)".
        for &at in &named {
            let AttrKind::Enum(names) = table[at].kind else {
                panic!("{} is not a named choice", table[at].id);
            };
            let unnamed: Vec<i64> = seen
                .keys()
                .filter(|(id, value)| {
                    *id == table[at].id && !(0..names.len() as i64).contains(value)
                })
                .map(|(_, value)| *value)
                .collect();
            let expected: &[i64] = if table[at].id == "shape_info.volume_type" {
                &[15]
            } else {
                &[]
            };
            assert_eq!(unnamed, expected, "{}", table[at].id);
        }
    }

    /// Every group is filed in exactly one section, and the texture groups by their slot.
    #[test]
    fn every_settings_group_has_one_section() {
        for group in crate::eff_attrs::GROUPS {
            let homes = crate::eff_attrs::SECTIONS
                .iter()
                .filter(|(_, members)| members.contains(group))
                .count();
            assert_eq!(homes, 1, "{group}");
        }
        let textures = crate::eff_attrs::SECTIONS
            .iter()
            .find(|(name, _)| *name == "Textures")
            .unwrap();
        for group in textures.1 {
            assert!(
                crate::eff_attrs::texture_slot_of(group).is_some(),
                "{group}"
            );
        }
        assert_eq!(
            crate::eff_attrs::texture_slot_of(crate::eff_attrs::G_TEXUV2),
            Some(2)
        );
        assert_eq!(
            crate::eff_attrs::texture_slot_of(crate::eff_attrs::G_SHADER),
            None
        );
        assert_eq!(enum_label(crate::eff_attrs::BLEND_TYPES, 1), "Add");
        assert_eq!(enum_label(crate::eff_attrs::VOLUME_TYPES, 15), "Other (15)");
        assert_eq!(enum_label(crate::eff_attrs::FOLLOW_TYPES, -1), "Other (-1)");
    }

    #[test]
    fn the_game_split_leaves_the_settings_half_the_panel() {
        for height in [300.0, 500.0, 900.0] {
            let (min, default, max) = game_split_sizes(height);
            assert!(max <= height * 0.5 + 0.5 || max == 80.0, "{height}");
            assert!(min <= default && default <= max, "{height}");
        }
    }

    /// Texture 2 and 3 are found by id, and a slot whose id is "none" or names nothing in the
    /// file is not offered.
    #[test]
    fn texture_slots_are_read_by_id_and_picked_by_id() {
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.selected_entry = Some(0);
        let ptcl = editor.ptcl.as_mut().expect("ptcl");
        ptcl.bntx_textures = [(Some(11), "ef_a"), (Some(22), "ef_b"), (None, "ef_added")]
            .iter()
            .map(|(id, name)| TextureInfo {
                tex_name: (*name).into(),
                id: *id,
                width: 64,
                height: 64,
                layers: 1,
                format: "BC7Srgb".into(),
                convertible: true,
            })
            .collect();
        let em = &mut ptcl.emitter_sets[0].emitters[0];
        em.texture_index = Some(0);
        set_attr(em, TEXTURE_SLOT_IDS[1], AttrValue::UInt(22));
        set_attr(em, TEXTURE_SLOT_IDS[2], AttrValue::UInt(u64::MAX));
        let textures = ptcl.bntx_textures.clone();
        assert_eq!(
            texture_slots(&ptcl.emitter_sets[0].emitters[0], &textures),
            vec![(0, 0), (1, 1)]
        );
        // An id the file has no texture for is not a slot either.
        set_attr(
            &mut ptcl.emitter_sets[0].emitters[0],
            TEXTURE_SLOT_IDS[2],
            AttrValue::UInt(99),
        );
        assert_eq!(
            texture_slots(&ptcl.emitter_sets[0].emitters[0], &textures).len(),
            2
        );
        // Picking for texture 2 writes the picked texture's id and leaves texture 1 alone.
        editor.point_slot_at(1, 0);
        let em = &editor.ptcl.as_ref().unwrap().emitter_sets[0].emitters[0];
        assert_eq!(attr(em, TEXTURE_SLOT_IDS[1]).map(|v| v.as_u64()), Some(11));
        assert_eq!(em.texture_index, Some(0));
        // A texture with no id yet cannot be given to texture 2.
        editor.point_slot_at(1, 2);
        let em = &editor.ptcl.as_ref().unwrap().emitter_sets[0].emitters[0];
        assert_eq!(attr(em, TEXTURE_SLOT_IDS[1]).map(|v| v.as_u64()), Some(11));
        // Both parts that sample ef_a count as its users, through either slot.
        assert_eq!(editor.texture_users(0), 1);
        assert_eq!(editor.texture_users(1), 0);
    }

    #[test]
    fn part_names_read_as_words_and_keep_colliding_names() {
        assert_eq!(part_words("flare1"), "Flare 1");
        assert_eq!(part_words("line2_add"), "Line 2 add");
        assert_eq!(part_words("impact1_add"), "Impact 1 add");
        assert_eq!(part_words("FlareBase"), "Flare base");
        assert_eq!(part_words("FireImpact_b"), "Fire impact b");
        assert_eq!(part_words("FlareAdd2"), "Flare add 2");
        assert_eq!(part_words("SmokeBomb"), "Smoke bomb");
        assert_eq!(
            part_list_names(&["flare1", "", "wind_2", "wind2", "FireBase", "fire_base"]),
            vec![
                "Flare 1",
                "Part 2",
                "wind_2",
                "wind2",
                "FireBase",
                "fire_base"
            ]
        );
    }

    #[test]
    fn effect_list_reads_as_the_timeline_and_keeps_colliding_file_names() {
        let entry = |name: &str| EffEntry {
            name: name.into(),
            hash: 0,
            set_idx: 0,
        };
        let entries = [
            entry("mariod_smash_aura"),
            entry("mariod_smash_impact"),
            entry("mario_hit"),
            entry("mariod_hit"),
        ];
        let names = entry_list_names(&entries);
        // The same words the timeline row shows for the effect.
        assert_eq!(names[0], crate::app::effect_words("mariod_smash_aura"));
        assert_eq!(names[0], "Smash aura");
        assert_eq!(names[1], "Smash impact");
        // Two effects that would both read "Hit" keep their file names.
        assert_eq!(names[2], "mario_hit");
        assert_eq!(names[3], "mariod_hit");
    }

    #[test]
    fn search_ranks_the_exact_name_before_longer_ones() {
        let rank = |search, name| search_rank(search, &[name]);
        assert_eq!(rank("mario", "Mario"), Some(0));
        assert_eq!(rank("mario", "Dr. Mario"), Some(2));
        assert_eq!(rank(" Forward Smash ", "Forward smash"), Some(0));
        assert_eq!(rank("forward smash", "Forward smash angled up"), Some(1));
        assert_eq!(rank("zelda", "Mario"), None);
        // Either name matches: the shown one or the internal one.
        assert_eq!(search_rank("mariod", &["Dr. Mario", "mariod"]), Some(0));
        // Sorted by rank, Mario comes before Dr. Mario whatever the list order.
        let mut choices = vec![("Dr. Mario", 0u8), ("Mario", 0u8)];
        for choice in &mut choices {
            choice.1 = rank("mario", choice.0).unwrap();
        }
        choices.sort_by_key(|c| c.1);
        assert_eq!(choices[0].0, "Mario");
    }

    #[test]
    fn whole_number_rows_hold_64_bit_ids_and_none() {
        // The no-primitive value reads None, not 2^64, and types back exactly.
        assert_eq!(uint_text(u64::MAX), "None");
        assert_eq!(parse_uint_text("None"), Some(u64::MAX));
        // Clearing the box to type a new id does not set "none" on the way.
        assert_eq!(parse_uint_text(""), None);
        // A texture GUID above 2^53 (where a drag's f64 rounds) survives text both ways.
        let guid = 0x9E37_79B9_7F4A_7C15_u64;
        assert_eq!(parse_uint_text(&uint_text(guid)), Some(guid));
        assert_eq!(parse_uint_text("0x10"), Some(16));
        assert_eq!(parse_uint_text("12a"), None);
        let primitive = crate::eff_attrs::table()
            .iter()
            .find(|a| a.id == "particle_data.primitive_id")
            .unwrap();
        assert_eq!(attr_text(primitive, AttrValue::UInt(u64::MAX)), "None");
    }

    /// A blank emitter at the version Smash's own files use, for the attribute vector.
    fn test_emitter_data() -> effect_library::EmitterData {
        crate::eff_attrs::blank_emitter_data(crate::eff_attrs::SSBU_VFX_VERSION)
    }

    /// An editor holding one emitter set with one emitter, loaded from `path`.
    fn editor_with_one_emitter(path: &str) -> EffEditor {
        // Attributes come from a default emitter rather than a hand-written vector, so the
        // fixture stays valid as the table grows and every attribute starts out present.
        let mut emitter = EmitterDef {
            name: "em0".into(),
            attrs: crate::eff_attrs::read_all(&test_emitter_data()),
            subsections: Vec::new(),
            depth: 0,
            source_idx: 0,
            color0: Vec::new(),
            color1: Vec::new(),
            alpha0_keys: Vec::new(),
            texture_index: None,
        };
        // A blank emitter is all zeros; give the one value these tests tune a baseline of its
        // own so "put it back to 1.0" is genuinely a return to pristine.
        set_attr(&mut emitter, TUNED, AttrValue::Float(1.0));
        let emitter = emitter;
        EffEditor {
            pristine: vec![vec![EmitterSnapshot::of(&emitter)]],
            ptcl: Some(PtclFile {
                emitter_sets: vec![EmitterSet {
                    name: "P_KirbyDash".into(),
                    emitters: vec![emitter],
                }],
                bntx_textures: Vec::new(),
            }),
            entries: vec![EffEntry {
                name: "kirby_dash".into(),
                hash: 0,
                set_idx: 0,
            }],
            loaded_path: Some(PathBuf::from(path)),
            ..Default::default()
        }
    }

    fn tuned(editor: &EffEditor) -> f32 {
        attr_f32(
            &editor.ptcl.as_ref().expect("ptcl").emitter_sets[0].emitters[0],
            TUNED,
        )
    }

    fn text_position(shape: &egui::epaint::Shape, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.size() / 2.0)
            }
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().find_map(|shape| text_position(shape, label))
            }
            _ => None,
        }
    }

    /// Draw the selected effect's panel, then click the control labelled `label` in it.
    fn click_in_emitter_editor(editor: &mut EffEditor, label: &str) {
        let context = egui::Context::default();
        let frame = |editor: &mut EffEditor, events: Vec<egui::Event>| {
            context.run_test_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| editor.draw_emitter_editor(ui),
            )
        };
        frame(editor, Vec::new());
        let output = frame(editor, Vec::new());
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| text_position(&shape.shape, label))
            .unwrap_or_else(|| panic!("no {label:?} on the panel"));
        for pressed in [true, false] {
            frame(
                editor,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    /// The button is on both tabs; on Parts and timing the panel returned before the reset ran.
    #[test]
    fn reset_effect_works_on_both_tabs() {
        for tab in [EmitterTab::Spawning, EmitterTab::Attributes] {
            let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
            editor.selected_entry = Some(0);
            editor.emitter_tab = tab;
            tune(&mut editor, 2.5);
            click_in_emitter_editor(&mut editor, "Reset effect");
            assert_eq!(tuned(&editor), 1.0, "{tab:?}");
        }
    }

    /// Steps close once the edits pause with the pointer up; one drag is one step.
    fn history_frame(editor: &mut EffEditor, at: Instant, pointer_down: bool, edit: Option<f32>) {
        editor.history_begin_frame(pointer_down, at);
        if let Some(value) = edit {
            tune(editor, value);
            editor.touch();
            editor.history_note_edit(at);
        }
    }

    #[test]
    fn effect_edits_undo_and_redo_in_steps() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.selected_entry = Some(0);
        history_frame(&mut editor, ms(0), false, None);
        assert!(!editor.can_undo());
        // One drag through three values, held for two seconds: one step.
        history_frame(&mut editor, ms(10), true, Some(1.5));
        history_frame(&mut editor, ms(20), true, Some(2.0));
        history_frame(&mut editor, ms(2000), true, Some(2.5));
        history_frame(&mut editor, ms(4000), true, None);
        assert!(
            editor.history.undo.is_empty(),
            "a held drag is still one open step"
        );
        // A second edit after a pause.
        history_frame(&mut editor, ms(5000), false, None);
        history_frame(&mut editor, ms(6000), false, Some(3.0));
        editor.undo();
        assert_eq!(tuned(&editor), 2.5);
        editor.undo();
        assert_eq!(tuned(&editor), 1.0);
        assert!(!editor.can_undo());
        editor.redo();
        assert_eq!(tuned(&editor), 2.5);
        editor.redo();
        assert_eq!(tuned(&editor), 3.0);
        assert!(!editor.can_redo());
        // A new edit after an undo drops what could be redone.
        editor.undo();
        history_frame(&mut editor, ms(7000), false, Some(4.0));
        history_frame(&mut editor, ms(8000), false, None);
        assert!(!editor.can_redo());
        editor.undo();
        assert_eq!(tuned(&editor), 2.5);
    }

    /// Undo shows the part that was open when the edit was made: a Whole effect change on the
    /// second part, undone, moved the tab to the part open when the step before it closed.
    #[test]
    fn undo_keeps_the_part_the_edit_was_made_on() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut editor = editor_with_two_parts();
        editor.selected_entry = Some(0);
        history_frame(&mut editor, ms(0), false, None);
        editor.selected_emitter = 1;
        history_frame(&mut editor, ms(600), false, None);
        history_frame(&mut editor, ms(700), false, Some(2.5));
        history_frame(&mut editor, ms(2000), false, None);
        editor.selected_emitter = 0;
        editor.undo();
        assert_eq!(editor.selected_emitter, 1);
        assert_eq!(tuned(&editor), 1.0);
    }

    /// Resets need no confirmation because they undo: both buttons, on either tab.
    #[test]
    fn resets_are_undone_like_any_edit() {
        for label in ["Reset effect", "Reset this part"] {
            let t0 = Instant::now();
            let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
            editor.selected_entry = Some(0);
            history_frame(&mut editor, t0, false, None);
            // Edited just before the reset, inside one step's gap: the reset is still its own.
            history_frame(&mut editor, t0, false, Some(2.5));
            let rev = editor.edit_rev;
            editor.history_begin_frame(false, t0);
            click_in_emitter_editor(&mut editor, label);
            assert!(editor.edit_rev != rev);
            editor.history_note_edit(t0);
            assert_eq!(tuned(&editor), 1.0, "{label}");
            editor.undo();
            assert_eq!(tuned(&editor), 2.5, "{label}");
            editor.undo();
            assert_eq!(tuned(&editor), 1.0, "{label}");
        }
    }

    /// Undo shows the effect the edit was made in, and a load forgets the old file's steps.
    #[test]
    fn undo_opens_the_edited_effect_and_a_load_clears_it() {
        let t0 = Instant::now();
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.entries.push(EffEntry {
            name: "kirby_other".into(),
            hash: 1,
            set_idx: 0,
        });
        editor.selected_entry = Some(0);
        history_frame(&mut editor, t0, false, None);
        history_frame(&mut editor, t0, false, Some(2.5));
        editor.selected_entry = Some(1);
        history_frame(&mut editor, t0, false, None);
        editor.undo();
        assert_eq!(editor.selected_entry, Some(0));
        assert_eq!(tuned(&editor), 1.0);
        editor.load_eff(Path::new("/no/such/file.eff"));
        assert!(!editor.can_undo() && !editor.can_redo());
    }

    /// One effect of two parts, with one extra part starting at frame 3.
    fn editor_with_two_parts() -> EffEditor {
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        let mut second = editor.ptcl.as_ref().unwrap().emitter_sets[0].emitters[0].clone();
        second.name = "em1".into();
        second.source_idx = 1;
        editor.pristine[0].push(EmitterSnapshot::of(&second));
        editor.ptcl.as_mut().unwrap().emitter_sets[0]
            .emitters
            .push(second);
        let info = EffEntryInfo {
            name: "kirby_dash".into(),
            set_idx: Some(0),
            variants: vec![EffVariantInfo {
                start_frame: 3,
                set_idx: Some(0),
                bone: String::new(),
            }],
            model: None,
        };
        editor.entry_info = vec![info.clone()];
        editor.entry_pristine = vec![info];
        editor.selected_entry = Some(0);
        editor
    }

    /// Copy a part, move the second above the first, and start the extra part later.
    fn change_structure(editor: &mut EffEditor) {
        let set = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0];
        let mut copy = set.emitters[0].clone();
        copy.name = "em0 copy".into();
        set.emitters.push(copy);
        set.emitters.swap(0, 1);
        editor.entry_info[0].variants[0].start_frame = 9;
    }

    /// Reset effect is the game's effect again: copies gone, the file's order, its timing, and
    /// no roster left to save. Before it, the copy and the move are marked as edits.
    #[test]
    fn reset_effect_puts_back_the_part_list_and_timing() {
        let mut editor = editor_with_two_parts();
        assert_eq!(editor.edited_entries(), vec![false]);
        assert!(!editor.effect_edited(0, "kirby_dash"));
        change_structure(&mut editor);
        // No value of any part changed: only the list and the timing did.
        assert!(editor.edited_emitters(0).is_empty());
        assert_eq!(editor.edited_entries(), vec![true]);
        assert!(editor.effect_edited(0, "kirby_dash"));
        assert_eq!(editor.moved_parts(0), vec![true, false, true]);
        assert_eq!(editor.collect_rosters().len(), 1);
        assert_eq!(editor.collect_entry_edits().len(), 1);

        click_in_emitter_editor(&mut editor, "Reset effect");
        let names: Vec<&str> = editor.ptcl.as_ref().unwrap().emitter_sets[0]
            .emitters
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, ["em0", "em1"]);
        assert!(editor.collect_rosters().is_empty());
        assert!(editor.collect_entry_edits().is_empty());
        assert_eq!(editor.edited_entries(), vec![false]);
        assert_eq!(editor.moved_parts(0), vec![false, false]);

        // Timing alone is an edit too.
        editor.entry_info[0].variants[0].start_frame = 4;
        assert_eq!(editor.edited_entries(), vec![true]);
    }

    /// A reset of an effect or a part that is as the game has it is greyed out and makes no
    /// step; the same buttons work once there is an edit (the paired case).
    #[test]
    fn resets_of_unedited_effects_do_nothing() {
        for label in ["Reset effect", "Reset this part"] {
            let t0 = Instant::now();
            let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
            editor.selected_entry = Some(0);
            history_frame(&mut editor, t0, false, None);
            let rev = editor.edit_rev;
            click_in_emitter_editor(&mut editor, label);
            assert_eq!(editor.edit_rev, rev, "{label} on an unedited part");
            assert!(!editor.can_undo(), "{label}");
            tune(&mut editor, 2.5);
            click_in_emitter_editor(&mut editor, label);
            assert_eq!(tuned(&editor), 1.0, "{label} on an edited part");
        }
    }

    /// An edit put back by hand before its step closed is no step.
    #[test]
    fn an_edit_that_changes_nothing_is_no_step() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.selected_entry = Some(0);
        history_frame(&mut editor, ms(0), false, None);
        history_frame(&mut editor, ms(10), false, Some(2.0));
        history_frame(&mut editor, ms(20), false, Some(1.0));
        history_frame(&mut editor, ms(2000), false, None);
        assert!(editor.history.undo.is_empty());
        // The same edit kept is one.
        history_frame(&mut editor, ms(2010), false, Some(2.0));
        history_frame(&mut editor, ms(4000), false, None);
        assert_eq!(editor.history.undo.len(), 1);
    }

    /// Undo says what it will undo and where; Redo says what it will redo.
    #[test]
    fn undo_and_redo_name_their_step() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.selected_entry = Some(0);
        history_frame(&mut editor, ms(0), false, None);
        history_frame(&mut editor, ms(10), false, Some(2.0));
        let field = crate::eff_attrs::table()
            .iter()
            .find(|a| a.id == TUNED)
            .unwrap()
            .label;
        let effect = entry_list_names(&editor.entries)[0].clone();
        let part = part_list_names(&["em0"])[0].clone();
        let want = format!("{field} on {part} in {effect}");
        // While the step is open, and once it has closed.
        assert_eq!(editor.undo_label().as_deref(), Some(want.as_str()));
        history_frame(&mut editor, ms(2000), false, None);
        assert_eq!(editor.undo_label().as_deref(), Some(want.as_str()));
        editor.undo();
        assert_eq!(editor.redo_label().as_deref(), Some(want.as_str()));
        // A reset is named by its button.
        history_frame(&mut editor, ms(3000), false, Some(2.0));
        history_frame(&mut editor, ms(5000), false, None);
        click_in_emitter_editor(&mut editor, "Reset effect");
        editor.history_note_edit(ms(5000));
        assert_eq!(
            editor.undo_label(),
            Some(format!("Reset effect in {effect}"))
        );
    }

    fn em(editor: &mut EffEditor, part: usize) -> &mut EmitterDef {
        &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[part]
    }

    fn draw_once(editor: &mut EffEditor) {
        let context = egui::Context::default();
        let _ = context.run_test_ui(Default::default(), |ui| editor.draw_emitter_editor(ui));
    }

    fn key(r: f32, g: f32, b: f32) -> ColorKey {
        ColorKey {
            frame: 0.0,
            r,
            g,
            b,
            a: 1.0,
        }
    }

    /// A colour picked with the swatch is named by its key, not by the attribute row the key
    /// is mirrored into ("Second colour key 1 X"), and every step names its part.
    #[test]
    fn steps_are_named_after_the_row_and_part_touched() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut editor = editor_with_two_parts();
        em(&mut editor, 1).color1 = vec![key(1.0, 0.0, 0.0), key(1.0, 0.5, 0.0)];
        history_frame(&mut editor, ms(0), false, None);
        em(&mut editor, 1).color1[1].g = 0.2;
        effective_keys_to_attrs(em(&mut editor, 1));
        editor.touch();
        editor.history_note_edit(ms(10));
        history_frame(&mut editor, ms(2000), false, None);
        let effect = entry_list_names(&editor.entries)[0].clone();
        let part = part_list_names(&["em0", "em1"])[1].clone();
        assert_eq!(
            editor.undo_label(),
            Some(format!("Second colour key 2 on {part} in {effect}"))
        );
        // A Look row that drives two speeds is named after the row.
        set_attr(em(&mut editor, 1), SPEED_ATTRS[0], AttrValue::Float(3.0));
        editor.history.touched = Some("Speed".into());
        editor.touch();
        editor.history_note_edit(ms(2010));
        history_frame(&mut editor, ms(4000), false, None);
        assert_eq!(
            editor.undo_label(),
            Some(format!("Speed on {part} in {effect}"))
        );
    }

    /// The history list goes back, and forward again, several steps in one go.
    #[test]
    fn the_history_list_moves_several_steps_at_once() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.selected_entry = Some(0);
        history_frame(&mut editor, ms(0), false, None);
        for (i, value) in [2.0, 3.0, 4.0].into_iter().enumerate() {
            let at = 1000 * (i as u64 + 1);
            history_frame(&mut editor, ms(at), false, Some(value));
            history_frame(&mut editor, ms(at + 900), false, None);
        }
        let (undo, redo) = editor.history_steps();
        assert_eq!((undo.len(), redo.len()), (3, 0));
        editor.undo_steps(2);
        assert_eq!(tuned(&editor), 2.0);
        let (undo, redo) = editor.history_steps();
        assert_eq!((undo.len(), redo.len()), (1, 2));
        editor.redo_steps(2);
        assert_eq!(tuned(&editor), 4.0);
        // An open step is the first in the list, and going back past it closes it first.
        history_frame(&mut editor, ms(9000), false, Some(5.0));
        assert_eq!(editor.history_steps().0.len(), 4);
        editor.undo_steps(2);
        assert_eq!(tuned(&editor), 3.0);
    }

    #[test]
    fn a_hue_turn_keeps_brightness_and_works_over_one() {
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4);
        assert!(close(rotate_hue([1.0, 0.0, 0.0], 120.0), [0.0, 1.0, 0.0]));
        assert!(close(rotate_hue([2.0, 0.5, 0.5], 240.0), [0.5, 0.5, 2.0]));
        assert!(close(rotate_hue([1.0, 0.5, 0.0], 360.0), [1.0, 0.5, 0.0]));
        assert!(close(rotate_hue([1.0, 0.5, 0.0], -30.0), [1.0, 0.0, 0.0]));
        // Greys have no hue to turn.
        assert_eq!(rotate_hue([0.4, 0.4, 0.4], 90.0), [0.4, 0.4, 0.4]);
    }

    /// The whole-effect controls change every part, always from where they started: Size × at 2
    /// then 1.5 is 1.5 times each part's size, not 3. A drag is one undo step, an undo puts the
    /// controls back at neutral, and an edit made on a part starts them from neutral again.
    #[test]
    fn whole_effect_controls_change_every_part_in_one_step() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut editor = editor_with_two_parts();
        for (part, size, life) in [(0, 1.0, 40), (1, 2.0, FOREVER_LIFE)] {
            let e = em(&mut editor, part);
            for id in PARTICLE_SCALE_ATTRS {
                set_attr(e, id, AttrValue::Float(size));
            }
            set_attr(e, "particle_data.life", AttrValue::Int(life));
            set_attr(e, SPEED_ATTRS[0], AttrValue::Float(0.5));
            e.color0 = vec![key(1.0, 0.0, 0.0)];
        }
        // These are the game's values here.
        editor.pristine[0] = editor.ptcl.as_ref().unwrap().emitter_sets[0]
            .emitters
            .iter()
            .map(EmitterSnapshot::of)
            .collect();
        history_frame(&mut editor, ms(0), false, None);
        let factors = |size: f32| WholeFactors {
            size,
            life: 1.5,
            hue: 120.0,
            speed: 2.0,
        };
        for (at, size) in [(10, 2.0), (20, 1.5)] {
            editor.history_begin_frame(true, ms(at));
            editor.set_whole_effect(0, factors(size));
            editor.history_note_edit(ms(at));
        }
        history_frame(&mut editor, ms(2000), false, None);
        assert_eq!(attr_f32(em(&mut editor, 0), PARTICLE_SCALE_ATTRS[2]), 1.5);
        assert_eq!(attr_f32(em(&mut editor, 1), PARTICLE_SCALE_ATTRS[0]), 3.0);
        assert_eq!(
            attr(em(&mut editor, 0), "particle_data.life"),
            Some(AttrValue::Int(60))
        );
        // A life that never ends stays so.
        assert_eq!(
            attr(em(&mut editor, 1), "particle_data.life"),
            Some(AttrValue::Int(FOREVER_LIFE))
        );
        assert_eq!(attr_f32(em(&mut editor, 1), SPEED_ATTRS[0]), 1.0);
        let green = &em(&mut editor, 1).color0[0];
        assert!(green.g > 0.99 && green.r < 1e-4);
        assert_eq!(editor.history.undo.len(), 1);
        // The send and the project see an ordinary per-part edit of each.
        assert_eq!(editor.edited_emitters(0), vec![0, 1]);
        assert_eq!(editor.whole_effect_for(0).unwrap().factors, factors(1.5));

        editor.undo();
        assert_eq!(attr_f32(em(&mut editor, 1), PARTICLE_SCALE_ATTRS[0]), 2.0);
        assert!(editor.edited_emitters(0).is_empty());
        assert_eq!(
            editor.whole_effect_for(0).unwrap().factors,
            WholeFactors::default()
        );
        // An edit on a part's own row starts the controls from neutral on what is there.
        editor.redo();
        set_attr(
            em(&mut editor, 0),
            PARTICLE_SCALE_ATTRS[0],
            AttrValue::Float(9.0),
        );
        assert_eq!(
            editor.whole_effect_for(0).unwrap().factors,
            WholeFactors::default()
        );
        editor.set_whole_effect(
            0,
            WholeFactors {
                size: 2.0,
                ..Default::default()
            },
        );
        assert_eq!(attr_f32(em(&mut editor, 0), PARTICLE_SCALE_ATTRS[0]), 18.0);
    }

    /// After a project reload the controls read what was done, from the game's values: Hue 120
    /// and Size 1.5 read back as such, so a new turn starts from the game's colours instead of
    /// stacking on the old one.
    #[test]
    fn whole_effect_controls_read_back_after_a_reload() {
        let mut editor = editor_with_two_parts();
        for (part, size, life) in [(0, 1.0, 40), (1, 2.0, 17)] {
            let e = em(&mut editor, part);
            for id in PARTICLE_SCALE_ATTRS {
                set_attr(e, id, AttrValue::Float(size));
            }
            set_attr(e, "particle_data.life", AttrValue::Int(life));
            set_attr(e, SPEED_ATTRS[0], AttrValue::Float(0.5));
            e.color0 = vec![key(1.0, 0.2, 0.0), key(0.3, 0.3, 0.3)];
            e.color1 = vec![key(0.0, 0.4, 1.0)];
        }
        editor.pristine[0] = editor.ptcl.as_ref().unwrap().emitter_sets[0]
            .emitters
            .iter()
            .map(EmitterSnapshot::of)
            .collect();
        let done = WholeFactors {
            hue: 120.0,
            size: 1.5,
            life: 1.25,
            speed: 0.7,
        };
        editor.set_whole_effect(0, done);
        // A reload builds the controls again from what the parts hold.
        editor.whole_effect = None;
        assert_eq!(editor.whole_effect_for(0).unwrap().factors, done);
        // Hue 90 now is 90 from the game's colours, not 120 + 90.
        editor.set_whole_effect(0, WholeFactors { hue: 90.0, ..done });
        let red = em(&mut editor, 0).color0[0];
        assert!(
            (hue_of([red.r, red.g, red.b]).unwrap() - (hue_of([1.0, 0.2, 0.0]).unwrap() + 90.0))
                .abs()
                < 0.5
        );
        // Turned back by hand, it reads neutral.
        editor.set_whole_effect(0, WholeFactors::default());
        editor.whole_effect = None;
        assert_eq!(
            editor.whole_effect_for(0).unwrap().factors,
            WholeFactors::default()
        );
        // A part edited on its own rows: no one setting explains it, so neutral on what is there.
        editor.set_whole_effect(0, done);
        set_attr(
            em(&mut editor, 1),
            PARTICLE_SCALE_ATTRS[1],
            AttrValue::Float(9.0),
        );
        editor.whole_effect = None;
        assert_eq!(
            editor.whole_effect_for(0).unwrap().factors,
            WholeFactors::default()
        );
    }

    /// The Particle size, Speed, Spread and Position rows show they are edited and put back what
    /// the game had, like the attribute rows.
    #[test]
    fn the_look_rows_reset_what_they_drive() {
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.selected_entry = Some(0);
        tune(&mut editor, 2.5);
        click_in_emitter_editor(&mut editor, "was 1.00");
        // The reset is the row's own ↺, beside its "was".
        assert_eq!(tuned(&editor), 2.5, "the was label is not a button");
        click_in_emitter_editor(&mut editor, "↺");
        assert_eq!(tuned(&editor), 1.0);
        assert_eq!(editor.history.touched.as_deref(), Some("Particle size"));
        for id in POSITION_ATTRS {
            assert!(crate::eff_attrs::index_of(id).is_some(), "{id}");
        }
    }

    /// "Show only this part" is for the effect it was turned on in: an effect opened next shows
    /// all its parts.
    #[test]
    fn show_only_this_part_ends_with_its_effect() {
        let mut editor = editor_with_two_parts();
        editor.entries.push(EffEntry {
            name: "kirby_run".into(),
            hash: 1,
            set_idx: 0,
        });
        editor.solo_selected = Some("kirby_dash".into());
        editor.selected_emitter = 1;
        assert_eq!(editor.solo_emitter(), Some(1));
        editor.selected_entry = Some(1);
        assert_eq!(editor.solo_emitter(), None);
        draw_once(&mut editor);
        editor.selected_entry = Some(0);
        assert_eq!(editor.solo_emitter(), None);
    }

    /// Picking a move opens the effect it spawns first, not the first in the list; the move's
    /// shared effects are listed in the order they spawn, and not from the shared file itself.
    #[test]
    fn a_picked_move_opens_its_first_effect() {
        let mut editor = editor_with_one_emitter("/fx/ef_mario.eff");
        editor.entries = ["mario_superjump_fnish", "mario_superjump_power"]
            .iter()
            .enumerate()
            .map(|(i, name)| EffEntry {
                name: (*name).into(),
                hash: i as u64,
                set_idx: 0,
            })
            .collect();
        let uses = vec![MoveUse {
            label: "Up special".into(),
            motion: Some("special_hi".into()),
            bones: Vec::new(),
        }];
        editor.set_effect_uses([(0, uses.clone()), (1, uses)].into(), true);
        editor.set_open_move(Some("Up special".into()), Some("special_hi".into()));
        assert_eq!(editor.first_spawned_in_open_move(), Some(0));
        editor.set_open_move_effects(vec![
            ("sys_flame".into(), 12),
            ("mario_superjump_fnish".into(), 20),
            ("mario_superjump_power".into(), 4),
            ("sys_smash_flash".into(), 2),
        ]);
        assert_eq!(editor.first_spawned_in_open_move(), Some(1));
        assert_eq!(
            editor.open_move_shared_effects(),
            vec!["sys_smash_flash".to_owned(), "sys_flame".to_owned()]
        );
        editor.loaded_path = Some("/fx/effect/system/common/ef_common.eff".into());
        assert!(editor.loaded_is_shared());
        assert!(editor.open_move_shared_effects().is_empty());
    }

    /// An editor on Mario's super jump effects, both played by Up special, and a third no move
    /// spawns.
    fn editor_with_a_move() -> EffEditor {
        let mut editor = editor_with_one_emitter("/fx/ef_mario.eff");
        editor.entries = [
            "mario_superjump_fnish",
            "mario_superjump_power",
            "mario_unused_spark",
        ]
        .iter()
        .enumerate()
        .map(|(i, name)| EffEntry {
            name: (*name).into(),
            hash: i as u64,
            set_idx: 0,
        })
        .collect();
        let uses = vec![MoveUse {
            label: "Up special".into(),
            motion: Some("special_hi".into()),
            bones: Vec::new(),
        }];
        editor.set_effect_uses([(0, uses.clone()), (1, uses)].into(), true);
        editor
    }

    /// A file opening with a move open shows that move's first effect at once rather than an
    /// empty panel, and does it once: a later frame with nothing open is left alone. With no
    /// move open, nothing is opened for an unedited file.
    #[test]
    fn a_file_opens_on_its_moves_first_effect() {
        let mut editor = editor_with_a_move();
        // Nothing is decided before the moves are read: the open move may not be set yet.
        editor.effect_uses_ready = false;
        editor.open_first_effect = true;
        editor.open_first_effect_once();
        assert!(editor.open_first_effect && editor.show_in_picked_move.is_none());
        editor.effect_uses_ready = true;
        editor.set_open_move(Some("Up special".into()), Some("special_hi".into()));
        editor.set_open_move_effects(vec![
            ("mario_superjump_fnish".into(), 20),
            ("mario_superjump_power".into(), 4),
        ]);
        editor.open_first_effect = true;
        editor.open_first_effect_once();
        editor.settle_picked_move();
        assert_eq!(editor.selected_entry, Some(1));
        assert_eq!(editor.view, EffectView::InMove);

        editor.selected_entry = None;
        editor.open_first_effect_once();
        editor.settle_picked_move();
        assert_eq!(editor.selected_entry, None);

        let mut editor = editor_with_a_move();
        editor.open_first_effect = true;
        editor.open_first_effect_once();
        editor.settle_picked_move();
        assert_eq!(editor.selected_entry, None);
        assert!(!editor.open_first_effect);
    }

    /// Effects no move spawns are listed in a closed fold at the end, not greyed in place; a
    /// search opens it, and so does opening one of them.
    #[test]
    fn effects_no_move_spawns_are_listed_in_a_closed_fold() {
        let link = GameLink::default();
        let shows = |editor: &mut EffEditor, text: &str| {
            let context = egui::Context::default();
            let mut found = false;
            for _ in 0..2 {
                let output = context.run_test_ui(egui::RawInput::default(), |ui| {
                    editor.draw_entry_list(ui, &link)
                });
                found = output
                    .shapes
                    .iter()
                    .any(|shape| text_position(&shape.shape, text).is_some());
            }
            found
        };
        let mut editor = editor_with_a_move();
        let names = entry_list_names(&editor.entries);
        assert!(shows(&mut editor, &names[0]));
        assert!(shows(&mut editor, "Not used by any move (1)"));
        assert!(!shows(&mut editor, &names[2]), "closed to begin with");
        editor.entry_filter = "spark".into();
        assert!(shows(&mut editor, &names[2]), "a search opens it");
        assert!(!shows(&mut editor, &names[0]));

        let mut editor = editor_with_a_move();
        editor.queue_select("mario_unused_spark");
        editor.apply_pending_select();
        assert!(shows(&mut editor, &names[2]), "the open effect shows");
        // An edited effect no move spawns is never hidden: the fold opens for it, and only
        // for it.
        let mut editor = editor_with_a_move();
        let set = editor.ptcl.as_ref().unwrap().emitter_sets[0].clone();
        editor.ptcl.as_mut().unwrap().emitter_sets.push(set);
        editor.pristine.push(editor.pristine[0].clone());
        editor.entries[2].set_idx = 1;
        assert!(!shows(&mut editor, &names[2]));
        let mut editor = editor_with_a_move();
        let set = editor.ptcl.as_ref().unwrap().emitter_sets[0].clone();
        editor.ptcl.as_mut().unwrap().emitter_sets.push(set);
        editor.pristine.push(editor.pristine[0].clone());
        editor.entries[2].set_idx = 1;
        set_attr(
            &mut editor.ptcl.as_mut().unwrap().emitter_sets[1].emitters[0],
            TUNED,
            AttrValue::Float(2.5),
        );
        assert!(shows(&mut editor, &names[2]), "an edited effect shows");
    }

    /// The replacement PNG's date is read at most once a second, and not at all with no PNG:
    /// the Look tab read it from the disk on every frame.
    #[test]
    fn the_replacement_pngs_date_is_not_read_every_frame() {
        let dir = std::env::temp_dir().join(format!("visionary-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("tex.png");
        std::fs::write(&png, b"x").unwrap();
        let path = png.to_string_lossy().into_owned();
        let set = |secs: u64| {
            let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
            std::fs::File::options()
                .write(true)
                .open(&png)
                .unwrap()
                .set_modified(when)
                .unwrap();
        };
        let mut editor = EffEditor::default();
        let start = Instant::now();
        assert_eq!(editor.replacement_stamp("", start), 0);
        assert!(editor.replacement_stamp.is_none(), "no path, no read");
        set(1_000);
        assert_eq!(editor.replacement_stamp(&path, start), 1_000_000);
        // Painted again: within the second the kept date stands, after it the new one is read.
        set(2_000);
        let soon = start + std::time::Duration::from_millis(500);
        assert_eq!(editor.replacement_stamp(&path, soon), 1_000_000);
        let later = start + std::time::Duration::from_millis(1_100);
        assert_eq!(editor.replacement_stamp(&path, later), 2_000_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Resetting the open effect in the fold, its only edit there, leaves the fold open with
    /// the effect in it: keyed on whether it held an edit, the fold closed under the user.
    #[test]
    fn resetting_the_open_effect_keeps_its_fold_open() {
        let link = GameLink::default();
        let context = egui::Context::default();
        let shows = |editor: &mut EffEditor, text: &str| {
            let mut found = false;
            for _ in 0..2 {
                let output = context.run_test_ui(egui::RawInput::default(), |ui| {
                    editor.draw_entry_list(ui, &link)
                });
                found = output
                    .shapes
                    .iter()
                    .any(|shape| text_position(&shape.shape, text).is_some());
            }
            found
        };
        let mut editor = editor_with_a_move();
        let names = entry_list_names(&editor.entries);
        let set = editor.ptcl.as_ref().unwrap().emitter_sets[0].clone();
        editor.ptcl.as_mut().unwrap().emitter_sets.push(set);
        editor.pristine.push(editor.pristine[0].clone());
        editor.entries[2].set_idx = 1;
        fn em(editor: &mut EffEditor) -> &mut EmitterDef {
            &mut editor.ptcl.as_mut().unwrap().emitter_sets[1].emitters[0]
        }
        let game = attr_f32(em(&mut editor), TUNED);
        set_attr(em(&mut editor), TUNED, AttrValue::Float(2.5));
        assert!(shows(&mut editor, &names[2]), "the edit opens it");
        editor.selected_entry = Some(2);
        assert!(shows(&mut editor, &names[2]));
        set_attr(em(&mut editor), TUNED, AttrValue::Float(game));
        assert!(shows(&mut editor, &names[2]), "still open after the reset");
        // Another file with nothing wanted in its fold starts with it closed, though this
        // file's is open.
        let mut other = editor_with_a_move();
        other.loaded_path = Some(PathBuf::from("/fx/ef_luigi.eff"));
        assert!(
            !shows(&mut other, &names[2]),
            "another file's fold is its own"
        );
    }

    /// The list says a search matched nothing when there is one, even with Only in the move
    /// on; it said the move plays none of the file's effects.
    #[test]
    fn a_search_that_matches_nothing_says_so() {
        let link = GameLink::default();
        let shows = |editor: &mut EffEditor, text: &str| {
            let context = egui::Context::default();
            let output = context.run_test_ui(egui::RawInput::default(), |ui| {
                editor.draw_entry_list(ui, &link)
            });
            output
                .shapes
                .iter()
                .any(|shape| text_position(&shape.shape, text).is_some())
        };
        let mut editor = editor_with_a_move();
        editor.only_open_move = true;
        editor.set_open_move(Some("Up special".into()), Some("special_hi".into()));
        editor.entry_filter = "batw".into();
        assert!(shows(&mut editor, "No matching effects"));
        assert!(!shows(
            &mut editor,
            "This move plays none of this file's effects"
        ));
        editor.entry_filter.clear();
        editor.set_open_move(Some("Down special".into()), Some("special_lw".into()));
        assert!(shows(
            &mut editor,
            "This move plays none of this file's effects"
        ));
    }

    /// Once every move is read, the list reveals the open effect again: the effects no move
    /// spawns leave the list for their fold then, and the open row moved out of view.
    #[test]
    fn the_open_effect_is_revealed_when_the_moves_are_read() {
        let mut editor = editor_with_a_move();
        editor.set_effect_uses(Default::default(), false);
        editor.selected_entry = Some(1);
        editor.scroll_to_selected = false;
        editor.set_effect_uses(Default::default(), true);
        assert!(editor.scroll_to_selected);
        // Only when the scan finishes, not on every later update.
        editor.scroll_to_selected = false;
        editor.set_effect_uses(Default::default(), true);
        assert!(!editor.scroll_to_selected);
    }

    /// On the shared file the fold names the character whose moves were read, not every move.
    #[test]
    fn the_shared_files_fold_names_whose_moves_were_read() {
        let link = GameLink::default();
        let shows = |editor: &mut EffEditor, text: &str| {
            let context = egui::Context::default();
            let output = context.run_test_ui(egui::RawInput::default(), |ui| {
                editor.draw_entry_list(ui, &link)
            });
            output
                .shapes
                .iter()
                .any(|shape| text_position(&shape.shape, text).is_some())
        };
        let mut editor = editor_with_a_move();
        editor.set_effect_uses_of(Some("Bayonetta".into()));
        assert!(shows(&mut editor, "Not in Bayonetta's moves (1)"));
        assert!(!shows(&mut editor, "Not used by any move (1)"));
        editor.set_effect_uses_of(None);
        assert!(shows(&mut editor, "Not used by any move (1)"));
    }

    /// The Send button carries no marker, and "unsent" shows only once a game has connected:
    /// before that every edit is unsent.
    #[test]
    fn unsent_shows_only_after_a_game_has_connected() {
        let link = GameLink::default();
        let shows = |editor: &mut EffEditor, text: &str| {
            let context = egui::Context::default();
            let output =
                context.run_test_ui(egui::RawInput::default(), |ui| editor.draw_send(ui, &link));
            output
                .shapes
                .iter()
                .any(|shape| text_position(&shape.shape, text).is_some())
        };
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.mark_unsent();
        assert!(
            shows(&mut editor, "⬆ Send edits"),
            "the whole label, with no marker"
        );
        assert!(!shows(&mut editor, "unsent"));
        editor.game_seen = true;
        assert!(shows(&mut editor, "unsent"));
    }

    /// Any texture of the file can be picked but one that stacks another number of pictures,
    /// and for texture 2 and 3 one added here without an id yet.
    #[test]
    fn the_texture_picker_offers_every_size() {
        let texture = |width, layers, id| TextureInfo {
            tex_name: "t".into(),
            id,
            width,
            height: width,
            layers,
            format: "BC7Srgb".into(),
            convertible: true,
        };
        let current = texture(512, 1, Some(1));
        assert_eq!(
            texture_swap_blocked(&texture(64, 1, Some(2)), &current, 0),
            None
        );
        assert_eq!(
            texture_swap_blocked(&texture(64, 1, Some(2)), &current, 1),
            None
        );
        assert!(texture_swap_blocked(&texture(512, 2, Some(2)), &current, 0).is_some());
        assert_eq!(
            texture_swap_blocked(&texture(64, 1, None), &current, 0),
            None
        );
        assert!(texture_swap_blocked(&texture(64, 1, None), &current, 2).is_some());
    }

    /// Opening another file and coming back keeps the first file's steps; a file that comes
    /// back with other effects or textures loses them, and the Undo hover says so.
    #[test]
    fn undo_history_is_kept_per_file() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.selected_entry = Some(0);
        history_frame(&mut editor, ms(0), false, None);
        history_frame(&mut editor, ms(10), false, Some(2.0));
        history_frame(&mut editor, ms(2000), false, None);
        assert!(editor.can_undo());
        let kirby = editor.loaded_path.clone().unwrap();

        // Away to another file: its own, empty history.
        editor.history_close_step();
        editor.stash_history();
        editor.loaded_path = Some(PathBuf::from("/fx/ef_mario.eff"));
        editor.restore_history(Path::new("/fx/ef_mario.eff"));
        assert!(!editor.can_undo());

        // Back, as loaded before: the step is there and undoes.
        editor.stash_history();
        editor.loaded_path = Some(kirby.clone());
        editor.restore_history(&kirby);
        assert!(editor.can_undo());
        editor.undo();
        assert_eq!(tuned(&editor), 1.0);
        editor.redo();
        assert_eq!(tuned(&editor), 2.0);

        // Back with another set list (a borrowed effect added): dropped, and said.
        editor.stash_history();
        editor.ptcl.as_mut().unwrap().emitter_sets[0].name = "P_Other".into();
        editor.restore_history(&kirby);
        assert!(!editor.can_undo() && !editor.can_redo());
        assert!(editor.history.cleared.is_some());
    }

    /// "Only in <move>" comes on with a move that plays this file's effects, not with one that
    /// plays none, and stays off once the user turns it off.
    #[test]
    fn only_in_move_turns_on_for_moves_with_effects() {
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        let uses = vec![MoveUse {
            motion: Some("attack_s4_s".into()),
            label: "Forward smash".into(),
            bones: Vec::new(),
        }];
        editor.effect_uses.insert(editor.entries[0].hash, uses);
        assert!(editor.only_in_move_on_pick("attack_s4_s"));
        assert!(!editor.only_in_move_on_pick("attack_11"));
        editor.only_open_move_declined = true;
        assert!(!editor.only_in_move_on_pick("attack_s4_s"));
    }

    /// Copies are always marked; of the rest, the part dragged up is, not the one it passed.
    #[test]
    fn copied_and_moved_parts_are_found() {
        let plain = |list: &[usize]| list.iter().map(|&i| (i, false)).collect::<Vec<_>>();
        assert_eq!(added_or_moved(&plain(&[0, 1, 2])), [false; 3]);
        assert_eq!(
            added_or_moved(&plain(&[1, 0, 2, 3])),
            [true, false, false, false]
        );
        assert_eq!(
            added_or_moved(&plain(&[0, 2, 3, 1])),
            [false, false, false, true]
        );
        assert_eq!(
            added_or_moved(&[(0, false), (0, true), (1, false)]),
            [false, true, false]
        );
        // A removed part leaves the rest in place.
        assert_eq!(added_or_moved(&plain(&[0, 2])), [false, false]);
    }

    /// Once the filter is cleared, each section is as it was before the search: a search for
    /// "gravity" opened the sections it matched for every part from then on.
    #[test]
    fn clearing_the_filter_closes_what_it_opened() {
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        let em = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[0];
        let pr = editor.pristine[0][0].clone();
        let context = egui::Context::default();
        let row = "Gravity strength";
        let mut shows_row = |filter: &str| {
            let mut filter = filter.to_owned();
            let mut found = false;
            for _ in 0..2 {
                let output = context.run_test_ui(egui::RawInput::default(), |ui| {
                    draw_attribute_groups(ui, em, &pr, &mut filter, &[], egui::Id::NULL);
                });
                found = output
                    .shapes
                    .iter()
                    .any(|shape| text_position(&shape.shape, row).is_some());
            }
            found
        };
        assert!(!shows_row(""), "closed to begin with");
        assert!(shows_row("gravity"), "the filter opens it");
        assert!(!shows_row(""), "and clearing it closes it again");
    }

    /// A part with an edit opens the section holding it; the next part, with none, does not
    /// open that section too.
    #[test]
    fn an_edited_part_opens_its_sections_for_itself_only() {
        let mut editor = editor_with_two_parts();
        tune(&mut editor, 2.5);
        let row = crate::eff_attrs::table()
            .iter()
            .find(|a| a.id == TUNED)
            .unwrap()
            .label;
        let context = egui::Context::default();
        let pr = editor.pristine[0].clone();
        let mut shows_row = |part: usize| {
            let em = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[part];
            let mut filter = String::new();
            let mut found = false;
            for _ in 0..2 {
                let output = context.run_test_ui(egui::RawInput::default(), |ui| {
                    draw_attribute_groups(
                        ui,
                        em,
                        &pr[part],
                        &mut filter,
                        &[],
                        egui::Id::unique(part),
                    );
                });
                found = output
                    .shapes
                    .iter()
                    .any(|shape| text_position(&shape.shape, row).is_some());
            }
            found
        };
        assert!(shows_row(0), "the edited part shows its edit");
        assert!(!shows_row(1), "the other part's section stays closed");
        assert!(shows_row(0), "and the edited one is still open");
    }

    /// Undo and Redo move to the next row together when the header has no room for them, at
    /// any panel width: nested in the wrapping row, they ran past a narrow panel's edge.
    #[test]
    fn undo_and_redo_stay_inside_a_narrow_panel() {
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.selected_entry = Some(0);
        let context = egui::Context::default();
        let mut overflowed = Vec::new();
        for width in (200..=420).step_by(2) {
            let width = width as f32;
            let mut right = 0.0;
            let mut output = None;
            for _ in 0..2 {
                output = Some(context.run_test_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        right = ui.max_rect().right();
                        editor.draw_emitter_editor(ui);
                    },
                ));
            }
            let output = output.unwrap();
            let redo_right = output
                .shapes
                .iter()
                .find_map(|shape| text_right(&shape.shape, "Redo"))
                .expect("Redo is drawn");
            if redo_right > right + 0.5 {
                overflowed.push(width);
            }
        }
        assert!(
            overflowed.is_empty(),
            "Redo past the edge at widths {overflowed:?}"
        );
    }

    /// Every text the Look tab draws stays inside the panel, down to a 260 px panel, with an
    /// edited X Y Z row: its reset and the Whole effect boxes ran past a 385 px panel's edge.
    #[test]
    fn the_look_rows_fit_a_narrow_panel() {
        fn texts(shape: &egui::epaint::Shape, out: &mut Vec<(String, f32)>) {
            match shape {
                egui::epaint::Shape::Text(text) => out.push((
                    text.galley.job.text.clone(),
                    text.pos.x + text.galley.size().x,
                )),
                egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let mut editor = editor_with_one_emitter("/fx/ef_kirby.eff");
        editor.selected_entry = Some(0);
        let em = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[0];
        set_attr(em, SPREAD_ATTRS[2], AttrValue::Float(12.5));
        let context = egui::Context::default();
        let mut overflowed = Vec::new();
        let mut shown = Vec::new();
        for width in (260..=720).step_by(5) {
            let width = width as f32;
            let mut right = 0.0;
            let mut output = None;
            for _ in 0..3 {
                output = Some(context.run_test_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 2000.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        right = ui.max_rect().right();
                        editor.draw_emitter_editor(ui);
                    },
                ));
            }
            let mut all = Vec::new();
            for shape in &output.unwrap().shapes {
                texts(&shape.shape, &mut all);
            }
            shown = all.iter().map(|(text, _)| text.clone()).collect();
            if let Some((text, _)) = all.iter().find(|(_, end)| *end > right + 0.5) {
                overflowed.push((width, text.clone()));
            }
        }
        // The rows the claim is about were drawn at all.
        for text in ["↺", "Speed ×", "Spread"] {
            assert!(shown.iter().any(|t| t == text), "{text} is drawn");
        }
        assert!(
            shown.iter().any(|t| t.starts_with("was ")),
            "the was line is drawn"
        );
        assert!(overflowed.is_empty(), "past the edge: {overflowed:?}");
    }

    fn text_right(shape: &egui::epaint::Shape, label: &str) -> Option<f32> {
        match shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos.x + text.galley.size().x)
            }
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().find_map(|shape| text_right(shape, label))
            }
            _ => None,
        }
    }

    /// A search finds rows by what people call them, through the hover as well as the label.
    #[test]
    fn the_filter_finds_speed_and_count() {
        let finds = |needle: &str, id: &str| {
            let a = crate::eff_attrs::table()
                .iter()
                .find(|a| a.id == id)
                .unwrap();
            filter_terms(needle).iter().any(|term| {
                a.label.to_lowercase().contains(term) || a.doc.to_lowercase().contains(term)
            })
        };
        assert!(finds("speed", "particle_velocity.all_direction"));
        assert!(finds("speed", "particle_velocity.designated_dir_scale"));
        assert!(finds("count", "emission.rate"));
        assert!(finds("size", "particle_scale.scale_x"));
        assert!(finds("color", "particle_color.color0_r"));
        assert!(!finds("count", "particle_velocity.all_direction"));
    }

    /// Speed scales both starting speeds together and shows the larger.
    #[test]
    fn the_speed_row_scales_both_speeds() {
        assert_eq!(shown_speed([0.5, 2.0]), 2.0);
        assert_eq!(scaled_speeds([0.5, 2.0], 4.0), [1.0, 4.0]);
        // A part that did not move gets an outward speed.
        assert_eq!(scaled_speeds([0.0, 0.0], 1.5), [1.5, 0.0]);
    }

    /// The Edited chip of another character opens that character, so Character, Move and file
    /// agree; the open character's chip just reopens its file.
    #[test]
    fn the_edited_chip_opens_its_character() {
        let source = EditSource {
            fighter: "bayonetta".into(),
            label: "Bayonetta".into(),
            base: PathBuf::from("/fx/ef_bayonetta.eff"),
            alt_base: None,
            merged: None,
            transplant_count: 1,
            changed_effects: 2,
            textures: 0,
            transplants: Vec::new(),
        };
        let mut editor = EffEditor {
            fighter_choices: vec![
                ("bayonetta".into(), "Bayonetta".into()),
                ("edge".into(), "Sephiroth".into()),
            ],
            open_fighter: Some("edge".into()),
            ..Default::default()
        };
        editor.open_edit_source(&source);
        assert_eq!(editor.fighter_request.as_deref(), Some("bayonetta"));
        assert!(editor.pending_load.is_none());

        let mut editor = EffEditor {
            fighter_choices: vec![("bayonetta".into(), "Bayonetta".into())],
            open_fighter: Some("bayonetta".into()),
            ..Default::default()
        };
        editor.open_edit_source(&source);
        assert!(editor.fighter_request.is_none());
        assert_eq!(editor.pending_load, Some(source.base.clone()));
    }

    /// A file loaded before a project opened holds none of that project's edits: reading its
    /// diff back then wrote "no edits" over the project's (Mario's saved effect edit vanished
    /// on every relaunch). Paired: an untouched editor's working copy is read back.
    #[test]
    fn a_working_copy_from_before_a_project_opened_is_not_read_back() {
        let mut editor = editor_with_one_emitter("/fx/ef_mario.eff");
        assert!(editor.working_is_current());
        editor.reset_project_state();
        assert!(!editor.working_is_current());
        // It loads again, for the new project.
        assert_eq!(
            editor.pending_load.as_deref(),
            Some(Path::new("/fx/ef_mario.eff"))
        );
        // Queued once more while stale, it still reloads instead of being skipped.
        editor.pending_load = None;
        editor.queue_load(Path::new("/fx/ef_mario.eff"));
        assert!(editor.pending_load.is_some());
    }

    /// Edits queued for a file that has not loaded yet are that file's: another file queued
    /// over it must not receive them. Paired: queuing the same file keeps them.
    #[test]
    fn edits_queued_for_one_file_never_reach_another() {
        let mut editor = editor_with_one_emitter("/fx/ef_bayonetta.eff");
        editor.queue_load(Path::new("/fx/ef_mario.eff"));
        editor.queue_edits(Vec::new());
        editor.queue_load(Path::new("/fx/ef_mario.eff"));
        assert!(
            editor.pending_edits.is_some(),
            "the same file keeps its edits"
        );
        editor.load_as(
            Path::new("/fx/ef_bayonetta.eff"),
            "effect/fighter/bayonetta/ef_bayonetta.eff".into(),
        );
        assert!(
            editor.pending_edits.is_none(),
            "Mario's edits went to Bayonetta's file"
        );
    }

    /// The attribute these tests move to make the fixture "edited".
    const TUNED: &str = "particle_scale.scale_x";

    /// Tune the one emitter so the working copy differs from its pristine snapshot.
    fn tune(editor: &mut EffEditor, scale: f32) {
        set_attr(
            &mut editor.ptcl.as_mut().expect("ptcl").emitter_sets[0].emitters[0],
            TUNED,
            AttrValue::Float(scale),
        );
    }

    /// What an edit record says the tuned attribute is now.
    fn tuned_value(edit: &AuthoredEdit) -> Option<f32> {
        edit.fields.attrs.get(TUNED).map(|v| v.as_f32())
    }

    /// Give the fixture a two-texture pool and point its emitter at the first one.
    fn with_textures(editor: &mut EffEditor) {
        let ptcl = editor.ptcl.as_mut().expect("ptcl");
        ptcl.bntx_textures = ["ef_a", "ef_b"]
            .iter()
            .map(|n| TextureInfo {
                tex_name: (*n).into(),
                id: None,
                width: 64,
                height: 64,
                layers: 1,
                format: "BC7Srgb".into(),
                convertible: true,
            })
            .collect();
        ptcl.emitter_sets[0].emitters[0].texture_index = Some(0);
        editor.pristine = vec![vec![EmitterSnapshot::of(&ptcl.emitter_sets[0].emitters[0])]];
    }

    /// Moving to an entry with fewer emitters must not leave the selection past the end.
    ///
    /// It used to: the clamp went into a local index while `selected_emitter` stayed out of
    /// range. The visible symptom was a tab strip with nothing highlighted, but the damaging
    /// one was silent — the texture panel writes its swap through `selected_emitter`, so the
    /// swap landed on no emitter and was simply lost.
    #[test]
    fn selecting_a_smaller_entry_pulls_the_emitter_selection_back_in_range() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        // A second set with three emitters, then back to the one-emitter set.
        let ptcl = editor.ptcl.as_mut().unwrap();
        let template = ptcl.emitter_sets[0].emitters[0].clone();
        ptcl.emitter_sets.push(EmitterSet {
            name: "P_Big".into(),
            emitters: vec![template.clone(), template.clone(), template],
        });
        editor.pristine.push(
            editor.ptcl.as_ref().unwrap().emitter_sets[1]
                .emitters
                .iter()
                .map(EmitterSnapshot::of)
                .collect(),
        );

        editor.selected_emitter = 2;
        editor.clamp_selected_emitter(1);
        assert_eq!(editor.selected_emitter, 2, "in range — leave it alone");

        editor.clamp_selected_emitter(0);
        assert_eq!(
            editor.selected_emitter, 0,
            "out of range for a one-emitter set"
        );

        // A set index that does not exist at all must not leave a stale selection either.
        editor.selected_emitter = 5;
        editor.clamp_selected_emitter(99);
        assert_eq!(editor.selected_emitter, 0);
    }

    /// The entry list and the tab strip both highlight what is edited; they must agree with
    /// what `collect_authored_edits` will actually ship.
    #[test]
    fn edited_sets_match_what_gets_sent() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        assert!(editor.edited_sets().is_empty());

        tune(&mut editor, 2.5);
        assert_eq!(
            editor.edited_sets(),
            std::collections::HashSet::from([0]),
            "the tuned set must be marked"
        );
        assert_eq!(editor.collect_authored_edits().len(), 1);

        // A texture swap is an edit too — the markers must not miss it just because no
        // numeric field moved.
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        with_textures(&mut editor);
        assert!(editor.edited_sets().is_empty());
        editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[0].texture_index = Some(1);
        assert_eq!(
            editor.edited_sets(),
            std::collections::HashSet::from([0]),
            "a swap with no field change must still mark the set"
        );
    }

    /// The texture picker used to write `em.texture_index` and stop there: no snapshot field,
    /// no edit record, no export path. It changed the editor's own view and nothing shipped.
    #[test]
    fn a_texture_swap_becomes_an_edit_record_naming_the_texture() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        with_textures(&mut editor);
        assert!(
            editor.collect_authored_edits().is_empty(),
            "an untouched fixture must have no edits"
        );

        editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[0].texture_index = Some(1);

        let edits = editor.collect_authored_edits();
        assert_eq!(edits.len(), 1, "the swap must produce an edit record");
        assert_eq!(
            edits[0].fields.texture_name.as_deref(),
            Some("ef_b"),
            "the record must name the texture, not its pool index"
        );
    }

    /// Reset must undo a swap like any other field — it restores from the same snapshot.
    #[test]
    fn resetting_an_emitter_undoes_a_texture_swap() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        with_textures(&mut editor);
        editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[0].texture_index = Some(1);

        let pristine = editor.pristine[0][0].clone();
        pristine.restore(&mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[0]);

        assert!(
            editor.collect_authored_edits().is_empty(),
            "reset must clear the swap"
        );
    }

    /// A swap round-trips through the project: saved as a name, reapplied as an index against
    /// whatever pool the reloaded file has.
    #[test]
    fn a_saved_texture_swap_reapplies_onto_the_reloaded_eff() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        with_textures(&mut editor);
        editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[0].texture_index = Some(1);
        let saved = editor.collect_authored_edits();

        let mut reloaded = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        with_textures(&mut reloaded);
        reloaded.apply_authored_edits(&saved);

        assert_eq!(
            reloaded.ptcl.as_ref().unwrap().emitter_sets[0].emitters[0].texture_index,
            Some(1)
        );
    }

    /// A transplant reloads the eff from a baseline that deliberately excludes authored edits,
    /// so the edits must be carried across explicitly or they vanish from the panel — and the
    /// next `sync_eff_mods_from_editor` then writes the empty diff back over the project.
    #[test]
    fn transplanting_carries_emitter_edits_across_the_reload() {
        let base = PathBuf::from("effect/fighter/kirby/ef_kirby.eff");
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        tune(&mut editor, 2.5);
        assert_eq!(
            editor.collect_authored_edits().len(),
            1,
            "fixture is edited"
        );

        editor.set_merged_overlay(
            &base,
            Some(Path::new(crate::scratch_dirs::TRANSPLANT_PREVIEW_FILE)),
        );

        let carried = editor.pending_edits.as_ref().expect("edits carried across");
        assert_eq!(carried.len(), 1);
        assert_eq!(carried[0].set_name, "P_KirbyDash");
        assert_eq!(carried[0].entry_name, "kirby_dash");
        assert_eq!(tuned_value(&carried[0]), Some(2.5));
        assert!(
            !editor.pending_edits_push_live,
            "a reload we caused must not deploy to the running game on its own"
        );
    }

    /// Re-applying them lands the values back on the working copy, so the diff is non-empty
    /// again and the project keeps its `authored` list.
    #[test]
    fn carried_edits_reapply_onto_the_reloaded_eff() {
        let base = PathBuf::from("effect/fighter/kirby/ef_kirby.eff");
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        tune(&mut editor, 2.5);
        editor.set_merged_overlay(
            &base,
            Some(Path::new(crate::scratch_dirs::TRANSPLANT_PREVIEW_FILE)),
        );
        let carried = editor.pending_edits.take().expect("edits carried across");

        // Stand in for the reload: the merged baseline has the emitter back at pristine.
        tune(&mut editor, 1.0);
        assert!(editor.collect_authored_edits().is_empty(), "reload resets");

        editor.apply_authored_edits(&carried);
        let after = editor.collect_authored_edits();
        assert_eq!(after.len(), 1, "the edit is back in the panel's diff");
        assert_eq!(tuned_value(&after[0]), Some(2.5));
    }

    /// A project saved before the attribute table existed carries its edits in five named
    /// fields. Opening one has to put those values back on the emitter — and re-collecting has
    /// to record them in the new form, so the project migrates itself by being opened.
    #[test]
    fn a_legacy_projects_named_fields_still_land() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        let legacy = AuthoredEdit {
            set_name: "P_KirbyDash".into(),
            entry_name: "kirby_dash".into(),
            set_idx: 0,
            emitter_name: "em0".into(),
            emitter_idx: 0,
            fields: EmitterFieldEdits {
                emission_rate: Some(12.5),
                lifetime: Some(30.0),
                color_scale: Some(2.0),
                emitter_scale: Some([3.0, 4.0, 5.0]),
                ..Default::default()
            },
        };
        editor.apply_authored_edits(&[legacy]);

        let em = &editor.ptcl.as_ref().expect("ptcl").emitter_sets[0].emitters[0];
        assert_eq!(attr_f32(em, "emission.rate"), 12.5);
        assert_eq!(attr(em, "particle_data.life"), Some(AttrValue::Int(30)));
        assert_eq!(attr_f32(em, "emitter_static.color_scale"), 2.0);
        assert_eq!(attr_f32(em, "emitter_info.scale_x"), 3.0);
        assert_eq!(attr_f32(em, "emitter_info.scale_z"), 5.0);

        // Re-collected in the general form, with the named fields no longer in play.
        let collected = editor.collect_authored_edits();
        assert_eq!(collected.len(), 1);
        let fields = &collected[0].fields;
        assert_eq!(
            fields.attrs.get("emission.rate").map(|v| v.as_f32()),
            Some(12.5)
        );
        assert_eq!(
            fields.attrs.get("emitter_info.scale_y").map(|v| v.as_f32()),
            Some(4.0)
        );
        assert!(
            fields.emission_rate.is_none() && fields.emitter_scale.is_none(),
            "the named fields are re-recorded as attributes, not carried forward"
        );
    }

    #[test]
    fn friendly_key_controls_and_attribute_rows_stay_in_sync() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        let em = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[0];
        set_attr(em, "emitter_static.num_color0_keys", AttrValue::Int(1));
        set_attr(em, "particle_color.color0_type", AttrValue::Int(1));
        em.color0 = vec![ColorKey {
            frame: 7.5,
            r: 0.25,
            g: 0.5,
            b: 0.75,
            a: 1.0,
        }];

        effective_keys_to_attrs(em);
        assert_eq!(
            attr(em, "emitter_static.color0.keys[0].time"),
            Some(AttrValue::Float(7.5))
        );
        assert_eq!(
            attr(em, "emitter_static.color0.keys[0].z"),
            Some(AttrValue::Float(0.75))
        );

        set_attr(
            em,
            "emitter_static.color0.keys[0].time",
            AttrValue::Float(19.0),
        );
        set_attr(em, "emitter_static.color0.keys[0].x", AttrValue::Float(0.9));
        attrs_to_effective_keys(em);
        assert_eq!(em.color0[0].frame, 19.0);
        assert_eq!(em.color0[0].r, 0.9);
    }

    /// A duplicated emitter has to be addressable: its own name, its own row in the list, and a
    /// roster the exporter can rebuild the set from.
    #[test]
    fn duplicating_an_emitter_records_a_roster() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        assert!(
            editor.collect_rosters().is_empty(),
            "an untouched set records no emitter list"
        );

        let set = &mut editor.ptcl.as_mut().expect("ptcl").emitter_sets[0];
        let mut copy = set.emitters[0].clone();
        copy.name = unique_emitter_name(&copy.name, &set.emitters);
        set.emitters.push(copy);

        let rosters = editor.collect_rosters();
        assert_eq!(rosters.len(), 1);
        assert_eq!(rosters[0].set_name, "P_KirbyDash");
        assert_eq!(rosters[0].entry_name, "kirby_dash");
        let slots = &rosters[0].slots;
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[1].source_idx, 0, "the copy clones the original");
        assert_eq!(slots[1].source_name, "em0");
        assert_eq!(slots[1].name, "em0_copy");

        // Re-applying the saved list must give the same two emitters, not four — this is what a
        // second project open over an already-edited eff does.
        editor.apply_structure_edits(&rosters, &[]);
        editor.apply_structure_edits(&rosters, &[]);
        let names: Vec<String> = editor.ptcl.as_ref().expect("ptcl").emitter_sets[0]
            .emitters
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(names, vec!["em0".to_string(), "em0_copy".to_string()]);
    }

    /// The emitter list is flat but the emitters are a tree, and the nesting is carried by
    /// `depth` plus position. So a list operation that moves a parent without its children hands
    /// those children to whichever emitter lands above them — the effect keeps playing, with the
    /// wrong emitter driving the wrong particles. `subtree` is what every one of those operations
    /// is scoped by, so it is checked directly.
    #[test]
    fn a_subtree_covers_an_emitter_and_its_children() {
        let at = |depth: u8| EmitterDef {
            name: String::new(),
            attrs: Vec::new(),
            subsections: Vec::new(),
            depth,
            source_idx: 0,
            color0: Vec::new(),
            color1: Vec::new(),
            alpha0_keys: Vec::new(),
            texture_index: None,
        };
        //  0  A
        //  1    A1
        //  2      A1a
        //  3    A2
        //  4  B
        let list: Vec<EmitterDef> = [0u8, 1, 2, 1, 0].iter().map(|d| at(*d)).collect();
        assert_eq!(
            subtree(&list, 0),
            0..4,
            "a root takes all of its descendants"
        );
        assert_eq!(subtree(&list, 1), 1..3, "a child takes its own grandchild");
        assert_eq!(subtree(&list, 2), 2..3, "a leaf is just itself");
        assert_eq!(subtree(&list, 3), 3..4);
        assert_eq!(subtree(&list, 4), 4..5, "the last emitter runs to the end");
        assert_eq!(subtree(&list, 9), 9..9, "an index past the end is empty");
    }

    /// The part list stops at what the game loads: 16 parts made from one part, and one level of
    /// parts made from parts. Parts on their own have no limit (24 load and play in game).
    #[test]
    fn copying_and_nesting_stop_at_what_the_game_loads() {
        assert!(
            room_to_copy(&[0u8; 24], 3),
            "parts on their own are not capped"
        );

        // A part made from part 0, which already has 15 others made from it.
        let mut family = vec![0u8];
        family.extend(std::iter::repeat_n(1u8, 15));
        family.push(0);
        assert!(room_to_copy(&family, 1), "the 16th child has room");
        assert!(
            room_to_nest(&family, 16),
            "and so does nesting the part below"
        );
        family.insert(1, 1);
        assert!(!room_to_copy(&family, 1), "a 17th child does not");
        assert!(!room_to_nest(&family, 17));

        // Only a top-level part with nothing made from it can be nested.
        assert!(room_to_nest(&[0, 0], 1));
        assert!(
            !room_to_nest(&[0, 0, 1], 1),
            "its child would become a grandchild"
        );
        assert!(!room_to_nest(&[0, 1, 1], 2), "a child is already nested");
        assert!(
            !room_to_nest(&[0], 0),
            "the first part has nothing above it"
        );
    }

    #[test]
    fn nesting_a_parent_keeps_its_descendants_attached() {
        let at = |depth: u8| EmitterDef {
            name: String::new(),
            attrs: Vec::new(),
            subsections: Vec::new(),
            depth,
            source_idx: 0,
            color0: Vec::new(),
            color1: Vec::new(),
            alpha0_keys: Vec::new(),
            texture_index: None,
        };
        // X, A(A1(A1a), A2), B. Nesting A under X must shift A's entire subtree.
        let mut list: Vec<_> = [0u8, 0, 1, 2, 1, 0].iter().map(|d| at(*d)).collect();
        assert!(shift_subtree_depth(&mut list, 1, 1));
        assert_eq!(
            list.iter().map(|e| e.depth).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 2, 0]
        );
    }

    #[test]
    fn removing_every_emitter_survives_project_reapply() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        editor.ptcl.as_mut().unwrap().emitter_sets[0]
            .emitters
            .clear();
        let rosters = editor.collect_rosters();
        assert_eq!(rosters.len(), 1);
        assert!(rosters[0].slots.is_empty());

        let mut reloaded = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        reloaded.apply_structure_edits(&rosters, &[]);
        assert!(reloaded.ptcl.unwrap().emitter_sets[0].emitters.is_empty());
    }

    /// Draw the whole attribute tree, for real, over a real emitter.
    ///
    /// Four hundred rows built from a table means the failure modes are drawing ones: an id
    /// collision between two widgets, a slice index taken from a lookup that returned None, a
    /// group whose rows do not exist on this emitter. None of those show up in a type check, and
    /// all of them are a panic or a frozen control in front of the user. Running the panel
    /// headlessly is the cheapest way to find them.
    #[test]
    fn the_attribute_tree_draws() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let loaded = match load_effect(&root.join("effect/fighter/mario/ef_mario.eff")) {
            Ok(loaded) => loaded,
            Err(err) => {
                eprintln!("skipped: {err}");
                return;
            }
        };
        let mut em = loaded.ptcl.emitter_sets[0].emitters[0].clone();
        let pr = EmitterSnapshot::of(&em);
        let mut filter = String::new();
        egui::__run_test_ui(|ui| {
            draw_attribute_groups(ui, &mut em, &pr, &mut filter, &[], egui::Id::NULL);
            let basics: Vec<usize> = BASIC_ATTRS
                .iter()
                .filter_map(|id| crate::eff_attrs::index_of(id))
                .collect();
            attr_rows_by_index(ui, "basics", &mut em, &pr, &basics, &[]);
        });

        // A filter matching almost everything opens almost every group, so this pass actually
        // lays out the rows rather than just their headers — which is where an id collision or a
        // bad index would surface.
        let mut filter = "e".to_string();
        egui::__run_test_ui(|ui| {
            draw_attribute_groups(ui, &mut em, &pr, &mut filter, &[], egui::Id::NULL);
        });
        let mut filter = "gravity".to_string();
        egui::__run_test_ui(|ui| {
            draw_attribute_groups(ui, &mut em, &pr, &mut filter, &[], egui::Id::NULL);
        });
        let mut filter = "no such attribute".to_string();
        egui::__run_test_ui(|ui| {
            draw_attribute_groups(ui, &mut em, &pr, &mut filter, &[], egui::Id::NULL);
        });
    }

    /// Recording a transplant marks the project unsent AND reloads the eff from the merged
    /// preview. The reload used to clear the marker, so the Send button lit for a frame and went
    /// back to grey — the user was never told the running game hadn't been given the transplant.
    #[test]
    fn a_reload_keeps_the_unsent_marker() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        editor.mark_unsent();
        // The load itself fails (nothing on disk here); what matters is that it no longer wipes
        // the marker on its way in, which the old reset did before this early return.
        editor.load_eff(Path::new("effect/fighter/kirby/_transplant_preview.eff"));
        assert!(
            editor.eff_dirty_at.is_some(),
            "a reload cleared the unsent marker"
        );
    }

    /// Handing everything to the game is what clears it — from any caller, not just the button.
    #[test]
    fn requesting_a_live_apply_clears_the_unsent_marker() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        editor.mark_unsent();
        editor.request_live_apply();
        assert!(editor.eff_dirty_at.is_none());
    }

    /// "Use in <move>" is greyed when the move already plays the effect, so one click cannot
    /// add a second copy; another effect can still be added.
    #[test]
    fn use_in_a_move_that_plays_it_already_is_greyed() {
        let mut editor = EffEditor::default();
        assert!(!editor.use_in_move_state("mario_fb_shoot").0);
        editor.set_open_move(Some("Forward smash".into()), Some("attack_s4_s".into()));
        editor.set_open_move_effects(vec![("mario_fb_shoot".into(), 16)]);
        assert_eq!(
            editor.use_in_move_state("MARIO_FB_SHOOT"),
            (
                false,
                "Forward smash already plays it from frame 16".to_owned()
            )
        );
        assert!(editor.use_in_move_state("mario_fb_fire").0);
    }

    /// Every float on the page shows two decimals, more only where two would read as zero, and
    /// a key's time reads as a percentage of the particle's life without changing what is
    /// stored.
    #[test]
    fn numbers_read_alike_and_key_times_as_percent() {
        assert_eq!(float_decimals(1.0), 2);
        assert_eq!(float_decimals(0.4), 2);
        assert_eq!(float_decimals(0.0), 2);
        assert_eq!(float_decimals(0.005), 4);
        assert_eq!(float_decimals(-0.02), 2);
        let table = crate::eff_attrs::table();
        let at = |id| &table[crate::eff_attrs::index_of(id).unwrap()];
        assert_eq!(
            attr_text(at("particle_velocity.vel_random"), AttrValue::Float(40.0)),
            "40.0%"
        );
        assert_eq!(
            attr_text(
                at("particle_velocity.diffusion_dir_angle"),
                AttrValue::Float(15.0)
            ),
            "15.0°"
        );
        assert_eq!(
            attr_text(at("emission.rate"), AttrValue::Float(1.0)),
            "1.00"
        );
        assert!((life_fraction(51.0) - 0.51).abs() < 1e-6);
    }

    /// The send on connect marks everything sent too, without asking for another send.
    #[test]
    fn marking_sent_clears_the_unsent_marker_only() {
        let mut editor = editor_with_one_emitter("effect/fighter/kirby/ef_kirby.eff");
        editor.mark_unsent();
        editor.mark_sent();
        assert!(editor.eff_dirty_at.is_none());
        assert!(!editor.take_live_deploy_request());
    }

    /// A project load still pushes, so opening a project gets the game showing it.
    #[test]
    fn a_project_load_still_requests_a_live_push() {
        let mut editor = EffEditor::default();
        editor.queue_edits(vec![AuthoredEdit {
            set_name: "P_KirbyDash".into(),
            entry_name: "kirby_dash".into(),
            set_idx: 0,
            emitter_name: "em0".into(),
            emitter_idx: 0,
            fields: EmitterFieldEdits {
                scale: Some(2.5),
                ..Default::default()
            },
        }]);
        assert!(editor.pending_edits.is_some());
        assert!(editor.pending_edits_push_live);
    }
}
