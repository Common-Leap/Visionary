//! The orientation widget in the viewport's top right corner, as axis balls or a cube.
//! Clicking an axis or face looks along it, and dragging orbits like the viewport.
//!
//! Ported from SSBH Studio (MIT), with its numpad view keys.
use crate::preferences::NavigationWidget;
use crate::renderer::{camera_rotation, Camera};
use egui::{Color32, Id, Order, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use glam::{Mat4, Vec3};
use std::f32::consts::{FRAC_PI_2, PI};

/// Distance from the viewport edges and from the toolbar.
const MARGIN: f32 = 8.0;

const RADIUS: f32 = 32.0;
const BALL: f32 = 9.0;
const AXES_SIZE: f32 = (RADIUS + BALL + 2.0) * 2.0;

/// Half the cube's edge length in points.
const CUBE_HALF: f32 = 28.0;
/// Distance from the cube's center to the turn arrows.
const ARROW_REACH: f32 = 60.0;
const CUBE_AREA: f32 = (ARROW_REACH + 8.0) * 2.0;
/// How much of each face its tile covers. The rest shows the darker silhouette as a seam.
const FACE_INSET: f32 = 0.85;
/// Corner rounding for the cube's faces and silhouette, in points.
const CUBE_ROUNDING: f32 = 5.0;

fn widget_size(style: NavigationWidget) -> Option<Vec2> {
    match style {
        NavigationWidget::Axes => Some(Vec2::splat(AXES_SIZE)),
        NavigationWidget::Cube => Some(Vec2::splat(CUBE_AREA)),
        NavigationWidget::Hidden => None,
    }
}

const COLOR_X: Color32 = Color32::from_rgb(232, 76, 76);
const COLOR_Y: Color32 = Color32::from_rgb(80, 200, 80);
const COLOR_Z: Color32 = Color32::from_rgb(70, 130, 255);

/// A direction to look from, and the camera rotation that looks back along it. The view
/// matrix is translation * rotation, so these rotations turn the direction toward +Z,
/// which faces the camera.
struct View {
    direction: Vec3,
    label: &'static str,
    name: &'static str,
    color: Color32,
    rotation: Vec3,
    positive: bool,
}

/// Listed in positive and negative pairs, so `i ^ 1` is the opposite view.
const VIEWS: [View; 6] = [
    View {
        direction: Vec3::X,
        label: "X",
        name: "Right",
        color: COLOR_X,
        rotation: Vec3::new(0.0, -FRAC_PI_2, 0.0),
        positive: true,
    },
    View {
        direction: Vec3::NEG_X,
        label: "",
        name: "Left",
        color: COLOR_X,
        rotation: Vec3::new(0.0, FRAC_PI_2, 0.0),
        positive: false,
    },
    View {
        direction: Vec3::Y,
        label: "Y",
        name: "Top",
        color: COLOR_Y,
        rotation: Vec3::new(FRAC_PI_2, 0.0, 0.0),
        positive: true,
    },
    View {
        direction: Vec3::NEG_Y,
        label: "",
        name: "Bottom",
        color: COLOR_Y,
        rotation: Vec3::new(-FRAC_PI_2, 0.0, 0.0),
        positive: false,
    },
    View {
        direction: Vec3::Z,
        label: "Z",
        name: "Front",
        color: COLOR_Z,
        rotation: Vec3::ZERO,
        positive: true,
    },
    View {
        direction: Vec3::NEG_Z,
        label: "",
        name: "Back",
        color: COLOR_Z,
        rotation: Vec3::new(0.0, PI, 0.0),
        positive: false,
    },
];

fn view_rotation(camera: &Camera) -> Mat4 {
    camera_rotation(camera.rotation)
}

/// Whether the camera already looks along `view`, ignoring whole turns.
fn is_looking_along(camera: &Camera, view: &View) -> bool {
    view_rotation(camera).transform_vector3(view.direction).z > 0.999
}

/// The rotation for clicking `index`: its view, or the opposite one if already there.
fn clicked_rotation(camera: &Camera, index: usize) -> Vec3 {
    if is_looking_along(camera, &VIEWS[index]) {
        VIEWS[index ^ 1].rotation
    } else {
        VIEWS[index].rotation
    }
}

/// Draws and interacts only inside the viewport, below `header` (the viewport toolbar) when
/// it reaches the corner. Returns whether the camera changed.
pub fn orientation_widget(
    ui: &Ui,
    viewport: Rect,
    header: Option<Rect>,
    style: NavigationWidget,
    camera: &mut Camera,
) -> bool {
    let viewport = viewport.intersect(ui.clip_rect());
    let Some(rect) = widget_rect(viewport, header, style) else {
        return false;
    };
    egui::Area::new(Id::unique("viewport_orientation"))
        .order(Order::Middle)
        .movable(false)
        // Screen-wide Area constraints can move the widget outside an inset viewport.
        // Placement is already clamped to the viewport, including after a style change.
        .constrain_to(viewport)
        .constrain(false)
        .fixed_pos(rect.min)
        .show(ui.ctx(), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(viewport));
            match style {
                NavigationWidget::Axes => axes_widget(ui, rect.size(), camera),
                NavigationWidget::Cube => cube_widget(ui, rect.size(), camera),
                NavigationWidget::Hidden => false,
            }
        })
        .inner
}

/// Height of the axis ball letters, in points.
const LETTER_HEIGHT: f32 = 7.0;
const LETTER_WIDTH: f32 = 5.2;
const MINUS_WIDTH: f32 = 3.2;
const MINUS_GAP: f32 = 1.4;

/// The strokes of `letter` in a box centered on the origin, y down.
fn letter_strokes(letter: &str) -> Vec<Vec<Vec2>> {
    let (w, h) = (LETTER_WIDTH * 0.5, LETTER_HEIGHT * 0.5);
    match letter {
        "X" => vec![
            vec![Vec2::new(-w, -h), Vec2::new(w, h)],
            vec![Vec2::new(-w, h), Vec2::new(w, -h)],
        ],
        "Y" => vec![
            vec![Vec2::new(-w, -h), Vec2::ZERO, Vec2::new(w, -h)],
            vec![Vec2::ZERO, Vec2::new(0.0, h)],
        ],
        _ => vec![vec![
            Vec2::new(-w, -h),
            Vec2::new(w, -h),
            Vec2::new(-w, h),
            Vec2::new(w, h),
        ]],
    }
}

/// The strokes of an axis label, a letter with a minus in front for `negative`, in
/// points around the origin with y down. The minus and letter are centered as one shape.
fn label_strokes(letter: &str, negative: bool) -> Vec<Vec<Vec2>> {
    let total = if negative {
        MINUS_WIDTH + MINUS_GAP + LETTER_WIDTH
    } else {
        LETTER_WIDTH
    };
    let letter_offset = Vec2::new(total * 0.5 - LETTER_WIDTH * 0.5, 0.0);
    let mut strokes: Vec<Vec<Vec2>> = letter_strokes(letter)
        .into_iter()
        .map(|points| points.into_iter().map(|p| p + letter_offset).collect())
        .collect();
    if negative {
        let left = -total * 0.5;
        strokes.push(vec![
            Vec2::new(left, 0.0),
            Vec2::new(left + MINUS_WIDTH, 0.0),
        ]);
    }
    strokes
}

/// Draws an axis label centered exactly on `center`. The letters are strokes rather than
/// font glyphs, so they stay centered at any display scale.
fn axis_label(
    painter: &egui::Painter,
    center: Pos2,
    letter: &str,
    negative: bool,
    color: Color32,
    scale: f32,
) {
    let stroke = Stroke::new(1.5 * scale.sqrt(), color);
    for points in label_strokes(letter, negative) {
        let points: Vec<Pos2> = points.iter().map(|p| center + *p * scale).collect();
        painter.add(egui::Shape::line(points, stroke));
    }
}

/// Orbits like dragging the viewport. Returns `true` if the camera moved.
fn drag_orbit(response: &egui::Response, camera: &mut Camera) -> bool {
    if !response.dragged() {
        return false;
    }
    let delta = response.drag_delta();
    if delta == Vec2::ZERO {
        return false;
    }
    camera.rotate(delta.x, delta.y);
    true
}

fn axes_widget(ui: &mut Ui, size: Vec2, camera: &mut Camera) -> bool {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let center = rect.center();
    let rotation = view_rotation(camera);
    // Far axes first so near ones draw on top.
    let mut order: Vec<(usize, Pos2, f32)> = VIEWS
        .iter()
        .enumerate()
        .map(|(i, view)| {
            let v = rotation.transform_vector3(view.direction);
            (i, center + Vec2::new(v.x, -v.y) * RADIUS, v.z)
        })
        .collect();
    order.sort_by(|a, b| a.2.total_cmp(&b.2));

    let pointer = response.hover_pos();
    let hovered = pointer.and_then(|p| {
        order
            .iter()
            .rev()
            .find(|(_, pos, _)| pos.distance(p) <= BALL + 2.0)
            .map(|(i, ..)| *i)
    });

    let painter = ui.painter();
    if response.hovered() || response.dragged() {
        painter.circle_filled(center, RADIUS + BALL, Color32::from_white_alpha(18));
    }
    // Every line before any ball, ending at the ball's edge, so no line shows across a ball.
    for &(i, pos, _) in &order {
        let view = &VIEWS[i];
        let reach = pos - center;
        if view.positive && reach.length() > BALL {
            let end = pos - reach.normalized() * BALL;
            painter.line_segment([center, end], Stroke::new(2.0, view.color));
        }
    }
    for &(i, pos, _) in &order {
        let view = &VIEWS[i];
        // Hovering turns the letter white, and gives the negative balls a letter and a
        // full fill so it reads.
        let lit = hovered == Some(i);
        let letter = if view.positive {
            view.label
        } else {
            VIEWS[i ^ 1].label
        };
        if view.positive {
            painter.circle_filled(pos, BALL, view.color);
            let color = if lit {
                Color32::WHITE
            } else {
                Color32::from_black_alpha(220)
            };
            axis_label(painter, pos, letter, false, color, 1.0);
        } else if lit {
            painter.circle_filled(pos, BALL, view.color);
            axis_label(painter, pos, letter, true, Color32::WHITE, 1.0);
        } else {
            painter.circle(
                pos,
                BALL - 2.0,
                view.color.gamma_multiply(0.35),
                Stroke::new(1.0, view.color.gamma_multiply(0.8)),
            );
        }
    }

    let response = response.on_hover_text(match hovered {
        Some(i) => format!(
            "{} view. Click again to look from the opposite side.",
            VIEWS[i].name
        ),
        None => "Click an axis to look along it. Drag to orbit.".to_owned(),
    });
    if drag_orbit(&response, camera) {
        return true;
    }
    if let Some(i) = hovered.filter(|_| response.clicked()) {
        let rotation = clicked_rotation(camera, i);
        camera.turn_to(rotation);
        return true;
    }
    false
}

/// The two axes along a face, so `normal +- u +- v` gives its corners.
fn face_axes(normal: Vec3) -> (Vec3, Vec3) {
    if normal.x != 0.0 {
        (Vec3::Y, Vec3::Z)
    } else if normal.y != 0.0 {
        (Vec3::Z, Vec3::X)
    } else {
        (Vec3::X, Vec3::Y)
    }
}

/// Whether `p` is inside a convex polygon, in either winding.
fn inside_polygon(p: Pos2, points: &[Pos2]) -> bool {
    let mut sign = 0.0;
    for (i, a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        let cross = (b - *a).x * (p - *a).y - (b - *a).y * (p - *a).x;
        if cross.abs() < f32::EPSILON {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// A turn arrow next to the cube: where it sits, which way it points, and the turn it
/// makes. Each brings the cube's face on that side to the front.
struct TurnArrow {
    direction: Vec2,
    turn: Vec3,
    name: &'static str,
}

const ARROWS: [TurnArrow; 4] = [
    TurnArrow {
        direction: Vec2::new(0.0, -1.0),
        turn: Vec3::new(FRAC_PI_2, 0.0, 0.0),
        name: "Tilt to see the top",
    },
    TurnArrow {
        direction: Vec2::new(0.0, 1.0),
        turn: Vec3::new(-FRAC_PI_2, 0.0, 0.0),
        name: "Tilt to see the bottom",
    },
    TurnArrow {
        direction: Vec2::new(-1.0, 0.0),
        turn: Vec3::new(0.0, FRAC_PI_2, 0.0),
        name: "Turn to see the left side",
    },
    TurnArrow {
        direction: Vec2::new(1.0, 0.0),
        turn: Vec3::new(0.0, -FRAC_PI_2, 0.0),
        name: "Turn to see the right side",
    },
];

/// Mixes two colors, `t` of the way from `a` to `b`.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

/// A convex polygon with each corner replaced by a short curve of up to `radius` points.
fn rounded_polygon(points: &[Pos2], radius: f32) -> Vec<Pos2> {
    let count = points.len();
    let mut out = Vec::with_capacity(count * 4);
    for i in 0..count {
        let corner = points[i];
        let prev = points[(i + count - 1) % count];
        let next = points[(i + 1) % count];
        let toward = |other: Pos2| {
            let edge = other - corner;
            corner + edge.normalized() * radius.min(edge.length() * 0.4)
        };
        let (a, b) = (toward(prev), toward(next));
        // A quadratic curve from `a` to `b` with the corner as its control point.
        for k in 0..=3 {
            let t = k as f32 / 3.0;
            let p = a.to_vec2() * (1.0 - t) * (1.0 - t)
                + corner.to_vec2() * 2.0 * t * (1.0 - t)
                + b.to_vec2() * t * t;
            out.push(p.to_pos2());
        }
    }
    out
}

/// The convex hull of `points`, counterclockwise on screen.
fn convex_hull(mut points: Vec<Pos2>) -> Vec<Pos2> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let cross = |o: Pos2, a: Pos2, b: Pos2| (a - o).x * (b - o).y - (a - o).y * (b - o).x;
    let mut hull: Vec<Pos2> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &Pos2>> = if pass == 0 {
            Box::new(points.iter())
        } else {
            Box::new(points.iter().rev())
        };
        for &p in iter {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// A chamfered cube: rounded faces sit just inside a darker silhouette, so thin seams read
/// as beveled edges. Faces shade by how much they turn toward a light at the upper left.
fn cube_widget(ui: &mut Ui, size: Vec2, camera: &mut Camera) -> bool {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let center = rect.center();
    let rotation = view_rotation(camera);
    let project = |p: Vec3| {
        let v = rotation.transform_vector3(p);
        center + Vec2::new(v.x, -v.y) * CUBE_HALF
    };

    // Faces toward the camera, far ones first so near ones draw on top.
    let mut faces: Vec<(usize, [Pos2; 4], Vec3)> = VIEWS
        .iter()
        .enumerate()
        .filter_map(|(i, view)| {
            let normal = rotation.transform_vector3(view.direction);
            (normal.z > 0.01).then(|| {
                let (u, v) = face_axes(view.direction);
                let n = view.direction;
                let corners = [n - u - v, n + u - v, n + u + v, n - u + v].map(project);
                (i, corners, normal)
            })
        })
        .collect();
    faces.sort_by(|a, b| a.2.z.total_cmp(&b.2.z));

    let pointer = response.hover_pos();
    let hovered_face = pointer.and_then(|p| {
        faces
            .iter()
            .rev()
            .find(|(_, corners, _)| inside_polygon(p, corners))
            .map(|(i, ..)| *i)
    });
    let arrow_tips: Vec<(usize, Pos2)> = ARROWS
        .iter()
        .enumerate()
        .map(|(i, a)| (i, center + a.direction * ARROW_REACH))
        .collect();
    let hovered_arrow = pointer.and_then(|p| {
        arrow_tips
            .iter()
            .find(|(_, tip)| tip.distance(p) <= 11.0)
            .map(|(i, _)| *i)
    });

    let painter = ui.painter();
    let visuals = ui.visuals();
    let base = visuals.widgets.inactive.bg_fill;
    let seam = mix(base, Color32::BLACK, 0.6);
    if response.hovered() || response.dragged() {
        painter.circle_filled(center, ARROW_REACH + 8.0, Color32::from_white_alpha(12));
    }

    // A soft contact shadow under the cube, stacked thin ellipses standing in for a blur.
    let ground = center + Vec2::new(0.0, CUBE_HALF * 1.85);
    for (spread, alpha) in [(1.0, 18), (0.8, 22), (0.6, 26)] {
        painter.add(egui::Shape::ellipse_filled(
            ground,
            Vec2::new(CUBE_HALF * 1.5 * spread, 5.0 * spread),
            Color32::from_black_alpha(alpha),
        ));
    }

    // The silhouette shows through the gaps between faces as dark seams.
    let silhouette: Vec<Pos2> = faces.iter().flat_map(|(_, corners, _)| *corners).collect();
    painter.add(egui::Shape::convex_polygon(
        rounded_polygon(&convex_hull(silhouette), CUBE_ROUNDING + 1.0),
        seam,
        Stroke::new(1.0, Color32::from_white_alpha(22)),
    ));

    let light = Vec3::new(-0.45, 0.65, 0.6).normalize();
    for &(i, corners, normal) in &faces {
        let view = &VIEWS[i];
        let lit = hovered_face == Some(i);
        let face_center = corners.iter().fold(Vec2::ZERO, |sum, c| sum + c.to_vec2()) / 4.0;
        let inset: Vec<Pos2> = corners
            .iter()
            .map(|c| (face_center + (c.to_vec2() - face_center) * FACE_INSET).to_pos2())
            .collect();
        let shade = 0.7 + 0.3 * normal.dot(light).max(0.0);
        let fill = if lit {
            mix(base, view.color, 0.75)
        } else {
            // A dim tint of the face's axis color, stronger as the face turns toward you.
            let tinted = mix(base, view.color, 0.18 + 0.12 * normal.z);
            mix(Color32::BLACK, tinted, shade)
        };
        // A hairline highlight on the face's edge, brighter on faces toward the light.
        let rim = Stroke::new(1.0, Color32::from_white_alpha((14.0 + 30.0 * shade) as u8));
        painter.add(egui::Shape::convex_polygon(
            rounded_polygon(&inset, CUBE_ROUNDING),
            fill,
            rim,
        ));
        // Hide labels on faces seen nearly edge on.
        let facing = normal.z;
        if facing > 0.3 {
            let (letter, scale, color) = if view.positive {
                (view.label, 1.4 + 0.4 * facing, view.color)
            } else {
                (
                    VIEWS[i ^ 1].label,
                    1.1 + 0.3 * facing,
                    view.color.gamma_multiply(0.7),
                )
            };
            let color = if lit { Color32::WHITE } else { color };
            axis_label(
                painter,
                face_center.to_pos2(),
                letter,
                !view.positive,
                color,
                scale,
            );
        }
    }

    // Thin chevrons that brighten when hovered.
    for (i, tip) in &arrow_tips {
        let dir = ARROWS[*i].direction;
        let side = Vec2::new(-dir.y, dir.x);
        let color = if hovered_arrow == Some(*i) {
            visuals.strong_text_color()
        } else {
            visuals.weak_text_color().gamma_multiply(0.6)
        };
        let point = *tip + dir * 3.0;
        let stroke = Stroke::new(1.8, color);
        painter.line_segment([point, point - dir * 5.0 + side * 6.0], stroke);
        painter.line_segment([point, point - dir * 5.0 - side * 6.0], stroke);
    }

    let response = response.on_hover_text(match (hovered_face, hovered_arrow) {
        (_, Some(i)) => ARROWS[i].name.to_owned(),
        (Some(i), _) => format!(
            "{} view. Click again to look from the opposite side.",
            VIEWS[i].name
        ),
        _ => "Click a face to look at it. Drag to orbit.".to_owned(),
    });
    if drag_orbit(&response, camera) {
        return true;
    }
    if response.clicked() {
        if let Some(i) = hovered_arrow {
            camera.turn_to(camera.rotation + ARROWS[i].turn);
            return true;
        }
        if let Some(i) = hovered_face {
            let rotation = clicked_rotation(camera, i);
            camera.turn_to(rotation);
            return true;
        }
    }
    false
}

/// The camera rotation for a numpad view key, like Blender: 1 front, 3 right, 7 top, with
/// Ctrl for the opposite side, and 9 for the opposite of the current view.
pub fn numpad_view(key: egui::Key, ctrl: bool, camera: &Camera) -> Option<Vec3> {
    let index = match key {
        egui::Key::Num1 => 4,
        egui::Key::Num3 => 0,
        egui::Key::Num7 => 2,
        egui::Key::Num9 => {
            return Some(
                match VIEWS.iter().position(|v| is_looking_along(camera, v)) {
                    Some(i) => VIEWS[i ^ 1].rotation,
                    None => camera.rotation + Vec3::new(0.0, PI, 0.0),
                },
            );
        }
        _ => return None,
    };
    Some(VIEWS[if ctrl { index ^ 1 } else { index }].rotation)
}

/// Prefer the top right, below the toolbar when it reaches that far, but never leave the
/// viewport. A viewport too small for the widget, or a hidden one, has none.
fn widget_rect(viewport: Rect, header: Option<Rect>, style: NavigationWidget) -> Option<Rect> {
    let size = widget_size(style)?;
    if !viewport.is_finite() || viewport.width() < size.x || viewport.height() < size.y {
        return None;
    }
    let padding = ((viewport.size() - size) * 0.5).min(Vec2::splat(MARGIN));
    let min = viewport.min + padding;
    let max = viewport.max - padding - size;
    let x = max.x;
    let preferred_y = match header {
        Some(header) if header.max.x + MARGIN > x => header.max.y + MARGIN,
        _ => min.y,
    };
    let y = preferred_y.clamp(min.y, max.y);
    Some(Rect::from_min_size(Pos2::new(x, y), size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_widget_stays_inside_the_viewport_and_moves_below_a_wide_toolbar() {
        for style in [NavigationWidget::Axes, NavigationWidget::Cube] {
            let size = widget_size(style).unwrap();
            for width in [0.0, 40.0, 86.0, 136.0, 300.0, 900.0] {
                for height in [0.0, 40.0, 86.0, 136.0, 300.0, 600.0] {
                    let viewport =
                        Rect::from_min_size(Pos2::new(350.0, 75.0), Vec2::new(width, height));
                    for toolbar_height in [0.0, 32.0, 120.0, 800.0] {
                        let toolbar =
                            Rect::from_min_size(viewport.min, Vec2::new(width, toolbar_height));
                        match widget_rect(viewport, Some(toolbar), style) {
                            Some(rect) => {
                                assert!(viewport.contains_rect(rect), "{style:?} {rect:?}");
                                assert_eq!(rect.size(), size);
                            }
                            None => assert!(width < size.x || height < size.y),
                        }
                    }
                }
            }
        }
        let viewport = Rect::from_min_size(Pos2::new(200.0, 50.0), Vec2::new(600.0, 500.0));
        let short = Rect::from_min_size(viewport.min, Vec2::new(100.0, 32.0));
        let wide = Rect::from_min_size(viewport.min, Vec2::new(600.0, 96.0));
        for style in [NavigationWidget::Axes, NavigationWidget::Cube] {
            let top = |toolbar| widget_rect(viewport, toolbar, style).unwrap().top();
            assert_eq!(top(None), viewport.top() + MARGIN);
            assert_eq!(top(Some(short)), viewport.top() + MARGIN);
            assert_eq!(top(Some(wide)), wide.bottom() + MARGIN);
        }
        assert_eq!(widget_rect(viewport, None, NavigationWidget::Hidden), None);
    }

    fn camera_with(rotation: Vec3) -> Camera {
        Camera::from_view(&crate::renderer::CameraView {
            translation: [0.0, 0.0, -100.0],
            rotation: rotation.to_array(),
            ..Default::default()
        })
    }

    #[test]
    fn each_view_puts_the_camera_on_its_axis() {
        for view in &VIEWS {
            let camera = camera_with(view.rotation);
            let position = camera.transforms(800.0, 600.0).camera_pos.truncate();
            assert!(
                position.normalize().abs_diff_eq(view.direction, 1e-4),
                "{}: {position}",
                view.name
            );
            assert!(is_looking_along(&camera, view), "{}", view.name);
        }
    }

    #[test]
    fn numpad_keys_pick_blenders_views() {
        let front = camera_with(Vec3::ZERO);
        let view = |key, ctrl| numpad_view(key, ctrl, &front).unwrap();
        assert_eq!(view(egui::Key::Num1, false), VIEWS[4].rotation);
        assert_eq!(view(egui::Key::Num1, true), VIEWS[5].rotation);
        assert_eq!(view(egui::Key::Num3, false), VIEWS[0].rotation);
        assert_eq!(view(egui::Key::Num7, false), VIEWS[2].rotation);
        assert_eq!(view(egui::Key::Num9, false), VIEWS[5].rotation);
        assert_eq!(numpad_view(egui::Key::Num5, false, &front), None);
    }

    #[test]
    fn clicking_the_axis_already_looked_along_turns_to_the_opposite_side() {
        let front = camera_with(Vec3::ZERO);
        assert_eq!(clicked_rotation(&front, 4), VIEWS[5].rotation);
        assert_eq!(clicked_rotation(&front, 0), VIEWS[0].rotation);
    }

    #[test]
    fn turning_keeps_the_target_where_it_was_on_screen() {
        let mut camera = camera_with(Vec3::new(0.3, 0.7, 0.0));
        camera.target = Vec3::new(4.0, 10.0, -2.0);
        let on_screen = |camera: &Camera| {
            camera_rotation(camera.rotation).transform_vector3(camera.target) + camera.translation
        };
        let before = on_screen(&camera);
        for view in &VIEWS {
            camera.turn_to(view.rotation);
            assert!(
                on_screen(&camera).abs_diff_eq(before, 1e-3),
                "{}",
                view.name
            );
        }
    }
}
