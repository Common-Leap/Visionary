//! Application preferences edited in the Settings window and kept across launches.
//!
//! One JSON file in the config directory. Missing fields take their defaults, so a file
//! written by an older build still loads and keeps what it does have.

use serde::{Deserialize, Serialize};

/// SSBH Editor's viewport background: its dark theme's panel colour.
pub const DEFAULT_VIEWPORT_COLOR: [u8; 3] = [27, 27, 27];

/// Which mouse buttons move the viewport camera (SSBH Studio's two schemes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NavigationScheme {
    /// Left drag orbits; right or middle drag pans.
    #[default]
    Ssbh,
    /// Middle drag orbits, Shift + middle pans and Ctrl + middle zooms.
    Blender,
}

/// The orientation widget in the viewport's top right corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NavigationWidget {
    /// Coloured axis balls, like Blender's navigation gizmo.
    #[default]
    Axes,
    /// A labelled cube with turn arrows.
    Cube,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Open a console window for Visionary's log output. Ignored when Visionary was started
    /// from a terminal, which already shows it.
    pub show_console: bool,
    /// egui zoom factor applied to the whole interface.
    pub ui_scale: f32,
    /// sRGB colour behind the model.
    pub viewport_color: [u8; 3],
    pub floor_grid: bool,
    /// Red X and blue Z lines through the origin on the ground.
    pub world_axes: bool,
    pub bloom: bool,
    pub navigation_scheme: NavigationScheme,
    /// With Blender navigation, Alt + left drag stands in for the middle button.
    pub emulate_three_button_mouse: bool,
    pub navigation_widget: NavigationWidget,
    /// The view the camera starts at and resets to. `None` is the built-in side view.
    pub default_camera: Option<crate::renderer::CameraView>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            show_console: false,
            ui_scale: 1.0,
            viewport_color: DEFAULT_VIEWPORT_COLOR,
            floor_grid: true,
            world_axes: true,
            bloom: false,
            navigation_scheme: NavigationScheme::default(),
            emulate_three_button_mouse: false,
            navigation_widget: NavigationWidget::default(),
            default_camera: None,
        }
    }
}

impl Preferences {
    pub const UI_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.5..=2.5;

    /// The view the camera starts at and resets to.
    pub fn home_camera(&self) -> crate::renderer::CameraView {
        self.default_camera.unwrap_or_default()
    }

    fn path() -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|base| base.join("visionary").join("preferences.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|path| std::fs::read(path).ok())
            .map(|bytes| Self::parse(&bytes))
            .unwrap_or_default()
    }

    fn parse(bytes: &[u8]) -> Self {
        let mut preferences: Self = serde_json::from_slice(bytes).unwrap_or_default();
        if !preferences.ui_scale.is_finite() {
            preferences.ui_scale = 1.0;
        }
        preferences.ui_scale = preferences
            .ui_scale
            .clamp(*Self::UI_SCALE_RANGE.start(), *Self::UI_SCALE_RANGE.end());
        preferences
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(error) = std::fs::write(&path, json) {
                    eprintln!("[visionary] Could not save preferences to {path:?}: {error}");
                }
            }
            Err(error) => eprintln!("[visionary] Could not serialize preferences: {error}"),
        }
    }

    /// The viewport colour as the renderer's clear colour. egui converts the final image to
    /// sRGB, so the stored sRGB value is linearized first.
    pub fn viewport_clear_color(&self) -> [f64; 4] {
        let [r, g, b] = self
            .viewport_color
            .map(|c| egui::ecolor::linear_f32_from_gamma_u8(c) as f64);
        [r, g, b, 1.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_file_keeps_its_values_and_defaults_the_rest() {
        let preferences = Preferences::parse(br#"{ "viewport_color": [1, 2, 3] }"#);
        assert_eq!(preferences.viewport_color, [1, 2, 3]);
        assert_eq!(preferences.ui_scale, 1.0);
        assert!(preferences.floor_grid);
        // Fields newer than the file take their defaults too.
        assert!(preferences.world_axes);
        assert_eq!(preferences.navigation_scheme, NavigationScheme::Ssbh);
        assert_eq!(preferences.navigation_widget, NavigationWidget::Axes);
        assert_eq!(
            preferences.home_camera(),
            crate::renderer::CameraView::default()
        );
    }

    #[test]
    fn camera_settings_round_trip() {
        let preferences = Preferences {
            navigation_scheme: NavigationScheme::Blender,
            emulate_three_button_mouse: true,
            navigation_widget: NavigationWidget::Cube,
            world_axes: false,
            default_camera: Some(crate::renderer::CameraView {
                translation: [1.0, -3.0, -90.0],
                ..Default::default()
            }),
            ..Default::default()
        };
        let json = serde_json::to_vec(&preferences).unwrap();
        assert_eq!(Preferences::parse(&json), preferences);
    }

    #[test]
    fn an_unreadable_file_or_scale_falls_back() {
        assert_eq!(Preferences::parse(b"not json"), Preferences::default());
        assert_eq!(Preferences::parse(br#"{ "ui_scale": 40.0 }"#).ui_scale, 2.5);
    }
}
