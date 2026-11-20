//! The character select screen preview and layout editor.
//!
//! What this is authoritative about is the **sequence**: `disp_order` is a linear index, and
//! the sequence is exactly what an edit here changes. The grid the sequence is drawn in is a
//! reflow at a user-chosen column count — the real cell geometry lives in `ui/layout/`, and
//! this preview does not claim to reproduce it. The panel says so, because a preview that
//! silently implies pixel accuracy invites bug reports about a thing it never promised.
//!
//! Edits are sparse and go into the project, never into the loaded database: the database is
//! rebuilt from base + overrides at export time, so a project stays correct when the mods
//! underneath it change. Nothing here touches the running game — roster changes are saved
//! with the project and shipped with the exported mod.

use std::collections::BTreeSet;
use std::path::PathBuf;

use egui::{Color32, Pos2, Rect, RichText, Ui};

use crate::mod_project::RosterMod;

use super::css::{self, CharaDb};
use super::icons::PortraitCache;
use super::index::RosterIndex;
use super::{EntryOrigin, RosterEntry, RosterKey};

/// Colour per origin, so a modded roster reads at a glance.
fn origin_color(origin: EntryOrigin) -> Color32 {
    match origin {
        EntryOrigin::Vanilla => Color32::from_rgb(170, 170, 180),
        EntryOrigin::Imported => Color32::from_rgb(120, 185, 235),
        EntryOrigin::Authored => Color32::from_rgb(130, 225, 150),
    }
}

fn origin_badge_color(origin: EntryOrigin) -> Color32 {
    match origin {
        EntryOrigin::Vanilla => Color32::from_rgb(70, 70, 78),
        EntryOrigin::Imported => Color32::from_rgb(30, 65, 95),
        EntryOrigin::Authored => Color32::from_rgb(30, 80, 45),
    }
}

fn is_aegis_combined(entry: &RosterEntry) -> bool {
    entry
        .name_id
        .as_deref()
        .map(|n| n.eq_ignore_ascii_case("element"))
        .unwrap_or(false)
        || entry
            .fighter
            .as_deref()
            .map(|n| n.eq_ignore_ascii_case("element"))
            .unwrap_or(false)
        || entry.key.as_str().eq_ignore_ascii_case("element")
        || entry.key.as_str().eq_ignore_ascii_case("ui:element")
}

fn is_aegis_group(entry: &RosterEntry) -> bool {
    for cand in [
        entry.name_id.as_deref(),
        entry.fighter.as_deref(),
        Some(entry.key.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        let lower = cand.to_ascii_lowercase();
        let base = lower.strip_prefix("ui:").unwrap_or(&lower);
        if matches!(
            base,
            "eflame" | "elight" | "element" | "eflame_first" | "elight_first" | "pyra" | "mythra"
        ) {
            return true;
        }
    }
    false
}

/// One entry per cell: Pyra and Mythra share a position, and the cell is theirs together.
fn one_per_cell<'a>(entries: impl IntoIterator<Item = &'a RosterEntry>) -> Vec<&'a RosterEntry> {
    let mut out: Vec<&'a RosterEntry> = Vec::new();
    let mut seen_order: std::collections::HashMap<i8, usize> = std::collections::HashMap::new();
    for e in entries {
        if let Some(order) = e.css_order {
            if let Some(&idx) = seen_order.get(&order) {
                if is_aegis_combined(e) && !is_aegis_combined(out[idx]) {
                    out[idx] = e;
                }
                continue;
            }
            seen_order.insert(order, out.len());
        }
        out.push(e);
    }
    out
}

/// The cells of the select screen with echoes separate, as the game ships: every character
/// but the one Mii Fighter cell that stands in for the three when echoes are stacked.
fn filtered_visible<'a>(entries: Vec<&'a RosterEntry>) -> Vec<&'a RosterEntry> {
    one_per_cell(entries.into_iter().filter(|e| {
        !e.name_id
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case("miiall"))
    }))
}

/// The game lays the select screen out 13 cells across, each row centred, and this page does
/// the same so it reads as the screen it edits. Cells are as wide as the page allows.
const GAME_COLUMNS: usize = 13;
/// A cell's height for its width: the game's cell art (`chara_7`) is 454 by 300.
const CELL_ASPECT: f32 = 300.0 / 454.0;
const GRID_SPACING: f32 = 4.0;
/// Narrowest a cell gets before the grid scrolls sideways instead.
const CELL_MIN_W: f32 = 52.0;
const CELL_MAX_W: f32 = 150.0;

/// The width of one cell when `GAME_COLUMNS` of them fill `avail_w`.
fn cell_width(avail_w: f32) -> f32 {
    (((avail_w - 4.0) - (GAME_COLUMNS - 1) as f32 * GRID_SPACING) / GAME_COLUMNS as f32)
        .clamp(CELL_MIN_W, CELL_MAX_W)
}

/// The character an entry is a part of, for the entries the game folds into another's cell.
fn part_of(name_id: &str) -> Option<&'static str> {
    match name_id {
        "pzenigame" | "pfushigisou" | "plizardon" => Some("part of Pokémon Trainer"),
        "eflame_only" | "elight_only" | "element" => Some("part of Pyra / Mythra"),
        "miiall" => Some("the one cell for all three Mii Fighters"),
        _ => None,
    }
}

/// The Random cell the game puts after the last character. Nothing here edits it.
fn random_cell(ui: &mut Ui, look: crate::ui::Look, cell_w: f32) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(cell_w, (cell_w * CELL_ASPECT).round()),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, look.sunk);
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, look.edge),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "RANDOM",
        egui::FontId::proportional((rect.height() * 0.2).clamp(8.0, 14.0)),
        look.dim,
    );
    response.on_hover_text("The game's Random cell, always last");
}

/// One dossier dot: bright when the slot has it, dim when missing, with
/// the counts behind a hover.
fn slot_dot(ui: &mut Ui, on: bool, label: &str, hover: String) {
    ui.label(RichText::new(label).small().strong().color(if on {
        Color32::from_rgb(130, 225, 150)
    } else {
        ui.visuals().weak_text_color()
    }))
    .on_hover_text(hover);
}

/// Slots to show name fields for: everything on disk, plus anything with
/// an override that the disk scan missed, sorted and deduplicated. Listed
/// overrides survive a rescan that moved their files rather than vanishing
/// from the panel.
fn costume_slots_for(
    disk: &[u8],
    overrides: Option<&std::collections::BTreeMap<u8, String>>,
) -> Vec<u8> {
    let mut slots: Vec<u8> = disk.to_vec();
    if let Some(map) = overrides {
        slots.extend(map.keys().copied());
    }
    slots.sort_unstable();
    slots.dedup();
    slots
}

/// Costume choices for one image row: the entry's own slot always, plus
/// every skin on disk and every skin already overridden for another row.
fn image_slot_candidates(disk: &[u8], entry_slot: u8, overridden: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = disk.to_vec();
    out.push(entry_slot);
    out.extend(overridden.iter().copied());
    out.sort_unstable();
    out.dedup();
    out
}

/// One looked-up stage texture, owned so the game and custom lookups never
/// hold the portrait cache at the same time.
enum StageTex {
    Ready(egui::TextureId, egui::Vec2),
    Missing(String),
    Queued,
}

/// True when the grid filter leaves `entry` visible. Empty query shows all.
fn matches_grid_filter(entry: &RosterEntry, query: &str) -> bool {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return true;
    }
    entry.display_name.to_ascii_lowercase().contains(&query)
        || entry.key.as_str().contains(&query)
        || entry
            .name_id
            .as_deref()
            .is_some_and(|nid| nid.to_ascii_lowercase().contains(&query))
}

/// The character select tab's own state.
#[derive(Default)]
pub struct CssView {
    db: Option<CharaDb>,
    /// The arc root the database was loaded from, and the roots portraits are searched in.
    ui_root: Option<PathBuf>,
    load_error: Option<String>,
    /// Set once a load has been attempted, so a missing `ui/` dump is reported rather than
    /// retried every frame.
    attempted: bool,
    portraits: PortraitCache,
    /// Narrows the grid to matching entries (name, key, or name_id). Reorder
    /// stays sound while filtered: positions are reused from the shown
    /// subset, so unshown entries keep theirs.
    grid_filter: String,
    /// Preview the screen with the game's "Echo Fighters: Stacked" option on.
    echo_stacked: bool,
    /// Narrows the off-roster list. Separate from the grid filter: finding
    /// someone to restore is a different hunt from arranging the screen.
    off_roster_filter: String,
    /// Narrows the bulk-rename table.
    bulk_filter: String,
    /// Scroll the detail panel into view on the next frame. Set when a new
    /// selection is made (click, marquee end, select-all): the panel lives
    /// below a long grid, and a selection whose editor stays off-screen
    /// reads as "clicking does nothing".
    focus_detail: bool,
    /// Scroll the off-roster list into view: set when it is opened and when
    /// hiding sends entries there.
    focus_off_roster: bool,
    /// Scroll the rename table into view when it is opened.
    focus_names: bool,
    /// Ask the window to switch to the New Character tab (the CSS tab cannot
    /// switch tabs itself). Read and cleared by the window each frame.
    goto_new_character_tab: bool,
    selected: BTreeSet<RosterKey>,
    last_selected: Option<RosterKey>,
    /// Keys being dragged as a group
    dragging: Option<BTreeSet<RosterKey>>,
    /// Start of marquee drag in screen space
    marquee_start: Option<Pos2>,
    show_off_roster: bool,
    /// Set by the toolbar and consumed after it draws — the hide action needs the index, which
    /// the toolbar closure cannot borrow while `self` is mutably held.
    pending_hide: bool,
    status: String,
    /// Show the bulk-name editor table.
    show_bulk_names: bool,
    /// Typed `disp_order` text per entry. Held apart from the project so a
    /// multi-digit position can be typed without each keystroke being reset
    /// by the next frame's rebuild from the project value.
    order_drafts: std::collections::HashMap<RosterKey, String>,
    /// Costume picked per entry+kind in the Images editor. The picker is how
    /// one character carries a different portrait per skin.
    image_slot_pick: std::collections::HashMap<(RosterKey, String), u8>,
    /// Image kind shown on the preview stage. The stage is the big,
    /// game-vs-custom comparison; the rows below do the picking.
    img_preview_kind: Option<String>,
}

impl CssView {
    pub fn db(&self) -> Option<&CharaDb> {
        self.db.as_ref()
    }

    /// Select one entry and bring its editor into view. Used by cross-tab
    /// jumps (a new character's "finish look" button) so the landing shows
    /// the editor, not just the grid.
    pub fn select_key(&mut self, key: RosterKey) {
        self.selected.clear();
        self.selected.insert(key.clone());
        self.last_selected = Some(key);
        self.focus_detail = true;
    }

    /// Drop typed position drafts (edit-log revert path clears the project
    /// underneath them; a stale draft would resurrect old text).
    pub(super) fn clear_order_drafts(&mut self) {
        self.order_drafts.clear();
    }

    /// Take the pending "switch to the New Character tab" request, if any.
    pub fn take_goto_new_character(&mut self) -> bool {
        std::mem::replace(&mut self.goto_new_character_tab, false)
    }

    /// Forget the loaded database and every portrait.
    ///
    /// Called when the mod library changes: a newly enabled mod may now provide the roster
    /// database, or win a portrait an earlier root was providing.
    pub fn invalidate(&mut self) {
        self.db = None;
        self.ui_root = None;
        self.attempted = false;
        self.load_error = None;
        self.portraits.clear();
    }

    pub(super) fn ensure_loaded(&mut self, roots: &[PathBuf]) {
        if self.attempted {
            return;
        }
        self.attempted = true;
        let Some(root) = css::locate_ui_root(roots) else {
            return;
        };
        let path = root.join(css::CHARA_DB_PATH);
        match CharaDb::open(&path) {
            Ok(db) => {
                self.status = format!("Loaded {} roster entries.", db.entries().len());
                self.db = Some(db);
                self.ui_root = Some(root);
            }
            Err(error) => self.load_error = Some(format!("{error:#}")),
        }
    }

    fn stacking(&self, entry: &RosterEntry) -> css::Stacking {
        let row = entry
            .name_id
            .as_deref()
            .and_then(|name_id| self.db.as_ref()?.row(name_id));
        row.map_or(css::Stacking::Always, css::CharaRow::stacking)
    }

    /// The cells the game shows, in order, with echoes stacked or separate as picked here.
    fn stage<'a>(&self, index: &'a RosterIndex) -> Vec<&'a RosterEntry> {
        let stacked = self.echo_stacked;
        one_per_cell(
            index
                .visible()
                .into_iter()
                .filter(|entry| match self.stacking(entry) {
                    css::Stacking::Always => true,
                    css::Stacking::SeparateOnly => !stacked,
                    css::Stacking::StackedOnly => stacked,
                }),
        )
    }

    /// The echo that shares `entry`'s cell when echoes are stacked.
    fn echo_of<'a>(&self, entry: &RosterEntry, index: &'a RosterIndex) -> Option<&'a RosterEntry> {
        let db = self.db.as_ref()?;
        let row = db.row(entry.name_id.as_deref()?)?;
        if row.alt_chara_id == 0 || row.stacking() != css::Stacking::Always {
            return None;
        }
        let echo = db.entries().iter().find(|other| {
            other.ui_chara_id == row.alt_chara_id && other.stacking() == css::Stacking::SeparateOnly
        })?;
        index
            .visible()
            .into_iter()
            .find(|other| other.name_id.as_deref() == Some(echo.name_id.as_str()))
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        roots: &[PathBuf],
        index: &RosterIndex,
        project: &mut RosterMod,
    ) {
        self.ensure_loaded(roots);
        self.portraits.begin_frame();

        if self.db.is_none() {
            self.draw_missing_database(ui);
            return;
        }

        self.draw_toolbar(ui, index, project);
        // Consume-and-clear: the toolbar sets this above, but the detail
        // panel below sets it too — a reset at the top of the frame would
        // wipe the panel's request before it is ever read, silently
        // disabling its Hide button.
        if std::mem::replace(&mut self.pending_hide, false) {
            self.hide_selected(index, project);
        }
        ui.add_space(4.0);

        let entries = self.stage(index);
        // Measured here, outside the grid's sideways scroller, where the width is the page's.
        let cell_w = cell_width(ui.available_width());
        if entries.is_empty() {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(14, 14))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("The stage is empty").size(13.0).strong());
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Every entry is either hidden by this project or marked off-roster by the database.",
                            )
                            .small()
                            .weak(),
                        );
                        ui.add_space(6.0);
                        // Escape hatch in place: no need to hunt the list below.
                        if !project.hidden.is_empty()
                            && ui
                                .button(RichText::new("Bring everyone back").strong())
                                .on_hover_text("Restore every hidden character to the select screen")
                                .clicked()
                        {
                            project.hidden.clear();
                            self.status = "The full roster is back on the select screen.".into();
                        }
                    });
                });
        } else {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(2, 2))
                .show(ui, |ui| {
                    // Sideways scroll is the last resort: the layout already
                    // fits the window, so this only appears below the minimum
                    // cell size. Vertical scroll stays with the outer roster
                    // scroll, so no double bars. Content-drag is off: cell
                    // drags and marquee own the pointer here.
                    egui::ScrollArea::horizontal()
                        .id_salt("roster_css_grid")
                        .auto_shrink([false, true])
                        .scroll_source(super::GESTURE_AREA_SCROLL)
                        .show(ui, |ui| {
                            self.draw_grid(ui, roots, &entries, project, cell_w, index);
                        });
                });
        }

        // Under the grid: the picked cell's editor, with the characters that are not on the
        // screen beside it. Stacked when the page is too narrow for both.
        ui.add_space(8.0);
        let look = crate::ui::look(ui);
        let panel = move || {
            egui::Frame::new()
                .fill(look.sunk)
                .stroke(egui::Stroke::new(1.0, look.edge))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(12))
        };
        let (width, side) = (ui.available_width(), 360.0);
        let beside = width >= 900.0;
        let editor = |view: &mut Self, ui: &mut Ui, project: &mut RosterMod| {
            panel().show(ui, |ui| {
                ui.set_width(ui.available_width());
                view.draw_picked(ui, look, roots, index, project);
            });
            if view.show_bulk_names {
                ui.add_space(8.0);
                panel().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    view.draw_bulk_names(ui, index, project);
                });
            }
        };
        if beside {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(width - side - 10.0);
                    editor(self, ui, project);
                });
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.set_width(side);
                    panel().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        self.draw_off_roster(ui, index, project);
                    });
                });
            });
        } else {
            editor(self, ui, project);
            ui.add_space(8.0);
            panel().show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.draw_off_roster(ui, index, project);
            });
        }

        if !self.status.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(&self.status).color(look.text));
        }
    }

    /// The editor for the picked cell, or what to do when none is.
    fn draw_picked(
        &mut self,
        ui: &mut Ui,
        look: crate::ui::Look,
        roots: &[PathBuf],
        index: &RosterIndex,
        project: &mut RosterMod,
    ) {
        let picked = self
            .last_selected
            .clone()
            .or_else(|| self.selected.iter().next().cloned())
            .filter(|_| !self.selected.is_empty());
        let Some(entry) = picked.as_ref().and_then(|key| index.by_key(key)) else {
            ui.label(
                RichText::new("No cell picked")
                    .size(19.0)
                    .strong()
                    .color(look.dim),
            );
            return;
        };
        if self.focus_detail {
            self.focus_detail = false;
            ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover())
                .scroll_to_me(Some(egui::Align::TOP));
        }
        if self.selected.len() > 1 {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} picked", self.selected.len()))
                        .strong()
                        .color(look.text),
                );
                if crate::ui::act(ui, look, "Pick none", None).clicked() {
                    self.selected.clear();
                    self.last_selected = None;
                }
            });
            ui.add_space(4.0);
        }
        // The echo stacked in this cell is edited from here too.
        if self.echo_stacked {
            if let Some(echo) = self.echo_of(entry, index) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("In this cell").strong().color(look.dim));
                    ui.label(RichText::new(&entry.display_name).strong().color(look.text));
                    if crate::ui::act(ui, look, &format!("Edit {}", echo.display_name), None)
                        .clicked()
                    {
                        self.selected = BTreeSet::from([echo.key.clone()]);
                        self.last_selected = Some(echo.key.clone());
                    }
                });
                ui.add_space(4.0);
            }
        }
        self.draw_selection(ui, entry, project, roots, index);
    }

    fn draw_missing_database(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        Color32::from_rgb(230, 180, 80),
                        RichText::new("⚠").size(18.0),
                    );
                    ui.label(RichText::new("No roster database found").heading());
                });
                ui.add_space(6.0);
                if let Some(error) = &self.load_error {
                    ui.horizontal(|ui| {
                        ui.colored_label(Color32::from_rgb(240, 140, 140), "✘");
                        ui.label(
                            RichText::new(format!("Failed to read: {error}"))
                                .small()
                                .weak(),
                        );
                    });
                    ui.add_space(6.0);
                }
                ui.label(
                    RichText::new(format!("Place ui/ so {} exists.", css::CHARA_DB_PATH)).small(),
                );
                ui.add_space(10.0);
                if ui
                    .add(egui::Button::new(RichText::new("Look again").strong()))
                    .clicked()
                {
                    self.attempted = false;
                    self.load_error = None;
                }
            });
    }

    fn draw_toolbar(&mut self, ui: &mut Ui, index: &RosterIndex, project: &mut RosterMod) {
        let visible = self.stage(index);
        let visible_len = visible.len();
        let filtering = !self.grid_filter.trim().is_empty();
        let shown_len = if filtering {
            visible
                .iter()
                .filter(|e| matches_grid_filter(e, &self.grid_filter))
                .count()
        } else {
            visible_len
        };
        let look = crate::ui::look(ui);
        let toggle = |on: bool| on.then_some(look.accent);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Echo fighters").strong().color(look.dim));
            for (stacked, word, tip) in [
                (false, "Separate", "Every echo fighter in a cell of its own, as the game starts out"),
                (true, "Stacked", "Echo fighters share their original's cell and the three Mii Fighters share one, as with the game's Echo Fighters option set to Stacked"),
            ] {
                if crate::ui::act(ui, look, word, toggle(self.echo_stacked == stacked)).on_hover_text(tip).clicked() {
                    self.echo_stacked = stacked;
                }
            }
            ui.add_space(10.0);
            if crate::ui::act(ui, look, "Edit names", toggle(self.show_bulk_names))
                .on_hover_text("Every shown character's name in one table")
                .clicked()
            {
                self.show_bulk_names = !self.show_bulk_names;
                if self.show_bulk_names {
                    self.focus_names = true;
                }
            }
            ui.scope(|ui| {
                ui.set_width(200.0);
                crate::ui::search_field(ui, "roster_search", &mut self.grid_filter, "Find a character");
            });
            // Hiding is the one change made from here. Restoring happens in the hidden list;
            // clearing one entry's edits happens in its own panel.
            let has_selection = !self.selected.is_empty();
            let can_hide = has_selection && visible_len.saturating_sub(self.selected.len()) >= 8;
            if ui
                .add_enabled_ui(can_hide, |ui| crate::ui::act(ui, look, "Hide selected", None))
                .inner
                .on_hover_text(if can_hide {
                    "Take the selected characters off the select screen"
                } else if !has_selection {
                    "Select a character first"
                } else {
                    "The select screen needs at least 8 fighters"
                })
                .clicked()
            {
                self.pending_hide = true;
            }
            let changes = project.order.len() + project.hidden.len();
            if ui
                .add_enabled_ui(changes > 0, |ui| crate::ui::act(ui, look, "Reset order", None))
                .inner
                .on_hover_text("Undo every move and hide")
                .clicked()
            {
                project.order.clear();
                project.hidden.clear();
                self.order_drafts.clear();
                self.status = "Select screen order reset.".into();
            }
            let count = if changes > 0 {
                format!("{changes} changed")
            } else if filtering {
                format!("{shown_len} of {visible_len} shown")
            } else {
                format!("{visible_len} characters")
            };
            ui.label(RichText::new(count).color(if changes > 0 { look.accent } else { look.dim }));
        });
    }

    // Eight traversal args (ui, roots, index, project, …) is the shape of
    // every draw_* here; bundling them would churn all call sites for nothing.
    #[allow(clippy::too_many_arguments)]
    fn draw_grid(
        &mut self,
        ui: &mut Ui,
        roots: &[PathBuf],
        entries: &[&RosterEntry],
        project: &mut RosterMod,
        cell_w: f32,
        index: &RosterIndex,
    ) {
        let mut drop_on: Option<RosterKey> = None;
        let mut cell_rects: Vec<(RosterKey, Rect)> = Vec::new();

        // Narrow the grid to the filter query, if any. Reorder stays sound:
        // positions are reused from the shown subset, so unshown entries keep
        // theirs and nothing collides.
        let shown: Vec<&RosterEntry> = if self.grid_filter.trim().is_empty() {
            entries.to_vec()
        } else {
            entries
                .iter()
                .filter(|e| matches_grid_filter(e, &self.grid_filter))
                .copied()
                .collect()
        };
        if shown.is_empty() {
            ui.label(
                RichText::new(format!(
                    "No characters match “{}”.",
                    self.grid_filter.trim()
                ))
                .small()
                .weak(),
            );
            return;
        }

        // Marquee state - check before grid so we can use cell rects after
        let pointer_pos = ui.input(|i| i.pointer.interact_pos());
        let pointer_down = ui.input(|i| i.pointer.primary_down());
        let pointer_pressed = ui.input(|i| i.pointer.primary_pressed());
        let pointer_released = ui.input(|i| i.pointer.any_released());

        // Rows of 13, each centred, with Random after the last character, as the game does.
        let look = crate::ui::look(ui);
        let cells = shown.len() + 1;
        let grid_response = ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(GRID_SPACING, GRID_SPACING);
            for row in 0..cells.div_ceil(GAME_COLUMNS) {
                let range = row * GAME_COLUMNS..((row + 1) * GAME_COLUMNS).min(cells);
                let missing = (GAME_COLUMNS - range.len()) as f32;
                ui.horizontal(|ui| {
                    ui.add_space(missing * (cell_w + GRID_SPACING) / 2.0);
                    for position in range {
                        let Some(entry) = shown.get(position) else {
                            random_cell(ui, look, cell_w);
                            continue;
                        };
                        let echo = if self.echo_stacked {
                            self.echo_of(entry, index)
                        } else {
                            None
                        };
                        let response = self.draw_cell(ui, roots, entry, project, cell_w, echo);
                        cell_rects.push((entry.key.clone(), response.rect));

                        if response.drag_started() {
                            if !self.selected.contains(&entry.key) {
                                // Dragging an unselected entry -> select it alone
                                self.selected.clear();
                                self.selected.insert(entry.key.clone());
                                self.last_selected = Some(entry.key.clone());
                            }
                            self.dragging = Some(self.selected.clone());
                        }
                        if self.dragging.is_some() && response.hovered() && pointer_released {
                            drop_on = Some(entry.key.clone());
                        }
                        // A drag that ends over a cell also produces a click; ignore
                        // clicks while a reorder drag is in flight so dropping does
                        // not reselect to just the target.
                        if response.clicked() && self.dragging.is_none() {
                            let mods = ui.input(|i| i.modifiers);
                            if mods.ctrl || mods.command {
                                if self.selected.contains(&entry.key) {
                                    self.selected.remove(&entry.key);
                                } else {
                                    self.selected.insert(entry.key.clone());
                                }
                                self.last_selected = Some(entry.key.clone());
                            } else if mods.shift {
                                if let Some(last) = self.last_selected.clone() {
                                    if let (Some(a), Some(b)) = (
                                        shown.iter().position(|e| e.key == last),
                                        shown.iter().position(|e| e.key == entry.key),
                                    ) {
                                        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                                        for e in &shown[lo..=hi] {
                                            self.selected.insert(e.key.clone());
                                        }
                                    } else {
                                        self.selected.insert(entry.key.clone());
                                    }
                                } else {
                                    self.selected.insert(entry.key.clone());
                                }
                                self.last_selected = Some(entry.key.clone());
                            } else {
                                self.selected.clear();
                                self.selected.insert(entry.key.clone());
                                self.last_selected = Some(entry.key.clone());
                                // A fresh single selection almost always wants the
                                // editor below; bring it into view.
                                self.focus_detail = true;
                            }
                        }
                    }
                });
            }
        });

        // Marquee drag-select: start on background press, update while dragging
        let grid_rect = grid_response.response.rect;
        let hovered_any_cell = cell_rects
            .iter()
            .any(|(_, r)| r.contains(pointer_pos.unwrap_or(Pos2::ZERO)));

        if pointer_pressed
            && !hovered_any_cell
            && grid_rect.contains(pointer_pos.unwrap_or(Pos2::ZERO))
        {
            self.marquee_start = pointer_pos;
        }
        if let (Some(start), Some(current)) = (self.marquee_start, pointer_pos) {
            if pointer_down {
                let marquee = Rect::from_two_pos(start, current);
                // Marquee select. Without Ctrl it replaces the selection;
                // with Ctrl it adds to it. Either way a reorder drag
                // suppresses it so the two gestures never fight.
                if self.dragging.is_none() {
                    let mods = ui.input(|i| i.modifiers);
                    let add = mods.ctrl || mods.command;
                    let mut new_sel = if add {
                        self.selected.clone()
                    } else {
                        BTreeSet::new()
                    };
                    for (key, rect) in &cell_rects {
                        if marquee.intersects(*rect) {
                            new_sel.insert(key.clone());
                        }
                    }
                    self.selected = new_sel;
                    if let Some(last) = cell_rects.iter().rfind(|(_, r)| marquee.intersects(*r)) {
                        self.last_selected = Some(last.0.clone());
                    }
                }
                // Draw marquee rect
                ui.painter().rect_stroke(
                    marquee,
                    2.0,
                    egui::Stroke::new(1.0, Color32::from_rgb(120, 185, 235)),
                    egui::StrokeKind::Inside,
                );
                ui.painter().rect_filled(
                    marquee,
                    2.0,
                    Color32::from_rgba_premultiplied(30, 65, 95, 40),
                );
            }
        }
        if pointer_released {
            // A finished marquee with a selection in it was somebody drawing
            // around what they want to edit — show them the editor.
            if self.marquee_start.is_some() && !self.selected.is_empty() {
                self.focus_detail = true;
            }
            self.marquee_start = None;
        }

        // Handle drop of group. Operates on the shown subset: positions
        // are reused from it, so unshown entries keep theirs.
        if let Some(target) = drop_on {
            if let Some(dragged) = self.dragging.take() {
                if !dragged.contains(&target) {
                    if dragged.len() == 1 {
                        let source = dragged.iter().next().unwrap().clone();
                        self.reorder(&shown, &source, &target, project);
                    } else {
                        self.reorder_group(&shown, &dragged, &target, project);
                    }
                } else {
                    self.dragging = None;
                }
            }
        }
        if pointer_released {
            self.dragging = None;
        }

        // Keyboard: Delete hides the selection, Ctrl+A selects the shown set.
        // Gated on keyboard focus so typing in a name/position/filter field
        // never hides anyone.
        if !ui.ctx().egui_wants_keyboard_input() {
            if ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
                && !self.selected.is_empty()
            {
                self.hide_selected(index, project);
            }
            if ui
                .input(|i| (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::A))
            {
                self.selected = shown.iter().map(|e| e.key.clone()).collect();
                self.last_selected = shown.last().map(|e| e.key.clone());
                self.focus_detail = true;
            }
        }
    }

    /// One cell, drawn as the game draws it: the character's cell art with its name across the
    /// bottom. `echo` is the echo fighter stacked in this cell, if any.
    fn draw_cell(
        &mut self,
        ui: &mut Ui,
        roots: &[PathBuf],
        entry: &RosterEntry,
        project: &crate::mod_project::RosterMod,
        cell_w: f32,
        echo: Option<&RosterEntry>,
    ) -> egui::Response {
        let look = crate::ui::look(ui);
        let size = egui::vec2(cell_w, (cell_w * CELL_ASPECT).round());
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let selected = self.selected.contains(&entry.key);
        let dragging = self
            .dragging
            .as_ref()
            .is_some_and(|set| set.contains(&entry.key));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, look.sunk);

        let png_override: Option<std::path::PathBuf> = project
            .ui_images
            .get(&entry.key)
            .and_then(|map| {
                let own = entry.backing.slot().unwrap_or(0);
                ["chara_7", "chara_1", "chara_2", "chara_0"]
                    .into_iter()
                    .find_map(|kind| crate::roster::ui_images::find_override(map, kind, own, own))
            })
            .map(|picked| std::path::PathBuf::from(&picked.png_path));
        // A character made here files its art under its own first colour.
        let art = entry.name_id.as_deref().and_then(|name_id| {
            self.portraits.get_image_with_gamma(
                ui.ctx(),
                roots,
                "chara_7",
                name_id,
                0,
                true,
                png_override.as_deref(),
            )
        });
        match art {
            Some(Ok(texture)) => {
                // Fill the cell, cropping whichever way the picture overhangs.
                let [w, h] = texture.size();
                let (picture, cell) = (w as f32 / h.max(1) as f32, rect.width() / rect.height());
                let (u, v) = if picture > cell {
                    (cell / picture, 1.0)
                } else {
                    (1.0, picture / cell)
                };
                let uv = egui::Rect::from_center_size(egui::pos2(0.5, 0.5), egui::vec2(u, v));
                painter.image(texture.id(), rect, uv, Color32::WHITE);
            }
            _ => {
                painter.text(
                    rect.center() - egui::vec2(0.0, rect.height() * 0.12),
                    egui::Align2::CENTER_CENTER,
                    initials(&entry.display_name),
                    egui::FontId::proportional(rect.height() * 0.34),
                    look.dim,
                );
            }
        }

        // The name, in capitals across the bottom, shrunk to fit like the game's.
        let name = if is_aegis_group(entry) && entry.css_order == Some(80) {
            "PYRA / MYTHRA".to_owned()
        } else {
            entry.display_name.to_uppercase()
        };
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - rect.height() * 0.30),
            rect.max,
        );
        painter.rect_filled(band, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 170));
        let mut font = (band.height() * 0.72).clamp(8.0, 15.0);
        let mut galley = painter.layout_no_wrap(
            name.clone(),
            egui::FontId::proportional(font),
            Color32::WHITE,
        );
        while galley.size().x > band.width() - 6.0 && font > 7.0 {
            font -= 0.5;
            galley = painter.layout_no_wrap(
                name.clone(),
                egui::FontId::proportional(font),
                Color32::WHITE,
            );
        }
        painter.galley(band.center() - galley.size() / 2.0, galley, Color32::WHITE);

        // Where it stands in the order, and whether this project moved it.
        if let Some(order) = entry.css_order {
            let moved = self.moved(entry);
            let tag =
                egui::Rect::from_min_size(rect.min + egui::vec2(3.0, 3.0), egui::vec2(20.0, 13.0));
            painter.rect_filled(
                tag,
                3.0,
                if moved {
                    Color32::from_rgb(240, 190, 70)
                } else {
                    Color32::from_rgba_unmultiplied(0, 0, 0, 150)
                },
            );
            painter.text(
                tag.center(),
                egui::Align2::CENTER_CENTER,
                order.to_string(),
                egui::FontId::monospace(9.0),
                if moved {
                    Color32::from_rgb(18, 18, 22)
                } else {
                    Color32::WHITE
                },
            );
        }
        if let Some(echo) = echo {
            let words = painter.layout_no_wrap(
                format!("+ {}", echo.display_name),
                egui::FontId::proportional(9.5),
                Color32::from_rgb(18, 18, 22),
            );
            let tag = egui::Rect::from_min_size(
                egui::pos2(rect.right() - words.size().x - 9.0, rect.top() + 3.0),
                words.size() + egui::vec2(6.0, 2.0),
            );
            if tag.left() > rect.left() + 26.0 {
                painter.rect_filled(tag, 3.0, Color32::from_rgb(255, 115, 160));
                painter.galley(tag.min + egui::vec2(3.0, 1.0), words, Color32::BLACK);
            } else {
                painter.circle_filled(
                    egui::pos2(rect.right() - 8.0, rect.top() + 8.0),
                    4.5,
                    Color32::from_rgb(255, 115, 160),
                );
            }
        }

        let edge = if selected {
            look.accent
        } else if dragging {
            Color32::from_rgb(240, 190, 70)
        } else if response.hovered() {
            look.text
        } else {
            match entry.origin {
                EntryOrigin::Vanilla => look.edge,
                other => origin_color(other),
            }
        };
        let width = if selected || dragging {
            3.0
        } else if entry.origin == EntryOrigin::Vanilla {
            1.0
        } else {
            2.0
        };
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(width, edge),
            egui::StrokeKind::Inside,
        );

        let echo_name = echo.map(|echo| echo.display_name.clone());
        response.on_hover_ui(|ui| {
            self.hover_details(ui, entry);
            if let Some(echo) = &echo_name {
                ui.label(format!(
                    "{echo} shares this cell when echo fighters are stacked."
                ));
            }
        })
    }

    fn hover_details(&self, ui: &mut Ui, entry: &RosterEntry) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&entry.display_name).strong());
            egui::Frame::new()
                .fill(origin_badge_color(entry.origin))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(6, 1))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(entry.origin.label())
                            .small()
                            .strong()
                            .color(origin_color(entry.origin)),
                    );
                });
        });
        if let Some(name_id) = &entry.name_id {
            ui.label(
                RichText::new(format!("Roster entry: {name_id}"))
                    .small()
                    .weak(),
            );
        }
        match (&entry.fighter, entry.backing.slot()) {
            (Some(fighter), Some(slot)) => {
                ui.label(RichText::new(format!("Plays as {fighter}, slot c{slot:02}")).small());
            }
            (Some(fighter), None) => {
                ui.label(RichText::new(format!("Plays as {fighter}")).small());
            }
            (None, _) => {
                ui.label(
                    RichText::new(
                        "No fighter behind this entry. It can be reordered and renamed but has no moveset.",
                    )
                    .small()
                    .weak(),
                );
            }
        }
        if let Some(order) = entry.css_order {
            ui.label(RichText::new(format!("Position {order}")).small().weak());
        }
        if !entry.providers.is_empty() {
            ui.label(
                RichText::new(format!(
                    "Provided by {} mod(s) in the library",
                    entry.providers.len()
                ))
                .small()
                .weak(),
            );
        }
        ui.separator();
        ui.label(
            RichText::new("Drag onto another character to move it there. Click to edit it.")
                .small()
                .weak(),
        );
    }

    /// True when the project has moved or hidden this entry relative to the database.
    fn moved(&self, entry: &RosterEntry) -> bool {
        let Some(db) = &self.db else { return false };
        let Some(name_id) = &entry.name_id else {
            return false;
        };
        let Some(row) = db.row(name_id) else {
            return true;
        };
        entry.hidden || entry.css_order != Some(row.disp_order)
    }

    /// Move `source` to `target`'s position, shifting the entries between them.
    fn reorder(
        &mut self,
        entries: &[&RosterEntry],
        source: &RosterKey,
        target: &RosterKey,
        project: &mut RosterMod,
    ) {
        let Some(from) = entries.iter().position(|entry| &entry.key == source) else {
            return;
        };
        let Some(to) = entries.iter().position(|entry| &entry.key == target) else {
            return;
        };
        let database = |entry: &RosterEntry| {
            self.db
                .as_ref()
                .zip(entry.name_id.as_deref())
                .and_then(|(db, name_id)| db.row(name_id))
                .map(|row| row.disp_order)
        };
        let changes = renumber(entries, from, to, database);
        let mut applied = 0;
        for (key, position) in changes {
            // The grid moved underneath the detail panel — drop any typed
            // position draft for the entry so it resyncs next frame.
            self.order_drafts.remove(&key);
            match position {
                Some(position) => {
                    project.order.insert(key, position);
                    applied += 1;
                }
                None => {
                    project.order.remove(&key);
                }
            }
        }
        self.status = format!(
            "Moved {} to position {}. {applied} entr{} now differ from the game's order. Saved with your project, included on next export.",
            entries[from].display_name,
            entries[to].css_order.unwrap_or(0),
            if applied == 1 { "y" } else { "ies" }
        );
    }

    fn reorder_group(
        &mut self,
        entries: &[&RosterEntry],
        group: &BTreeSet<RosterKey>,
        target: &RosterKey,
        project: &mut RosterMod,
    ) {
        if group.contains(target) {
            return;
        }
        let mut sequence: Vec<&RosterEntry> = entries.to_vec();
        let mut group_entries: Vec<&RosterEntry> = Vec::new();
        sequence.retain(|e| {
            if group.contains(&e.key) {
                group_entries.push(*e);
                false
            } else {
                true
            }
        });
        let Some(target_idx) = sequence.iter().position(|e| &e.key == target) else {
            return;
        };
        for (i, ge) in group_entries.into_iter().enumerate() {
            sequence.insert(target_idx + i, ge);
        }
        let mut positions: Vec<i8> = entries.iter().filter_map(|e| e.css_order).collect();
        positions.sort_unstable();
        positions.dedup();
        let database = |entry: &RosterEntry| {
            self.db
                .as_ref()
                .zip(entry.name_id.as_deref())
                .and_then(|(db, nid)| db.row(nid))
                .map(|r| r.disp_order)
        };
        let mut changes = Vec::new();
        for (slot, entry) in sequence.iter().enumerate() {
            let Some(pos) = positions.get(slot).copied() else {
                break;
            };
            if database(entry) == Some(pos) {
                changes.push((entry.key.clone(), None));
            } else {
                changes.push((entry.key.clone(), Some(pos)));
            }
        }
        let mut applied = 0;
        for (key, pos) in changes {
            self.order_drafts.remove(&key);
            match pos {
                Some(p) => {
                    project.order.insert(key, p);
                    applied += 1;
                }
                None => {
                    project.order.remove(&key);
                }
            }
        }
        self.status = format!(
            "Moved {} fighters as a group. {} position{} updated.",
            group.len(),
            applied,
            if applied == 1 { "" } else { "s" }
        );
    }

    /// Details and per-entry edits for the selected cell: the entry editor.
    ///
    /// Order is by how often each part is touched: display name, then the
    /// costume slots (the per-slot editing lives here), then portraits,
    /// then the technical row fields and name variants behind headers.
    fn draw_selection(
        &mut self,
        ui: &mut Ui,
        entry: &RosterEntry,
        project: &mut RosterMod,
        roots: &[PathBuf],
        _index: &RosterIndex,
    ) {
        let look = crate::ui::look(ui);
        let changed_colour = Color32::from_rgb(240, 190, 70);
        let has_edits = project.order.contains_key(&entry.key)
            || project.names.contains_key(&entry.key)
            || project.name_variants.contains_key(&entry.key)
            || project.chara_overrides.contains_key(&entry.key)
            || project.ui_images.contains_key(&entry.key);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(&entry.display_name)
                        .size(24.0)
                        .strong()
                        .color(look.text),
                );
                let mut line = match entry.origin {
                    EntryOrigin::Vanilla => "Base game".to_owned(),
                    EntryOrigin::Imported => "From a mod".to_owned(),
                    EntryOrigin::Authored => "Made in this project".to_owned(),
                };
                if let (EntryOrigin::Authored, Some(fighter)) = (entry.origin, &entry.fighter) {
                    line = format!(
                        "Made in this project, plays as {}",
                        crate::data::fighter_display_name(fighter)
                    );
                }
                if let Some(order) = entry.css_order {
                    line.push_str(&format!(", place {order}"));
                }
                if let Some(owner) = entry.name_id.as_deref().and_then(part_of) {
                    line.push_str(&format!(", {owner}"));
                }
                ui.label(RichText::new(line).color(look.dim));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if crate::ui::act(ui, look, "Hide", None)
                    .on_hover_text("Take it off the select screen")
                    .clicked()
                {
                    self.pending_hide = true;
                }
                let undo = ui
                    .add_enabled_ui(has_edits, |ui| {
                        crate::ui::act(ui, look, "Undo changes", None)
                    })
                    .inner;
                if undo
                    .on_hover_text("Its name, place, skin count and pictures go back to the game's")
                    .clicked()
                {
                    project.order.remove(&entry.key);
                    project.names.remove(&entry.key);
                    project.name_variants.remove(&entry.key);
                    project.chara_overrides.remove(&entry.key);
                    project.ui_images.remove(&entry.key);
                    self.order_drafts.remove(&entry.key);
                    self.status = format!("{} is as the game has it again.", entry.display_name);
                }
            });
        });
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Name").strong().color(look.dim));
            let mut name = project
                .names
                .get(&entry.key)
                .cloned()
                .unwrap_or_else(|| entry.display_name.clone());
            let is_edited = project.names.contains_key(&entry.key);
            let width = (ui.available_width() - 160.0).clamp(120.0, 320.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut name)
                    .desired_width(width)
                    .text_color(if is_edited { changed_colour } else { look.text }),
            );
            if response.changed() {
                if name.trim().is_empty() || name == entry.display_name {
                    project.names.remove(&entry.key);
                } else {
                    project.names.insert(entry.key.clone(), name);
                }
            }
            if is_edited && crate::ui::act(ui, look, "Game's name", None).clicked() {
                project.names.remove(&entry.key);
            }
        });
        ui.add_space(6.0);

        // The rest in sections, most used first, each saying how much of it is changed.
        let title = |words: &str, changed: usize| {
            let text = if changed > 0 {
                format!("{words} ({changed} changed)")
            } else {
                words.to_owned()
            };
            RichText::new(text)
                .size(15.0)
                .strong()
                .color(if changed > 0 {
                    changed_colour
                } else {
                    look.text
                })
        };
        if let Some(fighter) = entry.fighter.clone() {
            let named = project
                .per_costume_names
                .get(&fighter)
                .map_or(0, |names| names.len());
            egui::CollapsingHeader::new(title("Skins", named))
                .default_open(true)
                .id_salt(format!("costume_names_{fighter}"))
                .show(ui, |ui| {
                    self.draw_per_costume_names(ui, entry, &fighter, project, roots);
                });
        }
        let pictures = project
            .ui_images
            .get(&entry.key)
            .map_or(0, |images| images.len());
        egui::CollapsingHeader::new(title("Pictures", pictures))
            .default_open(pictures > 0)
            .id_salt(format!("images_{}", entry.key.as_str()))
            .show(ui, |ui| {
                self.draw_images(ui, entry, project, roots);
            });
        let placed = project.order.contains_key(&entry.key) as usize
            + project.chara_overrides.contains_key(&entry.key) as usize;
        egui::CollapsingHeader::new(title("Place and skin count", placed))
            .id_salt(format!("roster_row_{}", entry.key.as_str()))
            .show(ui, |ui| {
                self.draw_roster_row(ui, entry, project);
            });
        let variants = project.name_variants.get(&entry.key).map_or(0, |v| {
            v.chr0.is_some() as usize + v.chr1.is_some() as usize + v.chr2.is_some() as usize
        });
        egui::CollapsingHeader::new(title("A different name per screen", variants))
            .id_salt(format!("name_variants_{}", entry.key.as_str()))
            .show(ui, |ui| {
                self.draw_name_variants(ui, entry, project);
            });

        if entry.name_id.is_none() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(240, 200, 120), "⚠");
                ui.label(
                    RichText::new("No roster row. Add from off-roster or the New Character tab.")
                        .small()
                        .weak(),
                );
            });
        }
    }

    fn draw_name_variants(&mut self, ui: &mut Ui, entry: &RosterEntry, project: &mut RosterMod) {
        ui.add_space(4.0);
        let variants = project
            .name_variants
            .get(&entry.key)
            .cloned()
            .unwrap_or_default();
        let mut chr0 = variants.chr0.clone().unwrap_or_default();
        let mut chr1 = variants.chr1.clone().unwrap_or_default();
        let mut chr2 = variants.chr2.clone().unwrap_or_default();
        let has_detail = !variants.is_empty();
        if has_detail && ui.small_button("Clear detailed variants").clicked() {
            project.name_variants.remove(&entry.key);
            self.status = "Cleared detailed name variants. Now using Display for all.".into();
            return;
        }
        egui::Grid::new(format!("detailed_names_{}", entry.key.as_str()))
            .num_columns(3)
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("In menus").small().weak());
                let r0 = ui.add(
                    egui::TextEdit::singleline(&mut chr0)
                        .desired_width(140.0)
                        .hint_text("CSS banner"),
                );
                ui.label(RichText::new("CSS banner").small().weak());
                ui.end_row();
                ui.label(RichText::new("Select screen").small().weak());
                let r1 = ui.add(
                    egui::TextEdit::singleline(&mut chr1)
                        .desired_width(140.0)
                        .hint_text("Stock / results"),
                );
                ui.label(RichText::new("Stock / results").small().weak());
                ui.end_row();
                ui.label(RichText::new("In capitals").small().weak());
                let r2 = ui.add(
                    egui::TextEdit::singleline(&mut chr2)
                        .desired_width(140.0)
                        .hint_text("Uppercase banner"),
                );
                ui.label(RichText::new("Uppercase banner").small().weak());
                ui.end_row();

                if r0.changed() || r1.changed() || r2.changed() {
                    let mut v = crate::mod_project::NameVariants::default();
                    if !chr0.trim().is_empty() {
                        v.chr0 = Some(chr0.clone());
                    }
                    if !chr1.trim().is_empty() {
                        v.chr1 = Some(chr1.clone());
                    }
                    if !chr2.trim().is_empty() {
                        v.chr2 = Some(chr2.clone());
                    }
                    if v.is_empty() {
                        project.name_variants.remove(&entry.key);
                    } else {
                        project.name_variants.insert(entry.key.clone(), v);
                    }
                }
            });
    }

    fn draw_per_costume_names(
        &mut self,
        ui: &mut Ui,
        entry: &RosterEntry,
        fighter: &str,
        project: &mut RosterMod,
        roots: &[PathBuf],
    ) {
        // Every skin the game has on disk gets a row — no slot picker, no
        // add/remove dance. An empty field means "uses the display name";
        // typing names it, clearing un-names it. The letter dots say what
        // each skin actually holds, so missing models and untextured grey
        // read at a glance instead of requiring a folder hunt.
        let slots = costume_slots_for(
            &crate::data::discover_costume_slots(roots, fighter),
            project.per_costume_names.get(fighter),
        );
        if slots.is_empty() {
            ui.label(
                RichText::new(format!(
                    "No costume slots found on disk for {fighter}. Dump its fighter folder to work skins individually."
                ))
                .small()
                .weak(),
            );
            return;
        }
        // This entry's own files, when it is a character the project added —
        // the one folder whose contents this project owns.
        let files_root = project
            .authored
            .iter()
            .find(|a| a.key == entry.key)
            .and_then(|a| a.files_root.clone());
        ui.horizontal_wrapped(|ui| {
            if let Some(root) = files_root {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(RichText::new("Open files").small())
                        .on_hover_text(format!("Show {} in the file manager", root.display()))
                        .clicked()
                    {
                        match super::reveal::reveal(&root) {
                            Ok(()) => {
                                self.status = format!("Opened {}.", root.display());
                            }
                            Err(error) => {
                                self.status = format!("Could not open files: {error:#}");
                            }
                        }
                    }
                });
            }
        });
        ui.add_space(4.0);
        let entry_slot = entry.backing.slot().unwrap_or(0);
        let field_w = (ui.available_width() - 200.0).clamp(80.0, 220.0);
        let mut to_update: Vec<(u8, String)> = Vec::new();
        for slot in slots {
            let inv =
                crate::roster::scaffold::inventory(roots, fighter, slot, entry.name_id.as_deref());
            let has_png = project.ui_images.get(&entry.key).is_some_and(|map| {
                ["chara_1", "chara_2", "chara_0"].iter().any(|kind| {
                    map.contains_key(&crate::roster::ui_images::image_key(kind, Some(slot)))
                        || (slot == entry_slot && map.contains_key(*kind))
                })
            });
            let current = project
                .per_costume_names
                .get(fighter)
                .and_then(|m| m.get(&slot))
                .cloned()
                .unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("c{slot:02}"))
                        .small()
                        .monospace()
                        .weak(),
                );
                slot_dot(
                    ui,
                    inv.meshes > 0,
                    "M",
                    if inv.meshes > 0 {
                        format!("{} mesh file(s), {} texture(s)", inv.meshes, inv.textures)
                    } else {
                        "no model files in this slot".to_string()
                    },
                );
                slot_dot(
                    ui,
                    inv.anims > 0,
                    "A",
                    if inv.anims > 0 {
                        format!(
                            "{} animation(s){}",
                            inv.anims,
                            if inv.has_motion_list {
                                ", motion_list.bin present"
                            } else {
                                ". NO motion_list.bin, the game will not find them"
                            }
                        )
                    } else {
                        "no animations in this slot".to_string()
                    },
                );
                slot_dot(
                    ui,
                    inv.has_effect,
                    "E",
                    if inv.has_effect {
                        "own effect file for this slot".to_string()
                    } else {
                        "no effect file. Uses the donor's".to_string()
                    },
                );
                slot_dot(
                    ui,
                    inv.has_portrait || has_png,
                    "P",
                    if has_png {
                        "custom portrait picked in Images below".to_string()
                    } else if inv.has_portrait {
                        "portrait file on disk".to_string()
                    } else {
                        "no portrait yet. Pick one in Images below".to_string()
                    },
                );
                let mut edit = current;
                let is_named = project
                    .per_costume_names
                    .get(fighter)
                    .is_some_and(|m| m.contains_key(&slot));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut edit)
                        .desired_width(field_w)
                        .hint_text(entry.display_name.as_str())
                        .text_color(if is_named {
                            Color32::from_rgb(240, 200, 120)
                        } else {
                            ui.visuals().text_color()
                        }),
                );
                if resp.changed() {
                    to_update.push((slot, edit));
                }
            });
        }
        for (slot, new_name) in to_update {
            if new_name.trim().is_empty() {
                if let Some(map) = project.per_costume_names.get_mut(fighter) {
                    map.remove(&slot);
                    if map.is_empty() {
                        project.per_costume_names.remove(fighter);
                    }
                }
            } else {
                project
                    .per_costume_names
                    .entry(fighter.to_string())
                    .or_default()
                    .insert(slot, new_name);
            }
        }
    }

    fn draw_roster_row(&mut self, ui: &mut Ui, entry: &RosterEntry, project: &mut RosterMod) {
        ui.label(
            RichText::new(
                "What the game reads for CSS placement. Dragging the grid edits position too.",
            )
            .small()
            .weak(),
        );
        ui.add_space(4.0);
        let db_row = entry
            .name_id
            .as_deref()
            .and_then(|nid| self.db.as_ref().and_then(|db| db.row(nid)));
        let db_order = db_row.map(|r| r.disp_order);
        let db_color = db_row.map(|r| r.color_num);
        let db_save = db_row.map(|r| r.save_no);

        // disp_order — persistent draft so multi-digit values are typable.
        // The draft is taken out of the map for the duration of the draw so
        // the row closure can use `&mut self` (status, portrait cache)
        // without fighting the draft borrow. It is put back unless the edit
        // committed, was reset, or was invalid.
        let current_order = entry.css_order;
        let key = entry.key.clone();
        let mut draft = self.order_drafts.remove(&key).unwrap_or_else(|| {
            project
                .order
                .get(&key)
                .map(|v| v.to_string())
                .unwrap_or_else(|| current_order.map(|o| o.to_string()).unwrap_or_default())
        });
        enum OrderAfter {
            Keep,
            Drop,
            Invalid(String),
            Sentinel,
        }
        let mut after = OrderAfter::Keep;
        let mut clear_override = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Position").small().weak());
            let resp = ui.add(
                egui::TextEdit::singleline(&mut draft)
                    .desired_width(60.0)
                    .hint_text("-1..98"),
            );
            if resp.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let text = draft.trim().to_string();
                if text.is_empty() {
                    project.order.remove(&key);
                    after = OrderAfter::Drop;
                } else if let Ok(val) = text.parse::<i8>() {
                    if val == crate::roster::css::RANDOM_SLOT_ORDER {
                        after = OrderAfter::Sentinel;
                    } else if Some(val) == db_order {
                        project.order.remove(&key);
                        after = OrderAfter::Drop;
                    } else {
                        project.order.insert(key.clone(), val);
                        after = OrderAfter::Keep;
                    }
                } else {
                    after = OrderAfter::Invalid(text);
                }
            }
            if let Some(o) = current_order {
                ui.label(RichText::new(format!("now {o}")).small().weak());
            }
            if project.order.contains_key(&key)
                && ui
                    .small_button("Reset")
                    .on_hover_text("Use the game's position")
                    .clicked()
            {
                project.order.remove(&key);
                clear_override = true;
            }
        });
        match after {
            OrderAfter::Keep => {
                if clear_override {
                    // Reset button — drop the draft so it resyncs.
                } else {
                    self.order_drafts.insert(key, draft);
                }
            }
            OrderAfter::Drop => {}
            OrderAfter::Sentinel => {
                self.status = "99 is the Random slot. Pick another position.".into();
                self.order_drafts.insert(key, draft);
            }
            OrderAfter::Invalid(text) => {
                self.status = format!("“{text}” is not a position (-1..98).");
            }
        }

        // color_num / save_no
        let current_patch = project
            .chara_overrides
            .get(&entry.key)
            .cloned()
            .unwrap_or_default();
        let mut color = current_patch
            .color_num
            .unwrap_or_else(|| db_color.unwrap_or(8));
        let mut save = current_patch
            .save_no
            .unwrap_or_else(|| db_save.unwrap_or(0));
        let mut patch = current_patch.clone();
        let mut patch_touched = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Costumes offered").small().weak());
            if ui
                .add(egui::DragValue::new(&mut color).range(1..=64).speed(1))
                .on_hover_text("8 is vanilla. Higher needs a slot-expansion mod")
                .changed()
            {
                patch.color_num = if Some(color) == db_color {
                    None
                } else {
                    Some(color)
                };
                patch_touched = true;
            }
            if let Some(dbv) = db_color {
                ui.label(RichText::new(format!("was {dbv}")).small().weak());
            }
            if patch.color_num.is_some() {
                ui.colored_label(
                    Color32::from_rgb(240, 200, 120),
                    RichText::new("edited").small(),
                );
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Save slot").small().weak());
            if ui
                .add(egui::DragValue::new(&mut save).range(-1..=127).speed(1))
                .changed()
            {
                patch.save_no = if Some(save) == db_save {
                    None
                } else {
                    Some(save)
                };
                patch_touched = true;
            }
            if let Some(v) = db_save {
                ui.label(RichText::new(format!("was {v}")).small().weak());
            }
            if patch.save_no.is_some() {
                ui.colored_label(
                    Color32::from_rgb(240, 200, 120),
                    RichText::new("edited").small(),
                );
                if ui
                    .small_button("Reset")
                    .on_hover_text("Use the game's save slot")
                    .clicked()
                {
                    patch.save_no = None;
                    patch_touched = true;
                }
            }
        });
        if patch_touched {
            if patch.color_num.is_none() && patch.save_no.is_none() {
                project.chara_overrides.remove(&entry.key);
            } else {
                project.chara_overrides.insert(entry.key.clone(), patch);
            }
        }
        let _ = entry.display_name.clone();
    }

    /// Look up one stage texture, owned. The portrait cache cannot be held
    /// across two lookups, so game and custom are snapshotted one at a time.
    /// `kind` selects the game file (stocks preview stock files); a picked
    /// PNG wins regardless of kind.
    // Same traversal-bundle note as `draw_grid`.
    #[allow(clippy::too_many_arguments)]
    fn stage_tex(
        &mut self,
        ctx: &egui::Context,
        roots: &[PathBuf],
        kind: &str,
        name: &str,
        slot: u8,
        gamma: bool,
        png: Option<&std::path::Path>,
    ) -> StageTex {
        match self
            .portraits
            .get_image_with_gamma(ctx, roots, kind, name, slot, gamma, png)
        {
            Some(Ok(texture)) => StageTex::Ready(texture.id(), texture.size_vec2()),
            Some(Err(error)) => StageTex::Missing(error.to_string()),
            None => StageTex::Queued,
        }
    }

    /// The preview stage: the selected image kind large, custom next to
    /// game when an override exists. This is where a portrait is judged —
    /// the rows below only pick files and flags.
    fn draw_image_stage(
        &mut self,
        ui: &mut Ui,
        entry: &RosterEntry,
        project: &mut RosterMod,
        roots: &[PathBuf],
    ) {
        use crate::roster::ui_images as images;
        let key = entry.key.clone();
        let entry_slot = entry.backing.slot().unwrap_or(0);
        let kinds = images::UI_IMAGE_KINDS;
        // Kinds carrying any override, for the tab dots and the default.
        let mut overridden_kinds: Vec<&str> = Vec::new();
        if let Some(map) = project.ui_images.get(&key) {
            for stored in map.keys() {
                let (kk, _) = images::split_image_key(stored);
                if kinds.contains(&kk) && !overridden_kinds.contains(&kk) {
                    overridden_kinds.push(kk);
                }
            }
        }
        let mut kind = self
            .img_preview_kind
            .clone()
            .filter(|k| kinds.contains(&k.as_str()))
            .unwrap_or_else(|| {
                overridden_kinds
                    .first()
                    .map(|k| k.to_string())
                    .unwrap_or_else(|| "chara_1".to_string())
            });
        ui.horizontal_wrapped(|ui| {
            for k in kinds {
                let dot = if overridden_kinds.contains(k) {
                    " •"
                } else {
                    ""
                };
                ui.selectable_value(&mut kind, k.to_string(), format!("{k}{dot}"));
            }
        });
        self.img_preview_kind = Some(kind.clone());
        // The stage follows the row's own skin pick for this kind.
        let slot = self
            .image_slot_pick
            .get(&(key.clone(), kind.clone()))
            .copied()
            .unwrap_or(entry_slot);
        let ov = project
            .ui_images
            .get(&key)
            .and_then(|map| images::find_override(map, &kind, slot, entry_slot))
            .cloned();

        ui.add_space(4.0);
        let cache_name = entry.name_id.as_deref().unwrap_or(entry.key.as_str());
        // Game file for this kind+skin, when the entry has a roster row.
        let game_tex = match entry.name_id.as_deref() {
            Some(nid) => self.stage_tex(ui.ctx(), roots, &kind, nid, slot, true, None),
            None => StageTex::Missing("no roster row".to_string()),
        };
        // Custom PNG, when one is picked for this kind+skin.
        let custom_tex = match ov.as_ref() {
            Some(o) => self.stage_tex(
                ui.ctx(),
                roots,
                &kind,
                cache_name,
                slot,
                o.gamma_render,
                Some(std::path::Path::new(&o.png_path)),
            ),
            None => StageTex::Missing("none picked".to_string()),
        };
        let has_custom = ov.is_some();
        match (custom_tex, game_tex, has_custom) {
            (StageTex::Ready(id, px), game, true) => {
                ui.horizontal(|ui| {
                    let longest = px.x.max(px.y).max(1.0);
                    let show = px * (200.0 / longest);
                    ui.vertical(|ui| {
                        ui.image((id, show));
                        ui.label(
                            RichText::new(format!("custom {kind} c{slot:02}"))
                                .small()
                                .strong(),
                        );
                    });
                    match game {
                        StageTex::Ready(id, px) => {
                            let longest = px.x.max(px.y).max(1.0);
                            ui.vertical(|ui| {
                                ui.image((id, px * (84.0 / longest)));
                                ui.label(RichText::new(format!("game c{slot:02}")).small().weak());
                            });
                        }
                        _ => {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("no game file. Custom only").small().weak());
                            });
                        }
                    }
                });
            }
            (StageTex::Missing(error), _, true) => {
                ui.label(
                    RichText::new(format!("Can't preview: {error}"))
                        .small()
                        .weak(),
                );
            }
            (_, StageTex::Ready(id, px), false) => {
                let longest = px.x.max(px.y).max(1.0);
                ui.image((id, px * (200.0 / longest)));
                ui.label(
                    RichText::new(format!(
                        "game {kind} c{slot:02}. Pick a PNG below to replace"
                    ))
                    .small()
                    .weak(),
                );
            }
            (_, _, false) => {
                // No override picked: whatever the game lookup said, the
                // answer is "nothing to show yet" — a decode error here
                // means the file is absent, not broken.
                ui.label(
                    RichText::new(format!(
                        "No {kind} c{slot:02} anywhere yet. Pick a PNG below"
                    ))
                    .small()
                    .weak(),
                );
            }
            (StageTex::Queued, _, true) => {
                // Override picked but still queued behind the decode budget:
                // ask for another frame rather than flashing "missing".
                ui.spinner();
            }
        }
        ui.label(
            RichText::new("Change skins in the rows below. The stage follows.")
                .small()
                .weak(),
        );
    }

    fn draw_images(
        &mut self,
        ui: &mut Ui,
        entry: &RosterEntry,
        project: &mut RosterMod,
        roots: &[PathBuf],
    ) {
        ui.label(
            RichText::new("One portrait per skin: pick the costume, then its PNG. Preview fix only changes this editor's preview. Upload fix changes the exported bytes so the game shows what you saw here.")
                .small()
                .weak(),
        );
        ui.add_space(4.0);
        self.draw_image_stage(ui, entry, project, roots);
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);
        let entry_slot = entry.backing.slot().unwrap_or(0);
        // Whole fighters have many skins; a slot-backed entry is one costume
        // and needs no picker.
        let multi_slot = entry.backing.slot().is_none() && entry.fighter.is_some();
        let disk_slots: Vec<u8> = if multi_slot {
            entry
                .fighter
                .as_deref()
                .map(|f| crate::data::discover_costume_slots(roots, f))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let kinds = crate::roster::ui_images::UI_IMAGE_KINDS;
        for kind in kinds {
            let key = entry.key.clone();
            // Slots already overridden for this kind, from stored keys.
            let mut overridden: Vec<u8> = project
                .ui_images
                .get(&key)
                .map(|map| {
                    map.keys()
                        .filter_map(|k| {
                            let (kk, slot) = crate::roster::ui_images::split_image_key(k);
                            (kk == *kind).then(|| slot.unwrap_or(entry_slot))
                        })
                        .collect()
                })
                .unwrap_or_default();
            overridden.sort_unstable();
            overridden.dedup();
            let candidates = if multi_slot {
                image_slot_candidates(&disk_slots, entry_slot, &overridden)
            } else {
                vec![entry_slot]
            };
            // The pick persists per entry+kind; fall back to the entry slot
            // when the stored pick is no longer a candidate.
            let mut sel_slot = self
                .image_slot_pick
                .get(&(key.clone(), kind.to_string()))
                .copied()
                .unwrap_or(entry_slot);
            if !candidates.contains(&sel_slot) {
                sel_slot = entry_slot;
            }
            let ov = project
                .ui_images
                .get(&key)
                .and_then(|map| {
                    crate::roster::ui_images::find_override(map, kind, sel_slot, entry_slot)
                })
                .cloned();
            let has = ov.is_some();
            let png_path = ov.as_ref().map(|o| o.png_path.clone()).unwrap_or_default();
            let mut gamma_render = ov.as_ref().map(|o| o.gamma_render).unwrap_or(false);
            let mut gamma_upload = ov.as_ref().map(|o| o.gamma_upload).unwrap_or(false);
            // Storage key: bare kind for the entry's own slot (the legacy
            // spelling), suffixed for other skins.
            let stored_key = |sel: u8| {
                if sel == entry_slot {
                    kind.to_string()
                } else {
                    crate::roster::ui_images::image_key(kind, Some(sel))
                }
            };

            ui.horizontal(|ui| {
                ui.label(RichText::new(*kind).small().monospace().strong());
                if overridden.is_empty() {
                    ui.label(RichText::new("game files").small().weak());
                } else {
                    let list = overridden
                        .iter()
                        .map(|s| format!("c{s:02}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    ui.colored_label(
                        Color32::from_rgb(130, 225, 150),
                        RichText::new(format!("set {list}")).small(),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if has && ui.small_button("Clear").clicked() {
                        if let Some(map) = project.ui_images.get_mut(&key) {
                            // Drop both spellings for this slot: a legacy bare
                            // key and a suffixed one would otherwise shadow.
                            map.remove(&stored_key(sel_slot));
                            map.remove(&crate::roster::ui_images::image_key(kind, Some(sel_slot)));
                            if map.is_empty() {
                                project.ui_images.remove(&key);
                            }
                        }
                        self.portraits.clear_for(
                            entry.name_id.as_deref().unwrap_or(entry.key.as_str()),
                            sel_slot,
                        );
                    }
                    if ui.small_button("Choose PNG").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("PNG", &["png"])
                            .add_filter("Image", &["png", "jpg", "jpeg", "webp"])
                            .pick_file()
                        {
                            let display = entry.display_name.clone();
                            let cache_name = entry
                                .name_id
                                .clone()
                                .unwrap_or_else(|| entry.key.to_string());
                            let map = project.ui_images.entry(key.clone()).or_default();
                            let store = stored_key(sel_slot);
                            let is_new = !map.contains_key(&store);
                            let ov_entry = map.entry(store).or_default();
                            ov_entry.png_path = path.display().to_string();
                            // Keep this image's own gamma flags; a fresh pick
                            // previews corrected so it matches the grid, and
                            // the toggle below can compare against raw.
                            if is_new {
                                ov_entry.gamma_render = true;
                            } else {
                                ov_entry.gamma_render = gamma_render;
                            }
                            ov_entry.gamma_upload = gamma_upload;
                            self.portraits.clear_for(&cache_name, sel_slot);
                            self.status = format!("Set {kind} c{sel_slot:02} for {display}");
                        }
                    }
                });
            });
            // Which skin this row edits. Fixed for single-costume entries.
            if multi_slot {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Costume").small().weak());
                    egui::ComboBox::from_id_salt(format!("img_slot_{}_{kind}", entry.key.as_str()))
                        .selected_text(format!("c{sel_slot:02}"))
                        .show_ui(ui, |ui| {
                            for cand in &candidates {
                                let set_here = overridden.contains(cand);
                                let label = if set_here {
                                    format!("c{cand:02} • set")
                                } else {
                                    format!("c{cand:02}")
                                };
                                if ui.selectable_label(*cand == sel_slot, label).clicked() {
                                    self.image_slot_pick
                                        .insert((key.clone(), kind.to_string()), *cand);
                                }
                            }
                        });
                    ui.label(
                        RichText::new("each skin exports its own file")
                            .small()
                            .weak(),
                    );
                });
            }
            if has {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&png_path).small().weak().monospace());
                        let mut preview_changed = false;
                        if ui
                            .checkbox(&mut gamma_render, RichText::new("Preview fix").small())
                            .on_hover_text("Brighten the stage preview with gamma 2.2 (fixes dark/sRGB PNGs). Does not touch the export.")
                            .changed()
                        {
                            preview_changed = true;
                        }
                        if ui
                            .checkbox(&mut gamma_upload, RichText::new("Upload fix").small())
                            .on_hover_text("Darken when encoding so the game shows what this preview shows")
                            .changed()
                        {
                            if let Some(map) = project.ui_images.get_mut(&key) {
                                let store = stored_key(sel_slot);
                                if let Some(ov) = map.get_mut(&store) {
                                    ov.gamma_upload = gamma_upload;
                                }
                            }
                        }
                        if preview_changed {
                            if let Some(map) = project.ui_images.get_mut(&key) {
                                let store = stored_key(sel_slot);
                                if let Some(ov) = map.get_mut(&store) {
                                    ov.gamma_render = gamma_render;
                                }
                            }
                            self.portraits.clear_for(
                                entry.name_id.as_deref().unwrap_or(entry.key.as_str()),
                                sel_slot,
                            );
                        }
                    });
                });
                ui.add_space(2.0);
            }
        }
    }

    fn draw_bulk_names(&mut self, ui: &mut Ui, index: &RosterIndex, project: &mut RosterMod) {
        let visible = filtered_visible(index.visible());
        if visible.is_empty() {
            ui.label(RichText::new("No visible entries.").small().weak());
            return;
        }
        if self.focus_names {
            self.focus_names = false;
            ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover())
                .scroll_to_me(Some(egui::Align::TOP));
        }
        let renamed = project.names.len() + project.name_variants.len();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("Rename many ({})", visible.len())).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        renamed > 0,
                        egui::Button::new(RichText::new("Reset all").small()),
                    )
                    .on_hover_text("Use the game's names for every entry")
                    .clicked()
                {
                    project.names.clear();
                    project.name_variants.clear();
                    self.status = "All display names reset.".into();
                }
                if renamed > 0 {
                    ui.colored_label(
                        Color32::from_rgb(240, 200, 120),
                        RichText::new(format!("{renamed} renamed")).small(),
                    );
                }
            });
        });
        ui.add_space(4.0);
        crate::ui::search_field(
            ui,
            "roster_names_search",
            &mut self.bulk_filter,
            "Find a character",
        );
        ui.add_space(4.0);

        let query = self.bulk_filter.clone();
        let rows: Vec<&&RosterEntry> = visible
            .iter()
            .filter(|e| matches_grid_filter(e, &query))
            .collect();
        if rows.is_empty() {
            ui.label(
                RichText::new(format!("No characters match “{}”", query.trim()))
                    .small()
                    .weak(),
            );
            return;
        }
        // One line per character: current name, edit field, per-row revert.
        // The raw key lives on hover — it identified nothing for most users.
        let field_w = (ui.available_width() - 250.0).clamp(80.0, 260.0);
        egui::ScrollArea::vertical()
            .id_salt("bulk_names_scroll")
            .max_height(220.0)
            .show(ui, |ui| {
                for e in rows {
                    ui.horizontal(|ui| {
                        let shown = elide(&e.display_name, 18);
                        ui.label(RichText::new(shown).small().strong())
                            .on_hover_text(e.key.as_str());
                        let mut name = project
                            .names
                            .get(&e.key)
                            .cloned()
                            .unwrap_or_else(|| e.display_name.clone());
                        let is_edited = project.names.contains_key(&e.key);
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut name)
                                .desired_width(field_w)
                                .hint_text("display name")
                                .text_color(if is_edited {
                                    Color32::from_rgb(240, 200, 120)
                                } else {
                                    ui.visuals().text_color()
                                }),
                        );
                        if resp.changed() {
                            if name.trim().is_empty() || name == e.display_name {
                                project.names.remove(&e.key);
                            } else {
                                project.names.insert(e.key.clone(), name);
                            }
                        }
                        if is_edited {
                            if ui
                                .small_button(RichText::new("Reset").small())
                                .on_hover_text("Use the game's name for this entry")
                                .clicked()
                            {
                                project.names.remove(&e.key);
                            }
                        } else {
                            ui.label(RichText::new("·").small().weak());
                        }
                    });
                }
            });
    }

    /// Every character that is not on the screen: the ones this project hid, to bring back,
    /// and the ones the game never shows (bosses, the parts of a character), to add.
    fn draw_off_roster(&mut self, ui: &mut Ui, index: &RosterIndex, project: &mut RosterMod) {
        let look = crate::ui::look(ui);
        if self.focus_off_roster {
            self.focus_off_roster = false;
            ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover())
                .scroll_to_me(Some(egui::Align::TOP));
        }
        let mut entries = index.off_roster();
        // The half of a shared cell that is not its face (Mythra) can be added on its own too.
        let raw = index.visible();
        let faces = filtered_visible(raw.clone());
        for e in raw {
            let mii_cell = e.name_id.as_deref() == Some("miiall");
            if !mii_cell
                && !faces.iter().any(|f| f.key == e.key)
                && !entries.iter().any(|x| x.key == e.key)
            {
                entries.push(e);
            }
        }
        entries.sort_by_key(|e| (e.css_order.is_none(), e.css_order.unwrap_or(0), e.id.0));

        ui.label(
            RichText::new("Not on the screen")
                .size(19.0)
                .strong()
                .color(look.text),
        );
        crate::ui::search_field(
            ui,
            "roster_off_search",
            &mut self.off_roster_filter,
            "Find a character",
        );
        ui.add_space(4.0);
        let query = self.off_roster_filter.clone();
        let (hidden, never): (Vec<&RosterEntry>, Vec<&RosterEntry>) = entries
            .into_iter()
            .filter(|e| matches_grid_filter(e, &query))
            .partition(|e| e.hidden);
        if hidden.is_empty() && never.is_empty() {
            ui.label(RichText::new("None found.").color(look.dim));
            return;
        }
        let row = |ui: &mut Ui, name: &str, note: &str, button: &str, hover: &str| -> bool {
            ui.horizontal(|ui| {
                ui.label(RichText::new(elide(name, 22)).strong().color(look.text));
                ui.label(RichText::new(note).color(look.dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    crate::ui::act(ui, look, button, None)
                        .on_hover_text(hover)
                        .clicked()
                })
                .inner
            })
            .inner
        };
        egui::ScrollArea::vertical().id_salt("roster_css_off").max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
            if !hidden.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Hidden by this project ({})", hidden.len())).strong().color(look.dim));
                    if crate::ui::act(ui, look, "Bring all back", None).clicked() {
                        for e in &hidden {
                            project.hidden.remove(&e.key);
                        }
                        self.status = format!("Brought back {} hidden characters.", hidden.len());
                    }
                });
                for entry in &hidden {
                    let note = entry.css_order.map_or(String::new(), |order| format!("place {order}"));
                    if row(ui, &entry.display_name, &note, "Bring back", "Put it back on the select screen") {
                        project.hidden.remove(&entry.key);
                        self.status = format!("Brought back {}.", entry.display_name);
                    }
                }
                ui.add_space(6.0);
            }
            if !never.is_empty() {
                ui.label(RichText::new(format!("Never shown by the game ({})", never.len())).strong().color(look.dim));
                for entry in &never {
                    let note = entry.name_id.as_deref().and_then(part_of).unwrap_or("");
                    if entry.name_id.is_none() {
                        if row(ui, &entry.display_name, "no entry", "New character", "This fighter has no select screen entry. Make one on the New character page.") {
                            self.goto_new_character_tab = true;
                        }
                    } else if row(ui, &entry.display_name, note, "Add", "Put it on the select screen, after the last character") {
                        let next = next_free_order(index);
                        project.order.insert(entry.key.clone(), next);
                        project.hidden.remove(&entry.key);
                        self.order_drafts.remove(&entry.key);
                        self.status = format!("Added {} at place {next}.", entry.display_name);
                    }
                }
            }
        });
    }

    /// Hide the selected entries. Restoring happens in the off-roster
    /// list — which is opened and scrolled to, so the entries don't vanish
    /// into a collapsed section. Clearing one entry's edits happens in its
    /// detail panel.
    fn hide_selected(&mut self, index: &RosterIndex, project: &mut RosterMod) {
        if self.selected.is_empty() {
            return;
        }
        let visible_len = filtered_visible(index.visible()).len();
        if visible_len.saturating_sub(self.selected.len()) < 8 {
            self.status = "The select screen needs at least 8 fighters.".into();
            return;
        }
        let count = self.selected.len();
        for key in self.selected.iter().cloned().collect::<Vec<_>>() {
            project.hidden.insert(key);
        }
        self.show_off_roster = true;
        self.focus_off_roster = true;
        self.status = if count == 1 {
            let key = self.selected.iter().next().unwrap();
            let name = index
                .by_key(key)
                .map(|e| e.display_name.as_str())
                .unwrap_or("Selection");
            format!("{name} hidden. Restore it from the off-roster list below.")
        } else {
            format!("{count} fighters hidden. Restore them from the off-roster list below.")
        };
    }
}

fn next_free_order(index: &RosterIndex) -> i8 {
    let mut max = -1i8;
    for e in index.visible() {
        if let Some(o) = e.css_order {
            if o != crate::roster::css::OFF_ROSTER
                && o != crate::roster::css::RANDOM_SLOT_ORDER
                && o > max
            {
                max = o;
            }
        }
    }
    let mut next = max.saturating_add(1);
    if next == crate::roster::css::RANDOM_SLOT_ORDER {
        next = next.saturating_add(1);
    }
    if next == crate::roster::css::OFF_ROSTER {
        next = 0;
    }
    // i8 holds 0..=127 and 99 is the Random sentinel — allow growth past
    // the vanilla max (~86) for mods that add rows. The old clamp(0, 86)
    // silently pinned every added row onto a vanilla position.
    next.clamp(0, 98)
}

/// Compute the position overrides that moving entry `from` to index `to` implies.
///
/// Returns `(key, Some(position))` for an entry that needs an override and `(key, None)` for
/// one whose override should be dropped because the database already puts it there. Keeping
/// the project sparse matters: an override that agrees with the base file pins a value that
/// would otherwise track the mods underneath it.
///
/// Every displaced entry is renumbered, not just the dragged one. `disp_order` is an absolute
/// index, so moving one character without renumbering the ones it pushed aside would leave two
/// characters claiming a cell — which the game resolves by drawing one of them and not the
/// other, silently.
///
/// The positions in use are reused in ascending order rather than densely renumbered from
/// zero. Vanilla's sequence has gaps and duplicates (Pyra and Mythra share 80, and the Random
/// slot sits at 99), and rewriting every character's position to close them would produce an
/// enormous diff against the base file for a one-character move.
fn renumber(
    entries: &[&RosterEntry],
    from: usize,
    to: usize,
    database_position: impl Fn(&RosterEntry) -> Option<i8>,
) -> Vec<(RosterKey, Option<i8>)> {
    if from == to || from >= entries.len() || to >= entries.len() {
        return Vec::new();
    }
    let mut sequence: Vec<&RosterEntry> = entries.to_vec();
    let moved = sequence.remove(from);
    sequence.insert(to, moved);

    let mut positions: Vec<i8> = entries.iter().filter_map(|entry| entry.css_order).collect();
    positions.sort_unstable();
    positions.dedup();

    let mut changes = Vec::new();
    for (slot, entry) in sequence.iter().enumerate() {
        let Some(position) = positions.get(slot).copied() else {
            // More entries than distinct positions — every remaining one keeps what it had
            // rather than being pushed off the end of the sequence.
            break;
        };
        if database_position(entry) == Some(position) {
            changes.push((entry.key.clone(), None));
        } else {
            changes.push((entry.key.clone(), Some(position)));
        }
    }
    changes
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn elide(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max.saturating_sub(1)).collect::<String>() + "\u{2026}"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::{EntryOrigin, RosterBacking, RosterEntryId};

    fn entry(name: &str, order: i8) -> RosterEntry {
        RosterEntry {
            id: RosterEntryId(0),
            key: RosterKey::fighter(name),
            name_id: Some(name.to_string()),
            backing: RosterBacking::Fighter,
            fighter: Some(name.to_string()),
            display_name: name.to_string(),
            css_order: Some(order),
            providers: Vec::new(),
            origin: EntryOrigin::Vanilla,
            hidden: false,
            on_roster: true,
        }
    }

    /// The database positions the entries started at — what a project with no edits looks like.
    fn as_loaded(entry: &RosterEntry) -> Option<i8> {
        match entry.key.as_str() {
            "mario" => Some(0),
            "link" => Some(1),
            "samus" => Some(2),
            "fox" => Some(3),
            _ => None,
        }
    }

    fn roster() -> Vec<RosterEntry> {
        vec![
            entry("mario", 0),
            entry("link", 1),
            entry("samus", 2),
            entry("fox", 3),
        ]
    }

    fn changes(from: usize, to: usize) -> Vec<(String, Option<i8>)> {
        let owned = roster();
        let refs: Vec<&RosterEntry> = owned.iter().collect();
        renumber(&refs, from, to, as_loaded)
            .into_iter()
            .map(|(key, position)| (key.to_string(), position))
            .collect()
    }

    /// Moving one character renumbers everyone it displaced. Writing only the dragged entry
    /// would leave two characters claiming one cell, which the game resolves by drawing one of
    /// them and not the other.
    #[test]
    fn a_move_renumbers_every_displaced_entry() {
        // fox (3) dragged onto mario (0): fox, mario, link, samus.
        let result = changes(3, 0);
        assert_eq!(
            result,
            vec![
                ("fox".into(), Some(0)),
                ("mario".into(), Some(1)),
                ("link".into(), Some(2)),
                ("samus".into(), Some(3)),
            ]
        );
    }

    /// Entries the move did not disturb must not gain an override. An override that agrees
    /// with the base file pins a value that would otherwise track the mods underneath it.
    #[test]
    fn entries_the_move_did_not_disturb_get_no_override() {
        // link (1) and samus (2) swap; mario and fox keep their database positions.
        let result = changes(2, 1);
        assert_eq!(
            result,
            vec![
                ("mario".into(), None),
                ("samus".into(), Some(1)),
                ("link".into(), Some(2)),
                ("fox".into(), None),
            ]
        );
    }

    /// Dragging something back where it started must clear the overrides, not write a second
    /// set that happens to match.
    #[test]
    fn moving_an_entry_back_clears_every_override() {
        let owned = roster();
        let refs: Vec<&RosterEntry> = owned.iter().collect();
        let there = renumber(&refs, 3, 0, as_loaded);
        assert!(there.iter().all(|(_, position)| position.is_some()));

        // Now the roster as it looks after that move, dragged back.
        let moved = [
            entry("fox", 0),
            entry("mario", 1),
            entry("link", 2),
            entry("samus", 3),
        ];
        let refs: Vec<&RosterEntry> = moved.iter().collect();
        let back = renumber(&refs, 0, 3, as_loaded);
        assert!(
            back.iter().all(|(_, position)| position.is_none()),
            "returning to the original order left overrides behind: {back:?}"
        );
    }

    #[test]
    fn a_move_onto_itself_changes_nothing() {
        assert!(changes(2, 2).is_empty());
    }

    /// Vanilla's sequence has gaps and duplicates — Pyra and Mythra share 80, and the Random
    /// slot sits at 99. Renumbering densely from zero would rewrite every character's position
    /// for a one-character move, and would move entries into positions other rows occupy.
    #[test]
    fn existing_positions_are_reused_rather_than_densely_renumbered() {
        let owned = [entry("a", 10), entry("b", 20), entry("c", 30)];
        let refs: Vec<&RosterEntry> = owned.iter().collect();
        let result: Vec<Option<i8>> = renumber(&refs, 2, 0, |_| None)
            .into_iter()
            .map(|(_, position)| position)
            .collect();
        assert_eq!(result, vec![Some(10), Some(20), Some(30)]);
    }

    /// Two entries sharing a cell contribute one position, so the sequence is longer than the
    /// position list. The tail must keep what it had rather than being pushed off the end.
    #[test]
    fn a_shared_position_does_not_push_the_tail_off_the_sequence() {
        let owned = [entry("pyra", 80), entry("mythra", 80), entry("kazuya", 81)];
        let refs: Vec<&RosterEntry> = owned.iter().collect();
        let result = renumber(&refs, 2, 0, |_| None);
        assert_eq!(result.len(), 2, "an entry was dropped from the sequence");
        assert_eq!(result[0].1, Some(80));
        assert_eq!(result[1].1, Some(81));
    }

    #[test]
    fn labels_shorten_without_losing_the_start_of_the_name() {
        assert_eq!(elide("Mario", 11), "Mario");
        assert_eq!(elide("Pok\u{e9}mon Trainer", 11), "Pok\u{e9}mon Tr\u{2026}");
        assert_eq!(initials("Dark Samus"), "DS");
        assert_eq!(initials("Mario"), "M");
    }

    /// The layout fills the row: columns from the window, cells stretched to
    /// fit with no leftover gap. A filler column or a sideways scrollbar
    /// would mean the math drifted from what the grid draws.
    #[test]
    fn thirteen_cells_fill_the_row_until_they_would_be_too_small_to_read() {
        for width in [800.0_f32, 1100.0, 1400.0, 1900.0] {
            let cell = cell_width(width);
            let row = GAME_COLUMNS as f32 * cell + (GAME_COLUMNS - 1) as f32 * GRID_SPACING;
            assert!(
                row <= width - 3.9 && row > width - 4.1,
                "{width}: row of {row}"
            );
        }
        // Narrower than that the cells stop shrinking and the grid scrolls; wider, they stop
        // growing and the page keeps its margins.
        assert_eq!(cell_width(300.0), CELL_MIN_W);
        assert_eq!(cell_width(4000.0), CELL_MAX_W);
    }

    #[test]
    fn the_grid_filter_matches_names_keys_and_name_ids() {
        let mut jiggs = entry("purin", 5);
        jiggs.display_name = "Jigglypuff".into();
        assert!(matches_grid_filter(&jiggs, ""));
        assert!(matches_grid_filter(&jiggs, "jiggly"));
        assert!(matches_grid_filter(&jiggs, "JIGGLY"));
        // "purin" is nowhere in the display name — it matches via the key
        // and name_id, which is the whole point of searching all three.
        assert!(matches_grid_filter(&jiggs, "purin"));
        assert!(!matches_grid_filter(&jiggs, "kirby"));
    }

    /// The slot list is the disk scan plus any override the scan missed —
    /// sorted, deduplicated, and never dropping a named slot whose files
    /// moved.
    #[test]
    fn costume_rows_cover_disk_slots_and_orphaned_overrides() {
        let overrides: std::collections::BTreeMap<u8, String> =
            [(2, "Alt".into()), (9, "Extra".into())]
                .into_iter()
                .collect();
        assert_eq!(
            costume_slots_for(&[0, 1, 2, 7], Some(&overrides)),
            vec![0, 1, 2, 7, 9]
        );
        assert_eq!(costume_slots_for(&[], Some(&overrides)), vec![2, 9]);
        assert_eq!(costume_slots_for(&[0, 1], None), vec![0, 1]);
        assert!(costume_slots_for(&[], None).is_empty());
    }

    /// Image rows offer the entry slot plus every skin on disk and every
    /// skin already overridden — merged, sorted, deduplicated.
    #[test]
    fn image_rows_offer_every_skin_once() {
        assert_eq!(image_slot_candidates(&[0, 1, 7], 0, &[2]), vec![0, 1, 2, 7]);
        assert_eq!(image_slot_candidates(&[], 3, &[]), vec![3]);
    }
}
