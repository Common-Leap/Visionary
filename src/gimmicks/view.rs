//! Compact fighter/path/value editor (slice F0.4).
//!
//! One tunables card per fighter: a discovered fighter selector, a scalar-leaf
//! selector displayed in the stable dotted spelling
//! (`param_special_lw[0].limit_gauge_add`), and a typed value. The card reads
//! through the F0.1/F0.2 accessors in [`super::params`]
//! and writes through [`record_personal_edit`](super::params::record_personal_edit),
//! so the project stays sparse and typed exactly like the traits editor's.
//!
//! Two gates are structural here, not advisory:
//!
//! * **No free-form type choice.** There is no type picker: the leaf's type in
//!   the base `vl.prc` decides how the typed text parses, the way
//!   `src/roster/traits_view.rs` lets the field decide. Without a readable
//!   base file there is no type to coerce to, so nothing can be saved.
//! * **Invalid/unresolved paths cannot be silently saved.** [`validate`]
//!   rejects an empty or malformed fighter name, an unparsable path, a path
//!   with raw `0x…` segments, a leaf missing from the base file, a
//!   non-scalar leaf, and text that does not parse as the leaf's type. The
//!   Save control is disabled until every check passes, and the blocking
//!   reason is always shown naming fighter, path, and expected type.
//!
//! The egui [`PersonalView`] is the thin card over the pure [`validate`] /
//! [`commit`] core; tests cover the core, mirroring how the traits editor
//! tests its own render/parse pair. The main Gimmicks editor owns this advanced
//! single-parameter component and supplies the same sparse per-fighter map
//! used by traits and export. Selection is locked to the main editor's character.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use egui::{Color32, RichText, Ui};
use prc::ParamKind;

use super::params::{
    locate_personal, open_personal, path_is_resolved, personal_edits, personal_path_for, read_leaf,
    record_personal_edit, value_kind_name,
};
use super::paths::NestedPath;
use crate::mod_project::{ParamMod, ParamValue};
use crate::roster::traits::to_param_value;

/// Fighters with a readable-looking personal-file location under the active
/// roots. This is discovery only; [`load_base`] still parses the selected file
/// and reports corruption rather than hiding it from the picker.
pub fn available_fighters(roots: &[PathBuf]) -> Vec<String> {
    let mut fighters = std::collections::BTreeSet::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root.join("fighter")) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if personal_path_for(&name).is_ok() && entry.path().join("param/vl.prc").is_file() {
                fighters.insert(name);
            }
        }
    }
    fighters.into_iter().collect()
}

/// The base tree a draft is coerced and resolved against.
pub enum BaseState {
    /// No personal file under any root. Stored edits stay visible, but no new
    /// value can be saved: without the base leaf there is no type to coerce
    /// to, and guessing one would be the free-form type choice F0.4 forbids.
    Missing,
    /// The file exists but failed to read, with the reason.
    Unreadable(String),
    /// The tree to coerce and resolve against.
    Present(prc::ParamStruct),
}

/// Load the base tree for one fighter. Never touches the dump beyond reading.
pub fn load_base(roots: &[PathBuf], fighter: &str) -> BaseState {
    let Some(path) = locate_personal(roots, fighter) else {
        return BaseState::Missing;
    };
    match open_personal(&path, fighter) {
        Ok(tree) => BaseState::Present(tree),
        Err(error) => BaseState::Unreadable(format!("{error:#}")),
    }
}

/// The two selector values and typed value draft. Held apart from the committed
/// project so half-typed values never reach the sparse map, as in the traits editor.
#[derive(Default)]
pub struct PersonalDraft {
    pub fighter: String,
    pub path: String,
    pub value: String,
}

/// A draft that passed every save gate: the fighter, the parsed path, the
/// value in the leaf's own type, and the base value it overrides (or agrees
/// with, in which case saving clears the override).
#[derive(Debug)]
pub struct ResolvedEdit {
    pub fighter: String,
    pub path: NestedPath,
    pub value: ParamValue,
    pub base: ParamValue,
}

/// One reason a draft cannot be saved. Every variant renders with the fighter,
/// path, and expected type involved — an error names those, never a bare
/// "invalid input".
#[derive(Debug)]
pub enum DraftProblem {
    /// The fighter name is empty or not a plain directory name.
    FighterInvalid(String),
    /// The path text does not parse, with the parser's reason.
    PathInvalid(String),
    /// The path parses but carries raw `0x…` segments: an unresolved authored
    /// label, which must not silently export (see `path_is_resolved`).
    PathUnresolved(String),
    /// No personal file for the fighter under any root.
    BaseMissing(String),
    /// The personal file failed to read, with the reason.
    BaseUnreadable(String),
    /// The leaf is missing from the base file, is a bad index, or holds no
    /// editable value (group, list, text) — with `read_leaf`'s reason, which
    /// already names fighter, spelling, and leaf kind.
    LeafUnreadable(String),
    /// The text does not parse as the leaf's type.
    ValueInvalid {
        spelling: String,
        expected: &'static str,
        text: String,
    },
}

impl DraftProblem {
    /// The line the card shows while this problem blocks saving.
    pub fn describe(&self) -> String {
        match self {
            Self::FighterInvalid(reason) => format!("Pick a fighter first: {reason}."),
            Self::PathInvalid(reason) => format!("That path does not parse: {reason}."),
            Self::PathUnresolved(spelling) => format!(
                "{spelling} has an unresolved 0x segment — resolve every label via \
                 ParamLabels before saving; an unknown field cannot silently export."
            ),
            Self::BaseMissing(fighter) => format!(
                "No fighter/{fighter}/param/vl.prc under any data root, so the leaf type \
                 is unknown and nothing can be saved. Dump fighter/ to enable this card."
            ),
            Self::BaseUnreadable(reason) => {
                format!("The personal file failed to read, so nothing can be saved: {reason}.")
            }
            Self::LeafUnreadable(reason) => format!("{reason}."),
            Self::ValueInvalid {
                spelling,
                expected,
                text,
            } => format!("{spelling} holds {expected}, so {text:?} cannot be saved there."),
        }
    }
}

/// Run every save gate over a draft. `Ok` means the Save control may commit;
/// `Err` is the exact reason shown while it stays disabled.
pub fn validate(base: &BaseState, draft: &PersonalDraft) -> Result<ResolvedEdit, DraftProblem> {
    let fighter = draft.fighter.trim();
    if let Err(error) = personal_path_for(fighter) {
        return Err(DraftProblem::FighterInvalid(error.to_string()));
    }
    let path = NestedPath::parse(draft.path.trim())
        .map_err(|error| DraftProblem::PathInvalid(error.to_string()))?;
    let spelling = path.to_string();
    if !path_is_resolved(&path) {
        return Err(DraftProblem::PathUnresolved(spelling));
    }
    let tree = match base {
        BaseState::Missing => return Err(DraftProblem::BaseMissing(fighter.to_string())),
        BaseState::Unreadable(reason) => return Err(DraftProblem::BaseUnreadable(reason.clone())),
        BaseState::Present(tree) => tree,
    };
    let leaf_base = read_leaf(tree, fighter, &path)
        .map_err(|error| DraftProblem::LeafUnreadable(error.to_string()))?;
    let text = draft.value.trim();
    let value = parse_value(text, leaf_base).ok_or(DraftProblem::ValueInvalid {
        spelling,
        expected: value_kind_name(&leaf_base),
        text: text.to_string(),
    })?;
    Ok(ResolvedEdit {
        fighter: fighter.to_string(),
        path,
        value,
        base: leaf_base,
    })
}

/// Commit a validated edit into one fighter's sparse params. The base travels
/// along so returning to the game's own value clears the override instead of
/// pinning it (see `record_personal_edit`).
pub fn commit(params: &mut ParamMod, edit: &ResolvedEdit) -> anyhow::Result<()> {
    record_personal_edit(
        params,
        &edit.fighter,
        &edit.path,
        edit.value,
        Some(edit.base),
    )
}

/// Parse typed text into the type the leaf already holds.
///
/// The base value decides the type rather than the text: typing `3` into a
/// decimal leaf means `3.0`, not an integer a later write would have to guess
/// about. Same contract as the traits editor's field parser.
fn parse_value(text: &str, base: ParamValue) -> Option<ParamValue> {
    let text = text.trim();
    Some(match base {
        ParamValue::Bool(_) => ParamValue::Bool(match text.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => true,
            "false" | "no" | "off" | "0" => false,
            _ => return None,
        }),
        ParamValue::Float(_) => ParamValue::Float(text.parse().ok()?),
        ParamValue::I8(_) => ParamValue::I8(text.parse().ok()?),
        ParamValue::U8(_) => ParamValue::U8(text.parse().ok()?),
        ParamValue::I16(_) => ParamValue::I16(text.parse().ok()?),
        ParamValue::U16(_) => ParamValue::U16(text.parse().ok()?),
        ParamValue::I32(_) => ParamValue::I32(text.parse().ok()?),
        ParamValue::U32(_) => ParamValue::U32(text.parse().ok()?),
        ParamValue::Hash(_) => {
            let raw = text.strip_prefix("0x").unwrap_or(text);
            ParamValue::Hash(u64::from_str_radix(raw, 16).ok()?)
        }
    })
}

/// How a stored or base value is shown.
fn render(value: ParamValue) -> String {
    match value {
        ParamValue::Bool(flag) => flag.to_string(),
        ParamValue::Float(number) => {
            if number.fract() == 0.0 {
                format!("{number:.1}")
            } else {
                format!("{number}")
            }
        }
        ParamValue::I8(number) => number.to_string(),
        ParamValue::U8(number) => number.to_string(),
        ParamValue::I16(number) => number.to_string(),
        ParamValue::U16(number) => number.to_string(),
        ParamValue::I32(number) => number.to_string(),
        ParamValue::U32(number) => number.to_string(),
        ParamValue::Hash(raw) => format!("{raw:#x}"),
    }
}

/// One validated save or reset, collected during the draw and applied after:
/// the rows borrow the fighters map while the card needs `&mut self`.
enum Commit {
    Save(ResolvedEdit),
    ClearOne { fighter: String, spelling: String },
    ClearAll { fighter: String },
}

/// The tunables card. Owns its drafts, the loaded base tree, and a status
/// line; the project map arrives each frame like the traits editor's.
#[derive(Default)]
pub struct PersonalView {
    draft: PersonalDraft,
    /// The fighter the base tree was loaded for. Reloads when the selection
    /// moves to a different fighter, and clears the path/value drafts with it.
    loaded_fighter: String,
    /// Roots used for the cached base. Changing the configured data/export
    /// folder must retry a previously missing fighter without requiring a new
    /// selection.
    loaded_roots: Vec<PathBuf>,
    discovered_fighters: Vec<String>,
    base: Option<BaseState>,
    status: String,
    selection_locked: bool,
}

impl PersonalView {
    /// Use the main editor's fighter selection, never a second independent selector.
    pub fn ui_for_fighter(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        roots: &[PathBuf],
        labels: &HashMap<u64, String>,
        fighters: &mut BTreeMap<String, ParamMod>,
    ) {
        self.draft.fighter = fighter.to_string();
        self.selection_locked = true;
        self.ui(ui, roots, labels, fighters);
        self.selection_locked = false;
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        roots: &[PathBuf],
        labels: &HashMap<u64, String>,
        fighters: &mut BTreeMap<String, ParamMod>,
    ) {
        if self.loaded_roots != roots {
            self.loaded_roots = roots.to_vec();
            self.discovered_fighters = available_fighters(roots);
            self.base = None;
        }
        let mut fighter_choices = self.discovered_fighters.clone();
        for fighter in fighters.keys() {
            if !fighter_choices.contains(fighter) {
                fighter_choices.push(fighter.clone());
            }
        }
        fighter_choices.sort();
        if self.draft.fighter.is_empty() {
            self.draft.fighter = fighter_choices
                .iter()
                .find(|fighter| fighter.as_str() == "cloud")
                .or_else(|| fighter_choices.first())
                .cloned()
                .unwrap_or_default();
        }
        // Header: fighter name plus the live personal-path preview.
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Fighter").small().strong());
                    ui.add_enabled_ui(
                        !self.selection_locked && !fighter_choices.is_empty(),
                        |ui| {
                            egui::ComboBox::from_id_salt("gimmick_personal_fighter")
                                .selected_text(if self.draft.fighter.is_empty() {
                                    "No fighters with personal values"
                                } else {
                                    self.draft.fighter.as_str()
                                })
                                .show_ui(ui, |ui| {
                                    for fighter in &fighter_choices {
                                        ui.selectable_value(
                                            &mut self.draft.fighter,
                                            fighter.clone(),
                                            fighter,
                                        );
                                    }
                                });
                        },
                    );
                    let fighter = self.draft.fighter.trim().to_string();
                    if fighter != self.loaded_fighter {
                        self.loaded_fighter = fighter;
                        self.base = None;
                        self.draft.path.clear();
                        self.draft.value.clear();
                        self.status.clear();
                    }
                    match personal_path_for(self.draft.fighter.trim()) {
                        Ok(rel) => {
                            ui.label(RichText::new(rel).small().weak().monospace());
                        }
                        Err(error) => {
                            ui.colored_label(
                                Color32::from_rgb(240, 180, 120),
                                RichText::new(error.to_string()).small(),
                            );
                        }
                    }
                });
            });
        ui.add_space(6.0);

        if self.base.is_none() {
            self.base = Some(load_base(roots, self.draft.fighter.trim()));
        }
        // Lifted out of `self` for the duration of the draw and put back below:
        // the form needs `&mut self` for its text drafts and the base tree at
        // the same time, as in the traits editor.
        let Some(base) = self.base.take() else {
            return;
        };

        match &base {
            BaseState::Missing => {
                self.draw_missing_file(ui);
            }
            BaseState::Unreadable(reason) => {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(Color32::from_rgb(240, 120, 120), "✘");
                            ui.label(RichText::new(reason.clone()).small().weak());
                        });
                    });
            }
            BaseState::Present(_) => {}
        }

        let mut commits: Vec<Commit> = Vec::new();
        // The add form stays visible in every base state so the blocker is
        // always named next to the fields; Save is gated on `validate`.
        self.draw_add_form(ui, &base, labels, &mut commits);

        let fighter = self.draft.fighter.trim().to_string();
        if personal_path_for(&fighter).is_ok() {
            self.draw_stored_edits(ui, &base, &fighter, fighters, &mut commits);
        }

        for action in commits {
            match action {
                Commit::Save(edit) => {
                    let spelling = edit.path.to_string();
                    match commit(fighters.entry(edit.fighter.clone()).or_default(), &edit) {
                        Ok(()) => {
                            self.status = format!(
                                "{}: {} saved as {} ({}).",
                                edit.fighter,
                                spelling,
                                render(edit.value),
                                value_kind_name(&edit.base)
                            );
                            self.draft.value.clear();
                        }
                        Err(error) => {
                            self.status = format!("Could not save {spelling}: {error:#}.");
                        }
                    }
                }
                Commit::ClearOne { fighter, spelling } => {
                    if let Ok(key) = personal_path_for(&fighter) {
                        if let Some(entry) = fighters.get_mut(&fighter) {
                            if let Some(file) = entry.files.get_mut(&key) {
                                file.remove(&spelling);
                                if file.is_empty() {
                                    entry.files.remove(&key);
                                }
                            }
                        }
                    }
                    self.status = format!("{fighter}: {spelling} returned to the game's value.");
                }
                Commit::ClearAll { fighter } => {
                    if let Ok(key) = personal_path_for(&fighter) {
                        if let Some(entry) = fighters.get_mut(&fighter) {
                            entry.files.remove(&key);
                        }
                    }
                    self.status = format!("{fighter} returned to the game's values.");
                }
            }
        }
        self.base = Some(base);

        if !self.status.is_empty() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(120, 185, 235), "●");
                ui.label(RichText::new(&self.status).small().weak());
            });
        }
    }

    fn draw_missing_file(&self, ui: &mut Ui) {
        ui.add_space(2.0);
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        Color32::from_rgb(230, 180, 80),
                        RichText::new("⚠").size(18.0),
                    );
                    ui.label(RichText::new("No personal values file found").heading());
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Personal tunables live in one file per fighter, \
                         fighter/<name>/param/vl.prc. Dump the fighter/ folder to add \
                         values — stored edits below are kept, but no new value can be \
                         saved without the base leaf to coerce to.",
                    )
                    .small(),
                );
            });
        ui.add_space(6.0);
    }

    fn draw_add_form(
        &mut self,
        ui: &mut Ui,
        base: &BaseState,
        labels: &HashMap<u64, String>,
        commits: &mut Vec<Commit>,
    ) {
        let paths = match base {
            BaseState::Present(tree) => editable_paths(tree, labels),
            _ => Vec::new(),
        };
        if !self.draft.path.is_empty() && !paths.contains(&self.draft.path) {
            self.draft.path.clear();
            self.draft.value.clear();
        }
        let verdict = validate(base, &self.draft);
        // A readable leaf preview: the expected type and base value beside the
        // value field, so the no-type-picker rule is visible, not implied.
        let preview = match base {
            BaseState::Present(tree) => NestedPath::parse(self.draft.path.trim())
                .ok()
                .filter(path_is_resolved)
                .and_then(|path| read_leaf(tree, self.draft.fighter.trim(), &path).ok()),
            _ => None,
        };
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.label(RichText::new("Add a personal value").strong());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Path").small().strong());
                    ui.add_enabled_ui(!paths.is_empty(), |ui| {
                        egui::ComboBox::from_id_salt("gimmick_personal_path")
                            .selected_text(if self.draft.path.is_empty() {
                                if matches!(base, BaseState::Present(_)) {
                                    "Choose a parameter…"
                                } else {
                                    "Personal values unavailable"
                                }
                            } else {
                                self.draft.path.as_str()
                            })
                            .show_ui(ui, |ui| {
                                for path in &paths {
                                    ui.selectable_value(
                                        &mut self.draft.path,
                                        path.clone(),
                                        RichText::new(path).monospace(),
                                    );
                                }
                            });
                    });
                    if matches!(base, BaseState::Present(_)) && paths.is_empty() {
                        ui.colored_label(
                            Color32::from_rgb(240, 180, 120),
                            RichText::new("No labeled scalar parameters found").small(),
                        );
                    }
                });
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Value").small().strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.draft.value)
                            .desired_width(110.0)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("0.667"),
                    );
                    if let Some(leaf) = preview {
                        ui.label(
                            RichText::new(format!(
                                "expects {} • base {}",
                                value_kind_name(&leaf),
                                render(leaf)
                            ))
                            .small()
                            .weak(),
                        );
                    }
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    let save_button =
                        egui::Button::new(RichText::new("  Save personal edit  ").small());
                    if self.draft.path.is_empty() || self.draft.value.trim().is_empty() {
                        ui.add_enabled(false, save_button);
                        let guidance = match base {
                            BaseState::Present(_) if paths.is_empty() => None,
                            BaseState::Present(_) if self.draft.path.is_empty() => {
                                Some("Choose a parameter to edit.")
                            }
                            BaseState::Present(_) => Some("Enter the new value."),
                            BaseState::Missing | BaseState::Unreadable(_) => None,
                        };
                        if let Some(guidance) = guidance {
                            ui.label(RichText::new(guidance).small().weak());
                        }
                    } else {
                        match verdict {
                            Ok(edit) => {
                                if ui.add_enabled(true, save_button).clicked() {
                                    commits.push(Commit::Save(edit));
                                }
                            }
                            Err(problem) => {
                                ui.add_enabled(false, save_button);
                                ui.colored_label(
                                    Color32::from_rgb(240, 150, 130),
                                    RichText::new(problem.describe()).small(),
                                );
                            }
                        }
                    }
                });
            });
        ui.add_space(6.0);
    }

    fn draw_stored_edits(
        &self,
        ui: &mut Ui,
        base: &BaseState,
        fighter: &str,
        fighters: &BTreeMap<String, ParamMod>,
        commits: &mut Vec<Commit>,
    ) {
        let stored: Vec<(String, ParamValue)> = fighters
            .get(fighter)
            .map(|entry| {
                personal_edits(entry, fighter)
                    .iter()
                    .map(|(spelling, value)| (spelling.clone(), *value))
                    .collect()
            })
            .unwrap_or_default();
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Stored personal edits").strong());
                    if stored.is_empty() {
                        ui.label(RichText::new("No changes").small().weak());
                    } else {
                        ui.colored_label(
                            Color32::from_rgb(240, 200, 120),
                            format!(
                                "{count} change{s}",
                                count = stored.len(),
                                s = if stored.len() == 1 { "" } else { "s" }
                            ),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button(RichText::new("  ↺ Reset all  ").small())
                                .on_hover_text("Return every personal value to the game's own")
                                .clicked()
                            {
                                commits.push(Commit::ClearAll {
                                    fighter: fighter.to_string(),
                                });
                            }
                        });
                    }
                });
                for (spelling, value) in stored {
                    ui.add_space(2.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&spelling).small().strong().monospace());
                        ui.label(
                            RichText::new(render(value))
                                .small()
                                .monospace()
                                .color(Color32::from_rgb(240, 200, 120)),
                        );
                        if let BaseState::Present(tree) = base {
                            if let Ok(path) = NestedPath::parse(&spelling) {
                                if let Ok(leaf) = read_leaf(tree, fighter, &path) {
                                    ui.label(
                                        RichText::new(format!("was {}", render(leaf)))
                                            .small()
                                            .weak(),
                                    );
                                }
                            }
                        }
                        if ui
                            .small_button(RichText::new(" ↺ ").small())
                            .on_hover_text("Return to the game's value")
                            .clicked()
                        {
                            commits.push(Commit::ClearOne {
                                fighter: fighter.to_string(),
                                spelling,
                            });
                        }
                    });
                }
            });
    }
}

/// Enumerate only scalar leaves that can be safely represented by the stable
/// dotted project spelling. Unknown hashes are omitted instead of becoming a
/// free-form path the user has to guess.
pub(crate) fn editable_paths(
    root: &prc::ParamStruct,
    labels: &HashMap<u64, String>,
) -> Vec<String> {
    fn walk_struct(
        tree: &prc::ParamStruct,
        prefix: &str,
        labels: &HashMap<u64, String>,
        paths: &mut Vec<String>,
    ) {
        for (hash, kind) in &tree.0 {
            let Some(label) = labels.get(&hash.0) else {
                continue;
            };
            let path = if prefix.is_empty() {
                label.clone()
            } else {
                format!("{prefix}.{label}")
            };
            walk_kind(kind, &path, labels, paths);
        }
    }

    fn walk_kind(
        kind: &ParamKind,
        path: &str,
        labels: &HashMap<u64, String>,
        paths: &mut Vec<String>,
    ) {
        if to_param_value(kind).is_some() {
            if NestedPath::parse(path).is_ok_and(|path| path_is_resolved(&path)) {
                paths.push(path.to_string());
            }
            return;
        }
        match kind {
            ParamKind::Struct(tree) => walk_struct(tree, path, labels, paths),
            ParamKind::List(list) => {
                for (index, item) in list.0.iter().enumerate() {
                    // Nested lists cannot be represented by the one-index-per-segment
                    // project grammar, so omit them rather than inventing a path.
                    if !matches!(item, ParamKind::List(_)) {
                        walk_kind(item, &format!("{path}[{index}]"), labels, paths);
                    }
                }
            }
            _ => {}
        }
    }

    let mut paths = Vec::new();
    walk_struct(root, "", labels, &mut paths);
    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::params::save_personal;
    use prc::hash40::Hash40;
    use prc::{ParamKind, ParamList, ParamStruct};

    fn field(name: &str, kind: ParamKind) -> (Hash40, ParamKind) {
        (Hash40(hash40::hash40(name).0), kind)
    }

    /// A compact Cloud-flavored subtree: one indexed group, scalar leaves of
    /// several kinds, a text leaf with no editable value, and a nested group.
    fn cloud_tree() -> ParamStruct {
        ParamStruct(vec![
            field(
                "param_special_lw",
                ParamKind::List(ParamList(vec![ParamKind::Struct(ParamStruct(vec![
                    field("limit_gauge_add", ParamKind::Float(0.5)),
                    field("limit_max", ParamKind::I32(100)),
                    field("charge_enabled", ParamKind::Bool(true)),
                ]))])),
            ),
            field("walk_speed_mul", ParamKind::Float(1.15)),
            field("display_name", ParamKind::Str("Cloud".to_string())),
            field(
                "nested_group",
                ParamKind::Struct(ParamStruct(vec![field(
                    "inner_float",
                    ParamKind::Float(2.0),
                )])),
            ),
        ])
    }

    fn draft(fighter: &str, path: &str, value: &str) -> PersonalDraft {
        PersonalDraft {
            fighter: fighter.to_string(),
            path: path.to_string(),
            value: value.to_string(),
        }
    }

    fn present() -> BaseState {
        BaseState::Present(cloud_tree())
    }

    #[test]
    fn editable_path_selector_lists_only_named_scalar_leaves() {
        let labels = [
            "param_special_lw",
            "limit_gauge_add",
            "limit_max",
            "charge_enabled",
            "walk_speed_mul",
            "display_name",
            "nested_group",
            "inner_float",
        ]
        .into_iter()
        .map(|label| (hash40::hash40(label).0, label.to_string()))
        .collect();
        assert_eq!(
            editable_paths(&cloud_tree(), &labels),
            vec![
                "nested_group.inner_float",
                "param_special_lw[0].charge_enabled",
                "param_special_lw[0].limit_gauge_add",
                "param_special_lw[0].limit_max",
                "walk_speed_mul",
            ]
        );

        let mut missing_label = labels;
        missing_label.remove(&hash40::hash40("limit_gauge_add").0);
        assert!(!editable_paths(&cloud_tree(), &missing_label)
            .iter()
            .any(|path| path.contains("limit_gauge_add")));
    }

    /// Every save gate fires with a reason naming what failed — nothing is
    /// silently dropped, and a failing draft never reaches the project.
    #[test]
    fn each_save_gate_blocks_with_a_reason_naming_what_failed() {
        let present = present();
        let missing = BaseState::Missing;
        let unreadable = BaseState::Unreadable("denied".into());
        for (fighter, path, value, base, expected) in [
            ("", "walk_speed_mul", "1.0", &present, "fighter"),
            ("cloud/x", "walk_speed_mul", "1.0", &present, "cloud/x"),
            (
                "cloud",
                "param_special_lw[x].limit_gauge_add",
                "1.0",
                &present,
                "does not parse",
            ),
            ("cloud", "", "1.0", &present, "does not parse"),
            (
                "cloud",
                "param_special_lw[0].0x1234abcd",
                "1.0",
                &present,
                "unresolved 0x",
            ),
            ("cloud", "walk_speed_mul", "1.0", &missing, "vl.prc"),
            ("cloud", "walk_speed_mul", "1.0", &unreadable, "denied"),
            ("cloud", "no_such_leaf", "1.0", &present, "no_such_leaf"),
            (
                "cloud",
                "param_special_lw[3].limit_gauge_add",
                "1.0",
                &present,
                "param_special_lw[3]",
            ),
            (
                "cloud",
                "nested_group",
                "1.0",
                &present,
                "holds no editable value",
            ),
            (
                "cloud",
                "display_name",
                "Sephiroth",
                &present,
                "holds no editable value",
            ),
            ("cloud", "walk_speed_mul", "fast", &present, "holds float"),
            (
                "cloud",
                "param_special_lw[0].limit_max",
                "1.5",
                &present,
                "holds i32",
            ),
        ] {
            let error = validate(base, &draft(fighter, path, value))
                .unwrap_err()
                .describe();
            assert!(error.contains(expected), "{error}");
        }
    }

    /// A valid draft resolves against the base tree: the leaf's own type wins,
    /// so `3` into a decimal leaf is `3.0` with no type picker involved.
    #[test]
    fn a_valid_draft_resolves_fighter_path_value_and_base() {
        let edit = validate(
            &present(),
            &draft("cloud", "param_special_lw[0].limit_gauge_add", "0.667"),
        )
        .expect("a resolved float leaf with float text saves");
        assert_eq!(edit.fighter, "cloud");
        assert_eq!(edit.path.to_string(), "param_special_lw[0].limit_gauge_add");
        assert_eq!(edit.value, ParamValue::Float(0.667));
        assert_eq!(edit.base, ParamValue::Float(0.5));

        let edit = validate(
            &present(),
            &draft("cloud", "param_special_lw[0].limit_max", "120"),
        )
        .expect("an int leaf with int text saves");
        assert_eq!(edit.value, ParamValue::I32(120));
        assert_eq!(edit.base, ParamValue::I32(100));

        let edit = validate(&present(), &draft("cloud", "walk_speed_mul", "3"))
            .expect("a whole number into a decimal leaf is a float, not an int");
        assert_eq!(edit.value, ParamValue::Float(3.0));
    }

    /// Typed text takes the type of the leaf it is typed into — the contract
    /// that lets the card omit a type picker.
    #[test]
    fn typed_text_takes_the_type_of_the_leaf_it_is_typed_into() {
        assert_eq!(
            parse_value("3", ParamValue::Float(1.0)),
            Some(ParamValue::Float(3.0))
        );
        assert_eq!(
            parse_value("3", ParamValue::I32(1)),
            Some(ParamValue::I32(3))
        );
        assert_eq!(parse_value("3.5", ParamValue::I32(1)), None);
        assert_eq!(
            parse_value("yes", ParamValue::Bool(false)),
            Some(ParamValue::Bool(true))
        );
        assert_eq!(parse_value("maybe", ParamValue::Bool(true)), None);
        assert_eq!(
            parse_value("0xdeadbeef", ParamValue::Hash(0)),
            Some(ParamValue::Hash(0xdeadbeef))
        );
        assert_eq!(parse_value("zz", ParamValue::Hash(0)), None);
    }

    /// Committing stores the sparse edit, and committing the base value back
    /// clears it — the same clear-on-agree the traits editor keeps.
    #[test]
    fn commit_records_and_clears_on_agree() {
        let mut params = ParamMod::default();
        let edit = validate(
            &present(),
            &draft("cloud", "param_special_lw[0].limit_gauge_add", "0.667"),
        )
        .unwrap();
        commit(&mut params, &edit).unwrap();
        assert_eq!(
            personal_edits(&params, "cloud")["param_special_lw[0].limit_gauge_add"],
            ParamValue::Float(0.667)
        );
        let revert = ResolvedEdit {
            value: ParamValue::Float(0.5),
            ..edit
        };
        commit(&mut params, &revert).unwrap();
        assert!(personal_edits(&params, "cloud").is_empty());
        assert!(params.is_empty());
    }

    /// The card finds the personal file across roots, reports a missing one as
    /// missing, and an unreadable one with its reason.
    #[test]
    fn locate_and_load_cover_present_missing_and_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let rel = "fighter/cloud/param/vl.prc";
        save_personal(&cloud_tree(), &dir.path().join(rel), "cloud").unwrap();

        let roots = [dir.path().to_path_buf()];
        std::fs::create_dir_all(dir.path().join("fighter/mario/param")).unwrap();
        std::fs::write(
            dir.path().join("fighter/mario/param/vl.prc"),
            b"not a prc file",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("fighter/no_personal/param")).unwrap();
        assert_eq!(
            available_fighters(&roots),
            vec!["cloud".to_string(), "mario".to_string()]
        );
        assert_eq!(locate_personal(&roots, "cloud"), Some(dir.path().join(rel)));
        assert!(matches!(load_base(&roots, "cloud"), BaseState::Present(_)));
        assert!(matches!(load_base(&roots, "sephiroth"), BaseState::Missing));
        assert_eq!(locate_personal(&roots, ""), None);
        assert_eq!(locate_personal(&[], "cloud"), None);

        std::fs::write(dir.path().join(rel), b"not a prc file").unwrap();
        assert!(matches!(
            load_base(&roots, "cloud"),
            BaseState::Unreadable(_)
        ));
    }

    /// A value survives being shown and typed back unchanged, or a stored edit
    /// would read as changed the moment the card renders it.
    #[test]
    fn every_value_type_round_trips_through_its_own_display() {
        let values = [
            ParamValue::Float(98.0),
            ParamValue::Float(0.667),
            ParamValue::I32(-3),
            ParamValue::I8(-128),
            ParamValue::U8(255),
            ParamValue::I16(-32768),
            ParamValue::U16(65535),
            ParamValue::U32(4_000_000_000),
            ParamValue::Bool(true),
            ParamValue::Hash(0x1234abcd),
        ];
        for value in values {
            assert_eq!(parse_value(&render(value), value), Some(value), "{value:?}");
        }
    }
}
