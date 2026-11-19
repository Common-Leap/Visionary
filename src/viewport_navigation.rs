//! Mouse and keyboard camera controls for the 3D preview, as SSBH Studio has them.
//!
//! - **SSBH** navigation: left drag orbits, right or middle drag pans.
//! - **Blender** navigation: middle drag orbits, Shift + middle pans, Ctrl + middle zooms.
//!   With three-button emulation, Alt + left drag stands in for the middle button.
//!
//! Either way the scroll wheel zooms, the arrow keys pan, numpad 1, 3 and 7 look from the
//! front, right and top (Ctrl for the opposite side, 9 flips the view), Period frames the
//! selection and Home frames the whole fighter.
use crate::preferences::NavigationScheme;
use crate::renderer::Camera;
use glam::Vec3;

/// What a Blender-style drag does, decided when it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDrag {
    Orbit,
    Pan,
    Zoom,
}

/// What the framing keys asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRequest {
    /// Period: the selected hitbox, or the fighter with nothing selected.
    Selection,
    /// Home: the whole fighter.
    All,
}

/// The settings that decide which buttons move the camera.
#[derive(Debug, Clone, Copy)]
pub struct Controls {
    pub scheme: NavigationScheme,
    pub emulate_three_button_mouse: bool,
}

/// The Blender drag for the modifiers held when it starts.
pub fn blender_drag(modifiers: egui::Modifiers) -> NavDrag {
    if modifiers.shift {
        NavDrag::Pan
    } else if modifiers.command {
        NavDrag::Zoom
    } else {
        NavDrag::Orbit
    }
}

/// Moves `camera` for this frame's input over the viewport. `response` is the viewport's
/// own (so drags start in it and overlays on top of it are left alone) and `keyboard` says
/// whether keys may move the camera: the pointer is over the viewport and no text field has
/// focus. Returns whether the camera moved, and any framing the keys asked for.
pub fn navigate(
    camera: &mut Camera,
    response: &egui::Response,
    input: &egui::InputState,
    controls: Controls,
    viewport_height: f32,
    drag: &mut Option<NavDrag>,
    keyboard: bool,
) -> (bool, Option<FrameRequest>) {
    let mut moved = match controls.scheme {
        NavigationScheme::Ssbh => {
            // No Blender drag survives a switch of scheme mid-drag.
            *drag = None;
            ssbh_mouse(camera, response, viewport_height)
        }
        NavigationScheme::Blender => blender_mouse(
            camera,
            response,
            input,
            controls.emulate_three_button_mouse,
            viewport_height,
            drag,
        ),
    };
    if response.hovered() && input.smooth_scroll_delta.y != 0.0 {
        camera.zoom(input.smooth_scroll_delta.y * 0.002);
        moved = true;
    }
    let mut frame = None;
    if keyboard {
        moved |= keyboard_pan(camera, input);
        let view = [
            egui::Key::Num1,
            egui::Key::Num3,
            egui::Key::Num7,
            egui::Key::Num9,
        ]
        .into_iter()
        .find(|key| input.key_pressed(*key) && !input.modifiers.shift && !input.modifiers.alt)
        .and_then(|key| crate::orientation_widget::numpad_view(key, input.modifiers.command, camera));
        if let Some(rotation) = view {
            camera.turn_to(rotation);
            moved = true;
        }
        if !input.modifiers.any() {
            if input.key_pressed(egui::Key::Period) {
                frame = Some(FrameRequest::Selection);
            } else if input.key_pressed(egui::Key::Home) {
                frame = Some(FrameRequest::All);
            }
        }
    }
    (moved, frame)
}

fn ssbh_mouse(camera: &mut Camera, response: &egui::Response, viewport_height: f32) -> bool {
    let delta = response.drag_delta();
    if delta == egui::Vec2::ZERO {
        return false;
    }
    if response.dragged_by(egui::PointerButton::Primary) {
        camera.rotate(delta.x, delta.y);
        return true;
    }
    if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        camera.pan(delta.x, delta.y, viewport_height);
        return true;
    }
    false
}

fn blender_mouse(
    camera: &mut Camera,
    response: &egui::Response,
    input: &egui::InputState,
    emulate_three_button_mouse: bool,
    viewport_height: f32,
    drag: &mut Option<NavDrag>,
) -> bool {
    let emulated = emulate_three_button_mouse && input.modifiers.alt;
    if response.drag_started_by(egui::PointerButton::Middle)
        || (emulated && response.drag_started_by(egui::PointerButton::Primary))
    {
        *drag = Some(blender_drag(input.modifiers));
    }
    let held = response.dragged_by(egui::PointerButton::Middle)
        || (emulate_three_button_mouse && response.dragged_by(egui::PointerButton::Primary));
    if !held {
        *drag = None;
        return false;
    }
    let delta = response.drag_delta();
    if delta == egui::Vec2::ZERO {
        return false;
    }
    match *drag {
        Some(NavDrag::Orbit) => camera.rotate(delta.x, delta.y),
        Some(NavDrag::Pan) => camera.pan(delta.x, delta.y, viewport_height),
        // Dragging up zooms in, like Blender.
        Some(NavDrag::Zoom) => camera.zoom(-delta.y * 0.005),
        None => return false,
    }
    true
}

/// Pans while arrow keys are held. Returns whether the camera moved.
fn keyboard_pan(camera: &mut Camera, input: &egui::InputState) -> bool {
    let step = 0.25;
    let mut shift = Vec3::ZERO;
    if input.key_down(egui::Key::ArrowLeft) {
        shift.x += step;
    }
    if input.key_down(egui::Key::ArrowRight) {
        shift.x -= step;
    }
    if input.key_down(egui::Key::ArrowUp) {
        shift.y -= step;
    }
    if input.key_down(egui::Key::ArrowDown) {
        shift.y += step;
    }
    if shift == Vec3::ZERO {
        return false;
    }
    camera.pan_view(shift);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{CameraView, camera_rotation};

    #[test]
    fn blender_drags_pan_with_shift_and_zoom_with_ctrl() {
        assert_eq!(blender_drag(egui::Modifiers::NONE), NavDrag::Orbit);
        assert_eq!(blender_drag(egui::Modifiers::SHIFT), NavDrag::Pan);
        assert_eq!(blender_drag(egui::Modifiers::COMMAND), NavDrag::Zoom);
    }

    /// Where the camera's target appears in view space.
    fn target_on_screen(camera: &Camera) -> Vec3 {
        camera_rotation(camera.rotation).transform_vector3(camera.target) + camera.translation
    }

    #[test]
    fn orbiting_and_panning_keep_the_target_centred() {
        let mut camera = Camera::default();
        let centred = |camera: &Camera| {
            let p = target_on_screen(camera);
            p.x.abs() < 1e-3 && p.y.abs() < 1e-3
        };
        assert!(centred(&camera));
        camera.rotate(40.0, -25.0);
        assert!(centred(&camera));
        camera.pan(30.0, 12.0, 600.0);
        assert!(centred(&camera));
        camera.rotate(-80.0, 10.0);
        assert!(centred(&camera));
        camera.pan_view(Vec3::new(0.25, -0.25, 0.0));
        assert!(centred(&camera));
    }

    #[test]
    fn framing_centres_the_sphere_and_keeps_the_angle() {
        let mut camera = Camera::from_view(&CameraView {
            rotation: [0.4, 1.1, 0.0],
            ..Default::default()
        });
        camera.follow = Vec3::new(30.0, 0.0, 5.0);
        let rotation = camera.rotation;
        let center = Vec3::new(32.0, 9.0, 4.0);
        camera.frame(center, 12.0);
        assert_eq!(camera.rotation, rotation);
        let view = camera.transforms(800.0, 600.0).model_view_matrix.transform_point3(center);
        assert!(view.x.abs() < 1e-3 && view.y.abs() < 1e-3, "{view}");
        assert!(view.z < -12.0, "the sphere is in front of the camera: {view}");
        assert!(target_on_screen(&camera).abs_diff_eq(view, 1e-3));
    }

    #[test]
    fn zoom_never_passes_the_target() {
        let mut camera = Camera::default();
        for _ in 0..200 {
            camera.zoom(5.0);
        }
        assert_eq!(camera.translation.z, -1.0);
        let far = camera.translation.z;
        camera.zoom(-1.0);
        assert!(camera.translation.z < far);
    }

    #[test]
    fn a_saved_view_comes_back_unchanged() {
        let view = CameraView {
            translation: [1.0, -4.0, -80.0],
            rotation: [0.2, 0.9, 0.0],
            fov_y: 0.6,
            near: 2.0,
            far: 9000.0,
        };
        let camera = Camera::from_view(&view);
        assert_eq!(camera.view(), view);
        assert!(camera.is_at(&view));
        assert!(!camera.is_at(&CameraView::default()));
    }
}
