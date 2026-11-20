//! The New character page: pick the fighter it plays as, name it, create it.
//!
//! A new character is a set of costumes of a base fighter with its own model, animations,
//! effects, sounds, name, moves and entry on the select screen. Once made it is a card on the
//! Project page like any other character; this page only makes them.

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Color32, RichText, Ui};

use crate::mod_project::{AuthoredEntry, RosterMod};
use crate::ui::{act, look, Look};

use super::index::RosterIndex;
use super::library::{ModLibrary, ModSource};
use super::{scaffold, RosterKey};

/// The select screen gives one character at most eight colours.
const MAX_SKINS: u8 = 8;

/// Inclusive `[start, end]` costume range: `(8, 15)` is eight skins on c08 to c15.
type SlotRange = (u8, u8);

fn range_slots(range: SlotRange) -> Vec<u8> {
    (range.0..=range.1).collect()
}

fn range_count(range: SlotRange) -> u8 {
    range.1.saturating_sub(range.0).saturating_add(1)
}

#[derive(Default)]
pub struct NewCharacterView {
    /// The project's folder. A new character's files go inside it, so there has to be one.
    pub project_dir: Option<PathBuf>,
    donor: Option<String>,
    /// The costumes it takes: the lowest free run once a fighter is picked.
    slot_range: Option<SlotRange>,
    show_slot_map: bool,
    display_name: String,
    search: String,
    /// A folder picked for its files in place of the project's.
    folder: Option<PathBuf>,
    status: String,
    error: Option<String>,
}

/// What the page wants done after it draws. The window owns every change to the library and
/// the project.
pub enum NewCharacterAction {
    None,
    /// Send move edits to this character's costumes, or (with `None`) back to the whole fighter.
    SetEditTarget(Option<RosterKey>),
    Create {
        donor: String,
        /// Every costume this character takes, ascending.
        slots: Vec<u8>,
        display_name: String,
        name_id: String,
        destination: PathBuf,
    },
    Remove(RosterKey),
}

impl NewCharacterView {
    /// `faces` are the fighters' portraits by fighter name.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        roots: &[PathBuf],
        index: &RosterIndex,
        project: &RosterMod,
        faces: &HashMap<String, egui::TextureHandle>,
    ) -> NewCharacterAction {
        let look = look(ui);
        let mut action = NewCharacterAction::None;
        // Side by side when there is room, stacked when there is not.
        if ui.available_width() >= 860.0 {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(380.0);
                    action = self.draw_form(ui, look, roots, index, project, faces);
                });
                ui.add_space(18.0);
                ui.vertical(|ui| self.draw_fighters(ui, look, roots, index, faces));
            });
        } else {
            action = self.draw_form(ui, look, roots, index, project, faces);
            ui.add_space(14.0);
            self.draw_fighters(ui, look, roots, index, faces);
        }
        action
    }

    fn draw_form(
        &mut self,
        ui: &mut Ui,
        look: Look,
        roots: &[PathBuf],
        index: &RosterIndex,
        project: &RosterMod,
        faces: &HashMap<String, egui::TextureHandle>,
    ) -> NewCharacterAction {
        let base = self.donor.as_deref().map(crate::data::fighter_display_name);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 96.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 10.0, look.sunk);
            if let Some(face) = self.donor.as_ref().and_then(|donor| faces.get(donor)) {
                egui::Image::new(face)
                    .corner_radius(10.0)
                    .paint_at(ui, rect);
            }
            ui.painter().rect_stroke(
                rect,
                10.0,
                egui::Stroke::new(1.5, look.edge),
                egui::StrokeKind::Inside,
            );
            ui.add_space(8.0);
            ui.vertical(|ui| {
                let name = self.display_name.trim();
                ui.label(
                    RichText::new(if name.is_empty() {
                        "New character"
                    } else {
                        name
                    })
                    .size(26.0)
                    .strong()
                    .color(look.text),
                );
                if let Some(base) = &base {
                    ui.label(
                        RichText::new(format!("Plays as {base}"))
                            .size(15.0)
                            .color(look.dim),
                    );
                }
            });
        });
        ui.add_space(10.0);

        let field = |ui: &mut Ui, label: &str| {
            ui.label(RichText::new(label).strong().color(look.dim));
        };
        field(ui, "Name");
        ui.add(egui::TextEdit::singleline(&mut self.display_name).desired_width(f32::INFINITY));
        ui.add_space(8.0);

        field(ui, "Skins");
        let mut range = self.slot_range.map(clamp_range);
        ui.horizontal(|ui| {
            let mut count = range.map_or(MAX_SKINS, range_count).clamp(1, MAX_SKINS);
            let changed = ui
                .add(
                    egui::DragValue::new(&mut count)
                        .range(1..=MAX_SKINS)
                        .speed(0.05),
                )
                .changed();
            if let Some(range) = &mut range {
                if changed {
                    range.1 = range.0.saturating_add(count - 1);
                }
                ui.label(
                    RichText::new(format!("costumes c{:02} to c{:02}", range.0, range.1))
                        .color(look.text),
                );
                if act(
                    ui,
                    look,
                    if self.show_slot_map { "Done" } else { "Change" },
                    None,
                )
                .clicked()
                {
                    self.show_slot_map = !self.show_slot_map;
                }
            }
        });
        if range.is_some() {
            self.slot_range = range;
        }
        let mut blocked = None;
        if let (Some(donor), Some(range)) = (self.donor.clone(), self.slot_range) {
            let statuses = slot_statuses(roots, index, &donor);
            let taken = conflicts_in_range(&statuses, range);
            if !taken.is_empty() {
                let line = format!("Already in use: {}", summarize_taken(&taken));
                ui.label(RichText::new(&line).color(Color32::from_rgb(240, 190, 90)));
                blocked = Some("Pick costumes that are free.");
                if let Some(next) = next_free_run_after(&statuses, range_count(range), range.0) {
                    if act(ui, look, "Use the next free ones", None).clicked() {
                        self.slot_range = Some(next);
                    }
                }
            }
            if self.show_slot_map {
                self.draw_slot_map(ui, look, &statuses, range);
            }
        }
        ui.add_space(8.0);

        field(ui, "Files");
        let home = self
            .folder
            .clone()
            .or_else(|| self.project_dir.as_ref().map(|dir| dir.join("characters")));
        ui.horizontal_wrapped(|ui| match &home {
            Some(home) => {
                // The character's own folder appears once it has a name.
                let name = self.display_name.trim();
                let shown = if name.is_empty() {
                    home.clone()
                } else {
                    home.join(crate::mod_export::slugify(name))
                };
                ui.label(
                    RichText::new(shown.display().to_string())
                        .monospace()
                        .color(look.text),
                );
            }
            None => {
                ui.label(RichText::new("Save the project first (File, Save as).").color(look.dim));
            }
        });
        ui.horizontal(|ui| {
            if act(ui, look, "Choose folder", None).clicked() {
                if let Some(picked) = rfd::FileDialog::new()
                    .set_title("Folder for this character's files")
                    .pick_folder()
                {
                    self.folder = Some(picked);
                }
            }
            if self.folder.is_some() && act(ui, look, "Use project folder", None).clicked() {
                self.folder = None;
            }
        });
        ui.add_space(12.0);

        let reason = if home.is_none() {
            Some("Save the project first.")
        } else if self.donor.is_none() {
            Some("Pick a fighter.")
        } else if self.display_name.trim().is_empty() {
            Some("Give it a name.")
        } else {
            blocked
        };
        let mut action = NewCharacterAction::None;
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(reason.is_none(), |ui| {
                let create = act(ui, look, "Create character", Some(look.accent))
                    .on_hover_text(
                        "Copies the base fighter's model, animations, effects, sounds and portraits into the character's own folder. Its own place on the select screen needs the CSK Collection plugin in the game.",
                    )
                    .on_disabled_hover_text(reason.unwrap_or_default());
                if create.clicked() {
                    if let Some(home) = &home {
                        action = self.build_create_action(project, home.clone());
                    }
                }
            });
        });
        if let Some(error) = &self.error {
            ui.add_space(8.0);
            ui.label(RichText::new(error).color(Color32::from_rgb(255, 120, 120)));
        } else if !self.status.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(&self.status).color(look.text));
        }
        action
    }

    /// Every fighter as a portrait tile, as many to a row as fit.
    fn draw_fighters(
        &mut self,
        ui: &mut Ui,
        look: Look,
        roots: &[PathBuf],
        index: &RosterIndex,
        faces: &HashMap<String, egui::TextureHandle>,
    ) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Plays as")
                    .size(21.0)
                    .strong()
                    .color(look.text),
            );
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .desired_width(220.0)
                    .hint_text("Find a fighter"),
            );
        });
        ui.add_space(6.0);
        let wanted = self.search.trim().to_lowercase();
        let fighters: Vec<(&str, &str)> = index
            .sorted()
            .into_iter()
            .filter(|entry| !entry.backing.shares_engine_fighter())
            .filter_map(|entry| Some((entry.fighter.as_deref()?, entry.display_name.as_str())))
            .filter(|(fighter, name)| {
                wanted.is_empty()
                    || name.to_lowercase().contains(&wanted)
                    || fighter.contains(&wanted)
            })
            .collect();
        // Tiles share the row exactly, about 104 wide each.
        let across = ((ui.available_width() + 8.0) / (104.0 + 8.0))
            .floor()
            .max(1.0);
        let tile_w = ((ui.available_width() + 8.0) / across - 8.0).floor();
        let tile = egui::vec2(tile_w, tile_w + 24.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            for (fighter, name) in fighters {
                let picked = self.donor.as_deref() == Some(fighter);
                let (rect, response) = ui.allocate_exact_size(tile, egui::Sense::click());
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let painter = ui.painter();
                painter.rect_filled(rect, 10.0, look.sunk);
                let face = egui::Rect::from_min_size(rect.min, egui::vec2(tile.x, tile.x));
                if let Some(texture) = faces.get(fighter) {
                    egui::Image::new(texture)
                        .corner_radius(10.0)
                        .paint_at(ui, face);
                }
                let edge = if picked {
                    look.accent
                } else if response.hovered() {
                    look.text
                } else {
                    look.edge
                };
                painter.rect_stroke(
                    rect,
                    10.0,
                    egui::Stroke::new(if picked { 3.0 } else { 1.5 }, edge),
                    egui::StrokeKind::Inside,
                );
                let label = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 4.0, face.bottom()),
                    rect.max - egui::vec2(4.0, 0.0),
                );
                let galley = painter.layout_no_wrap(
                    name.to_owned(),
                    egui::FontId::proportional(13.0),
                    look.text,
                );
                let at = egui::pos2(
                    label.center().x - galley.size().x.min(label.width()) / 2.0,
                    label.center().y - galley.size().y / 2.0,
                );
                painter.with_clip_rect(label).galley(at, galley, look.text);
                if response.on_hover_text(name).clicked() && !picked {
                    self.donor = Some(fighter.to_owned());
                    let width = self
                        .slot_range
                        .map_or(MAX_SKINS, range_count)
                        .clamp(1, MAX_SKINS);
                    self.slot_range = Some(
                        lowest_free_run(roots, index, fighter, width).unwrap_or((8, 7 + width)),
                    );
                    self.show_slot_map = false;
                    self.error = None;
                }
            }
        });
    }

    /// Every costume of the fighter, eight to a row. Clicking a free one moves the block there.
    fn draw_slot_map(
        &mut self,
        ui: &mut Ui,
        look: Look,
        statuses: &[SlotStatus],
        range: SlotRange,
    ) {
        let count = range_count(range);
        let map_end = range.1.saturating_add(8).clamp(31, 63) as usize;
        egui::Frame::new()
            .fill(look.sunk)
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.style_mut().spacing.item_spacing = egui::vec2(4.0, 4.0);
                for row in statuses
                    .iter()
                    .take(map_end + 1)
                    .enumerate()
                    .collect::<Vec<_>>()
                    .chunks(8)
                {
                    ui.horizontal(|ui| {
                        for (i, status) in row {
                            let slot = *i as u8;
                            slot_chip(
                                ui,
                                slot,
                                status,
                                slot >= range.0 && slot <= range.1,
                                count,
                                &mut self.slot_range,
                            );
                        }
                    });
                }
            });
    }

    /// Turn the form into the create action. A range that overlaps a character already in the
    /// project is refused: it would put the same files in the project twice.
    fn build_create_action(
        &mut self,
        project: &RosterMod,
        destination: PathBuf,
    ) -> NewCharacterAction {
        let (Some(donor), Some(range)) = (self.donor.clone(), self.slot_range) else {
            return NewCharacterAction::None;
        };
        let display_name = self.display_name.trim().to_string();
        if display_name.is_empty() {
            return NewCharacterAction::None;
        }
        let donor_lc = donor.to_ascii_lowercase();
        let overlaps = project.authored.iter().any(|entry| {
            entry.donor == donor_lc
                && entry
                    .all_slots()
                    .iter()
                    .any(|slot| *slot >= range.0 && *slot <= range.1)
        });
        if overlaps {
            self.error = Some(format!(
                "{display_name} would take costumes another character of this project already has."
            ));
            return NewCharacterAction::None;
        }
        NewCharacterAction::Create {
            name_id: name_id_for(&display_name),
            donor,
            slots: range_slots(range),
            display_name,
            destination,
        }
    }

    pub fn note_created(
        &mut self,
        display_name: &str,
        scaffolded: &scaffold::Scaffold,
        copied: scaffold::Copied,
    ) {
        self.error = None;
        self.display_name.clear();
        self.donor = None;
        self.slot_range = None;
        self.status = format!(
            "Created {display_name} with {} skins: {} files ({} MB) copied from the base fighter. It is on the Project page.",
            scaffolded.slots.len(),
            copied.files,
            copied.bytes / 1_000_000
        );
    }

    pub fn note_error(&mut self, error: String) {
        self.error = Some(error);
    }
}

/// What occupies a costume slot, for the slot strip and the conflict list.
///
/// Order is `Free` first so the strip's "nothing to do here" reading is
/// available without scanning. The `taken_label` returns `Some` for every
/// non-`Free` variant with the name to show the user, so the conflict list
/// and the legend share one source of truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotStatus {
    Free,
    /// Donor's own costume, 0..VANILLA_SLOT_COUNT. The donor's files live
    /// there; a new character would clobber them.
    Donor,
    /// A modded slot contributed by a loaded mod, with no roster row.
    Modded,
    /// The current project already authors this slot.
    Authored,
    /// A different character (imported mod) owns the slot — its roster row
    /// points at the same donor + slot.
    Imported,
}

impl SlotStatus {
    fn taken_label(self) -> Option<String> {
        match self {
            Self::Free => None,
            Self::Donor => Some("donor's own costume".to_string()),
            Self::Modded => Some("a loaded mod".to_string()),
            Self::Authored => Some("already in this project".to_string()),
            Self::Imported => Some("another imported character".to_string()),
        }
    }
}

/// `start <= end`, both `u8`. Out-of-order input is repaired.
fn clamp_range(range: SlotRange) -> SlotRange {
    if range.0 <= range.1 {
        range
    } else {
        (range.1, range.0)
    }
}

/// The lowest `start` such that `start..=start + width - 1` is entirely free
/// for `donor`. Returns `None` when no `width`-sized run exists before
/// `u8::MAX`. Used to seed a default range when the donor changes.
fn lowest_free_run(
    roots: &[PathBuf],
    index: &RosterIndex,
    donor: &str,
    width: u8,
) -> Option<SlotRange> {
    let statuses = slot_statuses(roots, index, donor);
    // Scanning "after the last slot" wraps to the start, which is exactly
    // the lowest run.
    next_free_run_after(&statuses, width, u8::MAX)
}

/// Status of every slot 0..=255 for `donor`. The donor's vanilla 0..7
/// default is the floor; a mod that adds a slot overrides the corresponding
/// entry. Authored and imported rows from the index take precedence so the
/// rule "I already have a character here" reads as the user wrote it.
fn slot_statuses(roots: &[PathBuf], index: &RosterIndex, donor: &str) -> Vec<SlotStatus> {
    let mut out: Vec<SlotStatus> = (0..=255u8)
        .map(|i| {
            if i < crate::data::VANILLA_SLOT_COUNT {
                SlotStatus::Donor
            } else {
                SlotStatus::Free
            }
        })
        .collect();
    for slot in crate::data::discover_costume_slots(roots, donor) {
        // Modded overrides Donor (the donor's own c00..c07 cannot be a
        // modded slot, but be defensive — if a mod adds files for c00, the
        // strip should still show "modded", not the donor's own).
        if (out[slot as usize] == SlotStatus::Donor || out[slot as usize] == SlotStatus::Free)
            && !index_slots_for(index, donor).contains(&slot)
            && !authored_slots_for(index, donor).contains(&slot)
        {
            out[slot as usize] = SlotStatus::Modded;
        }
    }
    for slot in index_slots_for(index, donor) {
        out[slot as usize] = SlotStatus::Imported;
    }
    for slot in authored_slots_for(index, donor) {
        out[slot as usize] = SlotStatus::Authored;
    }
    out
}

/// Every slot an index entry on this donor occupies, including a
/// multi-skin character's additional slots.
fn index_slots_for(index: &RosterIndex, donor: &str) -> Vec<u8> {
    index
        .entries
        .iter()
        .filter(|e| e.fighter.as_deref() == Some(donor))
        .flat_map(|e| e.backing.all_slots().into_iter())
        .collect()
}

/// Every slot the current project has authored on this donor.
fn authored_slots_for(index: &RosterIndex, donor: &str) -> Vec<u8> {
    index
        .entries
        .iter()
        .filter(|e| {
            e.fighter.as_deref() == Some(donor) && matches!(e.origin, super::EntryOrigin::Authored)
        })
        .flat_map(|e| e.backing.all_slots().into_iter())
        .collect()
}

/// Every taken slot in `range`, with its owner label. Pure over a status
/// slice so the panel computes statuses once per frame and reuses them.
fn conflicts_in_range(statuses: &[SlotStatus], range: SlotRange) -> Vec<(u8, String)> {
    let mut out = Vec::new();
    for slot in range.0..=range.1 {
        if let Some(label) = statuses[slot as usize].taken_label() {
            out.push((slot, label));
        }
    }
    out
}

/// One-line rendering of a conflict list for the status line. Caps at six
/// entries so a fully-blocked range does not flood the form; the map shows
/// the rest.
fn summarize_taken(taken: &[(u8, String)]) -> String {
    const SHOW: usize = 6;
    let mut parts: Vec<String> = taken
        .iter()
        .take(SHOW)
        .map(|(slot, owner)| format!("c{slot:02} ({owner})"))
        .collect();
    if taken.len() > SHOW {
        parts.push(format!("+{} more", taken.len() - SHOW));
    }
    parts.join(", ")
}

/// The first clear block of `width` slots starting strictly after `after`,
/// wrapping to the start. `None` when no block fits anywhere.
fn next_free_run_after(statuses: &[SlotStatus], width: u8, after: u8) -> Option<SlotRange> {
    let width = width.max(1) as usize;
    let cap = statuses.len();
    // Scan forward from `after + 1`, then wrap: the first start whose whole
    // block is free wins, so "next" means nearest, with wrap-around.
    let order = ((after as usize + 1)..cap).chain(0..=(after as usize).min(cap - 1));
    for start in order {
        if start + width > cap {
            continue;
        }
        if statuses[start..start + width]
            .iter()
            .all(|s| *s == SlotStatus::Free)
        {
            let end = (start + width - 1) as u8;
            return Some((start as u8, end));
        }
    }
    None
}

/// One slot chip in the map grid. Two visual states only — white for free,
/// grey for taken — with the taken reason on hover. A free chip inside the
/// current block is green so the block reads as one shape. Clicking a free
/// chip moves the whole block there, keeping the skin count.
fn slot_chip(
    ui: &mut Ui,
    slot: u8,
    status: &SlotStatus,
    in_range: bool,
    count: u8,
    slot_range: &mut Option<SlotRange>,
) {
    let label = format!("c{slot:02}");
    let free = matches!(status, SlotStatus::Free);
    let (fg, bg) = match (free, in_range) {
        (true, true) => (
            Color32::from_rgb(20, 60, 30),
            Color32::from_rgb(130, 225, 150),
        ),
        (true, false) => (
            Color32::from_rgb(60, 60, 60),
            Color32::from_rgb(250, 250, 250),
        ),
        (false, true) => (
            Color32::from_rgb(120, 50, 30),
            Color32::from_rgb(245, 200, 190),
        ),
        (false, false) => (
            Color32::from_rgb(150, 150, 150),
            Color32::from_rgb(228, 228, 228),
        ),
    };
    let button = egui::Button::new(RichText::new(label).small().color(fg))
        .fill(bg)
        .sense(if free {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        });
    let end = slot.saturating_add(count.saturating_sub(1));
    let hover = match status.taken_label() {
        Some(owner) => format!("c{slot:02}: {owner}"),
        None => format!("Move block here (c{slot:02} to c{end:02})"),
    };
    let resp = ui.add(button).on_hover_text(hover);
    if resp.clicked() && free {
        *slot_range = Some((slot, end.max(slot)));
    }
}

/// The internal id a character gets, derived from its display name.
pub fn name_id_for(display_name: &str) -> String {
    let mut out = String::new();
    for character in display_name.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let trimmed = out.trim_end_matches('_').to_string();
    if trimmed.is_empty() {
        "new_character".to_string()
    } else {
        trimmed
    }
}

/// Build the project record for a multi-skin character: one entry owning every
/// slot in `slots` (which must be non-empty and already validated free).
/// The key and primary slot are the lowest slot, so the entry sorts with it.
/// A single-element slice is a one-skin character; there is no separate
/// single-slot constructor.
pub fn authored_entry_multi(
    donor: &str,
    slots: &[u8],
    display_name: &str,
    name_id: &str,
) -> AuthoredEntry {
    let mut sorted: Vec<u8> = slots.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let primary = sorted.first().copied().unwrap_or(0);
    AuthoredEntry {
        key: RosterKey::slot(donor, primary),
        donor: donor.to_ascii_lowercase(),
        slot: primary,
        slots: sorted.into_iter().filter(|slot| *slot != primary).collect(),
        display_name: display_name.to_string(),
        name_id: name_id.to_string(),
        moveset_scaffolded: false,
        // Set by the window when it creates the scaffold; the helper alone
        // knows no destination.
        files_root: None,
    }
}

/// Make a new character's folder, fill it with the base fighter's files (`roots` are the game
/// files and the mods on top; `ui_name` is the name its portraits are filed under) and add the
/// folder as a mod, so every skin can be edited at once. One mod folder holds all the skins.
#[allow(clippy::too_many_arguments)]
pub fn create_and_import_range(
    library: &mut ModLibrary,
    destination: &std::path::Path,
    roots: &[std::path::PathBuf],
    donor: &str,
    ui_name: &str,
    slots: &[u8],
    display_name: &str,
    name_id: &str,
) -> anyhow::Result<(scaffold::Scaffold, scaffold::Copied)> {
    if slots.is_empty() {
        anyhow::bail!("a new character needs at least one costume slot");
    }
    let root = destination.join(crate::mod_export::slugify(display_name));
    let scaffolded = scaffold::create_many(&root, donor, slots)?;
    let copied = scaffold::copy_base(&root, roots, donor, ui_name, name_id, &scaffolded.slots)?;
    if copied.files == 0 {
        anyhow::bail!("none of {donor}'s game files were found, so there is nothing to make the character from");
    }
    library.import_directory(ModSource::Folder(root), Some(display_name.to_string()))?;
    Ok((scaffolded, copied))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the game's files: Mario with eight costumes.
    fn game() -> (tempfile::TempDir, Vec<std::path::PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        for costume in 0..8 {
            let model = dir
                .path()
                .join(format!("fighter/mario/model/body/c{costume:02}"));
            std::fs::create_dir_all(&model).unwrap();
            std::fs::write(model.join("model.numdlb"), b"model").unwrap();
        }
        let roots = vec![dir.path().to_path_buf()];
        (dir, roots)
    }

    fn create(
        library: &mut ModLibrary,
        destination: &std::path::Path,
        slots: &[u8],
        name: &str,
    ) -> scaffold::Scaffold {
        let (_game, roots) = game();
        create_and_import_range(
            library,
            destination,
            &roots,
            "mario",
            "mario",
            slots,
            name,
            &name_id_for(name),
        )
        .unwrap()
        .0
    }

    #[test]
    fn a_display_name_becomes_a_safe_stable_id() {
        assert_eq!(name_id_for("Vision"), "vision");
        assert_eq!(name_id_for("Dark Samus"), "dark_samus");
        assert_eq!(name_id_for("R.O.B. 64!"), "r_o_b_64");
        assert_eq!(name_id_for("  spaced  out  "), "spaced_out");
        assert_eq!(name_id_for("???"), "new_character");
        assert_eq!(name_id_for("Vision"), name_id_for("Vision"));
    }

    #[test]
    fn an_authored_entry_is_keyed_by_its_donor_and_slot() {
        let entry = authored_entry_multi("Mario", &[8], "Vision", "vision");
        assert_eq!(entry.key, RosterKey::slot("mario", 8));
        assert_eq!(entry.donor, "mario");
        assert_eq!(entry.name_id, "vision");
        assert!(!entry.moveset_scaffolded);
    }

    /// A range that overlaps a character already in the project would put the same files in
    /// it twice, so the form refuses before anything is written.
    #[test]
    fn create_refuses_a_range_overlapping_a_character_of_the_project() {
        let mut view = NewCharacterView {
            donor: Some("mario".into()),
            slot_range: Some((12, 19)),
            display_name: "Vision".into(),
            ..Default::default()
        };
        let mut project = RosterMod::default();
        project.authored.push(authored_entry_multi(
            "mario",
            &[8, 9, 10, 11, 12, 13, 14, 15],
            "Vision",
            "vision",
        ));
        assert!(matches!(
            view.build_create_action(&project, PathBuf::from("nowhere")),
            NewCharacterAction::None
        ));
        assert!(view.error.is_some());
        view.slot_range = Some((16, 23));
        assert!(matches!(
            view.build_create_action(&project, PathBuf::from("nowhere")),
            NewCharacterAction::Create { .. }
        ));
    }

    #[test]
    fn a_multi_skin_range_creates_every_skin_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut library = ModLibrary::default();
        let scaffolded = create(
            &mut library,
            dir.path(),
            &[8, 9, 10, 11, 12, 13, 14, 15],
            "Vision",
        );

        assert_eq!(scaffolded.slots, vec![8, 9, 10, 11, 12, 13, 14, 15]);
        assert_eq!(scaffolded.slots[0], 8);
        assert_eq!(library.mods.len(), 1);
        let mod_root = dir.path().join(crate::mod_export::slugify("Vision"));
        for slot in 8..=15 {
            assert!(
                mod_root
                    .join(format!("fighter/mario/model/body/c{slot:02}"))
                    .is_dir(),
                "model dir for c{slot:02} missing"
            );
            assert!(
                mod_root
                    .join(format!("fighter/mario/motion/body/c{slot:02}"))
                    .is_dir(),
                "motion dir for c{slot:02} missing"
            );
        }
    }

    #[test]
    fn slot_statuses_separate_free_donor_modded_and_authored() {
        // Donor always owns c00..c07. A mod can add files beyond that. The
        // project can author its own. The index can hold an imported
        // character. None of these should be conflated.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for slot in 8..=11 {
            std::fs::create_dir_all(root.join(format!("fighter/mario/model/body/c{slot:02}")))
                .unwrap();
        }
        std::fs::create_dir_all(root.join("effect/fighter/mario")).unwrap();
        std::fs::write(root.join("effect/fighter/mario/ef_mario_c20.eff"), b"").unwrap();

        let index = RosterIndex::default();
        let s = slot_statuses(&[root.to_path_buf()], &index, "mario");
        for (i, status) in s.iter().enumerate().take(8) {
            assert_eq!(*status, SlotStatus::Donor, "c{i:02} should be donor's");
        }
        for (i, status) in s.iter().enumerate().take(12).skip(8) {
            assert_eq!(*status, SlotStatus::Modded, "c{i:02} should be modded");
        }
        assert_eq!(s[12], SlotStatus::Free);
        assert_eq!(s[19], SlotStatus::Free);
        assert_eq!(s[20], SlotStatus::Modded, "eff-only slot is modded");
    }

    #[test]
    fn lowest_free_run_picks_the_first_clear_run_of_the_needed_width() {
        // Donor owns 0..7. A mod occupies 8..11 (a 4-slot run). The lowest
        // free run of 8 slots starts at 12 and ends at 19.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for slot in 8..=11 {
            std::fs::create_dir_all(root.join(format!("fighter/mario/model/body/c{slot:02}")))
                .unwrap();
        }
        let index = RosterIndex::default();
        let run = lowest_free_run(&[root.to_path_buf()], &index, "mario", 8);
        assert_eq!(run, Some((12, 19)));
    }

    #[test]
    fn lowest_free_run_returns_none_when_no_clear_run_exists() {
        // A mod occupies 8..=255 (every slot past the donor's 8). No
        // 8-wide run exists anywhere.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for slot in 8..=255 {
            std::fs::create_dir_all(root.join(format!("fighter/mario/model/body/c{slot:02}")))
                .unwrap();
        }
        let index = RosterIndex::default();
        assert_eq!(
            lowest_free_run(&[root.to_path_buf()], &index, "mario", 8),
            None
        );
    }

    #[test]
    fn costumes_a_mod_already_fills_are_named_as_taken() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for slot in [8, 10] {
            std::fs::create_dir_all(root.join(format!("fighter/mario/model/body/c{slot:02}")))
                .unwrap();
        }
        let index = RosterIndex::default();
        let statuses = slot_statuses(&[root.to_path_buf()], &index, "mario");
        let taken: Vec<u8> = conflicts_in_range(&statuses, (8, 15))
            .iter()
            .map(|(slot, _)| *slot)
            .collect();
        assert_eq!(taken, vec![8, 10]);
        assert!(conflicts_in_range(&statuses, (11, 15)).is_empty());
    }

    fn free_statuses(taken: &[u8]) -> Vec<SlotStatus> {
        let mut out = vec![SlotStatus::Free; 256];
        for slot in taken {
            out[*slot as usize] = SlotStatus::Modded;
        }
        out
    }

    #[test]
    fn next_free_block_jumps_past_the_current_block() {
        // c08–c11 taken by a mod: from c08 the next clear 8-block is c12–c19.
        let statuses = free_statuses(&[8, 9, 10, 11]);
        assert_eq!(next_free_run_after(&statuses, 8, 8), Some((12, 19)));
    }

    #[test]
    fn next_free_block_wraps_to_the_start() {
        // c00–c07 are the donor's own and everything from c16 up is taken:
        // from c20 the only clear 8-block is back at c08–c15.
        let mut taken: Vec<u8> = (0..=7).collect();
        taken.extend(16..=255);
        let statuses = free_statuses(&taken);
        assert_eq!(next_free_run_after(&statuses, 8, 20), Some((8, 15)));
        let mut taken_all: Vec<u8> = (0..=7).collect();
        taken_all.extend(8..=255);
        let full = free_statuses(&taken_all);
        assert_eq!(next_free_run_after(&full, 8, 20), None);
    }

    #[test]
    fn taken_summary_caps_long_lists() {
        let taken: Vec<(u8, String)> = (8..=15).map(|s| (s, "a loaded mod".to_string())).collect();
        let summary = summarize_taken(&taken);
        assert!(summary.contains("c08"), "first conflict named: {summary}");
        assert!(summary.contains("+2 more"), "overflow counted: {summary}");
        let short = summarize_taken(&taken[..2]);
        assert!(!short.contains("more"), "short lists shown whole: {short}");
    }

    #[test]
    fn clamp_range_repairs_an_inverted_range() {
        assert_eq!(clamp_range((5, 3)), (3, 5));
        assert_eq!(clamp_range((5, 5)), (5, 5));
        assert_eq!(clamp_range((0, 255)), (0, 255));
    }

    #[test]
    fn range_count_and_slots_round_trip() {
        assert_eq!(range_count((8, 15)), 8);
        assert_eq!(range_count((0, 0)), 1);
        assert_eq!(range_count((0, 255)), 256.min(u8::MAX as usize) as u8);
        assert_eq!(range_slots((8, 11)), vec![8, 9, 10, 11]);
    }

    #[test]
    fn creating_a_character_also_makes_it_visible_to_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let mut library = ModLibrary::default();
        let scaffolded = create(&mut library, dir.path(), &[8], "Vision");

        assert_eq!(scaffolded.slots[0], 8);
        assert_eq!(library.mods.len(), 1);
        let imported = &library.mods[0];
        assert_eq!(imported.name, "Vision");
        assert!(imported.enabled);
        let provision = &imported.manifest.fighters["mario"];
        assert!(
            provision.slots.contains(&8),
            "the new slot was not visible in the imported mod's manifest"
        );
    }

    #[test]
    fn on_the_fly_creation_is_properly_scoped_per_slot() {
        let mut library = ModLibrary::default();
        let dir = tempfile::tempdir().unwrap();
        let a = create(&mut library, &dir.path().join("a"), &[8], "Vision");
        let b = create(&mut library, &dir.path().join("b"), &[9], "Vision2");
        assert_eq!(a.slots, vec![8]);
        assert_eq!(b.slots, vec![9]);
        assert_ne!(
            RosterKey::slot("mario", 8),
            RosterKey::slot("mario", 9),
            "slot keys must not collide"
        );
        assert_eq!(library.mods.len(), 2);
        assert!(library.mods[0].manifest.fighters["mario"]
            .slots
            .contains(&8));
        assert!(!library.mods[0].manifest.fighters["mario"]
            .slots
            .contains(&9));
        assert!(library.mods[1].manifest.fighters["mario"]
            .slots
            .contains(&9));
        let k8 = RosterKey::slot("mario", 8);
        let k9 = RosterKey::slot("mario", 9);
        assert_ne!(k8, k9);
    }

    #[test]
    fn quick_create_uses_authored_cache_without_picker() {
        let dir = tempfile::tempdir().unwrap();
        let mut library = ModLibrary::default();
        let root = dir.path().join("authored").join("vision");
        let s = crate::roster::scaffold::create_many(&root, "mario", &[12]).unwrap();
        std::fs::write(
            root.join("fighter/mario/model/body/c12/model.numdlb"),
            b"model",
        )
        .unwrap();
        library
            .import_directory(ModSource::Folder(root), Some("Vision".into()))
            .unwrap();
        assert_eq!(s.slots, vec![12]);
        assert!(library.mods[0].manifest.fighters["mario"]
            .slots
            .contains(&12));
    }
}
