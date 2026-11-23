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
    /// Speed gained each frame, up to `max_speed`.
    pub accel: f32,
    pub max_speed: Option<f32>,
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
            accel: 0.0,
            max_speed: None,
            returns: None,
        }
    }
}

/// Status names that represent impact, landing, explosion, death, or idle states rather than
/// initial flight. Their `set_speed(0.0)` / braking calls must not erase the projectile's launch
/// speed in the 3D preview.
fn is_terminal_status(status: &str) -> bool {
    let lower = status.to_ascii_lowercase();
    const TERMINAL: &[&str] = &[
        "ground", "explode", "burst", "hit", "vanish", "dead", "die", "death", "stop", "finish",
        "end", "bound", "landing", "cliff", "shield", "reflect", "wait", "break", "drown",
    ];
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| TERMINAL.contains(&tok))
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
        let (mut speed, mut step_up) = (self.speed, self.speed * radians.sin());
        let mut step_forward = speed * radians.cos();
        let whole = frames.max(0.0).floor() as usize;
        for _ in 0..whole {
            // Gravity first, then the move: measured in game, Mario's fireball falls one
            // gravity step further each frame than moving first would give (the gap grew by
            // exactly 0.1 a frame).
            step_up -= self.gravity;
            forward += step_forward;
            up += step_up;
            if self.accel > 0.0 {
                let before = speed;
                speed = (speed + self.accel).min(self.max_speed.unwrap_or(f32::MAX));
                step_forward = speed * radians.cos();
                step_up += (speed - before) * radians.sin();
            }
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

    /// Overlay resolved status settings on top of the params-derived flight.
    ///
    /// `Literal`, `Const` (with a known value) and `Param` settings replace the
    /// params-derived fields; `Computed` values leave them as they are. Per
    /// design ruling R3, only fields `Flight` simulates are mapped (`speed`,
    /// `angle`, `accel`, `gravity`, `max_speed`, `life`); `Brake`, `SpeedMul`
    /// and unmodelled kinetic calls stay on the Settings card without altering
    /// the flight path.
    pub fn with_settings(
        self,
        settings: &[crate::articles::settings::Setting],
        params: &crate::articles::params::ParamBlock,
    ) -> Self {
        self.with_settings_and_blocks(settings, params, &HashMap::new())
    }

    /// Like [`Flight::with_settings`], also resolving `SettingValue::Param` entries that read
    /// from another block (e.g. `param_common` or another article's block) when present in
    /// `other_blocks`.
    pub fn with_settings_and_blocks(
        mut self,
        settings: &[crate::articles::settings::Setting],
        params: &crate::articles::params::ParamBlock,
        other_blocks: &HashMap<String, crate::articles::params::ParamBlock>,
    ) -> Self {
        use crate::articles::settings::SettingField;

        let mut vx: Option<f32> = None;
        let mut vy: Option<f32> = None;

        for setting in settings {
            if is_terminal_status(&setting.status) {
                continue;
            }
            let Some(value) = resolve_setting_number(&setting.value, params, other_blocks) else {
                continue;
            };
            match setting.field {
                SettingField::SpeedX => {
                    // The 3D preview faces right (+1.0), so both signed and facing-multiplied
                    // speeds keep their value.
                    vx = Some(value);
                }
                SettingField::SpeedY => {
                    vy = Some(value);
                }
                SettingField::AccelX => {
                    if value > 0.0 {
                        self.accel = value;
                    }
                }
                SettingField::Gravity => {
                    self.gravity = value.abs();
                }
                SettingField::AccelY => {
                    if value < 0.0 {
                        self.gravity = value.abs();
                    }
                }
                SettingField::MaxSpeedX => {
                    if value.abs() > 0.0 {
                        self.max_speed = Some(value.abs());
                    }
                }
                SettingField::StableSpeedX => {
                    if self.max_speed.is_none() && value.abs() > 0.0 {
                        self.max_speed = Some(value.abs());
                    }
                }
                SettingField::Life => {
                    if value > 0.0 {
                        self.life = value;
                    }
                }
                SettingField::Scale
                | SettingField::StableSpeedY
                | SettingField::MaxSpeedY
                | SettingField::BrakeX
                | SettingField::BrakeY
                | SettingField::SpeedMul
                | SettingField::PowerMul
                | SettingField::OtherMovement => {}
            }
        }

        match (vx, vy) {
            (Some(x), Some(y)) => {
                self.speed = x.hypot(y);
                self.angle = y.atan2(x).to_degrees();
            }
            (Some(x), None) => {
                self.speed = x.abs();
                if x < 0.0 {
                    self.angle = 180.0;
                }
            }
            (None, Some(y)) => {
                let x = self.speed * self.angle.to_radians().cos();
                self.speed = x.hypot(y);
                self.angle = y.atan2(x).to_degrees();
            }
            (None, None) => {}
        }

        self
    }
}

fn resolve_setting_number(
    value: &crate::articles::settings::SettingValue,
    params: &crate::articles::params::ParamBlock,
    other_blocks: &HashMap<String, crate::articles::params::ParamBlock>,
) -> Option<f32> {
    use crate::articles::params::ParamValue;
    use crate::articles::settings::SettingValue;

    let num = match value {
        SettingValue::Literal(v) => Some(*v as f32),
        SettingValue::Const { value: Some(v), .. } => Some(*v as f32),
        SettingValue::Const { value: None, .. } => None,
        SettingValue::Param { block, field } => {
            let source = other_blocks.get(block).unwrap_or(params);
            let entry = source
                .fields
                .get(field)
                .or_else(|| source.fields.get(field.trim_end_matches('_')));
            match entry.map(|(v, _)| v) {
                Some(ParamValue::Float(v)) => Some(*v),
                Some(ParamValue::Int(v)) => Some(*v as f32),
                _ => None,
            }
        }
        SettingValue::Computed(_) => None,
    }?;
    num.is_finite().then_some(num)
}

/// Filter a multi-use carrier's status `settings` down to those belonging to `motion` (or its
/// ACMD handler function name, e.g. `motion = "carrot"` matching
/// `WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_CARROT`, or `motion = "fly"` with
/// `handler_fn = Some("game_fireball_fly")` matching `WEAPON_PACMAN_TRAMPOLINE_STATUS_KIND_FIREBALL`).
/// Returns an empty vec when no status specifically matches so callers can fall back to the
/// article's default flight.
pub fn settings_for_motion(
    settings: &[crate::articles::settings::Setting],
    motion: &str,
    handler_fn: Option<&str>,
) -> Vec<crate::articles::settings::Setting> {
    let mut tokens: Vec<String> = Vec::new();
    let add_tokens = |raw: &str, out: &mut Vec<String>| {
        let lower = raw.to_ascii_lowercase();
        let compact = lower.replace('_', "");
        if compact.len() >= 3 && !out.contains(&compact) {
            out.push(compact);
        }
        for part in lower.split('_') {
            if part.len() >= 4
                && !matches!(
                    part,
                    "game" | "effect" | "sound" | "expression" | "special" | "attack" | "main"
                )
                && !out.iter().any(|t| t == part)
            {
                out.push(part.to_owned());
            }
        }
    };
    add_tokens(motion, &mut tokens);
    if let Some(handler) = handler_fn {
        add_tokens(handler, &mut tokens);
    }
    if tokens.is_empty() {
        return Vec::new();
    }
    settings
        .iter()
        .filter(|s| {
            let status_lower = s.status.to_ascii_lowercase();
            let status_compact = status_lower.replace('_', "");
            // Match against the status suffix after `_STATUS_KIND_` when present.
            let suffix = status_lower
                .split_once("_status_kind_")
                .map(|(_, tail)| tail.replace('_', ""))
                .unwrap_or(status_compact);
            tokens
                .iter()
                .any(|tok| suffix == *tok || suffix.contains(tok) || tok.contains(&suffix))
        })
        .cloned()
        .collect()
}

/// Uniform scale resolved from an article's status settings (`ModelModule::set_scale` /
/// `PostureModule::set_scale`). `Computed` and non-positive values fall back to `1.0`.
pub fn scale_from_settings(
    settings: &[crate::articles::settings::Setting],
    params: &crate::articles::params::ParamBlock,
) -> f32 {
    scale_from_settings_and_blocks(settings, params, &HashMap::new())
}

/// Like [`scale_from_settings`], also resolving `SettingValue::Param` entries against
/// `other_blocks`.
pub fn scale_from_settings_and_blocks(
    settings: &[crate::articles::settings::Setting],
    params: &crate::articles::params::ParamBlock,
    other_blocks: &HashMap<String, crate::articles::params::ParamBlock>,
) -> f32 {
    let mut scale = 1.0f32;
    for setting in settings {
        if setting.field == crate::articles::settings::SettingField::Scale {
            if let Some(v) = resolve_setting_number(&setting.value, params, other_blocks) {
                if v > 0.0 {
                    scale = v;
                }
            }
        }
    }
    scale
}

/// Everything needed to show one kind of object.
#[derive(Clone)]
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
    /// Uniform visual scale applied on top of the object's world placement (default `1.0`).
    pub scale: f32,
    /// Every motion of the object that has an animation, by hash40 of the motion kind. A move
    /// picks one with `ArticleModule::change_motion`: Samus's grab beam plays `catch` alongside
    /// her grab.
    pub motions: Vec<(u64, Arc<AnimData>)>,
    /// The animation is one of those motions, made in step with the fighter's and in the
    /// fighter's space, so the object rides the fighter's `top` rather than a hand.
    pub synced: bool,
}

impl Default for ObjectKind {
    fn default() -> Self {
        Self {
            name: String::new(),
            model_dir: None,
            skel: None,
            anim: None,
            hitboxes: Vec::new(),
            effects: Vec::new(),
            flight: Flight::default(),
            scale: 1.0,
            motions: Vec::new(),
            synced: false,
        }
    }
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
pub fn object_effects(body: &str) -> Vec<ObjectEffect> {
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
            .filter(|call| crate::data::facing_arm_of(call.guard.as_deref()) != Some(true))
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
#[derive(Clone)]
pub struct PlacedObject {
    pub kind: Arc<ObjectKind>,
    /// The frame it was spawned on and the one it is removed on, to place it at earlier ages.
    pub spawned: u32,
    pub removed: Option<u32>,
    /// The frame the move throws it, when it is held first (Link's boomerang).
    pub shot: Option<u32>,
    pub hand: Option<String>,
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
    let Ok(param) = prc::open(param_path) else {
        return Flight::default();
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
            _ => return Flight::default(),
        },
        _ => return Flight::default(),
    };
    let listed: Vec<(&str, f32)> = fields
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
    flight_from_listed(&listed)
}

/// Reads one object's flight from a merged [`crate::articles::params::ParamBlock`]
/// (dump + reference mods + moveset `vl.prc`/`vl.prcxml` + `param_config` overrides).
pub fn flight_from_param_block(params: &crate::articles::params::ParamBlock) -> Flight {
    use crate::articles::params::ParamValue;
    if !params.present {
        return Flight::default();
    }
    let listed: Vec<(&str, f32)> = params
        .fields
        .iter()
        .filter_map(|(label, (value, _))| {
            let number = match value {
                ParamValue::Float(v) => *v,
                ParamValue::Int(v) => *v as f32,
                ParamValue::Other(_) => return None,
            };
            Some((label.trim_end_matches('_'), number))
        })
        .collect();
    flight_from_listed(&listed)
}

fn flight_from_listed(listed: &[(&str, f32)]) -> Flight {
    let mut flight = Flight::default();
    let mut values: HashMap<String, f32> = listed
        .iter()
        .map(|(label, number)| ((*label).to_owned(), *number))
        .collect();
    if !SPEED_NAMES.iter().any(|name| values.contains_key(*name)) {
        values.extend(first_variant(listed));
    }
    let first = |names: &[&str]| names.iter().find_map(|name| values.get(*name).copied());
    if let Some(life) = first(&["life", "life_frame", "max_life"]).filter(|life| *life > 0.0) {
        flight.life = life;
    }
    if let Some(speed) = first(SPEED_NAMES) {
        flight.speed = speed.abs();
    }
    // Some speed up as they fly: Sephiroth's flare starts at 0.01 and gains 0.07 a frame up
    // to 1.5.
    if let Some(accel) = first(&["accel_x", "acceleration"]).filter(|accel| *accel > 0.0) {
        flight.accel = accel;
        flight.max_speed = first(&["max_speed_x", "full_speed", "limit_speed_x"]);
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

/// The names an object's starting speed goes by, in the order they are trusted.
const SPEED_NAMES: &[&str] = &[
    "speed",
    "speed_x",
    "init_speed",
    "init_speed_x",
    "initial_speed_x",
    "start_speed",
    "start_speed_x",
    "shot_speed_x",
    "speed_min",
    "min_speed",
    "speed_max",
    "max_speed",
];

/// Names that are part of how an object flies, without any variant tag.
const FLIGHT_NAMES: &[&str] = &[
    "life",
    "life_frame",
    "angle",
    "gravity_accel",
    "accel_x",
    "acceleration",
    "max_speed_x",
    "full_speed",
    "limit_speed_x",
];

/// Words that mark a value as something other than the plain shot: a bounce, a spin, a
/// reflected or pulled-back speed.
const NOT_THE_SHOT: &[&str] = &[
    "rot", "angle", "reflect", "rewind", "back", "hop", "bound", "hold", "pull", "reach", "extend",
    "damage", "dash", "final", "vacuum", "inhale", "follow", "fail", "sp", "y", "air", "fall",
    "control",
];

/// Objects that come in sizes or stages keep one set of values per variant, tagged on the name:
/// Sephiroth's flare has `speed_x_s`, `speed_x_m`, `speed_x_l`, Ryu's hadoken `speed_w`, Hero's
/// spells `speed1`, Robin's thunder `special_n_thunder_speed`. This gives the first variant
/// that has a speed, under the plain names.
fn first_variant(listed: &[(&str, f32)]) -> HashMap<String, f32> {
    let mut variants: Vec<(String, HashMap<String, f32>)> = Vec::new();
    for (label, number) in listed {
        let Some((base, tag)) = split_variant(label) else {
            continue;
        };
        match variants.iter_mut().find(|(other, _)| *other == tag) {
            Some((_, values)) => {
                values.insert(base, *number);
            }
            None => variants.push((tag, HashMap::from([(base, *number)]))),
        }
    }
    variants
        .into_iter()
        .map(|(_, values)| values)
        .find(|values| {
            SPEED_NAMES
                .iter()
                .any(|name| values.get(*name).is_some_and(|speed| *speed != 0.0))
        })
        .unwrap_or_default()
}

/// A tagged name as (plain name, tag): `speed_x_s` is ("speed_x", "s"), `speed1` is
/// ("speed", "1"), `h_life` is ("life", "h").
fn split_variant(label: &str) -> Option<(String, String)> {
    let known = |name: &str| SPEED_NAMES.contains(&name) || FLIGHT_NAMES.contains(&name);
    let plain = |tag: &str| tag.split('_').all(|word| !NOT_THE_SHOT.contains(&word));
    let digits = label.trim_end_matches(|c: char| c.is_ascii_digit());
    if digits.len() < label.len() && known(digits) {
        return Some((digits.to_owned(), label[digits.len()..].to_owned()));
    }
    let words: Vec<&str> = label.split('_').collect();
    // The longest plain name wins: `speed_x_s` is `speed_x` tagged `s`, not `speed` tagged `x_s`.
    let split = |cut: usize| (words[..cut].join("_"), words[cut..].join("_"));
    (1..words.len())
        .rev()
        .map(split)
        .find(|(head, tail)| known(head) && plain(tail))
        .or_else(|| {
            (1..words.len())
                .map(split)
                .find(|(head, tail)| known(tail) && plain(head))
                .map(|(head, tail)| (tail, head))
        })
}

/// Read an article script from the linked moveset `SourceIndex` first, falling back to `dump`
/// (e.g. `acmd::fetch_object_script`) only when the source does not define it.
pub fn script_from_index_or_dump(
    index: Option<&crate::acmd_src::SourceIndex>,
    agent: &str,
    script_name: &str,
    dump: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(index) = index {
        let lower = script_name.to_ascii_lowercase();
        let snake = pascal_to_snake(script_name);
        for candidate in [lower.as_str(), snake.as_str()] {
            if candidate.is_empty() {
                continue;
            }
            if let Some(project) = index.script_source(agent, candidate) {
                if !project.body.trim().is_empty() {
                    return Some(project.body);
                }
            }
        }
    }
    dump(script_name)
}

fn pascal_to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 && !out.ends_with('_') {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// Optional moveset-resolved overrides for [`load_kind_resolved`].
#[derive(Default)]
pub struct ResolvedArticleInputs<'a> {
    /// Additional `fighter/<owner>` directories searched before `fighter_dir` (for example the
    /// moveset's `romfs/fighter/<owner>` so custom/cloned models and motions load).
    pub extra_fighter_dirs: &'a [PathBuf],
    /// Extra candidate script names (such as a clone's `source_scripts`) probed if
    /// `motion_list.bin` is absent or yields nothing.
    pub extra_scripts: &'a [String],
    /// Merged `ParamBlock` for this article (the new owner's `param_<new_name>` for clones).
    pub params: Option<&'a crate::articles::params::ParamBlock>,
    /// Any other param blocks referenced by `settings` (`param_common`, another article's block).
    pub other_blocks: Option<&'a HashMap<String, crate::articles::params::ParamBlock>>,
    /// Resolved status settings from `crate::articles::settings_for`.
    pub settings: &'a [crate::articles::settings::Setting],
}

/// Loads an object's model, skeleton, animation, hitboxes and flight. `script` gives the body of
/// one of the object's game scripts by name ("Regular"), or nothing.
#[cfg_attr(not(test), allow(dead_code))]
pub fn load_kind(
    fighter_dir: &Path,
    param_path: &Path,
    slot: u8,
    name: &str,
    labels: &HashMap<u64, String>,
    script: &dyn Fn(&str) -> Option<String>,
) -> Option<ObjectKind> {
    load_kind_resolved(
        fighter_dir,
        param_path,
        slot,
        name,
        labels,
        &ResolvedArticleInputs::default(),
        script,
    )
}

/// Like [`load_kind`], overlaying a moveset's resolved `ParamBlock`, status `Setting`s, extra
/// script candidates, and extra `fighter/<owner>` roots (such as `romfs/`).
pub fn load_kind_resolved(
    fighter_dir: &Path,
    param_path: &Path,
    slot: u8,
    name: &str,
    labels: &HashMap<u64, String>,
    inputs: &ResolvedArticleInputs<'_>,
    script: &dyn Fn(&str) -> Option<String>,
) -> Option<ObjectKind> {
    let search_dirs: Vec<&Path> = inputs
        .extra_fighter_dirs
        .iter()
        .map(PathBuf::as_path)
        .chain(std::iter::once(fighter_dir))
        .collect();
    let model_name = model_folder(name);
    let skel_path = search_dirs
        .iter()
        .find_map(|dir| crate::data::find_part_skel(&dir.join("model").join(model_name), slot));
    let model_dir = skel_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(Path::to_path_buf);
    let skel = skel_path.and_then(|path| SkelData::from_file(path).ok());
    let motion_dir = search_dirs
        .iter()
        .find_map(|dir| costume_dir(&dir.join("motion").join(name), slot));
    // The object plays the first motion in its list that has a script, "regular" first.
    let mut motions: Vec<(String, Option<PathBuf>)> = Vec::new();
    let mut by_kind: Vec<(u64, PathBuf)> = Vec::new();
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
                let anim = motion
                    .animations
                    .iter()
                    .find_map(|animation| files.get(&animation.name.0).cloned());
                if let Some(path) = &anim {
                    by_kind.push((kind.0, path.clone()));
                }
                let Some(label) = labels.get(&kind.0) else {
                    continue;
                };
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
    // Always include "regular" and any caller-supplied source script names as fallback candidates
    // so clones or articles without a local motion_list.bin still load their source hitboxes.
    for fallback in
        std::iter::once("regular").chain(inputs.extra_scripts.iter().map(String::as_str))
    {
        if !fallback.is_empty() && !motions.iter().any(|(label, _)| label == fallback) {
            motions.push((fallback.to_owned(), None));
        }
    }
    motions.sort_by_key(|(label, anim)| (label != "regular", anim.is_none()));
    let mut chosen = motions
        .iter()
        .find(|(_, anim)| anim.is_some())
        .or_else(|| motions.first())
        .cloned();
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
            if anim.is_some() || chosen.as_ref().and_then(|(_, p)| p.as_ref()).is_none() {
                chosen = Some((label.clone(), anim.clone()));
            }
            if !hitboxes.is_empty() {
                break;
            }
        }
    }
    let anim = chosen
        .and_then(|(_, path)| path)
        .and_then(|path| AnimData::from_file(path).ok())
        .map(Arc::new);
    // Kinds often share one file; each is read once.
    let mut read: HashMap<PathBuf, Arc<AnimData>> = HashMap::new();
    let motions = by_kind
        .into_iter()
        .filter_map(|(kind, path)| {
            let anim = match read.get(&path) {
                Some(anim) => anim.clone(),
                None => {
                    let anim = Arc::new(AnimData::from_file(&path).ok()?);
                    read.insert(path, anim.clone());
                    anim
                }
            };
            Some((kind, anim))
        })
        .collect();
    let base_flight = match inputs.params {
        Some(params) if params.present => flight_from_param_block(params),
        _ => flight_from_params(param_path, name, labels),
    };
    let empty_params = crate::articles::params::ParamBlock::default();
    let params_ref = inputs.params.unwrap_or(&empty_params);
    let flight = match inputs.other_blocks {
        Some(others) if !others.is_empty() => {
            base_flight.with_settings_and_blocks(inputs.settings, params_ref, others)
        }
        _ => base_flight.with_settings(inputs.settings, params_ref),
    };
    let scale = match inputs.other_blocks {
        Some(others) if !others.is_empty() => {
            scale_from_settings_and_blocks(inputs.settings, params_ref, others)
        }
        _ => scale_from_settings(inputs.settings, params_ref),
    };
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
        scale,
        motions,
        synced: false,
    })
}

/// The fighter's objects by the motions they have, as hash40 of the motion kind. Objects that
/// the fighter's status code creates rather than its scripts are found this way: Diddy Kong's
/// rocket barrels have a `final_start` motion for his final smash.
pub fn objects_by_motion(fighter_dir: &Path, slot: u8) -> HashMap<u64, Vec<String>> {
    let mut objects: HashMap<u64, Vec<String>> = HashMap::new();
    let parts = std::fs::read_dir(fighter_dir.join("motion"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path());
    for part in parts {
        let Some(name) = part.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        if name == "body" {
            continue;
        }
        let Some(dir) = costume_dir(&part, slot) else {
            continue;
        };
        let Ok(list) = motion_lib::open(dir.join("motion_list.bin")) else {
            continue;
        };
        for (kind, motion) in list.list.iter() {
            if !motion.animations.is_empty() {
                objects.entry(kind.0).or_default().push(name.clone());
            }
        }
    }
    objects
}

/// Whether this object is the fighter itself in another form, shown instead of its body:
/// Inkling's squid during her Super Jump. The game hides the body in status code, which no
/// script or file records, so these are known by name.
pub fn replaces_fighter(kind: &ObjectKind) -> bool {
    kind.synced && kind.name == "squid"
}

/// The fighter's objects by the animation files they have, by file name. An object animated
/// in step with one of the fighter's moves often has a file of the same name as the move's:
/// Inkling's roller has `d01specialsdash.nuanmb` for her side special dash, though its motion
/// is called `dash`.
pub fn objects_by_animation(
    fighter_dir: &Path,
    slot: u8,
) -> HashMap<String, Vec<(String, PathBuf)>> {
    let mut objects: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();
    let parts = std::fs::read_dir(fighter_dir.join("motion"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path());
    for part in parts {
        let Some(name) = part.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        if name == "body" {
            continue;
        }
        let Some(dir) = costume_dir(&part, slot) else {
            continue;
        };
        for file in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = file.path();
            if path.extension().is_some_and(|e| e == "nuanmb") {
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    objects
                        .entry(file_name.to_owned())
                        .or_default()
                        .push((name.clone(), path.clone()));
                }
            }
        }
    }
    objects
}

/// The model folder of an object whose model is kept under another name: Olimar's Pikmin
/// share one set of motions (`motion/pikmin`) but have a model per colour (`model/r` for the
/// red one, the first in his line).
fn model_folder(name: &str) -> &str {
    match name {
        "pikmin" => "r",
        _ => name,
    }
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
/// `hand` is the hand the move plays an effect on as it creates the object, when it does.
fn spawn_point(bones: &HashMap<String, glam::Mat4>, flies: bool, hand: Option<&str>) -> glam::Vec3 {
    let top = bones
        .get("top")
        .map_or(glam::Vec3::ZERO, |matrix| matrix.col(3).truncate());
    if flies {
        // Mario's fireball leaves the forward hand, where the move's shoot effect plays (havel);
        // Sephiroth's flare the hand his flame is in (haver).
        for bone in hand.into_iter().chain(["havel", "haver", "handl", "handr"]) {
            if let Some(matrix) = bones.get(bone) {
                return matrix.col(3).truncate();
            }
        }
        return top + glam::Vec3::Y * 8.0;
    }
    top
}

/// One object a move creates.
#[derive(Clone, Default)]
pub struct Spawn {
    pub kind: Arc<ObjectKind>,
    /// The frame it is created on.
    pub spawned: u32,
    /// The frame the move removes it on.
    pub removed: Option<u32>,
    /// The frame the move throws it (`ArticleModule::shoot`), if it is held until then:
    /// Link's boomerang is created in his hand on frame 5 and leaves it on frame 27.
    pub shot: Option<u32>,
    /// Where the move shows or hides it (`set_visibility_whole`): (frame, shown), in order.
    /// Samus's grab beam is created hidden on frame 1 and shown on frame 8.
    pub shown: Vec<(u32, bool)>,
    /// The hand it leaves from, when the move plays an effect on one as it creates it.
    pub hand: Option<String>,
}

/// The hands an object can leave from.
pub const HANDS: [&str; 4] = ["havel", "haver", "handl", "handr"];

/// Planned lifecycle for one spawned copy of an article, built from typed
/// [`crate::data::ArticleCall`]s while honoring `ArticleTarget::All`, `First`, and `Last`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnPlan {
    pub name: String,
    pub spawned: u32,
    pub removed: Option<u32>,
    pub shot: Option<u32>,
    pub shown: Vec<(u32, bool)>,
    pub motion: Option<String>,
}

fn target_live_indices(
    plans: &[SpawnPlan],
    name: &str,
    frame: u32,
    target: crate::data::ArticleTarget,
) -> Vec<usize> {
    use crate::data::ArticleTarget;
    let live: Vec<usize> = plans
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            p.name == name && p.spawned <= frame && p.removed.is_none_or(|r| frame < r)
        })
        .map(|(i, _)| i)
        .collect();
    match target {
        ArticleTarget::All | ArticleTarget::Unspecified => live,
        ArticleTarget::First => live.first().copied().into_iter().collect(),
        ArticleTarget::Last | ArticleTarget::Term => live.last().copied().into_iter().collect(),
        ArticleTarget::None => Vec::new(),
    }
}

fn parse_bool_arg(args: &[String], index: usize) -> Option<bool> {
    match args.get(index).map(|s| s.trim()) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

fn parse_quoted_motion(arg: &str) -> Option<String> {
    let (_, rest) = arg.split_once('"')?;
    let (motion, _) = rest.split_once('"')?;
    (!motion.is_empty()).then(|| motion.to_owned())
}

/// Turn a move's `(frame, ArticleCall)` stream into per-copy spawn plans, applying
/// `Remove`, `Shoot`, `SetVisibility*`, and `ChangeMotion` to `All`, `First`, or `Last`
/// live copies of each article.
pub fn plan_spawns_from_calls(
    calls: &[(u32, &crate::data::ArticleCall)],
    resolve_name: &dyn Fn(&crate::data::ArticleCall) -> Option<String>,
) -> Vec<SpawnPlan> {
    use crate::data::ArticleOp;

    let mut ordered: Vec<(usize, u32, &crate::data::ArticleCall)> = calls
        .iter()
        .enumerate()
        .map(|(i, (frame, call))| (i, *frame, *call))
        .collect();
    ordered.sort_by_key(|(i, frame, _)| (*frame, *i));

    let mut plans: Vec<SpawnPlan> = Vec::new();
    for (_, frame, call) in ordered {
        let Some(name) = resolve_name(call) else {
            continue;
        };
        match call.op {
            ArticleOp::Generate | ArticleOp::GenerateEnable => {
                plans.push(SpawnPlan {
                    name,
                    spawned: frame,
                    removed: None,
                    shot: None,
                    shown: Vec::new(),
                    motion: None,
                });
            }
            ArticleOp::Remove | ArticleOp::RemoveExist => {
                for idx in target_live_indices(&plans, &name, frame, call.target) {
                    plans[idx].removed = Some(frame);
                }
            }
            ArticleOp::Shoot | ArticleOp::ShootExist => {
                for idx in target_live_indices(&plans, &name, frame, call.target) {
                    if plans[idx].shot.is_none() {
                        plans[idx].shot = Some(frame);
                    }
                }
            }
            ArticleOp::SetVisibilityWhole => {
                if let Some(visible) = parse_bool_arg(&call.args, 0) {
                    for idx in target_live_indices(&plans, &name, frame, call.target) {
                        plans[idx].shown.push((frame, visible));
                    }
                }
            }
            ArticleOp::SetVisibility => {
                if let Some(visible) = parse_bool_arg(&call.args, 1) {
                    for idx in target_live_indices(&plans, &name, frame, call.target) {
                        plans[idx].shown.push((frame, visible));
                    }
                }
            }
            ArticleOp::ChangeMotion => {
                if let Some(motion) = call.args.first().and_then(|arg| parse_quoted_motion(arg)) {
                    for idx in target_live_indices(&plans, &name, frame, call.target) {
                        if plans[idx].motion.is_none() {
                            plans[idx].motion = Some(motion.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    plans
}

/// Inspect a fighter move script for project-defined article spawn wrappers such as
/// `generate_sceptile_article(boma, ArticleKind::LeafStorm)` or
/// `generate_stacked_article(boma, ArticleKind::Pellet)` so the 3D preview can spawn their
/// catalog articles even when the moveset wraps `ArticleModule::generate_article` in a helper.
pub fn plan_wrapper_spawns_from_script(
    script: &crate::data::AcmdScript,
    catalog: &crate::articles::ArticleCatalog,
    index: Option<&crate::acmd_src::SourceIndex>,
) -> Vec<SpawnPlan> {
    let mut plans = Vec::new();
    collect_wrapper_spawns(&script.stmts, 0.0, catalog, index, &mut plans);
    plans
}

fn collect_wrapper_spawns(
    stmts: &[crate::data::AcmdStmt],
    start_frame: f32,
    catalog: &crate::articles::ArticleCatalog,
    index: Option<&crate::acmd_src::SourceIndex>,
    out: &mut Vec<SpawnPlan>,
) -> f32 {
    use crate::data::{AcmdStmt, ExcuteStmt};
    let mut frame = start_frame;
    for stmt in stmts {
        match stmt {
            AcmdStmt::Frame(f) => frame = *f,
            AcmdStmt::Wait(w) => frame += *w,
            AcmdStmt::Raw(line) => {
                if let Some(plan) =
                    wrapper_spawn_from_line(line, frame.max(1.0).round() as u32, catalog, index)
                {
                    out.push(plan);
                }
            }
            AcmdStmt::Excute(inner) => {
                let at = frame.max(1.0).round() as u32;
                for s in inner {
                    if let ExcuteStmt::Raw(line) = s {
                        if let Some(plan) = wrapper_spawn_from_line(line, at, catalog, index) {
                            out.push(plan);
                        }
                    }
                }
            }
            AcmdStmt::Bare(inner) => {
                if let ExcuteStmt::Raw(line) = inner.as_ref() {
                    if let Some(plan) =
                        wrapper_spawn_from_line(line, frame.max(1.0).round() as u32, catalog, index)
                    {
                        out.push(plan);
                    }
                }
            }
            AcmdStmt::RawBlock { header, body, .. } => {
                if let Some(plan) =
                    wrapper_spawn_from_line(header, frame.max(1.0).round() as u32, catalog, index)
                {
                    out.push(plan);
                }
                frame = collect_wrapper_spawns(body, frame, catalog, index, out);
            }
            AcmdStmt::Loop { count, body, .. } => {
                for _ in 0..(*count).min(16) {
                    frame = collect_wrapper_spawns(body, frame, catalog, index, out);
                }
            }
            _ => {}
        }
    }
    frame
}

fn wrapper_spawn_from_line(
    line: &str,
    frame: u32,
    catalog: &crate::articles::ArticleCatalog,
    index: Option<&crate::acmd_src::SourceIndex>,
) -> Option<SpawnPlan> {
    let trimmed = line.trim();
    if trimmed.starts_with("//")
        || trimmed.starts_with("println!")
        || trimmed.starts_with("eprintln!")
        || trimmed.contains("ArticleModule::")
    {
        return None;
    }
    // Look for a helper call whose name suggests generating/spawning an article, e.g.
    // `generate_sceptile_article(..., ArticleKind::LeafStorm)` or
    // `generate_stacked_article(..., ArticleKind::Pellet)` or `generate_autobomb_article(...)`.
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.contains("generate_") || lower.contains("spawn_")) {
        return None;
    }
    // 1. Prefer an explicit `ArticleKind::<Variant>` argument.
    let mut hint: Option<String> = None;
    if let Some(pos) = trimmed.find("ArticleKind::") {
        let rest = &trimmed[pos + "ArticleKind::".len()..];
        let variant: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !variant.is_empty() {
            hint = Some(variant.to_ascii_lowercase());
        }
    }
    // 2. Otherwise inspect the helper function name itself (`generate_autobomb_article` → `autobomb`).
    if hint.is_none() {
        for prefix in ["generate_", "spawn_"] {
            if let Some(pos) = lower.find(prefix) {
                let ident: String = lower[pos + prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                let stem = ident
                    .trim_end_matches("_article")
                    .trim_start_matches("shot_")
                    .trim_start_matches("stacked_");
                if !stem.is_empty() && stem != "article" {
                    hint = Some(stem.replace('_', ""));
                    break;
                }
            }
        }
    }
    let hint = hint?;
    let hint_compact = hint.replace('_', "");
    // Direct match against a catalog article's own name (e.g. Sceptile's `leafstorm`, `bulletseed`).
    if let Some(article) = catalog
        .articles
        .iter()
        .find(|a| a.name == hint || a.name.replace('_', "") == hint_compact)
    {
        return Some(SpawnPlan {
            name: article.name.clone(),
            spawned: frame,
            removed: None,
            shot: None,
            shown: Vec::new(),
            motion: None,
        });
    }
    // Carrier match: prefer `game_` scripts over `effect_`/`sound_` ones so hitbox scripts win.
    // Match either the registered script name (`game_pellet_shoot`) or the Rust handler function
    // name in `SourceIndex` (`game_fireball_fly` registered as `"game_fly"`).
    for want_game in [true, false] {
        for article in &catalog.articles {
            let fighter_scripts = index.and_then(|idx| idx.fighters.get(&article.agent));
            let matched_script = article.source_scripts.iter().find_map(|raw| {
                let is_game = raw.starts_with("game_");
                if is_game != want_game {
                    return None;
                }
                let stripped = raw
                    .strip_prefix("game_")
                    .or_else(|| raw.strip_prefix("effect_"))
                    .or_else(|| raw.strip_prefix("sound_"))
                    .or_else(|| raw.strip_prefix("expression_"))
                    .unwrap_or(raw);
                let compact = stripped.replace('_', "");
                let fn_matches = fighter_scripts
                    .and_then(|fs| fs.scripts.get(raw))
                    .is_some_and(|site| {
                        site.function
                            .to_ascii_lowercase()
                            .replace('_', "")
                            .contains(&hint_compact)
                    });
                (compact.contains(&hint_compact) || fn_matches).then(|| stripped.to_owned())
            });
            if let Some(motion) = matched_script {
                return Some(SpawnPlan {
                    name: article.name.clone(),
                    spawned: frame,
                    removed: None,
                    shot: None,
                    shown: Vec::new(),
                    motion: Some(motion),
                });
            }
        }
    }
    None
}

/// Inspect a fighter status function body (and any same-file `fastshift` / `main_shift` /
/// `main_shift_status` loop function it hands off to, when `file_text` is provided) for direct
/// `ArticleModule::generate_article` or project-defined wrapper spawn calls (such as Shy Guy's
/// `special_n` spawning `ArticleKind::Fireball` or Sceptile's `final_smash` spawning
/// `ArticleKind::Sceptilem`).
pub fn plan_spawns_from_status_body(
    body: &str,
    file_text: Option<&str>,
    catalog: &crate::articles::ArticleCatalog,
    index: Option<&crate::acmd_src::SourceIndex>,
) -> Vec<SpawnPlan> {
    let mut plans: Vec<SpawnPlan> = Vec::new();
    scan_status_body_spawns(body, catalog, index, &mut plans);
    if let Some(full) = file_text {
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut frontier: Vec<String> = same_file_callees(body, full);
        for _depth in 0..4 {
            let mut next = Vec::new();
            for callee in frontier {
                if !visited.insert(callee.clone()) {
                    continue;
                }
                if let Some(span) = crate::acmd_src::function_item(full, &callee) {
                    let sub = &full[span];
                    scan_status_body_spawns(sub, catalog, index, &mut plans);
                    next.extend(same_file_callees(sub, full));
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
    }
    plans
}

/// Same-file function names referenced by `body` either as a `fastshift` / `main_shift` target
/// or as a direct `helper_name(...)` call, filtered to functions actually defined in `file_text`.
fn same_file_callees(body: &str, file_text: &str) -> Vec<String> {
    let mut out = shifted_status_callees(body);
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_lowercase() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &body[start..i];
            // Skip method calls (`.foo(`), module-qualified calls (`::foo(`), and keywords.
            let prev = body[..start].trim_end().as_bytes().last().copied();
            let next = body[i..].trim_start().as_bytes().first().copied();
            if next == Some(b'(')
                && !matches!(prev, Some(b'.' | b':'))
                && !matches!(
                    ident,
                    "if" | "while"
                        | "for"
                        | "match"
                        | "return"
                        | "let"
                        | "unsafe"
                        | "fn"
                        | "Some"
                        | "None"
                )
                && !out.iter().any(|s| s == ident)
                && crate::acmd_src::function_item(file_text, ident).is_some()
            {
                out.push(ident.to_owned());
            }
        } else {
            i += 1;
        }
    }
    out
}

fn shifted_status_callees(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in [
        "main_shift_status(",
        "main_shift(",
        "fastshift(",
        "subshift(",
    ] {
        let mut rest = body;
        while let Some(pos) = rest.find(marker) {
            let after = &rest[pos + marker.len()..];
            let end = after.find(')').unwrap_or(after.len());
            let inner = &after[..end];
            // Handles both `main_shift_status(special_n_loop)` and
            // `fastshift(L2CValue::Ptr(special_n_loop as *const () as _))`.
            let candidate = inner
                .rsplit('(')
                .next()
                .unwrap_or(inner)
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
            if !candidate.is_empty()
                && candidate
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
                && !out.iter().any(|s| s == candidate)
            {
                out.push(candidate.to_owned());
            }
            rest = &after[end..];
        }
    }
    out
}

fn scan_status_body_spawns(
    body: &str,
    catalog: &crate::articles::ArticleCatalog,
    index: Option<&crate::acmd_src::SourceIndex>,
    plans: &mut Vec<SpawnPlan>,
) {
    let empty_locals = HashMap::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let candidate = if let Some(call) = crate::data::ArticleCall::parse(trimmed) {
            if matches!(
                call.op,
                crate::data::ArticleOp::Generate | crate::data::ArticleOp::GenerateEnable
            ) {
                catalog
                    .resolve_id(&call.article, &empty_locals)
                    .map(|a| a.name.clone())
                    .or_else(|| article_name(&call.article))
                    .map(|name| SpawnPlan {
                        name,
                        spawned: 1,
                        removed: None,
                        shot: None,
                        shown: Vec::new(),
                        motion: None,
                    })
            } else {
                None
            }
        } else {
            wrapper_spawn_from_line(trimmed, 1, catalog, index)
        };
        if let Some(plan) = candidate {
            if !plans
                .iter()
                .any(|p| p.name == plan.name && p.motion == plan.motion)
            {
                plans.push(plan);
            }
        }
    }
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
        shown,
        hand,
    } in spawns
    {
        let age = frame - *spawned as f32;
        if age < 0.0 || removed.is_some_and(|removed| frame >= removed as f32) {
            continue;
        }
        let hidden = shown
            .iter()
            .rev()
            .find(|(at, _)| *at as f32 <= frame)
            .is_some_and(|(_, shown)| !shown);
        if hidden {
            continue;
        }
        let scale = if kind.scale.is_finite() && kind.scale > 0.0 {
            kind.scale
        } else {
            1.0
        };
        let scale_mat = glam::Mat4::from_scale(glam::Vec3::splat(scale));
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
                glam::Mat4::from_rotation_translation(
                    rotation,
                    spawn_point(&bones, true, hand.as_deref()),
                )
            } else if kind.synced {
                let top = bones.get("top").copied().unwrap_or(glam::Mat4::IDENTITY);
                let (_, rotation, position) = top.to_scale_rotation_translation();
                glam::Mat4::from_rotation_translation(rotation, position)
            } else {
                ride_bone(&bones, &kind.name)
            } * scale_mat;
            placed.push(PlacedObject {
                kind: kind.clone(),
                spawned: *spawned,
                removed: *removed,
                shot: *shot,
                hand: hand.clone(),
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
        let start = spawn_point(&bones, kind.flight.speed > 0.0, hand.as_deref());
        let (ahead, up) = kind.flight.offset(flight_age, start.y - top_position.y);
        let position = start + forward * ahead + glam::Vec3::Y * up;
        placed.push(PlacedObject {
            kind: kind.clone(),
            spawned: *spawned,
            removed: *removed,
            shot: *shot,
            hand: hand.clone(),
            age,
            placement: glam::Mat4::from_rotation_translation(rotation, position) * scale_mat,
        });
    }
    placed
}

/// Whether a motion places the object in the fighter's own space, the way the motions made
/// with a fighter's moves do (Samus's grab beam starts at her arm cannon, 15 up), rather than
/// around its grip (Fox's blaster opening in his hand). Read from the animation: its top bones
/// sit well away from the origin at some point.
pub fn plays_in_fighter_space(skel: &SkelData, anim: &AnimData) -> bool {
    let top: Vec<&str> = skel
        .bones
        .iter()
        .enumerate()
        .filter(|(_, bone)| {
            bone.parent_index
                .is_none_or(|parent| skel.bones[parent].parent_index.is_none())
        })
        .map(|(_, bone)| bone.name.as_str())
        .collect();
    let last = anim.final_frame_index.max(0.0);
    [0.0, last * 0.5, last].iter().any(|&frame| {
        let bones =
            crate::renderer::compute_bone_world_matrices(skel, Some(anim), None, &[], frame);
        top.iter().any(|name| {
            bones
                .get(*name)
                .is_some_and(|matrix| matrix.col(3).truncate().length() > 3.0)
        })
    })
}

/// An object's bones in the world at its age.
pub fn object_bones(object: &PlacedObject) -> HashMap<String, glam::Mat4> {
    let Some(skel) = &object.kind.skel else {
        return HashMap::from([("top".to_owned(), object.placement)]);
    };
    crate::renderer::compute_bone_world_matrices(
        skel,
        object.kind.anim.as_deref(),
        None,
        &[],
        object.age,
    )
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
        None,
        &[],
        object.age,
    )
    .get(&root.name)
    .copied()
    .unwrap_or(root.transform);
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
                        scale,
                        rotation,
                        translation,
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

    /// The names in the game's own param files (Sephiroth's fire, Ryu's hadoken, Hero's
    /// spells, Robin's thunder): before, none of them read as a speed and the shots stood still
    /// at the fighter's feet.
    #[test]
    fn sized_and_staged_objects_read_their_first_variant() {
        let edge = [
            ("effect_check_frame_burst", 23.0),
            ("life_s", 74.0),
            ("speed_x_s", 0.01),
            ("accel_x_s", 0.07),
            ("max_speed_x_s", 1.5),
            ("life_m", 28.0),
            ("speed_x_m", 0.01),
        ];
        let values = first_variant(&edge);
        assert_eq!(values.get("life"), Some(&74.0));
        assert_eq!(values.get("speed_x"), Some(&0.01));
        assert_eq!(values.get("accel_x"), Some(&0.07));
        assert_eq!(values.get("max_speed_x"), Some(&1.5));
        let ryu = [
            ("speed_w", 1.1),
            ("speed_m", 1.4),
            ("life_w", 75.0),
            ("speed_w_sp", 2.0),
        ];
        assert_eq!(first_variant(&ryu).get("speed"), Some(&1.1));
        assert_eq!(first_variant(&ryu).get("life"), Some(&75.0));
        let hero = [("speed1", 1.6), ("life1", 40.0), ("pillar_life1", 9.0)];
        assert_eq!(first_variant(&hero).get("speed"), Some(&1.6));
        let robin = [
            ("special_n_thunder_speed", 2.0),
            ("special_n_thunder_life", 30.0),
        ];
        assert_eq!(first_variant(&robin).get("life"), Some(&30.0));
        // A bounce or a spin is not the shot.
        let spin = [
            ("rot_speed_x", 3.0),
            ("hop_speed_y", 2.0),
            ("speed_y_m", 1.0),
        ];
        assert!(first_variant(&spin).is_empty());
    }

    #[test]
    fn a_shot_leaves_the_hand_the_move_s_effect_is_on() {
        let kind = Arc::new(ObjectKind {
            flight: Flight {
                speed: 1.0,
                ..Flight::default()
            },
            ..ObjectKind::default()
        });
        let left = glam::Vec3::new(0.0, 10.0, 2.0);
        let right = glam::Vec3::new(0.0, 12.0, -2.0);
        let bones = |_: f32| {
            HashMap::from([
                ("top".to_owned(), glam::Mat4::IDENTITY),
                ("havel".to_owned(), glam::Mat4::from_translation(left)),
                ("haver".to_owned(), glam::Mat4::from_translation(right)),
            ])
        };
        let start = |hand: Option<&str>| {
            let spawn = Spawn {
                kind: kind.clone(),
                hand: hand.map(str::to_owned),
                ..Spawn::default()
            };
            place(&[spawn], 0.0, &bones)[0].placement.col(3).truncate()
        };
        assert_eq!(
            start(None),
            left,
            "the forward hand when the move says nothing"
        );
        assert_eq!(start(Some("haver")), right);
    }

    #[test]
    fn an_object_that_speeds_up_stops_at_its_top_speed() {
        let flight = Flight {
            speed: 0.0,
            accel: 0.5,
            max_speed: Some(1.0),
            life: 60.0,
            ..Flight::default()
        };
        // Speeds 0, 0.5, then 1 a frame from then on.
        assert_eq!(flight.offset(2.0, 0.0), (0.5, 0.0));
        assert_eq!(flight.offset(5.0, 0.0), (3.5, 0.0));
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
        assert!((-8.0..0.0).contains(&up), "{up}");
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
        let calls: Vec<_> = script
            .to_effect_calls()
            .into_iter()
            .filter(|call| call.effect_name == "mario_fb_shoot")
            .collect();
        assert_eq!(calls.len(), 2);
        let arms: Vec<_> = calls
            .iter()
            .map(|call| crate::data::facing_arm_of(call.guard.as_deref()))
            .collect();
        assert_eq!(arms, [Some(true), Some(false)]);
        assert_eq!(calls[1].bone_name, "havel");
        assert_eq!(calls[0].rotation, [0.0, 45.0, 0.0]);
        assert_eq!(calls[1].rotation, [0.0, -45.0, 0.0]);
        assert!(calls.iter().all(|call| call.active_start == 13));
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
                            shown: Vec::new(),
                            hand: None,
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
            ("edge", "fire"),
            ("ryu", "hadoken"),
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
            shown: Vec::new(),
            hand: None,
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
mod motion_space_tests {
    use super::*;

    /// Needs VISIONARY_DATA_ROOT (an extracted game); skipped otherwise.
    #[test]
    fn grab_beam_plays_in_samus_space_and_the_blaster_in_fox_s_hand() {
        let Some(root) = std::env::var_os("VISIONARY_DATA_ROOT") else {
            return;
        };
        let fighter = Path::new(&root).join("fighter");
        let space = |part: &str, motion: &str| {
            let (name, article) = part.split_once('/').unwrap();
            let skel = SkelData::from_file(
                fighter
                    .join(name)
                    .join("model")
                    .join(article)
                    .join("c00/model.nusktb"),
            )
            .unwrap();
            let anim = AnimData::from_file(
                fighter
                    .join(name)
                    .join("motion")
                    .join(article)
                    .join("c00")
                    .join(motion),
            )
            .unwrap();
            plays_in_fighter_space(&skel, &anim)
        };
        assert!(space("samus/gbeam", "e00catch.nuanmb"));
        assert!(space("samus/gun", "e00catch.nuanmb"));
        assert!(!space("fox/blaster", "blasteropen.nuanmb"));
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

#[cfg(test)]
mod resolved_settings_preview_tests {
    use super::*;
    use crate::articles::params::{ParamBlock, ParamValue};
    use crate::articles::settings::{Setting, SettingField, SettingValue};
    use crate::data::{ArticleCall, ArticleOp, ArticleOrigin, ArticleTarget};

    fn make_setting(field: SettingField, value: SettingValue, facing_factor: bool) -> Setting {
        Setting {
            field,
            value,
            status: "STATUS_FLY".into(),
            phase: "Main".into(),
            file: PathBuf::from("status.rs"),
            span: 0..1,
            facing_factor,
            referenced_params: Vec::new(),
        }
    }

    #[test]
    fn settings_override_params() {
        let mut params = ParamBlock {
            present: true,
            ..Default::default()
        };
        params.fields.insert(
            "speed".into(),
            (ParamValue::Float(1.0), PathBuf::from("vl.prc")),
        );
        let base = flight_from_param_block(&params);
        assert_eq!(base.speed, 1.0);

        let settings = vec![
            make_setting(
                SettingField::SpeedX,
                SettingValue::Const {
                    name: "SHOT_SPEED".into(),
                    value: Some(2.0),
                },
                true,
            ),
            make_setting(SettingField::Life, SettingValue::Literal(45.0), false),
            make_setting(SettingField::Gravity, SettingValue::Literal(0.08), false),
        ];
        let resolved = base.with_settings(&settings, &params);
        assert_eq!(resolved.speed, 2.0);
        assert_eq!(resolved.life, 45.0);
        assert!((resolved.gravity - 0.08).abs() < 1e-6);
    }

    #[test]
    fn computed_keeps_params() {
        let mut params = ParamBlock {
            present: true,
            ..Default::default()
        };
        params.fields.insert(
            "speed".into(),
            (ParamValue::Float(1.0), PathBuf::from("vl.prc")),
        );
        let base = flight_from_param_block(&params);
        let settings = vec![make_setting(
            SettingField::SpeedX,
            SettingValue::Computed("lerp(1.0, 3.0, charge)".into()),
            true,
        )];
        let resolved = base.with_settings(&settings, &params);
        assert_eq!(resolved.speed, 1.0);
    }

    #[test]
    fn scale_applies_to_placement() {
        let scale_setting = Setting {
            field: SettingField::Scale,
            value: SettingValue::Literal(2.0),
            facing_factor: false,
            status: "regular".into(),
            phase: "main".into(),
            file: PathBuf::from("status.rs"),
            span: 0..3,
            referenced_params: Vec::new(),
        };
        let resolved_scale = scale_from_settings(&[scale_setting], &ParamBlock::default());
        assert!((resolved_scale - 2.0).abs() < 1e-5);
        let kind = Arc::new(ObjectKind {
            scale: resolved_scale,
            flight: Flight {
                speed: 1.0,
                life: 30.0,
                ..Flight::default()
            },
            ..ObjectKind::default()
        });
        let spawn = Spawn {
            kind,
            spawned: 1,
            ..Spawn::default()
        };
        let bones = |_: f32| HashMap::from([("top".to_owned(), glam::Mat4::IDENTITY)]);
        let placed = place(&[spawn], 5.0, &bones);
        assert_eq!(placed.len(), 1);
        let bx = placed[0].placement.x_axis.truncate().length();
        let by = placed[0].placement.y_axis.truncate().length();
        let bz = placed[0].placement.z_axis.truncate().length();
        assert!(
            (bx - 2.0).abs() < 1e-5,
            "expected x basis length 2.0, got {bx}"
        );
        assert!(
            (by - 2.0).abs() < 1e-5,
            "expected y basis length 2.0, got {by}"
        );
        assert!(
            (bz - 2.0).abs() < 1e-5,
            "expected z basis length 2.0, got {bz}"
        );
    }

    #[test]
    fn source_script_preferred() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"latios\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            r#"
unsafe extern "C" fn game_regular(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 14.0, 361, 80, 0, 40, 4.5, 0.0, 0.0, 0.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, true, false, false, false, false, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_elec"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_ELEC, *ATTACK_REGION_ENERGY);
    }
}

pub fn install() {
    smashline::Agent::new("samus_lativolt")
        .game_acmd("game_regular", game_regular, smashline::Priority::Default)
        .install();
}
"#,
        )
        .unwrap();
        let index = crate::acmd_src::SourceIndex::build(root).expect("build index");
        let dump_called = std::cell::Cell::new(false);
        let dump_body = r#"unsafe extern "C" fn game_regular(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 5.0, 361, 50, 0, 20, 2.0, 0.0, 0.0, 0.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, true, false, false, false, false, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_fire"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_FIRE, *ATTACK_REGION_NONE);
    }
}"#;
        let kind = load_kind(
            &root.join("fighter/samus"),
            &root.join("fighter/samus/param/vl.prc"),
            0,
            "lativolt",
            &HashMap::new(),
            &|script_name| {
                script_from_index_or_dump(Some(&index), "samus_lativolt", script_name, &|_| {
                    dump_called.set(true);
                    Some(dump_body.to_string())
                })
            },
        )
        .expect("load_kind should build from source index script");
        assert!(
            !dump_called.get(),
            "dump fallback must not be used when SourceIndex defines samus_lativolt game_regular"
        );
        assert_eq!(kind.hitboxes.len(), 1);
        assert_eq!(kind.hitboxes[0].damage, 14.0);
    }

    #[test]
    fn remove_all_removes_every_copy() {
        let mk = |op: ArticleOp, target: ArticleTarget| ArticleCall {
            op,
            article: "*FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
            args: Vec::new(),
            target,
            source: String::new(),
            edited: false,
            origin: ArticleOrigin::Script,
        };
        let c1 = mk(ArticleOp::Generate, ArticleTarget::None);
        let c2 = mk(ArticleOp::Generate, ArticleTarget::None);
        let rm = mk(ArticleOp::Remove, ArticleTarget::All);
        let plans = plan_spawns_from_calls(&[(5, &c1), (12, &c2), (20, &rm)], &|call| {
            article_name(&call.article)
        });
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].spawned, 5);
        assert_eq!(plans[0].removed, Some(20));
        assert_eq!(plans[1].spawned, 12);
        assert_eq!(plans[1].removed, Some(20));

        // Also check Last vs First targeting.
        let rm_last = mk(ArticleOp::Remove, ArticleTarget::Last);
        let last_plans = plan_spawns_from_calls(&[(5, &c1), (12, &c2), (20, &rm_last)], &|call| {
            article_name(&call.article)
        });
        assert_eq!(last_plans[0].removed, None);
        assert_eq!(last_plans[1].removed, Some(20));

        let rm_first = mk(ArticleOp::Remove, ArticleTarget::First);
        let first_plans =
            plan_spawns_from_calls(&[(5, &c1), (12, &c2), (20, &rm_first)], &|call| {
                article_name(&call.article)
            });
        assert_eq!(first_plans[0].removed, Some(20));
        assert_eq!(first_plans[1].removed, None);
    }

    #[test]
    fn terminal_status_speed_zero_does_not_stop_flight() {
        let params = ParamBlock::default();
        let base = Flight {
            speed: 2.5,
            life: 60.0,
            ..Flight::default()
        };
        let explode_stop = Setting {
            field: SettingField::SpeedX,
            value: SettingValue::Literal(0.0),
            facing_factor: false,
            status: "WEAPON_SAMUS_LATIVOLT_STATUS_KIND_GROUND".into(),
            phase: "main".into(),
            file: PathBuf::from("lati_speciallw/mod.rs"),
            span: 0..3,
            referenced_params: Vec::new(),
        };
        let result = base.with_settings(&[explode_stop], &params);
        assert!(
            (result.speed - 2.5).abs() < 1e-5,
            "impact/ground status speed 0.0 must not zero out flight speed, got {}",
            result.speed
        );
    }

    #[test]
    fn wrapper_spawn_calls_resolve_clones_and_carriers() {
        use crate::articles::{Article, ArticleCatalog, ArticleOrigin};
        // Sceptile-style clone article spawned through a wrapper helper.
        let sceptile_catalog = ArticleCatalog {
            articles: vec![Article {
                agent: "mario_leafstorm".into(),
                owner: "mario".into(),
                name: "leafstorm".into(),
                origin: ArticleOrigin::Rewritten,
                id_symbols: Vec::new(),
                source_scripts: vec!["game_fly".into()],
                conflicts: Vec::new(),
                id_note: None,
                also_declared: Vec::new(),
                clone_conflicts: Vec::new(),
            }],
            unresolved_clones: Vec::new(),
        };
        let sceptile_script = crate::acmd::parse_acmd_script(
            r#"
unsafe extern "C" fn game_special_n_shoot(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 18.0);
    if macros::is_excute(agent) {
        if generate_sceptile_article(agent.module_accessor, ArticleKind::LeafStorm) {
            macros::QUAKE(agent, *CAMERA_QUAKE_KIND_M);
        }
    }
}
"#,
        );
        let plans = plan_wrapper_spawns_from_script(&sceptile_script, &sceptile_catalog, None);
        assert_eq!(plans.len(), 1, "{plans:?}");
        assert_eq!(plans[0].name, "leafstorm");
        assert_eq!(plans[0].spawned, 18);

        // Shy Guy-style carrier article spawned via `ArticleKind::Pellet`.
        let shyguy_catalog = ArticleCatalog {
            articles: vec![Article {
                agent: "pacman_trampoline".into(),
                owner: "pacman".into(),
                name: "trampoline".into(),
                origin: ArticleOrigin::Rewritten,
                id_symbols: Vec::new(),
                source_scripts: vec!["game_pellet_shoot".into(), "game_fireball_fly".into()],
                conflicts: Vec::new(),
                id_note: None,
                also_declared: Vec::new(),
                clone_conflicts: Vec::new(),
            }],
            unresolved_clones: Vec::new(),
        };
        let shyguy_script = crate::acmd::parse_acmd_script(
            r#"
unsafe extern "C" fn game_attack11(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        generate_stacked_article(agent.module_accessor, ArticleKind::Pellet);
    }
}
"#,
        );
        let plans = plan_wrapper_spawns_from_script(&shyguy_script, &shyguy_catalog, None);
        assert_eq!(plans.len(), 1, "{plans:?}");
        assert_eq!(plans[0].name, "trampoline");
        assert_eq!(plans[0].spawned, 6);
        assert_eq!(plans[0].motion.as_deref(), Some("pellet_shoot"));
    }
}
