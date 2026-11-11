//! Shared controls for editor windows.
use egui::Ui;

/// Browse only expanded folders; files open in their default application.
pub(crate) fn file_tree(ui: &mut Ui, directory: &std::path::Path) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            ui.weak(format!("Could not read folder: {error}"));
            return;
        }
    };
    let project_root = directory
        .join(crate::mod_project::PROJECT_FILE_NAME)
        .is_file();
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| {
        (
            !(project_root && entry.file_name() == "romfs"),
            !entry.file_type().is_ok_and(|kind| kind.is_dir()),
            entry.file_name(),
        )
    });
    if entries.is_empty() {
        ui.weak("No files yet");
    }
    for entry in entries {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let label = if project_root {
            match name.as_str() {
                "romfs" => "Game files · romfs",
                "assets" => "Managed images · assets",
                "reference" => "Reference files · excluded from export",
                "modproject.json" => "Saved edits · modproject.json",
                _ => &name,
            }
        } else {
            &name
        };
        if kind.is_dir() {
            egui::CollapsingHeader::new(label)
                .id_salt(&path)
                .default_open(name == "romfs")
                .show(ui, |ui| file_tree(ui, &path));
        } else if kind.is_file() && ui.button(label).on_hover_text("Open file").clicked() {
            if let Err(error) = crate::roster::reveal::reveal(&path) {
                ui.colored_label(egui::Color32::LIGHT_RED, error.to_string());
            }
        }
    }
}

pub(crate) fn search_field(ui: &mut Ui, id: &str, query: &mut String, hint: &str) {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 24.0 - ui.spacing().item_spacing.x).max(1.0);
        let response = ui.add(
            egui::TextEdit::singleline(query)
                .id_salt(id)
                .hint_text(hint)
                .desired_width(width),
        );
        let escape = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
        if ui
            .add_enabled(
                !query.is_empty(),
                egui::Button::new("×").min_size(egui::vec2(24.0, 0.0)),
            )
            .on_hover_text("Clear search (Esc)")
            .clicked()
            || escape
        {
            query.clear();
            response.request_focus();
        }
    });
}

pub(crate) fn game_connection(ui: &mut Ui, status: crate::game_link::LinkStatus) {
    use crate::game_link::LinkStatus;
    let (color, label) = match status {
        LinkStatus::Connected => (egui::Color32::from_rgb(90, 220, 90), "Game connected"),
        LinkStatus::Connecting => (egui::Color32::YELLOW, "Connecting…"),
        LinkStatus::Disconnected => (egui::Color32::from_rgb(220, 90, 90), "Game offline"),
    };
    ui.colored_label(color, "●");
    ui.label(label);
}

/// One choice in a [`picker`].
pub(crate) struct Pick {
    pub label: String,
    /// Shown on hover, such as the game's own name for the choice.
    pub hover: String,
    /// The heading the choice is listed under, with its description. Choices without one are
    /// listed flat.
    pub group: Option<(&'static str, &'static str)>,
}

/// A dropdown with a search box whose choices are listed under collapsible headings, the same
/// way as the classic editor's Fighters and Moves lists. Returns the index of a picked choice.
pub(crate) fn picker(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    width: f32,
    selected_text: &str,
    choices: &[Pick],
    selected: Option<usize>,
) -> Option<usize> {
    let id = ui.make_persistent_id(id);
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .height(480.0)
        .selected_text(selected_text)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            let query_id = id.with("query");
            let mut query: String = ui.data_mut(|d| d.get_temp(query_id).unwrap_or_default());
            // Short lists need no search box.
            if choices.len() > 12 {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut query)
                        .hint_text("Search")
                        .desired_width(f32::INFINITY),
                );
                if !response.has_focus() && query.is_empty() {
                    response.request_focus();
                }
            }
            // Scroll to the current choice on the frame the list opens.
            let frame = ui.ctx().cumulative_frame_nr();
            let last: Option<u64> = ui.data(|d| d.get_temp(id.with("shown")));
            let just_opened = last.is_none_or(|last| last + 1 < frame);
            ui.data_mut(|d| d.insert_temp(id.with("shown"), frame));
            let needle = query.trim().to_lowercase();
            let matches = |pick: &Pick| {
                needle.is_empty()
                    || pick.label.to_lowercase().contains(&needle)
                    || pick.hover.to_lowercase().contains(&needle)
            };
            let mut row = |ui: &mut Ui, index: usize, pick: &Pick| {
                let response = ui
                    .selectable_label(selected == Some(index), &pick.label)
                    .on_hover_text(&pick.hover);
                if selected == Some(index) && just_opened {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                if response.clicked() {
                    picked = Some(index);
                }
            };
            egui::ScrollArea::vertical()
                .max_height(440.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let mut groups: Vec<(&'static str, &'static str)> = Vec::new();
                    for pick in choices {
                        if let Some(group) = pick.group {
                            if !groups.contains(&group) {
                                groups.push(group);
                            }
                        }
                    }
                    let mut any = false;
                    for (index, pick) in choices.iter().enumerate() {
                        if pick.group.is_none() && matches(pick) {
                            any = true;
                            row(ui, index, pick);
                        }
                    }
                    for group in groups {
                        let members: Vec<usize> = (0..choices.len())
                            .filter(|&i| choices[i].group == Some(group) && matches(&choices[i]))
                            .collect();
                        if members.is_empty() {
                            continue;
                        }
                        any = true;
                        let holds_selected = selected.is_some_and(|s| members.contains(&s));
                        let header =
                            egui::CollapsingHeader::new(format!("{} ({})", group.0, members.len()))
                                .id_salt((id, group.0, !needle.is_empty()))
                                .default_open(holds_selected);
                        let header = if needle.is_empty() {
                            header
                        } else {
                            header.open(Some(true))
                        };
                        header
                            .show(ui, |ui| {
                                for index in members {
                                    row(ui, index, &choices[index]);
                                }
                            })
                            .header_response
                            .on_hover_text(group.1);
                    }
                    if !any {
                        ui.weak("Nothing matches");
                    }
                });
            if picked.is_some() {
                query.clear();
                ui.close();
            }
            ui.data_mut(|d| d.insert_temp(query_id, query));
        });
    picked
}

/// A bone's name in plain words, such as "Hand (right)" for `handr`.
pub(crate) fn bone_label(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let whole = match lower.as_str() {
        "top" => Some("Whole body"),
        "trans" => Some("Root"),
        "rot" => Some("Root turn"),
        "hip" => Some("Hips"),
        "waist" => Some("Waist"),
        "bust" => Some("Chest"),
        "head" => Some("Head"),
        "neck" => Some("Neck"),
        "throw" => Some("Throw point"),
        "have" => Some("Held item"),
        _ => None,
    };
    if let Some(whole) = whole {
        return whole.to_string();
    }
    for base in [
        "hand", "arm", "shoulder", "clavicle", "leg", "knee", "foot", "toe", "finger", "eye",
        "ear", "have", "wing", "thumb", "elbow",
    ] {
        if let Some(rest) = lower.strip_prefix(base) {
            let side = match rest {
                "l" => "left",
                "r" => "right",
                _ => continue,
            };
            let mut title = base.to_string();
            title[..1].make_ascii_uppercase();
            return format!("{title} ({side})");
        }
    }
    name.to_string()
}

/// The body part a bone is listed under.
fn bone_group(name: &str) -> (&'static str, &'static str) {
    let lower = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    if has(&[
        "top", "trans", "rot", "hip", "waist", "bust", "spine", "body", "throw",
    ]) {
        ("Body", "The whole body and the trunk")
    } else if has(&[
        "head", "neck", "eye", "ear", "mouth", "jaw", "hair", "face", "nose",
    ]) {
        ("Head", "Head, face and hair")
    } else if has(&[
        "hand", "arm", "shoulder", "clavicle", "finger", "thumb", "elbow", "have",
    ]) {
        ("Arms and hands", "Arms, hands, fingers and held items")
    } else if has(&["leg", "knee", "foot", "toe", "thigh"]) {
        ("Legs and feet", "Legs, knees and feet")
    } else {
        ("Other", "Bones for clothing, weapons and other parts")
    }
}

/// Pick a bone, listed by body part with plain names. Returns true when the bone changed.
pub(crate) fn bone_picker(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    width: f32,
    bone: &mut String,
    bones: &[String],
) -> bool {
    let mut names: Vec<&String> = bones.iter().collect();
    if !bone.is_empty() && !bones.contains(bone) {
        names.insert(0, bone);
    }
    let picks: Vec<Pick> = names
        .iter()
        .map(|name| Pick {
            label: bone_label(name),
            hover: (*name).clone(),
            group: Some(bone_group(name)),
        })
        .collect();
    let selected = names.iter().position(|name| *name == bone);
    let current = if bone.is_empty() {
        "Choose bone".to_string()
    } else {
        bone_label(bone)
    };
    match picker(ui, id, width, &current, &picks, selected) {
        Some(index) if names[index] != bone => {
            *bone = names[index].clone();
            true
        }
        _ => false,
    }
}
