//! The trait editor: fighter-wide values, grouped and explained.
//!
//! Two things are always visible here because both are easy to get wrong silently:
//!
//!  * **What the value was.** Every field shows the base file's value beside the edited one,
//!    so an override is never mistaken for the game's own number.
//!  * **Who it affects.** These values are keyed by fighter and nothing below it, so a
//!    slot-backed character shares every one of them with its donor. That is stated once and
//!    prominently rather than as a per-field badge, because a badge on some fields would imply
//!    the unbadged ones are safe. None are.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use egui::{Color32, RichText, Ui};

use crate::mod_project::{ParamMod, ParamValue};

use super::index::RosterIndex;
use super::traits::{self, FighterTraits};
use super::RosterKey;

#[derive(Default)]
pub struct TraitsView {
    /// The entry whose traits are shown.
    selected: Option<RosterKey>,
    /// Narrows the fighter list.
    fighter_filter: String,
    /// The loaded row, and which fighter it belongs to.
    loaded: Option<FighterTraits>,
    load_error: Option<String>,
    /// Text being typed, per field. Held apart from the committed value so a half-typed
    /// number does not get written into the project on every keystroke.
    drafts: HashMap<String, String>,
    show_all_fields: bool,
    filter: String,
    status: String,
    /// Inside width of a value card, set so a row of cards spans the page.
    card_width: f32,
    /// Another page picked the fighter: bring it into view in the list once.
    reveal_selected: bool,
}

impl TraitsView {
    /// Drop the loaded file. Called when the library changes, since a mod may now provide it.
    /// Show `key`'s values next.
    pub fn select(&mut self, key: RosterKey) {
        self.selected = Some(key);
        self.reveal_selected = true;
    }

    pub fn invalidate(&mut self) {
        self.loaded = None;
        self.load_error = None;
        self.drafts.clear();
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        roots: &[PathBuf],
        labels: &HashMap<u64, String>,
        index: &RosterIndex,
        params: &mut BTreeMap<String, ParamMod>,
        faces: &HashMap<String, egui::TextureHandle>,
    ) {
        let Some(path) = FighterTraits::locate(roots) else {
            self.draw_missing_file(ui);
            return;
        };
        // The fighters down the side and the picked one's values beside them; stacked when
        // the page is narrow.
        let width = ui.available_width();
        if width >= 760.0 {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(250.0);
                    let height = ui.available_height() - 40.0;
                    self.draw_fighter_list(ui, index, params, faces, height);
                });
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    // Fixed both ways so the value cards wrap at the page's edge.
                    ui.set_width(width - 264.0);
                    ui.set_max_width(width - 264.0);
                    self.draw_values(ui, &path, labels, index, params);
                });
            });
        } else {
            self.draw_fighter_list(ui, index, params, faces, 220.0);
            ui.add_space(10.0);
            self.draw_values(ui, &path, labels, index, params);
        }
    }

    /// Every fighter, with how many of its values the project changes; click one to edit it.
    fn draw_fighter_list(
        &mut self,
        ui: &mut Ui,
        index: &RosterIndex,
        params: &BTreeMap<String, ParamMod>,
        faces: &HashMap<String, egui::TextureHandle>,
        height: f32,
    ) {
        let look = crate::ui::look(ui);
        crate::ui::search_field(
            ui,
            "traits_fighter_search",
            &mut self.fighter_filter,
            "Find a fighter",
        );
        ui.add_space(4.0);
        let wanted = self.fighter_filter.trim().to_lowercase();
        let mut entries: Vec<&super::RosterEntry> = index
            .sorted()
            .into_iter()
            .filter(|entry| entry.fighter.is_some() && !entry.backing.shares_engine_fighter())
            .filter(|entry| {
                wanted.is_empty() || entry.display_name.to_lowercase().contains(&wanted)
            })
            .collect();
        // Fighters this project changes come first, then by name.
        entries.sort_by_cached_key(|entry| {
            let changed = entry
                .fighter
                .as_ref()
                .and_then(|f| params.get(f))
                .is_some_and(|p| p.field_count() > 0);
            (!changed, entry.display_name.to_lowercase())
        });
        if self.selected.is_none() {
            self.selected = entries.first().map(|entry| entry.key.clone());
        }
        egui::ScrollArea::vertical()
            .id_salt("traits_fighters")
            .max_height(height.max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for entry in entries {
                    let fighter = entry.fighter.clone().unwrap_or_default();
                    let changed = params.get(&fighter).map_or(0, |p| p.field_count());
                    let picked = self.selected.as_ref() == Some(&entry.key);
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 40.0),
                        egui::Sense::click(),
                    );
                    let painter = ui.painter();
                    let fill = if picked {
                        look.accent.gamma_multiply(0.35)
                    } else if response.hovered() {
                        look.sunk
                    } else {
                        look.card
                    };
                    painter.rect_filled(rect, 6.0, fill);
                    let face = egui::Rect::from_min_size(
                        rect.min + egui::vec2(4.0, 4.0),
                        egui::vec2(32.0, 32.0),
                    );
                    if let Some(texture) = faces.get(&fighter) {
                        egui::Image::new(texture)
                            .corner_radius(4.0)
                            .paint_at(ui, face);
                    }
                    let painter = ui.painter();
                    painter.text(
                        egui::pos2(face.right() + 8.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &entry.display_name,
                        egui::FontId::proportional(15.0),
                        look.text,
                    );
                    if changed > 0 {
                        painter.text(
                            egui::pos2(rect.right() - 8.0, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            changed.to_string(),
                            egui::FontId::proportional(14.0),
                            look.accent,
                        );
                    }
                    if picked && std::mem::take(&mut self.reveal_selected) {
                        response.scroll_to_me(Some(egui::Align::Center));
                    }
                    if response.clicked() {
                        self.selected = Some(entry.key.clone());
                    }
                }
            });
    }

    fn draw_values(
        &mut self,
        ui: &mut Ui,
        path: &std::path::Path,
        labels: &HashMap<u64, String>,
        index: &RosterIndex,
        params: &mut BTreeMap<String, ParamMod>,
    ) {
        let look = crate::ui::look(ui);
        let path = path.to_path_buf();
        let Some(key) = self.selected.clone() else {
            ui.label(
                RichText::new("No fighter picked")
                    .size(19.0)
                    .strong()
                    .color(crate::ui::look(ui).dim),
            );
            return;
        };
        let Some(entry) = index.by_key(&key) else {
            return;
        };
        let Some(fighter) = entry.fighter.clone() else {
            ui.label(RichText::new("No fighter behind this entry").color(look.dim));
            return;
        };
        // The game keeps one set of values per fighter: characters made on this one use them.
        let sharing: Vec<&str> = index
            .entries
            .iter()
            .filter(|other| {
                other.backing.shares_engine_fighter() && other.fighter.as_ref() == Some(&fighter)
            })
            .map(|other| other.display_name.as_str())
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(&entry.display_name)
                    .size(24.0)
                    .strong()
                    .color(look.text),
            );
            if !sharing.is_empty() {
                ui.label(
                    RichText::new(format!("Also used by {}", sharing.join(", ")))
                        .color(look.accent)
                        .strong(),
                )
                .on_hover_text(
                    "The game keeps one set of values per fighter; every costume uses them",
                );
            }
        });

        // Reload when the selection moved to a different fighter.
        if self.loaded.as_ref().map(|traits| traits.fighter.as_str()) != Some(fighter.as_str()) {
            self.drafts.clear();
            match FighterTraits::open(&path, &fighter, labels) {
                Ok(traits) => {
                    self.loaded = Some(traits);
                    self.load_error = None;
                }
                Err(error) => {
                    self.loaded = None;
                    self.load_error = Some(format!("{error:#}"));
                }
            }
        }

        if let Some(error) = &self.load_error.clone() {
            ui.label(RichText::new(error).color(Color32::from_rgb(255, 120, 120)));
            return;
        }
        // Lifted out of `self` for the duration of the draw and put back below: the field
        // rows need `&mut self` for the per-field text drafts, and the loaded row at the same
        // time. Taking it is cheaper and clearer than splitting the state into two structs.
        let Some(loaded) = self.loaded.take() else {
            return;
        };

        let edits = params.entry(fighter.clone()).or_default();
        let count = traits::edits_for(edits).len();

        ui.horizontal_wrapped(|ui| {
            let all = self.show_all_fields;
            if crate::ui::act(ui, look, "Every value", all.then_some(look.accent))
                .on_hover_text(
                    "The sections hold the values most edits touch; this lists all of them",
                )
                .clicked()
            {
                self.show_all_fields = !all;
            }
            ui.scope(|ui| {
                ui.set_width(200.0);
                crate::ui::search_field(ui, "traits_search", &mut self.filter, "Find a value");
            });
            let reset = ui
                .add_enabled_ui(count > 0, |ui| crate::ui::act(ui, look, "Reset all", None))
                .inner;
            if reset
                .on_hover_text("Every value back to the game's")
                .clicked()
            {
                edits.files.remove(traits::FIGHTER_PARAM_PATH);
                self.drafts.clear();
                self.status = format!("{fighter} is back to the game's values.");
            }
            if count > 0 {
                ui.label(RichText::new(format!("{count} changed")).color(look.accent));
            }
        });
        ui.add_space(6.0);

        let mut pending: Vec<(String, Option<ParamValue>)> = Vec::new();
        // No inner scroll: the window already scrolls vertically, and a second
        // vertical scroller inside it fights for the wheel.
        if self.show_all_fields {
            self.draw_all_fields(ui, &loaded, edits, &mut pending);
        } else {
            self.draw_sections(ui, &loaded, edits, &mut pending);
        }

        for (key, value) in pending {
            let base = loaded.get(&key).copied();
            match value {
                Some(value) => {
                    traits::record_edit(edits, &key, value, base);
                    // The committed text may spell the value differently from
                    // how it renders ("3" vs "3.0") — drop the draft so the
                    // field shows the canonical spelling next frame.
                    self.drafts.remove(&key);
                }
                None => {
                    if let Some(file) = edits.files.get_mut(traits::FIGHTER_PARAM_PATH) {
                        file.remove(&key);
                    }
                    self.drafts.remove(&key);
                }
            }
        }
        if params.get(&fighter).is_some_and(ParamMod::is_empty) {
            params.remove(&fighter);
        }
        self.loaded = Some(loaded);

        if !self.status.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(&self.status).color(look.text));
        }
    }

    fn draw_missing_file(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(230, 180, 80), RichText::new("⚠").size(18.0));
                    ui.label(RichText::new("No fighter values file found").heading());
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "Weight, gravity, speeds, and the rest live in one shared file, {}. Dump the \
                         fighter/common folder along with the rest of fighter/ to edit them.",
                        traits::FIGHTER_PARAM_PATH
                    ))
                    .small(),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "This file is fighter/common/param/fighter_param.prc. Without it, trait edits cannot be loaded or exported \
                         — the project will keep them, but the export will note which values could not be written.",
                    )
                    .small()
                    .weak(),
                );
            });
    }

    fn draw_sections(
        &mut self,
        ui: &mut Ui,
        loaded: &FighterTraits,
        edits: &ParamMod,
        pending: &mut Vec<(String, Option<ParamValue>)>,
    ) {
        let filter = self.filter.to_lowercase();
        let mut any_visible = false;
        for section in traits::SECTIONS {
            let matching: Vec<&traits::TraitField> = section
                .fields
                .iter()
                .filter(|field| {
                    filter.is_empty()
                        || field.key.to_lowercase().contains(&filter)
                        || field.label.to_lowercase().contains(&filter)
                })
                .collect();
            if matching.is_empty() {
                continue;
            }
            any_visible = true;
            let look = crate::ui::look(ui);
            let edited_in_section = matching
                .iter()
                .filter(|f| traits::edits_for(edits).contains_key(f.key))
                .count();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(section.title)
                        .size(18.0)
                        .strong()
                        .color(look.text),
                )
                .on_hover_text(section.description);
                if edited_in_section > 0 {
                    ui.label(
                        RichText::new(format!("{edited_in_section} changed"))
                            .color(Color32::from_rgb(240, 190, 70)),
                    );
                }
            });
            ui.add_space(4.0);
            // As many cards to a row as fit, in rows: egui's wrapping row measures a framed
            // child only after placing it, so it ran off the edge instead of wrapping.
            let per_row = self.cards_per_row(ui.available_width());
            for row in matching.chunks(per_row) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    for field in row {
                        self.draw_field(
                            ui,
                            loaded,
                            edits,
                            field.key,
                            field.label,
                            Some(field.description),
                            pending,
                        );
                    }
                });
                ui.add_space(6.0);
            }
            ui.add_space(8.0);
        }
        if !any_visible && !filter.is_empty() {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(format!("No fields match \"{}\"", self.filter)).weak());
            });
        }
    }

    /// How many value cards fit in `width`, stretching them to fill it.
    fn cards_per_row(&mut self, width: f32) -> usize {
        const GAP: f32 = 8.0;
        let per_row = ((width + GAP) / (196.0 + GAP)).floor().max(1.0);
        // The frame adds an 8 px margin and a stroke of up to 2 px on each side.
        self.card_width = ((width - GAP * (per_row - 1.0)) / per_row - 20.0).floor();
        per_row as usize
    }

    fn draw_all_fields(
        &mut self,
        ui: &mut Ui,
        loaded: &FighterTraits,
        edits: &ParamMod,
        pending: &mut Vec<(String, Option<ParamValue>)>,
    ) {
        let filter = self.filter.to_lowercase();
        let keys: Vec<String> = loaded
            .values()
            .iter()
            .map(|value| value.key.clone())
            .filter(|key| filter.is_empty() || key.to_lowercase().contains(&filter))
            .collect();
        let look = crate::ui::look(ui);
        ui.label(
            RichText::new(format!("Every value ({})", keys.len()))
                .size(18.0)
                .strong()
                .color(look.text),
        );
        ui.add_space(4.0);
        let per_row = self.cards_per_row(ui.available_width());
        for row in keys.chunks(per_row) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for key in row {
                    self.draw_field(ui, loaded, edits, key, key, None, pending);
                }
            });
            ui.add_space(6.0);
        }
    }

    // One row needs ui, data, edits, key, label, description, draft state,
    // and the pending queue — the traversal bundle again, not a real smell.
    #[allow(clippy::too_many_arguments)]
    fn draw_field(
        &mut self,
        ui: &mut Ui,
        loaded: &FighterTraits,
        edits: &ParamMod,
        key: &str,
        label: &str,
        description: Option<&str>,
        pending: &mut Vec<(String, Option<ParamValue>)>,
    ) {
        let Some(base) = loaded.get(key).copied() else {
            return;
        };
        let edited = traits::edits_for(edits).get(key).copied();
        let current = edited.unwrap_or(base);
        let is_edited = edited.is_some();

        let look = crate::ui::look(ui);
        let changed = Color32::from_rgb(240, 190, 70);
        egui::Frame::new()
            .fill(look.sunk)
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .stroke(egui::Stroke::new(
                if is_edited { 2.0 } else { 1.0 },
                if is_edited { changed } else { look.edge },
            ))
            .show(ui, |ui| {
                ui.set_width(self.card_width.max(150.0));
                ui.vertical(|ui| {
                    let name = ui.label(RichText::new(label).strong().color(if is_edited {
                        changed
                    } else {
                        look.text
                    }));
                    if let Some(description) = description {
                        name.on_hover_text(description);
                    }
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        let draft = self
                            .drafts
                            .entry(key.to_string())
                            .or_insert_with(|| render(current));
                        let resp = ui.add(
                            egui::TextEdit::singleline(draft)
                                .desired_width(90.0)
                                .font(egui::TextStyle::Monospace),
                        );
                        if resp.lost_focus() || resp.ctx.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            match parse(draft, base) {
                                Some(value) if Some(value) != edited.or(Some(base)) => {
                                    pending.push((key.to_string(), Some(value)));
                                }
                                Some(_) => {
                                    *draft = render(current);
                                }
                                None => {
                                    *draft = render(current);
                                }
                            }
                        }
                        if is_edited
                            && crate::ui::act(ui, look, "Undo", None)
                                .on_hover_text(format!("Back to the game's {}", render(base)))
                                .clicked()
                        {
                            pending.push((key.to_string(), None));
                        }
                    });
                });
            });
    }
}

/// How a value is shown and typed.
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

/// Parse typed text back into the type the field already holds.
///
/// The base value decides the type rather than the text: typing `3` into a decimal field means
/// `3.0`, not an integer that a later write would have to guess about.
fn parse(text: &str, base: ParamValue) -> Option<ParamValue> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The typed text is interpreted as the field's own type, not guessed from the text. `3`
    /// in a decimal field is `3.0`; an integer there would have to be re-guessed on write.
    #[test]
    fn typed_text_takes_the_type_of_the_field_it_is_typed_into() {
        assert_eq!(
            parse("3", ParamValue::Float(1.0)),
            Some(ParamValue::Float(3.0))
        );
        assert_eq!(parse("3", ParamValue::I32(1)), Some(ParamValue::I32(3)));
        assert_eq!(parse("3.5", ParamValue::I32(1)), None);
    }

    #[test]
    fn yes_no_fields_accept_the_words_people_type() {
        for text in ["true", "yes", "on", "1"] {
            assert_eq!(
                parse(text, ParamValue::Bool(false)),
                Some(ParamValue::Bool(true))
            );
        }
        for text in ["false", "no", "off", "0"] {
            assert_eq!(
                parse(text, ParamValue::Bool(true)),
                Some(ParamValue::Bool(false))
            );
        }
        assert_eq!(parse("maybe", ParamValue::Bool(true)), None);
    }

    /// A value has to survive being shown and typed back unchanged, or every field would
    /// register as edited the moment it is focused.
    #[test]
    fn every_value_type_round_trips_through_its_own_display() {
        let values = [
            ParamValue::Float(98.0),
            ParamValue::Float(0.10978),
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
            assert_eq!(parse(&render(value), value), Some(value), "{value:?}");
        }
    }
}
