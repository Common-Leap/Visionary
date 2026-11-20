//! Shared controls for editor windows.
use egui::{Color32, RichText, Ui};

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

/// A colour: the picker, its colour code (`#FF8800`), and R G B numbers for exact values. The
/// numbers go above 1 as the game's do (`LAST_EFFECT_SET_COLOR(agent, 0.25, 1.3, 2.5)` is an
/// over-bright blue); the picker and the code show such a colour at its brightest.
pub(crate) fn colour_field(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    rgb: &mut [f32; 3],
) -> bool {
    let mut changed = false;
    let id = ui.make_persistent_id(id);
    let mut picked = rgb.map(|v| v.clamp(0.0, 1.0));
    if ui.color_edit_button_rgb(&mut picked).changed() {
        *rgb = picked;
        changed = true;
    }
    // The code being typed is kept while the box has focus, so a half-typed code is not
    // replaced by the colour's own on the next frame.
    let mut text = ui
        .data_mut(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| colour_code(*rgb));
    let response = ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .id(id)
                .font(egui::TextStyle::Monospace)
                .desired_width(76.0),
        )
        .on_hover_text("Colour code, like #FF8800");
    if response.has_focus() {
        if response.changed() {
            if let Some(code) = parse_colour_code(&text) {
                *rgb = code;
                changed = true;
            }
        }
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    for (value, name) in rgb.iter_mut().zip(["R ", "G ", "B "]) {
        changed |= ui
            .add(
                egui::DragValue::new(value)
                    .speed(0.01)
                    .range(0.0..=100.0)
                    .max_decimals(3)
                    .prefix(name),
            )
            .changed();
    }
    changed
}

/// `#RRGGBB` for a colour, each channel at most 1.
pub(crate) fn colour_code(rgb: [f32; 3]) -> String {
    let [r, g, b] = rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Reads `#RRGGBB`, `RRGGBB` or `#RGB`.
pub(crate) fn parse_colour_code(text: &str) -> Option<[f32; 3]> {
    let hex = text.trim().trim_start_matches('#');
    let hex: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_owned(),
        _ => return None,
    };
    let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?].map(|v| v as f32 / 255.0))
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
    id: impl std::hash::Hash + std::fmt::Debug,
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
    id: impl std::hash::Hash + std::fmt::Debug,
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

/// Runs a test frame like `Context::run_ui`, dropping the texture uploads no test applies
/// (egui panics when they are dropped unhandled).
/// The page colours of the Project and Characters pages: solid surfaces and full-strength text, in either theme.
#[derive(Clone, Copy)]
pub(crate) struct Look {
    pub page: Color32,
    pub card: Color32,
    pub sunk: Color32,
    pub edge: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub accent: Color32,
}

pub(crate) fn look(ui: &Ui) -> Look {
    if ui.visuals().dark_mode {
        Look {
            page: Color32::from_rgb(16, 17, 20),
            card: Color32::from_rgb(34, 36, 42),
            sunk: Color32::from_rgb(24, 25, 29),
            edge: Color32::from_rgb(78, 82, 94),
            text: Color32::from_rgb(245, 245, 247),
            dim: Color32::from_rgb(178, 182, 192),
            accent: Color32::from_rgb(90, 175, 255),
        }
    } else {
        Look {
            page: Color32::from_rgb(232, 234, 238),
            card: Color32::WHITE,
            sunk: Color32::from_rgb(240, 241, 244),
            edge: Color32::from_rgb(170, 175, 186),
            text: Color32::from_rgb(14, 15, 18),
            dim: Color32::from_rgb(70, 74, 84),
            accent: Color32::from_rgb(0, 110, 220),
        }
    }
}

pub(crate) fn chip(ui: &mut Ui, text: &str, colour: Color32) {
    ui.label(RichText::new(text).color(colour).strong());
}

/// A page button: solid in `colour` for the main action, outlined for the rest. Every state
/// draws the same outline width with no growth, so hovering only recolours it: egui sizes a
/// button's padding by its state's outline, and an outline set on top moved it on hover.
pub(crate) fn act(ui: &mut Ui, look: Look, label: &str, colour: Option<Color32>) -> egui::Response {
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        let (fill, text) = match colour {
            Some(colour) => (colour, Color32::from_rgb(18, 18, 22)),
            None => (look.sunk, look.text),
        };
        for (state, edge) in [
            (&mut widgets.inactive, colour.unwrap_or(look.edge)),
            (&mut widgets.hovered, look.accent),
            (&mut widgets.active, look.accent),
        ] {
            state.weak_bg_fill = fill;
            state.bg_fill = fill;
            state.bg_stroke = egui::Stroke::new(1.5, edge);
            state.expansion = 0.0;
            state.corner_radius = egui::CornerRadius::same(6);
        }
        let label = RichText::new(label).color(text);
        let label = if colour.is_some() { label.strong() } else { label };
        ui.add(egui::Button::new(label).min_size(egui::vec2(0.0, 28.0)))
    })
    .inner
}

#[cfg(test)]
pub(crate) trait TestFrame {
    fn run_test_ui(&self, input: egui::RawInput, ui: impl FnMut(&mut Ui)) -> egui::FullOutput;
}

#[cfg(test)]
impl TestFrame for egui::Context {
    fn run_test_ui(&self, input: egui::RawInput, ui: impl FnMut(&mut Ui)) -> egui::FullOutput {
        let mut output = self.run_ui(input, ui);
        output.textures_delta.clear();
        output
    }
}

#[cfg(test)]
mod colour_code_tests {
    use super::*;

    #[test]
    fn colour_codes_read_and_write() {
        assert_eq!(colour_code([1.0, 0.5333, 0.0]), "#FF8800");
        assert_eq!(colour_code([2.5, -1.0, 0.0]), "#FF0000");
        assert_eq!(
            parse_colour_code("#ff8800"),
            Some([1.0, 136.0 / 255.0, 0.0])
        );
        assert_eq!(parse_colour_code("f80"), Some([1.0, 136.0 / 255.0, 0.0]));
        assert_eq!(parse_colour_code("#ff88"), None);
        assert_eq!(parse_colour_code("#gg8800"), None);
    }
}
