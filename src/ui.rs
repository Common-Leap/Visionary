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
