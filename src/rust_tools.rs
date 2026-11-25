//! The Rust tools that build an exported mod's plugin.

use crate::console::NoConsoleWindow;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One thing a mod's plugin cannot be built without.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Rust,
    Git,
    CargoSkyline,
    SkylineToolchain,
}

/// Where a tool comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    Page(&'static str),
    Run(&'static str),
}

impl Tool {
    /// In the order they are installed: each one needs those before it.
    pub const ALL: [Self; 4] = [
        Self::Rust,
        Self::Git,
        Self::CargoSkyline,
        Self::SkylineToolchain,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Git => "Git",
            Self::CargoSkyline => "cargo-skyline",
            Self::SkylineToolchain => "Skyline toolchain",
        }
    }

    pub fn install(self) -> Install {
        match self {
            Self::Rust => Install::Page("https://rustup.rs"),
            Self::Git => Install::Page("https://git-scm.com/downloads"),
            Self::CargoSkyline => Install::Run("cargo install cargo-skyline"),
            Self::SkylineToolchain => Install::Run("cargo skyline update-std"),
        }
    }

    fn installed(self) -> bool {
        match self {
            // cargo-skyline builds through rustup, so a cargo from anywhere else is not enough.
            Self::Rust => runs("rustup", &["--version"]) && runs("cargo", &["--version"]),
            Self::Git => runs("git", &["--version"]),
            Self::CargoSkyline => runs("cargo", &["skyline", "--version"]),
            // The folder cargo-skyline looks for. Without it, it asks on a terminal the editor
            // does not have.
            Self::SkylineToolchain => home("RUSTUP_HOME", ".rustup")
                .is_some_and(|home| home.join("toolchains/skyline-v3").exists()),
        }
    }
}

/// What this computer still needs, in the order to install it.
pub fn missing() -> Vec<Tool> {
    Tool::ALL
        .into_iter()
        .filter(|tool| !tool.installed())
        .collect()
}

/// `program` with the folders Rust and Git install to behind the PATH. The editor keeps the PATH
/// it was started with, which has neither when it was started before they were installed, and
/// on Linux often has no Rust when it was started from the desktop.
pub fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.no_console_window();
    let git = std::env::var_os("ProgramFiles").map(|files| Path::new(&files).join("Git/cmd"));
    let installed = home("CARGO_HOME", ".cargo")
        .map(|home| home.join("bin"))
        .into_iter()
        .chain(git);
    let path = std::env::var_os("PATH").and_then(|path| path_with(&path, installed));
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command
}

fn runs(program: &str, arguments: &[&str]) -> bool {
    command(program)
        .args(arguments)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn home(variable: &str, folder: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(folder)))
}

fn path_with(path: &OsStr, installed: impl IntoIterator<Item = PathBuf>) -> Option<OsString> {
    std::env::join_paths(std::env::split_paths(path).chain(installed)).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    ExportAgain,
    Close,
}

/// Asks for the tools the mod's plugin cannot be built without.
pub fn window(ctx: &egui::Context, needed: &[Tool]) -> Option<Answer> {
    if needed.is_empty() {
        return None;
    }
    let mut answer = None;
    let title = if needed.contains(&Tool::Rust) {
        "Install Rust to export this mod"
    } else {
        "Install these to export this mod"
    };
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Grid::new("rust_needed")
                .num_columns(3)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    for (step, tool) in needed.iter().enumerate() {
                        ui.label(format!("{}. {}", step + 1, tool.name()));
                        match tool.install() {
                            Install::Page(page) => {
                                ui.hyperlink(page);
                            }
                            Install::Run(line) => {
                                ui.monospace(line).on_hover_text("Run this in a terminal");
                                if ui.button("Copy").clicked() {
                                    ui.ctx().copy_text(line.to_owned());
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Export again").clicked() {
                    answer = Some(Answer::ExportAgain);
                }
                if ui.button("Close").clicked() {
                    answer = Some(Answer::Close);
                }
            });
        });
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::TestFrame;

    #[test]
    fn rust_is_found_in_its_own_folder_behind_the_path_the_editor_started_with() {
        let (first, second, cargo) = (
            "/usr/bin",
            "/opt/tools",
            PathBuf::from("/home/a/.cargo/bin"),
        );
        let started_with = std::env::join_paths([first, second]).unwrap();
        let path = path_with(&started_with, [cargo.clone()]).unwrap();
        assert_eq!(
            std::env::split_paths(&path).collect::<Vec<_>>(),
            [PathBuf::from(first), PathBuf::from(second), cargo]
        );
        // What the editor runs is given that PATH.
        let given = command("cargo");
        let given = given.get_envs().find(|(name, _)| *name == "PATH");
        let given = given.and_then(|(_, value)| value).expect("PATH is set");
        let rust = home("CARGO_HOME", ".cargo").unwrap().join("bin");
        assert!(std::env::split_paths(given).any(|folder| folder == rust));
    }

    #[test]
    fn a_program_that_is_not_there_does_not_run() {
        assert!(!runs("visionary-has-no-such-program", &["--list"]));
        // The paired positive: the program running this test.
        let this = std::env::current_exe().unwrap();
        assert!(runs(this.to_str().unwrap(), &["--list"]));
    }

    /// Export source code writes the same commands into the build scripts beside the source.
    #[test]
    fn the_commands_are_the_ones_the_exported_build_scripts_run() {
        let acmd = include_str!("acmd.rs");
        for tool in Tool::ALL {
            if let Install::Run(line) = tool.install() {
                assert!(acmd.contains(line), "{line}");
            }
        }
    }
    fn texts(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.job.text.clone(),
                text.pos + text.galley.size() / 2.0,
            )),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| texts(shape, out)),
            _ => {}
        }
    }

    /// Draw the window, click the last control labelled `click`, and return what it showed,
    /// what it copied and what it answered.
    fn shown(needed: &[Tool], click: Option<&str>) -> (Vec<String>, Vec<String>, Option<Answer>) {
        let ctx = egui::Context::default();
        let mut answer = None;
        let mut frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let output = ctx.run_test_ui(input, |ui| {
                answer = answer.or(window(ui.ctx(), needed));
            });
            let mut drawn = Vec::new();
            for shape in &output.shapes {
                texts(&shape.shape, &mut drawn);
            }
            let copied = output.platform_output.commands.into_iter();
            let copied = copied.filter_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            });
            (drawn, copied.collect::<Vec<_>>())
        };
        for _ in 0..3 {
            frame(Vec::new());
        }
        let (drawn, mut copied) = frame(Vec::new());
        if let Some(label) = click {
            let at = drawn.iter().rfind(|(text, _)| text == label);
            let (_, pos) = *at.unwrap_or_else(|| panic!("no {label:?} in {drawn:?}"));
            for pressed in [true, false] {
                let button = egui::PointerButton::Primary;
                let modifiers = egui::Modifiers::NONE;
                copied.extend(
                    frame(vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button,
                            pressed,
                            modifiers,
                        },
                    ])
                    .1,
                );
            }
        }
        let drawn = drawn.into_iter().map(|(text, _)| text).collect();
        (drawn, copied, answer)
    }

    #[test]
    fn the_window_lists_what_to_install_in_order() {
        let (drawn, _, _) = shown(&Tool::ALL, None);
        for text in [
            "Install Rust to export this mod",
            "1. Rust",
            "https://rustup.rs",
            "2. Git",
            "https://git-scm.com/downloads",
            "3. cargo-skyline",
            "cargo install cargo-skyline",
            "4. Skyline toolchain",
            "cargo skyline update-std",
        ] {
            assert!(drawn.iter().any(|shown| shown == text), "{text}: {drawn:?}");
        }
        // Somebody who has Rust is asked for the rest of it only.
        let (drawn, _, _) = shown(&[Tool::SkylineToolchain], None);
        assert!(drawn.iter().any(|text| text == "1. Skyline toolchain"));
        assert!(drawn.iter().all(|text| !text.contains("Install Rust")));
        // Nothing missing, nothing asked.
        assert!(shown(&[], None).0.is_empty());
    }

    #[test]
    fn the_window_copies_a_command_exports_again_and_closes() {
        let (_, copied, answer) = shown(&Tool::ALL, Some("Copy"));
        assert_eq!(copied, ["cargo skyline update-std"]);
        assert_eq!(answer, None);
        let answer = |label| shown(&Tool::ALL, Some(label)).2;
        assert_eq!(answer("Export again"), Some(Answer::ExportAgain));
        assert_eq!(answer("Close"), Some(Answer::Close));
    }

    /// Asked before a folder is picked or a file is written, and never for a mod with no plugin
    /// or for Export source code, which builds nothing.
    #[test]
    fn export_asks_for_rust_before_it_writes_anything() {
        let app = include_str!("app.rs");
        let export = app
            .split("    fn export_mod(&mut self, developer: bool) {")
            .nth(1)
            .and_then(|rest| rest.split("\n    fn ").next())
            .expect("export_mod");
        let asks = export
            .find("if builds_plugin && !developer {\n                self.rust_needed = crate::rust_tools::missing();")
            .expect("asks");
        for later in ["FileDialog::new()", "run_export_preverified("] {
            assert!(asks < export.find(later).expect(later), "{later}");
        }
    }
}
