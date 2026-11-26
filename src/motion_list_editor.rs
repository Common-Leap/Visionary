//! The Motion List window: every field of `motion_list.bin` / `motion_list.yml`.
//!
//! Ported from SSBH Editor's motion list editor. Edits show at once in the move list's frame
//! data (the cancel frame is the FAF source), and reach disk on Save.

use std::path::{Path, PathBuf};

use egui::{Color32, DragValue, Grid, Id, RichText, ScrollArea, TextEdit, Ui};
use rfd::FileDialog;

use crate::motion_list::{
    flag_mut, insert_motion_after, label, new_motion, parse_hash, rename_motion, unique_key, Extra,
    Hash40, Motion, MotionListFile, SavePlace, FLAG_DESCRIPTIONS, FLAG_NAMES, MAX_ANIMATIONS,
};

const SAVE_HELP: &str = "The game reads a motion list when the fighter loads, so a saved change \
                         shows in game once the mod is installed and the match is started again.";
const NOWHERE_HELP: &str =
    "This is the game's own file, or an installed mod's. Open a project and \
                            Save keeps the change there, or use Save As.";

const XLU_HELP: &str = "Intangibility frames used by some fighter motions.";
const CANCEL_HELP: &str = "The frame where the motion can be interrupted by other actions (first \
                           actionable frame). 0 uses the end of the animation.";
const BLEND_HELP: &str = "Number of frames used to blend from the previous motion into this one.";
const NO_STOP_INTP_HELP: &str = "Keep interpolating while hitstop pauses the motion.";
/// Width of the label column, shared by every labelled grid so their fields line up.
const LABEL_WIDTH: f32 = 96.0;
const SECTION_SPACING: f32 = 8.0;
const ERROR_COLOR: Color32 = Color32::from_rgb(230, 110, 100);

/// The open motion list and the window's own state.
pub struct MotionListEditor {
    pub file: MotionListFile,
    pub selected: Option<Hash40>,
    search: String,
    message: Option<String>,
    /// Scroll the list to the selected entry on the next frame.
    scroll_to_selected: bool,
    /// Waiting on the user to confirm discarding edits before a reload.
    confirm_reload: bool,
    /// Waiting on the user to confirm a save that writes a YAML file without its comments.
    confirm_comments: bool,
    /// The window was asked to close with unsaved edits: waiting on what to do with them.
    confirm_close: bool,
}

/// What a frame of the window did, for the app to act on.
#[derive(Default)]
pub struct EditorResponse {
    pub open: bool,
    /// The list changed in memory; derived data (cancel frames) should be refreshed.
    pub changed: bool,
    pub saved: Option<PathBuf>,
    /// The close that was held for unsaved edits may go ahead.
    pub close_app: bool,
}

impl MotionListEditor {
    pub fn new(file: MotionListFile) -> Self {
        Self {
            file,
            selected: None,
            search: String::new(),
            message: None,
            scroll_to_selected: false,
            confirm_reload: false,
            confirm_comments: false,
            confirm_close: false,
        }
    }

    /// Hold the app's close for the unsaved edits: the window asks what to do with them.
    pub fn ask_before_close(&mut self) {
        self.confirm_close = true;
    }

    pub fn select(&mut self, key: Hash40) {
        self.selected = Some(key);
        self.scroll_to_selected = true;
        self.message = None;
    }

    pub fn show(&mut self, ctx: &egui::Context, anim_names: &[String]) -> EditorResponse {
        let mut open = true;
        let mut changed = false;
        let mut saved = None;
        let mut close_app = false;

        let parent = self
            .file
            .path
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title = format!("Motion List ({parent}/{})", self.file.file_name());

        egui::Window::new(title)
            .id(Id::unique("motion_list_editor"))
            .open(&mut open)
            .resizable(true)
            .default_size([860.0, 620.0])
            .show(ctx, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        let can_save = self.file.place != SavePlace::Nowhere;
                        let save = ui
                            .add_enabled(can_save, egui::Button::new("Save"))
                            .on_hover_text(SAVE_HELP)
                            .on_disabled_hover_text(NOWHERE_HELP);
                        if save.clicked() {
                            if self.file.has_comments() {
                                self.confirm_comments = true;
                            } else {
                                saved = self.save(None);
                            }
                            ui.close();
                        }
                        if ui.button("Save As").on_hover_text(SAVE_HELP).clicked() {
                            if let Some(path) = save_dialog(&self.file.path) {
                                saved = self.save(Some(path));
                            }
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button("Reload From Disk")
                            .on_hover_text("Discard unsaved changes to this motion list.")
                            .clicked()
                        {
                            if self.file.changed {
                                self.confirm_reload = true;
                            } else {
                                changed |= self.reload();
                            }
                            ui.close();
                        }
                    });
                    ui.menu_button("Labels", |ui| {
                        if ui
                            .button("Load Hash Labels")
                            .on_hover_text("Load a \"0xhash,label\" CSV such as ParamLabels.csv.")
                            .clicked()
                        {
                            if let Some(path) = FileDialog::new()
                                .add_filter("Labels", &["csv", "txt"])
                                .pick_file()
                            {
                                self.message = load_labels_csv(&path).err();
                            }
                            ui.close();
                        }
                    });
                });
                ui.separator();

                if self.confirm_comments {
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            ERROR_COLOR,
                            "This file has comments. Saving writes it without them.",
                        );
                        if ui.button("Save without comments").clicked() {
                            self.confirm_comments = false;
                            saved = self.save(None);
                        }
                        if ui.button("Keep editing").clicked() {
                            self.confirm_comments = false;
                        }
                    });
                }
                if self.confirm_close {
                    ui.horizontal(|ui| {
                        ui.colored_label(ERROR_COLOR, "This motion list has unsaved changes.");
                        let can_save = self.file.place != SavePlace::Nowhere;
                        let save = ui
                            .add_enabled(can_save, egui::Button::new("Save and close"))
                            .on_disabled_hover_text(NOWHERE_HELP);
                        if save.clicked() {
                            saved = self.save(None);
                            close_app = saved.is_some();
                        }
                        if ui.button("Close without saving").clicked() {
                            close_app = true;
                        }
                        if ui.button("Keep editing").clicked() {
                            self.confirm_close = false;
                        }
                    });
                }
                if self.confirm_reload {
                    ui.horizontal(|ui| {
                        ui.colored_label(ERROR_COLOR, "Discard unsaved changes and reload?");
                        if ui.button("Discard and reload").clicked() {
                            self.confirm_reload = false;
                            changed |= self.reload();
                        }
                        if ui.button("Keep editing").clicked() {
                            self.confirm_reload = false;
                        }
                    });
                }

                ui.horizontal(|ui| {
                    ui.label("Motion Path").on_hover_text(
                        "The folder hash for this list like fighter/mario/motion/body/c00.",
                    );
                    changed |= hash_edit(
                        ui,
                        Id::unique("motion_path"),
                        &mut self.file.mlist.motion_path,
                        300.0,
                    );
                    ui.label(format!("{} motions", self.file.mlist.list.len()));
                    if self.file.changed {
                        ui.label("[Modified]");
                    }
                });
                ui.label(
                    RichText::new(self.file.path.to_string_lossy())
                        .weak()
                        .small(),
                );
                if let SavePlace::Copy(to) = &self.file.place {
                    ui.label(
                        RichText::new(format!("Saves to {}", to.display()))
                            .weak()
                            .small(),
                    )
                    .on_hover_text(
                        "The open file is the game's or an installed mod's, which Visionary never changes. Save keeps the list in the project instead, and Export mod ships it.",
                    );
                }
                if let Some(message) = &self.message {
                    ui.colored_label(ERROR_COLOR, message);
                }
                ui.separator();

                egui::Panel::left("motion_list_entries")
                    .resizable(true)
                    .default_size(280.0)
                    .show(ui, |ui| {
                        changed |= self.entry_list(ui);
                    });
                egui::CentralPanel::default().show(ui, |ui| {
                    ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            changed |= self.selected_motion(ui, anim_names);
                        });
                });
            });

        if changed {
            self.file.changed = true;
        }
        EditorResponse {
            open,
            changed: changed || saved.is_some(),
            saved,
            close_app,
        }
    }

    fn save(&mut self, path: Option<PathBuf>) -> Option<PathBuf> {
        let result = match &path {
            Some(path) => self.file.save_to(path),
            None => self.file.save(),
        };
        match result {
            Ok(()) => {
                self.message = None;
                Some(self.file.path.clone())
            }
            Err(error) => {
                self.message = Some(format!("Save failed: {error:#}"));
                None
            }
        }
    }

    fn reload(&mut self) -> bool {
        match self.file.reload() {
            Ok(()) => {
                self.message = None;
                true
            }
            Err(error) => {
                self.message = Some(format!("Reload failed: {error:#}"));
                false
            }
        }
    }

    fn entry_list(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        let selected = self
            .selected
            .filter(|k| self.file.mlist.list.contains_key(k));

        ui.horizontal(|ui| {
            let mlist = &mut self.file.mlist;
            if ui
                .button("Add")
                .on_hover_text("Add a new motion after the selected motion.")
                .clicked()
            {
                let key = Hash40::new(&unique_key(mlist, "new_motion"));
                crate::motion_list::add_labels([label(key)]);
                let motion = new_motion(mlist, None);
                if insert_motion_after(mlist, selected, key, motion).is_ok() {
                    self.selected = Some(key);
                    self.scroll_to_selected = true;
                    changed = true;
                }
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Duplicate"))
                .clicked()
            {
                if let Some(selected) = selected {
                    let name = unique_key(mlist, &format!("{}_copy", label(selected)));
                    let key = Hash40::new(&name);
                    crate::motion_list::add_labels([name]);
                    let motion = mlist.list[&selected].clone();
                    if insert_motion_after(mlist, Some(selected), key, motion).is_ok() {
                        self.selected = Some(key);
                        self.scroll_to_selected = true;
                        changed = true;
                    }
                }
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Delete"))
                .clicked()
            {
                if let Some((index, _, _)) = selected.and_then(|s| mlist.list.shift_remove_full(&s))
                {
                    // Keep a nearby entry selected to make deleting several in a row easier.
                    self.selected = mlist
                        .list
                        .get_index(index.min(mlist.list.len().saturating_sub(1)))
                        .map(|(k, _)| *k);
                    changed = true;
                }
            }
        });

        ui.add(
            TextEdit::singleline(&mut self.search)
                .hint_text("Search")
                .desired_width(f32::INFINITY),
        );

        let search = self.search.to_lowercase();
        let entries: Vec<(Hash40, String, String)> = self
            .file
            .mlist
            .list
            .iter()
            .filter_map(|(key, motion)| {
                let name = label(*key);
                let anim = motion
                    .animations
                    .first()
                    .map(|a| label(a.name))
                    .unwrap_or_default();
                let matches = search.is_empty()
                    || name.to_lowercase().contains(&search)
                    || motion
                        .animations
                        .iter()
                        .any(|a| label(a.name).to_lowercase().contains(&search))
                    || std::iter::once(&motion.game_script)
                        .chain(&motion.scripts)
                        .any(|s| label(*s).to_lowercase().contains(&search));
                matches.then_some((*key, name, anim))
            })
            .collect();

        let row_height = ui.spacing().interact_size.y;
        let scroll_to = if self.scroll_to_selected {
            entries
                .iter()
                .position(|(k, _, _)| Some(*k) == self.selected)
        } else {
            None
        };
        self.scroll_to_selected = false;

        let mut scroll = ScrollArea::vertical().auto_shrink([false; 2]);
        if let Some(row) = scroll_to {
            let spacing = ui.spacing().item_spacing.y;
            scroll = scroll
                .vertical_scroll_offset((row as f32 * (row_height + spacing) - 100.0).max(0.0));
        }
        scroll.show_rows(ui, row_height, entries.len(), |ui, rows| {
            for (key, name, anim) in &entries[rows] {
                let response = ui
                    .selectable_label(self.selected == Some(*key), name)
                    .on_hover_text(if anim.is_empty() {
                        format!("0x{:010x}", key.0)
                    } else {
                        format!("0x{:010x}\n{anim}", key.0)
                    });
                if response.clicked() {
                    self.selected = Some(*key);
                    self.message = None;
                }
            }
        });
        changed
    }

    fn selected_motion(&mut self, ui: &mut Ui, anim_names: &[String]) -> bool {
        let Some(key) = self
            .selected
            .filter(|k| self.file.mlist.list.contains_key(k))
        else {
            ui.label("Select a motion to edit.");
            return false;
        };
        let mut changed = false;

        // One grid for every labelled field so the labels share a column and the fields share a
        // left edge. Fields sit in the last column, which is the only one egui lets grow.
        Grid::new("motion_list_fields")
            .num_columns(2)
            .min_col_width(LABEL_WIDTH)
            .show(ui, |ui| {
                ui.label("Motion Kind").on_hover_text(
                    "The motion name used by scripts like MotionModule::change_motion.",
                );
                let mut new_key = key;
                if hash_edit(ui, Id::unique("motion_key"), &mut new_key, f32::INFINITY) {
                    match rename_motion(&mut self.file.mlist, key, new_key) {
                        Ok(()) => {
                            self.selected = Some(new_key);
                            self.message = None;
                            changed = true;
                        }
                        Err(e) => self.message = Some(e),
                    }
                }
                ui.end_row();

                let key = self.selected.unwrap_or(key);
                let Some(motion) = self.file.mlist.list.get_mut(&key) else {
                    return;
                };

                ui.label("Game Script")
                    .on_hover_text("The ACMD game script for this motion.");
                changed |= hash_edit(
                    ui,
                    Id::unique("game_script"),
                    &mut motion.game_script,
                    f32::INFINITY,
                );
                ui.end_row();

                ui.label("Blend Frames").on_hover_text(BLEND_HELP);
                changed |= ui
                    .add(DragValue::new(&mut motion.blend_frames).range(0..=255))
                    .changed();
                ui.end_row();
            });

        let key = self.selected.unwrap_or(key);
        let Some(motion) = self.file.mlist.list.get_mut(&key) else {
            return changed;
        };

        ui.add_space(SECTION_SPACING);
        ui.heading("Flags");
        Grid::new("motion_list_flags")
            .num_columns(3)
            .show(ui, |ui| {
                for i in 0..6 {
                    changed |= flag_checkbox(ui, motion, i);
                    if i % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
        egui::CollapsingHeader::new("Unknown Flags")
            .id_salt("motion_list_unk_flags")
            .show(ui, |ui| {
                Grid::new("motion_list_unk_flags_grid")
                    .num_columns(4)
                    .show(ui, |ui| {
                        for i in 6..FLAG_NAMES.len() {
                            changed |= flag_checkbox(ui, motion, i);
                            if i % 4 == 1 {
                                ui.end_row();
                            }
                        }
                    });
            });

        ui.add_space(SECTION_SPACING);
        ui.heading("Animations").on_hover_text(format!(
            "Animation files played by this motion. The game reads at most {MAX_ANIMATIONS}."
        ));
        let mut anim_to_remove = None;
        for (i, anim) in motion.animations.iter_mut().enumerate() {
            trailing_row(ui, |ui| {
                if ui.button("Remove").clicked() {
                    anim_to_remove = Some(i);
                }
                changed |= ui
                    .add(DragValue::new(&mut anim.unk).range(0..=255))
                    .on_hover_text("Unknown per-animation value.")
                    .changed();
                ui.label("Unk");
                changed |= anim_name_menu(ui, &mut anim.name, anim_names);
                changed |= hash_edit(
                    ui,
                    Id::unique(("motion_anim", i)),
                    &mut anim.name,
                    f32::INFINITY,
                );
            });
        }
        if let Some(i) = anim_to_remove {
            motion.animations.remove(i);
            changed = true;
        }
        if ui
            .add_enabled(
                motion.animations.len() < MAX_ANIMATIONS,
                egui::Button::new("Add Animation"),
            )
            .clicked()
        {
            motion.animations.push(Default::default());
            changed = true;
        }

        ui.add_space(SECTION_SPACING);
        ui.heading("Scripts")
            .on_hover_text("Effect, sound and expression scripts for this motion.");
        let mut script_to_remove = None;
        for (i, script) in motion.scripts.iter_mut().enumerate() {
            trailing_row(ui, |ui| {
                if ui.button("Remove").clicked() {
                    script_to_remove = Some(i);
                }
                changed |= hash_edit(ui, Id::unique(("motion_script", i)), script, f32::INFINITY);
            });
        }
        if let Some(i) = script_to_remove {
            motion.scripts.remove(i);
            changed = true;
        }
        ui.horizontal(|ui| {
            if ui.button("Add Script").clicked() {
                motion.scripts.push(Hash40::new("dummy"));
                changed = true;
            }
            if ui
                .button("Add Default Scripts")
                .on_hover_text(
                    "Add missing effect, sound and expression scripts named after the game script.",
                )
                .clicked()
            {
                changed |= add_default_scripts(motion);
            }
        });

        ui.add_space(SECTION_SPACING);
        ui.heading("Extra");
        let mut has_extra = motion.extra.is_some();
        if ui
            .checkbox(&mut has_extra, "Fighter Data")
            .on_hover_text(
                "Cancel and intangibility frames. Fighter motion lists use this for every motion.",
            )
            .changed()
        {
            motion.extra = has_extra.then(Extra::default);
            changed = true;
        }
        if let Some(extra) = &mut motion.extra {
            changed |= extra_fields(ui, extra);
        }
        changed
    }
}

fn save_dialog(path: &Path) -> Option<PathBuf> {
    let mut dialog = FileDialog::new()
        .add_filter("Motion List", &["bin"])
        .add_filter("Motion List YAML", &["yml", "yaml"])
        .set_file_name(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "motion_list.bin".to_owned()),
        );
    if let Some(parent) = path.parent() {
        dialog = dialog.set_directory(parent);
    }
    dialog.save_file()
}

/// Loads a "0xhash,label" CSV like ParamLabels.csv. Labels that do not hash to their listed
/// value are skipped, since writing them to YAML would change the hash.
fn load_labels_csv(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    crate::motion_list::add_labels(text.lines().filter_map(|line| {
        let (hash, label) = line.split_once(',')?;
        let hash = Hash40::from_hex_str(hash.trim()).ok()?;
        let label = label.trim();
        (Hash40::new(label) == hash).then(|| label.to_owned())
    }));
    Ok(())
}

fn flag_checkbox(ui: &mut Ui, motion: &mut Motion, index: usize) -> bool {
    flag_mut(&mut motion.flags, index).is_some_and(|flag| {
        ui.checkbox(flag, FLAG_NAMES[index])
            .on_hover_text(FLAG_DESCRIPTIONS[index])
            .changed()
    })
}

fn add_default_scripts(motion: &mut Motion) -> bool {
    let game = label(motion.game_script);
    let Some(name) = game.strip_prefix("game_") else {
        return false;
    };
    let mut changed = false;
    for prefix in ["effect_", "sound_", "expression_"] {
        let script = format!("{prefix}{name}");
        let hash = Hash40::new(&script);
        if !motion.scripts.contains(&hash) {
            crate::motion_list::add_labels([script]);
            motion.scripts.push(hash);
            changed = true;
        }
    }
    changed
}

fn anim_name_menu(ui: &mut Ui, name: &mut Hash40, anim_names: &[String]) -> bool {
    let mut changed = false;
    ui.add_enabled_ui(!anim_names.is_empty(), |ui| {
        ui.menu_button("⏷", |ui| {
            ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                for anim in anim_names {
                    if ui.button(anim).clicked() {
                        *name = parse_hash(&anim.to_lowercase()).unwrap_or(*name);
                        changed = true;
                        ui.close();
                    }
                }
            });
        })
        .response
        .on_hover_text("Pick an animation from this costume's motion folder.");
    });
    changed
}

/// Editable cancel, intangibility and interpolation fields.
fn extra_fields(ui: &mut Ui, extra: &mut Extra) -> bool {
    let mut changed = false;
    Grid::new(ui.scope_id().with("motion_extra"))
        .num_columns(2)
        .min_col_width(LABEL_WIDTH)
        .show(ui, |ui| {
            ui.label("Cancel Frame").on_hover_text(CANCEL_HELP);
            changed |= ui
                .add(DragValue::new(&mut extra.cancel_frame).range(0..=255))
                .changed();
            ui.end_row();

            ui.label("XLU Start").on_hover_text(XLU_HELP);
            changed |= ui
                .add(DragValue::new(&mut extra.xlu_start).range(0..=255))
                .changed();
            ui.end_row();

            ui.label("XLU End").on_hover_text(XLU_HELP);
            changed |= ui
                .add(DragValue::new(&mut extra.xlu_end).range(0..=255))
                .changed();
            ui.end_row();

            ui.label("No Stop Intp").on_hover_text(NO_STOP_INTP_HELP);
            changed |= ui.checkbox(&mut extra.no_stop_intp, "").changed();
            ui.end_row();
        });
    changed
}

/// A list row whose controls are added right to left: trailing buttons first, then a field
/// that fills what is left. Keeps every row's buttons flush with the right edge.
fn trailing_row(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            add_contents,
        );
    });
}

/// Single line editor for a hash that accepts labels or hex values like "0x0123456789".
///
/// Edits apply when the text box loses focus or enter is pressed.
fn hash_edit(ui: &mut Ui, id: Id, hash: &mut Hash40, width: f32) -> bool {
    let id = ui.scope_id().with(id);
    let mut text = ui
        .data(|d| d.get_temp::<(u64, String)>(id))
        .filter(|(h, _)| *h == hash.0)
        .map(|(_, text)| text)
        .unwrap_or_else(|| label(*hash));

    let response = ui
        .add(TextEdit::singleline(&mut text).desired_width(width))
        .on_hover_text(format!(
            "0x{:010x}\nEnter a name or a hex value like 0x0123456789.",
            hash.0
        ));

    let mut changed = false;
    if response.lost_focus() {
        if let Some(new) = parse_hash(&text).filter(|new| new != hash) {
            *hash = new;
            changed = true;
        }
        text = label(*hash);
    }
    ui.data_mut(|d| d.insert_temp(id, (hash.0, text)));
    changed
}
