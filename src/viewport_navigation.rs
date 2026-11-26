//! Mouse and keyboard camera controls for the 3D preview.
//!
//! - **Visionary** navigation: left drag pans, right or middle drag turns around the fighter.
//! - **SSBH** navigation (SSBH Studio's): left drag orbits, right or middle drag pans.
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
///
/// `input` must be a copy taken outside `Context::input`: `response`'s drag checks lock the
/// input again, and calling this inside an input closure can hang the app mid-drag.
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
        NavigationScheme::Visionary => {
            // No Blender drag survives a switch of scheme mid-drag.
            *drag = None;
            // Turning is always around the fighter, wherever the view has been panned to.
            camera.target = Vec3::ZERO;
            visionary_mouse(camera, response)
        }
        NavigationScheme::Ssbh => {
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
        camera.zoom(input.smooth_scroll_delta.y * scroll_speed(controls.scheme));
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
        .and_then(|key| {
            crate::orientation_widget::numpad_view(key, input.modifiers.command, camera)
        });
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

fn scroll_speed(scheme: NavigationScheme) -> f32 {
    match scheme {
        NavigationScheme::Visionary => 0.005,
        NavigationScheme::Ssbh | NavigationScheme::Blender => 0.002,
    }
}

/// Turns the camera for a drag over the orientation widget, as `scheme`'s turning drag does.
pub fn turn(camera: &mut Camera, scheme: NavigationScheme, delta: egui::Vec2) {
    match scheme {
        NavigationScheme::Visionary => camera.orbit(delta.x, delta.y),
        NavigationScheme::Ssbh | NavigationScheme::Blender => camera.rotate(delta.x, delta.y),
    }
}

/// Whether camera keys may act: the pointer is over the viewport, and no text field or other
/// control is taking the keyboard.
pub fn takes_keys(ctx: &egui::Context, response: &egui::Response) -> bool {
    response.hovered() && !ctx.egui_wants_keyboard_input()
}

fn visionary_mouse(camera: &mut Camera, response: &egui::Response) -> bool {
    let delta = response.drag_delta();
    if response.dragged_by(egui::PointerButton::Primary) {
        camera.pan_camera(delta.x, delta.y);
        return true;
    }
    if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        camera.orbit(delta.x, delta.y);
        return true;
    }
    false
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
    use crate::renderer::{camera_rotation, CameraView};
    use crate::ui::TestFrame;

    #[test]
    fn blender_drags_pan_with_shift_and_zoom_with_ctrl() {
        assert_eq!(blender_drag(egui::Modifiers::NONE), NavDrag::Orbit);
        assert_eq!(blender_drag(egui::Modifiers::SHIFT), NavDrag::Pan);
        assert_eq!(blender_drag(egui::Modifiers::COMMAND), NavDrag::Zoom);
    }

    /// The default scheme's drags, scroll and tilt limit, as the camera moved before there were
    /// schemes: left drags pan against the pointer, other drags turn around the fighter, and
    /// the tilt stops short of straight down.
    #[test]
    fn the_default_scheme_moves_the_camera_as_before() {
        let drag = |camera: &mut Camera, button, delta: egui::Vec2| {
            let ctx = egui::Context::default();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            let at = screen.center();
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let mut nav = None;
            for events in [
                vec![egui::Event::PointerMoved(at)],
                vec![press(true)],
                vec![egui::Event::PointerMoved(at + delta)],
            ] {
                let input = egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                };
                let mut run = |ui: &mut egui::Ui| {
                    let response = ui.allocate_rect(screen, egui::Sense::click_and_drag());
                    let input = ui.input(|input| input.clone());
                    let controls = Controls {
                        scheme: NavigationScheme::Visionary,
                        emulate_three_button_mouse: false,
                    };
                    navigate(camera, &response, &input, controls, 600.0, &mut nav, true);
                };
                let _ = ctx.run_test_ui(input, &mut run);
            }
        };
        let mut camera = Camera::default();
        let start = camera.view();
        drag(
            &mut camera,
            egui::PointerButton::Primary,
            egui::vec2(40.0, -20.0),
        );
        // 0.001 of the distance for each pixel, with the rotation left alone.
        assert_eq!(camera.rotation.to_array(), start.rotation);
        assert!(camera
            .translation
            .abs_diff_eq(Vec3::new(-40.0 * 0.06, -8.0 - 20.0 * 0.06, -60.0), 1e-4));
        let panned = camera.translation;
        for button in [egui::PointerButton::Secondary, egui::PointerButton::Middle] {
            let before = camera.rotation;
            drag(&mut camera, button, egui::vec2(30.0, 10.0));
            // A hundredth of a radian for each pixel, around the fighter: nothing else moves.
            assert!(camera
                .rotation
                .abs_diff_eq(before + Vec3::new(0.1, 0.3, 0.0), 1e-4));
            assert_eq!(camera.translation, panned);
        }
        drag(
            &mut camera,
            egui::PointerButton::Secondary,
            egui::vec2(0.0, 500.0),
        );
        assert_eq!(camera.rotation.x, 1.4);
        drag(
            &mut camera,
            egui::PointerButton::Secondary,
            egui::vec2(0.0, -900.0),
        );
        assert_eq!(camera.rotation.x, -1.4);
        // A view picked past the limit is not pulled back to it, only stopped there.
        camera.turn_to(Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0));
        camera.orbit(0.0, 10.0);
        assert_eq!(camera.rotation.x, std::f32::consts::FRAC_PI_2);
        camera.orbit(0.0, -10.0);
        assert!((camera.rotation.x - (std::f32::consts::FRAC_PI_2 - 0.1)).abs() < 1e-5);
        // A scroll step moved a twentieth of a tenth of the distance.
        assert!((scroll_speed(NavigationScheme::Visionary) - 0.05 * 0.1).abs() < 1e-8);
    }

    /// Camera keys act only over the viewport and never while a text field has the keyboard.
    #[test]
    fn keys_stay_with_the_text_field_that_has_them() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let view = egui::Rect::from_min_max(egui::pos2(0.0, 100.0), screen.max);
        let mut text = String::new();
        let mut run = |focus: bool, pointer: egui::Pos2| {
            let mut takes = false;
            for _ in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![egui::Event::PointerMoved(pointer)],
                    ..Default::default()
                };
                let _ = ctx.run_test_ui(input, |ui| {
                    let field = ui.add(egui::TextEdit::singleline(&mut text));
                    if focus {
                        field.request_focus();
                    } else {
                        field.surrender_focus();
                    }
                    let response = ui.allocate_rect(view, egui::Sense::click_and_drag());
                    takes = takes_keys(ui.ctx(), &response);
                });
            }
            takes
        };
        let over = view.center();
        assert!(run(false, over));
        assert!(!run(true, over), "the text field has the keyboard");
        assert!(!run(false, egui::pos2(400.0, 50.0)), "not over the view");
        assert!(run(false, over));
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
        let view = camera
            .transforms(800.0, 600.0)
            .model_view_matrix
            .transform_point3(center);
        assert!(view.x.abs() < 1e-3 && view.y.abs() < 1e-3, "{view}");
        assert!(
            view.z < -12.0,
            "the sphere is in front of the camera: {view}"
        );
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
