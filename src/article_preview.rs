//! Fighter objects (projectiles and other articles) in the 3D preview: which ones a move
//! spawns, where they appear, how they fly, and the hitboxes their own scripts make.
//!
//! The game moves an object with its own status code, which is not in the scripts. The flight
//! here is read from the object's parameters (speed, angle, gravity, life, bounces), so it is
//! close to the game for straight shots and lobbed ones, and only a guide for anything that
//! homes, steers or sticks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ssbh_data::anim_data::AnimData;
use ssbh_data::skel_data::SkelData;

/// How an object moves after it is spawned, from the fighter's parameter file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    /// Frames before it disappears.
    pub life: f32,
    /// Distance per frame along `angle`.
    pub speed: f32,
    /// Degrees above forward; negative aims down.
    pub angle: f32,
    /// Downward speed gained per frame.
    pub gravity: f32,
    /// Upward speed after touching the ground, for objects that bounce.
    pub bounce: Option<f32>,
    /// How far in front of the fighter it starts.
    pub start_ahead: f32,
    /// For things that come back (Link's boomerang): it slows by `accel` a frame until it is
    /// down to `turn_speed`, then comes back speeding up by `accel` to `return_speed` until it
    /// is home.
    pub returns: Option<Return>,
}

/// How a returning object slows, turns and comes back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Return {
    pub accel: f32,
    pub turn_speed: f32,
    pub return_speed: f32,
}

impl Default for Flight {
    fn default() -> Self {
        Self {
            life: 60.0,
            speed: 0.0,
            angle: 0.0,
            gravity: 0.0,
            bounce: None,
            start_ahead: 0.0,
            returns: None,
        }
    }
}

impl Flight {
    /// Offset from the spawn point after `frames`, as (forward, up). The ground is at
    /// `-start_height`, so a bouncing object turns back up there.
    pub fn offset(&self, frames: f32, start_height: f32) -> (f32, f32) {
        if let Some(back) = self.returns {
            return (self.returning_distance(frames, back), 0.0);
        }
        let radians = self.angle.to_radians();
        let (mut forward, mut up) = (self.start_ahead, 0.0);
        let (step_forward, mut step_up) = (self.speed * radians.cos(), self.speed * radians.sin());
        let whole = frames.max(0.0).floor() as usize;
        for _ in 0..whole {
            // Gravity first, then the move: measured in game, Mario's fireball falls one
            // gravity step further each frame than moving first would give (the gap grew by
            // exactly 0.1 a frame).
            step_up -= self.gravity;
            forward += step_forward;
            up += step_up;
            if up < -start_height {
                match self.bounce {
                    Some(bounce) => {
                        up = -start_height;
                        step_up = bounce;
                    }
                    None => up = -start_height,
                }
            }
        }
        let part = frames.max(0.0) - whole as f32;
        (forward + step_forward * part, up + step_up * part)
    }

    /// Distance ahead of where a returning object was thrown from, after `frames`; it never
    /// goes behind the thrower.
    fn returning_distance(&self, frames: f32, back: Return) -> f32 {
        let (mut distance, mut speed, mut outward) = (self.start_ahead, self.speed, true);
        let whole = frames.max(0.0).floor() as usize;
        for _ in 0..whole {
            distance += if outward { speed } else { -speed };
            if outward {
                speed -= back.accel;
                if speed <= back.turn_speed {
                    outward = false;
                }
            } else {
                speed = (speed + back.accel).min(back.return_speed);
            }
            if distance <= 0.0 {
                return 0.0;
            }
        }
        let part = frames.max(0.0) - whole as f32;
        let next = if outward { speed } else { -speed };
        (distance + next * part).max(0.0)
    }
}

/// Everything needed to show one kind of object.
#[derive(Default)]
pub struct ObjectKind {
    /// "fireball", from `FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL`.
    pub name: String,
    /// Objects drawn only with effects (Samus's charge shot) have no model.
    pub model_dir: Option<PathBuf>,
    pub skel: Option<Arc<SkelData>>,
    pub anim: Option<Arc<AnimData>>,
    /// Hitboxes from the object's own script, with frames counted from its spawn.
    pub hitboxes: Vec<crate::data::Hitbox>,
    /// Effects from the object's own script; for many projectiles they are all you see.
    pub effects: Vec<ObjectEffect>,
    pub flight: Flight,
}

/// An effect an object's script shows, on one of its bones.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectEffect {
    pub name: String,
    pub bone: String,
    pub offset: [f32; 3],
    /// Degrees about X, Y and Z, in the script's own order.
    pub rotation: [f32; 3],
    pub scale: f32,
    /// Script frames it is shown, counted from the object's spawn.
    pub start: u32,
    pub end: u32,
}

/// The effects in an object's script. Objects often pick an effect by facing, which the
/// effect parser leaves out, so this reads the effect lines themselves and keeps the first of
/// each pair.
fn object_effects(body: &str) -> Vec<ObjectEffect> {
    // The preview faces right, so the script's left-facing choice is swapped for its pair.
    let mut effects = object_effects_as_written(body);
    for effect in &mut effects {
        if let Some(stem) = effect.name.strip_suffix("_l") {
            let right = format!("{stem}_r");
            if body.contains(&format!("\"{right}\"")) {
                effect.name = right;
            }
        }
    }
    effects
}

fn object_effects_as_written(body: &str) -> Vec<ObjectEffect> {
    let parsed = crate::acmd::parse_effect_script(body).to_effect_calls();
    if !parsed.is_empty() {
        return parsed
            .into_iter()
            .map(|call| ObjectEffect {
                name: call.effect_name,
                bone: call.bone_name,
                offset: call.offset,
                // Stored reversed on the call (see `app::script_rotation`).
                rotation: [call.rotation[2], call.rotation[1], call.rotation[0]],
                scale: call.scale,
                start: call.active_start,
                end: if call.follows_bone {
                    u32::MAX
                } else {
                    call.active_end
                },
            })
            .collect();
    }
    let Some(start) = body.find("fn effect_") else {
        return Vec::new();
    };
    let section = &body[start..];
    let section = &section[..section[1..]
        .find("unsafe extern")
        .map_or(section.len(), |end| end + 1)];
    let mut frame = 0.0f32;
    let mut effects: Vec<ObjectEffect> = Vec::new();
    for line in section.lines().map(str::trim) {
        let number = |text: &str| {
            text.rsplit(',').next().and_then(|value| {
                value
                    .trim()
                    .trim_end_matches(");")
                    .trim()
                    .parse::<f32>()
                    .ok()
            })
        };
        if line.starts_with("frame(") {
            frame = number(line).unwrap_or(frame);
        } else if line.starts_with("wait(") {
            frame += number(line).unwrap_or(0.0);
        } else if line.contains("EFFECT") && line.contains("Hash40::new(\"") {
            let names: Vec<&str> = line
                .split("Hash40::new(\"")
                .skip(1)
                .filter_map(|part| part.split('"').next())
                .collect();
            let (Some(name), Some(bone)) = (names.first(), names.get(1)) else {
                continue;
            };
            let after = line.rsplit("\")").next().unwrap_or_default();
            let numbers: Vec<f32> = after
                .split(',')
                .filter_map(|value| value.trim().trim_end_matches(");").parse().ok())
                .collect();
            let start = frame.max(1.0) as u32;
            if effects
                .iter()
                .any(|effect| effect.start == start && effect.bone == *bone)
            {
                continue;
            }
            effects.push(ObjectEffect {
                name: (*name).to_owned(),
                bone: (*bone).to_owned(),
                offset: [
                    numbers.first().copied().unwrap_or(0.0),
                    numbers.get(1).copied().unwrap_or(0.0),
                    numbers.get(2).copied().unwrap_or(0.0),
                ],
                rotation: [
                    numbers.get(3).copied().unwrap_or(0.0),
                    numbers.get(4).copied().unwrap_or(0.0),
                    numbers.get(5).copied().unwrap_or(0.0),
                ],
                scale: numbers.get(6).copied().unwrap_or(1.0),
                start,
                end: if line.contains("FOLLOW") {
                    u32::MAX
                } else {
                    start
                },
            });
        }
    }
    effects
}

/// One object out at the current frame, placed in the world.
pub struct PlacedObject {
    pub kind: Arc<ObjectKind>,
    /// The frame it was spawned on and the one it is removed on, to place it at earlier ages.
    pub spawned: u32,
    pub removed: Option<u32>,
    /// The frame the move throws it, when it is held first (Link's boomerang).
    pub shot: Option<u32>,
    /// Frames since it was spawned, from 0.
    pub age: f32,
    /// Object space to world space.
    pub placement: glam::Mat4,
}

/// "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL" becomes "fireball".
pub fn article_name(symbol: &str) -> Option<String> {
    symbol
        .trim_start_matches('*')
        .split_once("_GENERATE_ARTICLE_")
        .map(|(_, name)| name.to_ascii_lowercase())
}

/// Reads one object's flight from its parameter block, `param_<name>`.
pub fn flight_from_params(param_path: &Path, name: &str, labels: &HashMap<u64, String>) -> Flight {
    let mut flight = Flight::default();
    let Ok(param) = prc::open(param_path) else {
        return flight;
    };
    let wanted = format!("param_{name}");
    let block = param.0.iter().find_map(|(hash, value)| {
        let label = labels.get(&hash.0)?;
        (label == &wanted || label.trim_end_matches('s') == wanted).then_some(value)
    });
    let fields = match block {
        Some(prc::ParamKind::Struct(fields)) => fields,
        Some(prc::ParamKind::List(list)) => match list.0.first() {
            Some(prc::ParamKind::Struct(fields)) => fields,
            _ => return flight,
        },
        _ => return flight,
    };
    let values: HashMap<&str, f32> = fields
        .0
        .iter()
        .filter_map(|(hash, value)| {
            let label = labels.get(&hash.0)?;
            let number = match value {
                prc::ParamKind::Float(value) => *value,
                prc::ParamKind::I32(value) => *value as f32,
                prc::ParamKind::U32(value) => *value as f32,
                prc::ParamKind::I16(value) => *value as f32,
                prc::ParamKind::U16(value) => *value as f32,
                prc::ParamKind::I8(value) => *value as f32,
                prc::ParamKind::U8(value) => *value as f32,
                _ => return None,
            };
            Some((label.trim_end_matches('_'), number))
        })
        .collect();
    let first = |names: &[&str]| names.iter().find_map(|name| values.get(name).copied());
    if let Some(life) = first(&["life", "life_frame", "max_life"]).filter(|life| *life > 0.0) {
        flight.life = life;
    }
    if let Some(speed) = first(&[
        "speed",
        "speed_x",
        "init_speed",
        "start_speed",
        "speed_min",
        "min_speed",
        "speed_max",
        "max_speed",
    ]) {
        flight.speed = speed.abs();
    }
    if let Some(angle) = first(&["angle", "shoot_angle", "start_angle"]) {
        // A few objects store the angle in radians.
        flight.angle = if angle.abs() < std::f32::consts::PI && angle.fract() != 0.0 {
            angle.to_degrees()
        } else {
            angle
        };
    }
    if let Some(gravity) = first(&["gravity_accel", "accel_y", "gravity", "fall_accel"]) {
        flight.gravity = gravity.abs();
    }
    if let Some(ahead) = first(&["base_offset_x", "offset_x"]) {
        flight.start_ahead = ahead;
    }
    // A boomerang slows to its minimum speed and comes back (Link's: speed 2.4, accel 0.047;
    // measured in game, a thrown one loses exactly 0.047 a frame on its way out).
    if let (Some(accel), Some(turn_speed), Some(return_speed)) = (
        values.get("accel").copied(),
        values.get("speed_min").copied(),
        values.get("speed_max").copied(),
    ) {
        if values.contains_key("turn_dist") && accel > 0.0 {
            if let Some(speed) = values.get("speed").copied() {
                flight.speed = speed.abs();
            }
            flight.returns = Some(Return {
                accel,
                turn_speed,
                return_speed,
            });
        }
    }
    if values.contains_key("bound_count_max") {
        flight.bounce = first(&["bounded_speed_y_min", "bound_speed_y"]).or(Some(flight.speed));
    }
    flight
}

/// Loads an object's model, skeleton, animation, hitboxes and flight. `script` gives the body of
/// one of the object's game scripts by name ("Regular"), or nothing.
pub fn load_kind(
    fighter_dir: &Path,
    param_path: &Path,
    slot: u8,
    name: &str,
    labels: &HashMap<u64, String>,
    script: &dyn Fn(&str) -> Option<String>,
) -> Option<ObjectKind> {
    let skel_path = crate::data::find_part_skel(&fighter_dir.join("model").join(name), slot);
    let model_dir = skel_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(Path::to_path_buf);
    let skel = skel_path.and_then(|path| SkelData::from_file(path).ok());
    let motion_dir = costume_dir(&fighter_dir.join("motion").join(name), slot);
    // The object plays the first motion in its list that has a script, "regular" first.
    let mut motions: Vec<(String, Option<PathBuf>)> = Vec::new();
    if let Some(dir) = &motion_dir {
        let files: HashMap<u64, PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "nuanmb"))
            .filter_map(|path| {
                let file = path.file_name()?.to_str()?.to_owned();
                Some((hash40::hash40(&file).0, path))
            })
            .collect();
        if let Ok(list) = motion_lib::open(dir.join("motion_list.bin")) {
            for (kind, motion) in list.list.iter() {
                let Some(label) = labels.get(&kind.0) else {
                    continue;
                };
                let anim = motion
                    .animations
                    .iter()
                    .find_map(|animation| files.get(&animation.name.0).cloned());
                motions.push((label.clone(), anim));
            }
        }
        if motions.is_empty() {
            motions.extend(
                files
                    .into_values()
                    .take(1)
                    .map(|path| (String::new(), Some(path))),
            );
        }
    }
    motions.sort_by_key(|(label, _)| label != "regular");
    let mut chosen = motions.first().cloned();
    let mut hitboxes = Vec::new();
    let mut effects = Vec::new();
    for (label, anim) in &motions {
        if label.is_empty() {
            continue;
        }
        let Some(body) = script(&pascal(label)) else {
            continue;
        };
        let found = crate::acmd::parse_acmd_script(&body).to_hitboxes();
        let shown = object_effects(&body);
        if !found.is_empty() || !shown.is_empty() {
            hitboxes = found;
            effects = shown;
            chosen = Some((label.clone(), anim.clone()));
            if !hitboxes.is_empty() {
                break;
            }
        }
    }
    let anim = chosen
        .and_then(|(_, path)| path)
        .and_then(|path| AnimData::from_file(path).ok())
        .map(Arc::new);
    let flight = flight_from_params(param_path, name, labels);
    if skel.is_none() && hitboxes.is_empty() && effects.is_empty() && flight.speed == 0.0 {
        return None;
    }
    Some(ObjectKind {
        name: name.to_owned(),
        model_dir,
        skel: skel.map(Arc::new),
        anim,
        hitboxes,
        effects,
        flight,
    })
}

/// `part/cNN` for the slot, or the part's first costume folder.
fn costume_dir(part: &Path, slot: u8) -> Option<PathBuf> {
    let preferred = part.join(format!("c{slot:02}"));
    if preferred.is_dir() {
        return Some(preferred);
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(part)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter().next()
}

/// "special_n_bound" becomes "SpecialNBound", the dump's script file name.
fn pascal(motion: &str) -> String {
    motion
        .split('_')
        .map(|part| {
            let mut letters = part.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        })
        .collect()
}

/// Objects that don't fly but are held, by name: they ride the fighter's hand.
fn held(name: &str) -> bool {
    [
        "gun", "blaster", "hammer", "sword", "weapon", "kassar", "umbrella", "bat", "club", "bow",
        "fan", "racket", "pan", "staff", "tome", "book",
    ]
    .iter()
    .any(|part| name.contains(part))
}

/// The bone an object that doesn't fly rides on.
fn ride_bone(bones: &HashMap<String, glam::Mat4>, name: &str) -> glam::Mat4 {
    let top = bones.get("top").copied().unwrap_or(glam::Mat4::IDENTITY);
    if held(name) {
        for bone in ["haver", "havel"] {
            if let Some(matrix) = bones.get(bone) {
                return *matrix;
            }
        }
    }
    let (_, rotation, position) = top.to_scale_rotation_translation();
    glam::Mat4::from_rotation_translation(rotation, position)
}

/// Where the object starts: the fighter's hand for things that fly, its centre otherwise.
fn spawn_point(bones: &HashMap<String, glam::Mat4>, flies: bool) -> glam::Vec3 {
    let top = bones
        .get("top")
        .map_or(glam::Vec3::ZERO, |matrix| matrix.col(3).truncate());
    if flies {
        // Mario's fireball leaves the forward hand, where the move's shoot effect plays (havel).
        for bone in ["havel", "haver", "handl", "handr"] {
            if let Some(matrix) = bones.get(bone) {
                return matrix.col(3).truncate();
            }
        }
        return top + glam::Vec3::Y * 8.0;
    }
    top
}

/// One object a move creates.
#[derive(Clone)]
pub struct Spawn {
    pub kind: Arc<ObjectKind>,
    /// The frame it is created on.
    pub spawned: u32,
    /// The frame the move removes it on.
    pub removed: Option<u32>,
    /// The frame the move throws it (`ArticleModule::shoot`), if it is held until then:
    /// Link's boomerang is created in his hand on frame 5 and leaves it on frame 27.
    pub shot: Option<u32>,
}

/// The objects out at `frame`, placed in the world. `bones_at` gives the fighter's bone
/// matrices at a game frame.
pub fn place(
    spawns: &[Spawn],
    frame: f32,
    bones_at: &dyn Fn(f32) -> HashMap<String, glam::Mat4>,
) -> Vec<PlacedObject> {
    let mut placed = Vec::new();
    for Spawn {
        kind,
        spawned,
        removed,
        shot,
    } in spawns
    {
        let age = frame - *spawned as f32;
        if age < 0.0 || removed.is_some_and(|removed| frame >= removed as f32) {
            continue;
        }
        let flies = !(kind.flight.speed == 0.0 && kind.flight.start_ahead == 0.0);
        // Something that doesn't fly rides the fighter until the move removes it; something
        // thrown later is held in the hand until it is.
        let held_until_shot = shot.is_some_and(|shot| frame < shot as f32);
        if !flies || held_until_shot {
            let bones = bones_at(frame);
            let placement = if flies {
                let (_, rotation, _) = bones
                    .get("top")
                    .copied()
                    .unwrap_or(glam::Mat4::IDENTITY)
                    .to_scale_rotation_translation();
                glam::Mat4::from_rotation_translation(rotation, spawn_point(&bones, true))
            } else {
                ride_bone(&bones, &kind.name)
            };
            placed.push(PlacedObject {
                kind: kind.clone(),
                spawned: *spawned,
                removed: *removed,
                shot: *shot,
                age,
                placement,
            });
            continue;
        }
        // In flight: from where it leaves the fighter, counted from then.
        let launched = shot.unwrap_or(*spawned);
        let flight_age = frame - launched as f32;
        if flight_age > kind.flight.life {
            continue;
        }
        let bones = bones_at(launched as f32);
        let top = bones.get("top").copied().unwrap_or(glam::Mat4::IDENTITY);
        let (_, rotation, top_position) = top.to_scale_rotation_translation();
        let mut forward = top.z_axis.truncate();
        forward.y = 0.0;
        let forward = forward.try_normalize().unwrap_or(glam::Vec3::Z);
        let start = spawn_point(&bones, kind.flight.speed > 0.0);
        let (ahead, up) = kind.flight.offset(flight_age, start.y - top_position.y);
        let position = start + forward * ahead + glam::Vec3::Y * up;
        placed.push(PlacedObject {
            kind: kind.clone(),
            spawned: *spawned,
            removed: *removed,
            shot: *shot,
            age,
            placement: glam::Mat4::from_rotation_translation(rotation, position),
        });
    }
    placed
}

/// An object's bones in the world at its age.
pub fn object_bones(object: &PlacedObject) -> HashMap<String, glam::Mat4> {
    let Some(skel) = &object.kind.skel else {
        return HashMap::from([("top".to_owned(), object.placement)]);
    };
    crate::renderer::compute_bone_world_matrices(skel, object.kind.anim.as_deref(), &[], object.age)
        .into_iter()
        .map(|(name, matrix)| (name, object.placement * matrix))
        .collect()
}

/// A one-key animation that puts the object's root bone where `placement` says, on top of
/// the object's own animation for that frame.
pub fn placement_layer(object: &PlacedObject) -> Option<AnimData> {
    use ssbh_data::anim_data::{GroupData, GroupType, NodeData, TrackData, TrackValues, Transform};
    let skel = object.kind.skel.as_ref()?;
    let root = skel.bones.iter().find(|bone| bone.parent_index.is_none())?;
    let local = crate::renderer::compute_bone_world_matrices(
        skel,
        object.kind.anim.as_deref(),
        &[],
        object.age,
    )
    .get(&root.name)
    .copied()
    .unwrap_or_else(|| glam::Mat4::from_cols_array_2d(&root.transform));
    let (scale, rotation, translation) = (object.placement * local).to_scale_rotation_translation();
    Some(AnimData {
        major_version: 2,
        minor_version: 0,
        final_frame_index: 0.0,
        groups: vec![GroupData {
            group_type: GroupType::Transform,
            nodes: vec![NodeData {
                name: root.name.clone(),
                tracks: vec![TrackData {
                    name: "Transform".into(),
                    compensate_scale: false,
                    transform_flags: Default::default(),
                    values: TrackValues::Transform(vec![Transform {
                        scale: ssbh_data::Vector3::new(scale.x, scale.y, scale.z),
                        rotation: ssbh_data::Vector4::new(
                            rotation.x, rotation.y, rotation.z, rotation.w,
                        ),
                        translation: ssbh_data::Vector3::new(
                            translation.x,
                            translation.y,
                            translation.z,
                        ),
                    }]),
                }],
            }],
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_names_come_from_the_symbol() {
        assert_eq!(
            article_name("*FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL").as_deref(),
            Some("fireball")
        );
        assert_eq!(article_name("FIGHTER_MARIO_STATUS_KIND_X"), None);
    }

    #[test]
    fn a_lobbed_object_falls_and_bounces_off_the_ground() {
        let flight = Flight {
            life: 70.0,
            speed: 1.4,
            angle: -10.0,
            gravity: 0.1,
            bounce: Some(1.4),
            ..Flight::default()
        };
        let (ahead, up) = flight.offset(10.0, 8.0);
        assert!(ahead > 13.0 && ahead < 14.0, "{ahead}");
        assert!(up < 0.0 && up >= -8.0, "{up}");
        // Long after it would have fallen through, it is still at or above the ground.
        let lowest = (0..70)
            .map(|frame| flight.offset(frame as f32, 8.0).1)
            .fold(f32::INFINITY, f32::min);
        assert!(lowest >= -8.0, "{lowest}");
        let straight = Flight {
            speed: 2.0,
            ..Flight::default()
        };
        assert_eq!(straight.offset(5.0, 8.0), (10.0, 0.0));
    }

    #[test]
    fn effects_chosen_by_facing_use_the_right_facing_one() {
        // Mario's fireball, as the script dump writes it.
        let body = r#"unsafe extern "C" fn effect_regular(agent: &mut L2CAgentBase) {
    if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {
        if macros::is_excute(agent) {
            macros::EFFECT_FOLLOW(agent, Hash40::new("mario_fb_bullet_l"), Hash40::new("rot"), 0, 1.8, 0, 0, 0, 0, 1, true);
        }
        else {
        if macros::is_excute(agent) {
            macros::EFFECT_FOLLOW(agent, Hash40::new("mario_fb_bullet_r"), Hash40::new("rot"), 0, 1.8, 0, 0, 0, 0, 1, true);
        }
    }
}
}"#;
        let effects = object_effects(body);
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].name, "mario_fb_bullet_r");
        assert_eq!(effects[0].bone, "rot");
        assert_eq!(effects[0].offset, [0.0, 1.8, 0.0]);
    }

    #[test]
    fn a_fighter_script_s_facing_branch_is_previewed_facing_right() {
        // Mario's neutral special, as the script dump writes it: the `else` sits inside the `if`.
        let body = r#"unsafe extern "C" fn effect_specialn(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::LANDING_EFFECT(agent, Hash40::new("sys_action_smoke_h"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
    }
    frame(agent.lua_state_agent, 13.0);
    if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {
        if macros::is_excute(agent) {
            macros::EFFECT_FOLLOW(agent, Hash40::new("mario_fb_shoot"), Hash40::new("havel"), 0, 0, 0, 0, 45, 0, 1, true);
        }
        else {
        if macros::is_excute(agent) {
            macros::EFFECT_FOLLOW(agent, Hash40::new("mario_fb_shoot"), Hash40::new("havel"), 0, 0, 0, 0, -45, 0, 1, true);
        }
    }
}
}"#;
        let script = crate::acmd::parse_effect_script(body);
        assert!(script
            .to_effect_calls()
            .iter()
            .all(|call| call.effect_name != "mario_fb_shoot"));
        let extra = script.facing_right_calls();
        assert_eq!(extra.len(), 1);
        assert_eq!(extra[0].effect_name, "mario_fb_shoot");
        assert_eq!(extra[0].bone_name, "havel");
        assert_eq!(extra[0].rotation, [0.0, -45.0, 0.0]);
        assert_eq!(extra[0].active_start, 13);
    }

    #[test]
    fn script_names_are_pascal_case() {
        assert_eq!(pascal("regular"), "Regular");
        assert_eq!(pascal("special_n_bound"), "SpecialNBound");
    }

    /// Every object of every fighter loads, places and poses without panicking. Needs
    /// VISIONARY_DATA_ROOT (an extracted game); skipped otherwise.
    #[test]
    fn every_fighter_object_loads_and_poses_without_panicking() {
        let Some(root) = std::env::var_os("VISIONARY_DATA_ROOT") else {
            return;
        };
        let labels = crate::param_labels::parse_csv(
            &std::fs::read_to_string(
                crate::scratch_dirs::app_storage_root().join("ParamLabels.csv"),
            )
            .unwrap_or_default(),
        );
        let mut failures = Vec::new();
        let mut loaded = 0;
        let fighters = Path::new(&root).join("fighter");
        for fighter in std::fs::read_dir(&fighters).unwrap().flatten() {
            let fighter_dir = fighter.path();
            let Ok(parts) = std::fs::read_dir(fighter_dir.join("model")) else {
                continue;
            };
            for part in parts.flatten() {
                let name = part.file_name().to_string_lossy().into_owned();
                if name == "body" {
                    continue;
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let Some(kind) = load_kind(
                        &fighter_dir,
                        &fighter_dir.join("param/vl.prc"),
                        0,
                        &name,
                        &labels,
                        &|_| None,
                    ) else {
                        return false;
                    };
                    let kind = Arc::new(kind);
                    let bones = |_: f32| HashMap::from([("top".to_owned(), glam::Mat4::IDENTITY)]);
                    for frame in [0.0, 5.5, 30.0, 200.0] {
                        let spawn = Spawn {
                            kind: kind.clone(),
                            spawned: 0,
                            removed: None,
                            shot: None,
                        };
                        for object in place(&[spawn], frame, &bones) {
                            let _ = object_bones(&object);
                            let _ = placement_layer(&object);
                        }
                    }
                    true
                }));
                match result {
                    Ok(true) => loaded += 1,
                    Ok(false) => {}
                    Err(_) => {
                        failures.push(format!("{}/{name}", fighter.file_name().to_string_lossy()))
                    }
                }
            }
        }
        println!("{loaded} objects loaded");
        assert!(failures.is_empty(), "panicked: {failures:?}");
    }

    /// Reads real objects from the extracted game files.
    #[test]
    fn real_objects_load_with_flight_and_hitboxes() {
        let (Some(root), Some(dump)) = (
            std::env::var_os("VISIONARY_DATA_ROOT"),
            std::env::var_os("VISIONARY_SCRIPT_DUMP"),
        ) else {
            return;
        };
        let labels = crate::param_labels::parse_csv(
            &std::fs::read_to_string(
                crate::scratch_dirs::app_storage_root().join("ParamLabels.csv"),
            )
            .unwrap_or_default(),
        );
        for (fighter, name) in [
            ("mario", "fireball"),
            ("samus", "cshot"),
            ("link", "bowarrow"),
            ("fox", "blaster_bullet"),
        ] {
            let fighter_dir = Path::new(&root).join("fighter").join(fighter);
            let dump = Path::new(&dump).join(format!("lua2cpp_{fighter}/{fighter}_{name}"));
            let kind = load_kind(
                &fighter_dir,
                &fighter_dir.join("param/vl.prc"),
                0,
                name,
                &labels,
                &|script| std::fs::read_to_string(dump.join(format!("{script}.txt"))).ok(),
            )
            .unwrap_or_else(|| panic!("{fighter} {name} did not load"));
            println!(
                "{fighter} {name}: {:?}, {} hitboxes, effects {:?}",
                kind.flight,
                kind.hitboxes.len(),
                kind.effects
            );
            if name == "fireball" {
                assert_eq!(kind.effects.len(), 1, "one of the facing pair");
                assert_eq!(kind.effects[0].bone, "rot");
            }
            assert!(
                kind.flight.speed > 0.0 || name == "blaster_bullet",
                "{fighter} {name}"
            );
            assert!(!kind.hitboxes.is_empty(), "{fighter} {name}");
            assert_eq!(
                kind.model_dir.is_some(),
                kind.skel.is_some(),
                "{fighter} {name}"
            );
            assert!(
                kind.anim.is_none() || kind.skel.is_some(),
                "{fighter} {name}"
            );
        }
    }
}

#[cfg(test)]
mod thrown_object_tests {
    use super::*;

    /// Link's side special, as the game script has it: the boomerang is created on frame 5
    /// and thrown on frame 27.
    const LINK_SPECIAL_S1: &str = r#"unsafe extern "C" fn game_specials1(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        ArticleModule::generate_article(agent.module_accessor, *FIGHTER_LINK_GENERATE_ARTICLE_BOOMERANG, false, -1);
    }
    frame(agent.lua_state_agent, 27.0);
    if macros::is_excute(agent) {
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_LINK_GENERATE_ARTICLE_BOOMERANG, *ARTICLE_OPE_TARGET_ALL, false);
    }
}"#;

    #[test]
    fn the_script_says_when_the_boomerang_is_thrown() {
        let script = crate::acmd::parse_acmd_script(LINK_SPECIAL_S1);
        let shots = script.to_article_shots();
        assert_eq!(
            shots,
            [(27, "FIGHTER_LINK_GENERATE_ARTICLE_BOOMERANG".to_owned())]
        );
    }

    #[test]
    fn a_held_object_stays_in_the_hand_until_it_is_thrown() {
        let kind = Arc::new(ObjectKind {
            flight: Flight {
                life: 60.0,
                speed: 2.0,
                ..Flight::default()
            },
            ..ObjectKind::default()
        });
        let hand = glam::Vec3::new(1.0, 9.0, 3.0);
        let bones = |_: f32| {
            HashMap::from([
                ("top".to_owned(), glam::Mat4::IDENTITY),
                ("haver".to_owned(), glam::Mat4::from_translation(hand)),
            ])
        };
        let spawn = Spawn {
            kind,
            spawned: 5,
            removed: None,
            shot: Some(27),
        };
        let at = |frame: f32| {
            place(std::slice::from_ref(&spawn), frame, &bones)[0]
                .placement
                .col(3)
                .truncate()
        };
        // Held: on the hand, however long it has been out.
        assert_eq!(at(20.0), hand);
        assert_eq!(at(26.0), hand);
        // Thrown: flying forward from the hand, 2 a frame from frame 27.
        assert_eq!(at(27.0), hand);
        assert_eq!(at(37.0), hand + glam::Vec3::Z * 20.0);
    }
}

#[cfg(test)]
mod returning_flight_tests {
    use super::*;

    #[test]
    fn a_boomerang_slows_as_measured_then_comes_back_home() {
        // Link's boomerang params, thrown with the flick speed the game measured (3.0 a frame).
        // In game successive four-frame spans shrink by 0.76 (11.93, 11.17, 10.43): the
        // 0.047 a frame slowdown. The game's spans run about 0.2 longer than these, as if it
        // held full speed for its first frame; that offset is not modelled.
        let flight = Flight {
            life: 129.0,
            speed: 3.0,
            returns: Some(Return {
                accel: 0.047,
                turn_speed: 0.3,
                return_speed: 3.0,
            }),
            ..Flight::default()
        };
        let at = |f: f32| flight.offset(f, 8.0).0;
        let span = |n: f32| at(4.0 * (n + 1.0)) - at(4.0 * n);
        assert!(((span(0.0) - span(1.0)) - 0.76).abs() < 0.02);
        assert!(((span(1.0) - span(2.0)) - 0.74).abs() < 0.02);
        // Out and back: the farthest point, then home before its life runs out.
        let farthest = (0..129).map(|f| at(f as f32)).fold(0.0, f32::max);
        assert!(farthest > 80.0 && farthest < 100.0, "{farthest}");
        assert_eq!(at(128.0), 0.0);
    }
}

#[cfg(test)]
mod object_effect_rotation_tests {
    #[test]
    fn a_projectile_effect_keeps_its_rotation_in_script_order() {
        let body = r#"unsafe extern "C" fn effect_fly(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("x_trail"), Hash40::new("top"), 0, 1, 2, -4, 80, 96, 1, true);
    }
}"#;
        let effects = super::object_effects_as_written(body);
        assert_eq!(effects[0].rotation, [-4.0, 80.0, 96.0]);
        assert_eq!(effects[0].offset, [0.0, 1.0, 2.0]);
    }
}
