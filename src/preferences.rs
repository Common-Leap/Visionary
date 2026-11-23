//! Application preferences edited in the Settings window and kept across launches.
//!
//! One JSON file in the config directory. Missing fields take their defaults, so a file
//! written by an older build still loads and keeps what it does have.

use serde::{Deserialize, Serialize};

/// The viewport's backdrop, linear, as it appears on screen.
pub const VIEWPORT_BACKDROP_LINEAR: [f32; 3] = [0.05, 0.05, 0.1];
/// The floor under grounded moves, a shade darker than the backdrop.
const FLOOR_LINEAR: [f32; 3] = [0.028, 0.028, 0.055];

/// Which mouse buttons move the viewport camera: Visionary's own, or one of SSBH Studio's two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NavigationScheme {
    /// Left drag pans; right or middle drag turns around the fighter, short of straight down.
    #[default]
    Visionary,
    /// Left drag orbits; right or middle drag pans.
    Ssbh,
    /// Middle drag orbits, Shift + middle pans and Ctrl + middle zooms.
    Blender,
}

/// The orientation widget in the viewport's top right corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NavigationWidget {
    /// Coloured axis balls, like Blender's navigation gizmo.
    Axes,
    /// A labelled cube with turn arrows.
    Cube,
    #[default]
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
    /// sRGB colour behind the model. `None` is the built-in backdrop.
    pub viewport_color: Option<[u8; 3]>,
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
            viewport_color: None,
            floor_grid: false,
            world_axes: false,
            bloom: true,
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
            .and_then(|path| crate::user_files::read_settings(&path))
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

    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        crate::user_files::save_settings(&path, serde_json::to_vec_pretty(self)?)
    }

    /// The colour behind the model, linear, and the floor's under it.
    pub fn viewport_colors(&self) -> ([f32; 3], [f32; 3]) {
        let Some(color) = self.viewport_color else {
            return (VIEWPORT_BACKDROP_LINEAR, FLOOR_LINEAR);
        };
        let backdrop = color.map(egui::ecolor::linear_f32_from_gamma_u8);
        // As much darker than the picked colour as the built-in floor is than its backdrop.
        let shade = FLOOR_LINEAR[0] / VIEWPORT_BACKDROP_LINEAR[0];
        (backdrop, backdrop.map(|c| c * shade))
    }

    /// [`Self::viewport_color`] as the colour picker shows it.
    pub fn shown_viewport_color(&self) -> [u8; 3] {
        self.viewport_color
            .unwrap_or_else(|| VIEWPORT_BACKDROP_LINEAR.map(egui::ecolor::gamma_u8_from_linear_f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_file_keeps_its_values_and_defaults_the_rest() {
        let preferences = Preferences::parse(br#"{ "viewport_color": [1, 2, 3] }"#);
        assert_eq!(preferences.viewport_color, Some([1, 2, 3]));
        assert_eq!(preferences.ui_scale, 1.0);
        assert!(!preferences.floor_grid);
        // Fields newer than the file take their defaults too.
        assert!(preferences.bloom);
        assert_eq!(preferences.navigation_scheme, NavigationScheme::Visionary);
        assert_eq!(
            preferences.home_camera(),
            crate::renderer::CameraView::default()
        );
    }

    /// With no preferences file the view looks and moves as it did before there was one.
    #[test]
    fn the_defaults_are_the_view_as_it_was() {
        let preferences = Preferences::default();
        assert!(preferences.bloom);
        assert!(!preferences.floor_grid && !preferences.world_axes);
        assert_eq!(
            preferences.viewport_colors(),
            ([0.05, 0.05, 0.1], [0.028, 0.028, 0.055])
        );
        assert_eq!(preferences.navigation_scheme, NavigationScheme::Visionary);
        assert_eq!(preferences.navigation_widget, NavigationWidget::Hidden);
    }

    #[test]
    fn the_floor_stays_darker_than_a_picked_backdrop() {
        for color in [[27, 27, 27], [200, 40, 90], [255, 255, 255]] {
            let preferences = Preferences {
                viewport_color: Some(color),
                ..Default::default()
            };
            let (backdrop, floor) = preferences.viewport_colors();
            assert!((0..3).all(|i| floor[i] < backdrop[i]), "{color:?}");
        }
    }

    #[test]
    fn camera_settings_round_trip() {
        let preferences = Preferences {
            navigation_scheme: NavigationScheme::Blender,
            emulate_three_button_mouse: true,
            navigation_widget: NavigationWidget::Cube,
            world_axes: true,
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
