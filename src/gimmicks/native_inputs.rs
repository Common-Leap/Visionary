//! Closed, read-only native inputs for behavior expressions.
//!
//! A reference is a reviewed source selector, not a Work ID or Rust expression.
//! Snapshots are transient dispatch inputs and are deliberately not serde types.

pub const COMMAND_INPUT_FEATURE: &str = "command_input.v1";

/// Native category 4 command flags. Category indices passed to ControlModule are zero based.
pub fn command_symbols(fighter: &str) -> &'static [&'static str] {
    match fighter {
        "ryu" => &[
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N2_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_S_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI_COMMAND",
        ],
        "ken" => &[
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N2_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_S_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_ATTACK_COMMAND1",
        ],
        "dolly" => &[
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_S_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI2_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SUPER_SPECIAL_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SUPER_SPECIAL2_COMMAND",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N2_COMMAND",
        ],
        "demon" => &[
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_4N4",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623NB",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623A",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623STRICT",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623ALONG",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623BLONG",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6AB",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_323CATCH",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_1",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_3",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_7",
            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_9",
            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI_COMMAND",
        ],
        _ => &[],
    }
}

pub fn command_label(symbol: &str) -> &str {
    match symbol {
        "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND" => "Quarter-circle forward",
        "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N2_COMMAND" => "Half-circle forward",
        "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_S_COMMAND" => "Quarter-circle back",
        "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI_COMMAND" => "Dragon punch",
        "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI2_COMMAND" => "Up special command 2",
        "FIGHTER_PAD_CMD_CAT4_FLAG_SUPER_SPECIAL_COMMAND" => "Super special command",
        "FIGHTER_PAD_CMD_CAT4_FLAG_SUPER_SPECIAL2_COMMAND" => "Super special command 2",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6" => "Double forward",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_4N4" => "Double back",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623NB" => "Crouch dash",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623A" => "Dragon punch + attack",
        "FIGHTER_PAD_CMD_CAT4_FLAG_ATTACK_COMMAND1" => "Command attack",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623STRICT" => "Strict dragon punch",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623ALONG" => "Dragon punch with held attack",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623BLONG" => "Dragon punch with held special",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6AB" => "Double forward with attack and special",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_323CATCH" => "Down-forward down down-forward grab",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_1" => "Down back",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_3" => "Down forward",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_7" => "Up back",
        "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_9" => "Up forward",
        _ => symbol,
    }
}

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Shared numeric range accepted by live behavior values and native samples.
pub const MAX_ABS_NATIVE_VALUE: f64 = 1_000_000.0;
pub const MAX_NATIVE_SNAPSHOT_SOURCES: usize = 64;
pub const ATTACK_RESULT_FLAGS_FEATURE: &str = "attack_result_flags.v1";
pub const STATUS_PHASE_FEATURE: &str = "status_phase.v1";
pub const OWNED_ARTICLES_FEATURE: &str = "owned_articles.v1";
pub const ARTICLE_PRESENCE_FEATURE: &str = "article_presence.v1";
pub const REFLECT_ABSORB_FEATURE: &str = "receiver_deflect.v1";
pub const INCOMING_HIT_FEATURE: &str = "incoming_hit.v1";
pub const GRAB_EVENTS_FEATURE: &str = "grab_events.v1";
pub const HELD_OPPONENT_FEATURE: &str = "held_opponent.v1";
pub const MOVEMENT_FEATURE: &str = "movement.v1";
pub const ITEM_EVENTS_FEATURE: &str = "item_events.v1";
pub const HELD_ITEM_KIND_FEATURE: &str = "held_item_kind.v1";
pub const ITEMS_FEATURE: &str = "items.v1";
pub const FIGHTER_STOCKS_FEATURE: &str = "fighter_stocks.v1";
pub const FIGHTER_DAMAGE_FEATURE: &str = "fighter_damage.v1";
pub const PARTNER_FEATURE: &str = "partner.v1";
pub const OPPONENT_FEATURE: &str = "opponent.v1";
pub const FIGHTER_STATUS_IS_FEATURE: &str = "fighter_status_is.v2";
pub const FIGHTER_STATUS_CHANGING_FEATURE: &str = "fighter_status_changing.v1";
pub const DAMAGE_CAPTURE_STATUS_FEATURE: &str = "damage_capture_status.v2";
/// Closed operation capability; this does not declare a sampled input.
pub const CLOUD_LIMIT_ACTIONS_FEATURE: &str = "cloud_limit_actions.v2";

/// Bounded world-axis movement delivered on the owning fighter's Main line.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MovementAction {
    SetSpeed { x: Option<f32>, y: Option<f32> },
    SetGravity { enabled: bool },
    MoveBy { dx: f32, dy: f32 },
    SetFacing(Facing),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Facing {
    Left,
    Right,
    Turn,
    /// Toward the opponent `FighterValue::OpponentPresent` defines; no change without one.
    Opponent,
}
impl MovementAction {
    pub fn validate(&self) -> Result<(), String> {
        let bounded = |v: f32, max: f32| v.is_finite() && v.abs() <= max;
        let valid = match *self {
            Self::SetSpeed { x, y } => {
                (x.is_some() || y.is_some()) && x.into_iter().chain(y).all(|v| bounded(v, 10.0))
            }
            Self::MoveBy { dx, dy } => bounded(dx, 50.0) && bounded(dy, 50.0),
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err("movement requires finite speed within ±10 or displacement within ±50".into())
        }
    }
    /// Projectiles re-apply their own speed every frame (verified on Mario's fireball), so only
    /// a position nudge steers them; run it every frame to curve a shot.
    pub fn validate_for_article(&self) -> Result<(), String> {
        self.validate()?;
        match self {
            Self::MoveBy { .. } => Ok(()),
            _ => Err("projectiles support only Move by".into()),
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::SetSpeed { .. } => "Set speed",
            Self::SetGravity { .. } => "Set gravity",
            Self::MoveBy { .. } => "Move by",
            Self::SetFacing(_) => "Set facing",
        }
    }
}

/// What a controller does to the newest live object of one owned article kind.
/// Both run on that object's own Main line, where its lua agent is current.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ObjectOp<E> {
    /// World-axis speed through the vanilla weapon `set_speed` on its normal energy.
    SetSpeed { x: E, y: E },
    /// The vanilla weapon remove event, as the absorber uses.
    Remove,
}

impl<E> ObjectOp<E> {
    pub const MAX_SPEED: f32 = 10.0;
    pub fn label(&self) -> &'static str {
        match self {
            Self::SetSpeed { .. } => "Set object speed",
            Self::Remove => "Remove object",
        }
    }
    pub fn expressions(&self) -> Vec<&E> {
        match self {
            Self::SetSpeed { x, y } => vec![x, y],
            Self::Remove => vec![],
        }
    }
    pub fn expressions_mut(&mut self) -> Vec<&mut E> {
        match self {
            Self::SetSpeed { x, y } => vec![x, y],
            Self::Remove => vec![],
        }
    }
    pub fn try_map<T, F>(&self, mut f: impl FnMut(&E) -> Result<T, F>) -> Result<ObjectOp<T>, F> {
        Ok(match self {
            Self::SetSpeed { x, y } => ObjectOp::SetSpeed { x: f(x)?, y: f(y)? },
            Self::Remove => ObjectOp::Remove,
        })
    }
}

/// How long a combat policy holds before its channel returns to normal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyScope {
    /// Until this fighter's status changes.
    Status,
    /// For this many of the fighter's frames, across status changes.
    Frames(u16),
    /// Until a Clear on the same channel, a stock loss or a controller change.
    UntilCleared,
}

impl PolicyScope {
    pub const MAX_FRAMES: u16 = 3600;
    pub fn label(self) -> String {
        match self {
            Self::Status => "this status".into(),
            Self::Frames(n) => format!("{n} frames"),
            Self::UntilCleared => "until cleared".into(),
        }
    }
}

/// One way the fighter takes or deals hits; each op owns one channel, and a
/// later op on the same channel replaces the earlier one (never stacks).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum CombatOp<E> {
    /// `DamageModule::set_damage_mul`.
    DamageTaken(E),
    /// `DamageModule::set_reaction_mul`.
    KnockbackTaken(E),
    /// `AttackModule::set_power_mul`.
    DamageDealt(E),
    /// `AttackModule::set_reaction_mul`.
    KnockbackDealt(E),
    /// Scales the fighter's own param reads for this stat; see [`MovementStat`].
    Stat(MovementStat, E),
    /// `DamageModule::set_no_reaction_mode_status` DAMAGE_POWER: hits up to this damage don't flinch.
    Armor(E),
    /// The same with DAMAGE_NO_REACTION_MODE_ALWAYS.
    SuperArmor,
    /// `HitModule::set_whole` HIT_STATUS_XLU (attacks pass through).
    Intangible,
    /// `HitModule::set_whole` HIT_STATUS_INVINCIBLE (hit, but no damage).
    Invincible,
    /// Return one channel to normal now.
    Clear(CombatChannel),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CombatChannel {
    DamageTaken,
    KnockbackTaken,
    DamageDealt,
    KnockbackDealt,
    Armor,
    Hurtbox,
    RunSpeed,
    AirSpeed,
    Gravity,
    JumpHeight,
}

impl CombatChannel {
    pub const ALL: [Self; 10] = [
        Self::DamageTaken,
        Self::KnockbackTaken,
        Self::DamageDealt,
        Self::KnockbackDealt,
        Self::Armor,
        Self::Hurtbox,
        Self::RunSpeed,
        Self::AirSpeed,
        Self::Gravity,
        Self::JumpHeight,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::DamageTaken => "damage taken",
            Self::KnockbackTaken => "knockback taken",
            Self::DamageDealt => "damage dealt",
            Self::KnockbackDealt => "knockback dealt",
            Self::Armor => "armor",
            Self::Hurtbox => "intangibility",
            Self::RunSpeed => "run speed",
            Self::AirSpeed => "air speed",
            Self::Gravity => "gravity",
            Self::JumpHeight => "jump height",
        }
    }
    #[allow(dead_code)] // Plugin side.
    pub fn stat(self) -> Option<MovementStat> {
        MovementStat::ALL
            .into_iter()
            .find(|stat| stat.channel() == self)
    }
}

/// A fighter stat scaled where the engine reads it, so it holds across status
/// changes without any re-application. In 13.0.4 every reader below calls
/// WorkModule vtable +0x270 (main+0x4e53e0), which tail-calls the ParamData
/// getter the plugin hooks (main+0x77dbc0), with child key 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MovementStat {
    /// `run_speed_max`: read every frame by control energy update (main+0x6d388c).
    RunSpeed,
    /// `air_speed_x_stable`: read by control energy setup (main+0x6d40a8), so next status.
    AirSpeed,
    /// `air_accel_y`: read by gravity energy setup, so next status. Jump speeds
    /// are derived from gravity once at load (main+0x6f21d0) and do not follow
    /// it: every jump's height divides by this multiplier.
    Gravity,
    /// Short hops and aerial jumps start from a derived speed (main+0x6d1aa4,
    /// +0x6ce298). Height goes with speed squared, so the speed is scaled by the
    /// square root. A full hop from the ground is different, see [`Self::read_factor`].
    JumpHeight,
}

impl MovementStat {
    #[allow(dead_code)] // Plugin side.
    pub const ALL: [Self; 4] = [
        Self::RunSpeed,
        Self::AirSpeed,
        Self::Gravity,
        Self::JumpHeight,
    ];
    pub fn channel(self) -> CombatChannel {
        match self {
            Self::RunSpeed => CombatChannel::RunSpeed,
            Self::AirSpeed => CombatChannel::AirSpeed,
            Self::Gravity => CombatChannel::Gravity,
            Self::JumpHeight => CombatChannel::JumpHeight,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::RunSpeed => "Run speed ×",
            Self::AirSpeed => "Air speed ×",
            Self::Gravity => "Gravity ×",
            Self::JumpHeight => "Jump height ×",
        }
    }
    /// The stat a getter key belongs to. Kept as hashes: this runs on every param read.
    #[allow(dead_code)] // Plugin side.
    pub fn for_param(parent: u64, child: u64) -> Option<Self> {
        match (parent, child) {
            (0x0d_5aaa_bf9b, 0) => Some(Self::RunSpeed), // run_speed_max
            (0x12_20fc_2660, 0) => Some(Self::AirSpeed), // air_speed_x_stable
            (0x0b_a180_57d9, 0) => Some(Self::Gravity),  // air_accel_y
            (0x0c_bed5_c24c | 0x11_2ab2_4d60 | 0x13_bc11_42f6, 0) => Some(Self::JumpHeight), // jump_speed_y, mini_jump_speed_y, jump_aerial_speed_y
            (JUMP_Y | JUMP_INITIAL_SPEED_Y | JUMP_INITIAL_ACCEL_Y, 0) => Some(Self::JumpHeight),
            _ => None,
        }
    }
    /// What this read of `parent` is multiplied by, given all of a fighter's multipliers.
    ///
    /// A full hop (RESERVE_JUMP_FROM_SQUAT, main+0x6d2180) resets gravity energy to
    /// type 9: it climbs under `jump_initial_accel_y` for `jump_initial_frame` frames,
    /// then the energy update (main+0x6d5030) replaces the speed so that the apex lands
    /// exactly on `jump_y`, whatever the start speed or gravity was. Scaling all three
    /// by one factor scales the climb and the remainder alike, so the apex is
    /// `jump_y * height / gravity`, the same ratio the other jumps get from their speed.
    #[allow(dead_code)] // Plugin side.
    pub fn read_factor(self, parent: u64, multipliers: &[f32; Self::ALL.len()]) -> f32 {
        let of = |stat: Self| multipliers[stat as usize];
        match self {
            Self::JumpHeight
                if matches!(parent, JUMP_Y | JUMP_INITIAL_SPEED_Y | JUMP_INITIAL_ACCEL_Y) =>
            {
                of(Self::JumpHeight) / of(Self::Gravity)
            }
            Self::JumpHeight => of(Self::JumpHeight).sqrt(),
            stat => of(stat),
        }
    }
}
const JUMP_Y: u64 = 0x06_a0d8_2dad;
const JUMP_INITIAL_SPEED_Y: u64 = 0x14_2986_e401;
const JUMP_INITIAL_ACCEL_Y: u64 = 0x14_ddd6_bdcb;

impl<E> CombatOp<E> {
    pub fn channel(&self) -> CombatChannel {
        match self {
            Self::DamageTaken(_) => CombatChannel::DamageTaken,
            Self::KnockbackTaken(_) => CombatChannel::KnockbackTaken,
            Self::DamageDealt(_) => CombatChannel::DamageDealt,
            Self::KnockbackDealt(_) => CombatChannel::KnockbackDealt,
            Self::Stat(stat, _) => stat.channel(),
            Self::Armor(_) | Self::SuperArmor => CombatChannel::Armor,
            Self::Intangible | Self::Invincible => CombatChannel::Hurtbox,
            Self::Clear(channel) => *channel,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::DamageTaken(_) => "Damage taken ×",
            Self::KnockbackTaken(_) => "Knockback taken ×",
            Self::DamageDealt(_) => "Damage dealt ×",
            Self::KnockbackDealt(_) => "Knockback dealt ×",
            Self::Stat(stat, _) => stat.label(),
            Self::Armor(_) => "Armor up to %",
            Self::SuperArmor => "Super armor",
            Self::Intangible => "Intangible",
            Self::Invincible => "Invincible",
            Self::Clear(_) => "Clear",
        }
    }
    /// The valid range of the op's value, if it has one.
    pub fn bounds(&self) -> Option<std::ops::RangeInclusive<f32>> {
        match self {
            Self::DamageTaken(_)
            | Self::KnockbackTaken(_)
            | Self::DamageDealt(_)
            | Self::KnockbackDealt(_) => Some(0.0..=10.0),
            // Never zero: the engine divides by gravity and a jump of speed 0 never leaves the ground.
            Self::Stat(..) => Some(0.1..=10.0),
            Self::Armor(_) => Some(0.0..=999.0),
            _ => None,
        }
    }
    pub fn value(&self) -> Option<&E> {
        match self {
            Self::DamageTaken(v)
            | Self::KnockbackTaken(v)
            | Self::DamageDealt(v)
            | Self::KnockbackDealt(v)
            | Self::Stat(_, v)
            | Self::Armor(v) => Some(v),
            _ => None,
        }
    }
    pub fn value_mut(&mut self) -> Option<&mut E> {
        match self {
            Self::DamageTaken(v)
            | Self::KnockbackTaken(v)
            | Self::DamageDealt(v)
            | Self::KnockbackDealt(v)
            | Self::Stat(_, v)
            | Self::Armor(v) => Some(v),
            _ => None,
        }
    }
    pub fn try_map<T, F>(&self, f: impl FnOnce(&E) -> Result<T, F>) -> Result<CombatOp<T>, F> {
        Ok(match self {
            Self::DamageTaken(v) => CombatOp::DamageTaken(f(v)?),
            Self::KnockbackTaken(v) => CombatOp::KnockbackTaken(f(v)?),
            Self::DamageDealt(v) => CombatOp::DamageDealt(f(v)?),
            Self::KnockbackDealt(v) => CombatOp::KnockbackDealt(f(v)?),
            Self::Stat(stat, v) => CombatOp::Stat(*stat, f(v)?),
            Self::Armor(v) => CombatOp::Armor(f(v)?),
            Self::SuperArmor => CombatOp::SuperArmor,
            Self::Intangible => CombatOp::Intangible,
            Self::Invincible => CombatOp::Invincible,
            Self::Clear(channel) => CombatOp::Clear(*channel),
        })
    }
}

/// Reject a scope the plugin cannot count.
pub fn validate_policy_scope(scope: PolicyScope) -> Result<(), String> {
    match scope {
        PolicyScope::Frames(n) if n == 0 || n > PolicyScope::MAX_FRAMES => Err(format!(
            "combat policy frames must be 1..={}",
            PolicyScope::MAX_FRAMES
        )),
        _ => Ok(()),
    }
}

/// What a controller does with the item in the fighter's main hand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ItemAction {
    /// Hand the fighter a new item of an exact `ITEM_KIND_*`.
    Hold {
        item: String,
    },
    /// Throw the held item: angle in degrees (0 = forward, 90 = up), power 0..=10.
    Throw {
        angle: f32,
        power: f32,
    },
    Drop,
    /// Delete the held item without dropping it.
    Remove,
}

impl ItemAction {
    /// `ITEM_KIND_*` value from the generated registry.
    pub fn item_kind(symbol: &str) -> Option<i32> {
        super::registry::ENTRIES
            .iter()
            .find(|e| e.kind == super::registry::IdentifierKind::Item && e.symbol == symbol)
            .and_then(|e| i32::try_from(e.value).ok())
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Hold { item } if Self::item_kind(item).is_none() => {
                Err(format!("unknown item kind {item:?}"))
            }
            Self::Throw { angle, power }
                if !(0.0..=360.0).contains(angle) || !(0.0..=10.0).contains(power) =>
            {
                Err("item throw needs angle 0..=360 and power 0..=10".into())
            }
            _ => Ok(()),
        }
    }
}

/// Root contact inputs are transient and are never native sample selectors or
/// persisted state. The engine adapter supplies provenance and actor checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Constructed by plugin contact delivery and offline model fixtures.
pub enum DamageDirection {
    Deal,
    Take,
    AttackShielded,
    ShieldHit,
    Incoming,
    Reflect,
    Absorb,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageInput {
    direction: DamageDirection,
    pending: Option<f32>,
    projectile: bool,
    receiver_status: Option<(i32, i32)>,
}

#[allow(dead_code)] // Explicit plugin root input, not a desktop sampling source.
impl DamageInput {
    pub fn new(direction: DamageDirection, pending: f32) -> Result<Self, String> {
        let permits_zero = matches!(
            direction,
            DamageDirection::AttackShielded
                | DamageDirection::ShieldHit
                | DamageDirection::Incoming
                | DamageDirection::Reflect
                | DamageDirection::Absorb
        );
        if !pending.is_finite()
            || pending < 0.0
            || (!permits_zero && pending == 0.0)
            || f64::from(pending) > MAX_ABS_NATIVE_VALUE
        {
            return Err(
                "pending damage must be finite, positive (or zero incoming/blocked power) and within the numeric range".into(),
            );
        }
        Ok(Self {
            direction,
            pending: Some(pending),
            projectile: false,
            receiver_status: None,
        })
    }

    /// Attach copied native contact context after the engine adapter validates
    /// the exact receiver. This transient value is never a project selector.
    /// An absent status clears context instead of borrowing an earlier root.
    pub fn with_receiver_status(
        mut self,
        fighter_kind: i32,
        status: Option<i32>,
    ) -> Result<Self, String> {
        self.receiver_status = match status {
            Some(status)
                if status >= 0
                    && super::registry::FIGHTER_KINDS
                        .iter()
                        .any(|entry| i32::try_from(entry.value).ok() == Some(fighter_kind)) =>
            {
                Some((fighter_kind, status))
            }
            Some(_) => {
                return Err(
                    "damage receiver status has an invalid fighter kind or native status".into(),
                )
            }
            None => None,
        };
        Ok(self)
    }

    pub fn shield(direction: DamageDirection) -> Result<Self, String> {
        if !matches!(
            direction,
            DamageDirection::AttackShielded
                | DamageDirection::ShieldHit
                | DamageDirection::Reflect
                | DamageDirection::Absorb
        ) {
            return Err("missing power is only valid for a blocked or deflected contact".into());
        }
        Ok(Self {
            direction,
            pending: None,
            projectile: false,
            receiver_status: None,
        })
    }

    pub fn with_projectile(mut self, projectile: bool) -> Self {
        self.projectile = projectile;
        self
    }

    pub fn direction(self) -> DamageDirection {
        self.direction
    }
    pub fn pending(self) -> Option<f32> {
        self.pending
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttackResultQuery {
    DuringHitlag,
    CurrentStatus,
}

impl AttackResultQuery {
    fn as_str(self) -> &'static str {
        match self {
            Self::DuringHitlag => "during_hitlag",
            Self::CurrentStatus => "current_status",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttackResultMask {
    Hit,
    Shield,
}

impl AttackResultMask {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Shield => "shield",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeKind {
    Float,
    Int,
    Flag,
}

/// Reviewed native inputs currently available to behavior graphs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeInput {
    /// Cloud's exact fighter-owned instance Limit gauge float.
    CloudLimitGauge,
    /// Cloud's exact fighter-owned instance Limit clear-frame integer.
    CloudLimitClearFrame,
    /// Cloud's exact fighter-owned instance Limit Break flag.
    CloudLimitBreak,
    /// Samus's exact fighter-owned instance charge counter.
    SamusChargeCount,
    /// Sheik's exact fighter-owned instance needle counter.
    SheikNeedleCount,
    /// R.O.B.'s exact fighter-owned instance beam-energy value.
    RobotBeamEnergy,
    /// Robin's exact fighter-owned instance Thunder sword flag.
    RefletThunderSword,
    /// Hero's current command-selection spell kind, or -1 while the menu is closed.
    HeroMenuSpell,
    /// Read-only fighter attack-result sample using one closed query and mask.
    AttackResultFlag {
        fighter: String,
        query: AttackResultQuery,
        mask: AttackResultMask,
    },
    /// Whether this fighter currently owns an allowlisted article kind.
    ArticleExists {
        fighter: String,
        symbol: String,
    },
    ArticleCount {
        fighter: String,
        symbol: String,
    },
    /// Whether a live article of this kind was reflected (or absorbed) in its
    /// current status: AttackModule::is_infliction_status on each weapon.
    ArticleDeflected {
        fighter: String,
        symbol: String,
        absorbed: bool,
    },
    /// Whether this fighter's current native status equals an exact owned status.
    FighterStatusIs {
        fighter: String,
        symbol: String,
    },
    /// A value that needs only its owning fighter; `FighterValue` describes it.
    Fighter {
        fighter: String,
        value: FighterValue,
    },
    /// One reading of the newest live object of an owned article kind; zero
    /// (and not alive) while none exists.
    OwnedObject {
        fighter: String,
        symbol: String,
        field: ObjectField,
    },
    /// Buffered category 4 command flag, sampled on the fighter Main frame tick.
    Command {
        fighter: String,
        symbol: String,
    },
    /// Reviewed vanilla WorkModule parameters, carried by NativeFloat/NativeInt expressions.
    ParamFloat {
        fighter: String,
        table: String,
        key: String,
    },
    ParamInt {
        fighter: String,
        table: String,
        key: String,
    },
    EnergySpeed {
        fighter: String,
        energy: super::status_commands::Energy,
        axis: Axis,
    },
    /// An exact fighter-owned Work identifier admitted by the generated registry.
    Work {
        fighter: String,
        symbol: String,
        kind: NativeKind,
    },
    /// A bounded saved identifier not yet admitted by the closed catalog.
    Unsupported(String),
}

/// Native values read from the owning fighter alone. Each row is the whole
/// definition: reference key, label, type and the plugin feature that samples it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FighterValue {
    /// Read-only StatusModule::is_changing.
    StatusChanging,
    /// DamageModule::damage, using the fixed native slot zero.
    Damage,
    /// The live partner's (Nana, Luma) damage; 0 while it is absent.
    PartnerDamage,
    /// Current capture-link opponent, sampled only after proving both actors.
    HoldingOpponent,
    /// ItemModule::is_have_item on the main hand.
    HoldingItem,
    PartnerPresent,
    HeldOpponentDamage,
    HeldOpponentWeight,
    /// ControlModule::get_clatter_time: falls by 1 per frame during a hold, measured in game.
    HeldOpponentEscapeTime,
    SpeedX,
    SpeedY,
    PositionX,
    PositionY,
    Facing,
    /// Exact main-hand ITEM_KIND; ITEM_KIND_NONE when empty.
    HeldItemKind,
    StocksRemaining,
    OpponentStocksRemaining,
    /// The opponent is another player's live, on-stage fighter: the only one in a
    /// 1v1, otherwise the nearest (teams are not considered). Pokemon Trainer's
    /// trainer object never counts. Every reading below is defined without one:
    /// not present, and zero, so a KO'd opponent never stops a controller.
    OpponentPresent,
    OpponentDamage,
    /// Opponent X minus mine, in world axes.
    OpponentDx,
    /// `OpponentDx` along my facing: positive means the opponent is in front of me.
    OpponentDxForward,
    /// Opponent Y minus mine.
    OpponentDy,
}

impl FighterValue {
    pub const ALL: [Self; 22] = [
        Self::StatusChanging,
        Self::Damage,
        Self::PartnerDamage,
        Self::HoldingOpponent,
        Self::HoldingItem,
        Self::PartnerPresent,
        Self::HeldOpponentDamage,
        Self::HeldOpponentWeight,
        Self::HeldOpponentEscapeTime,
        Self::SpeedX,
        Self::SpeedY,
        Self::PositionX,
        Self::PositionY,
        Self::Facing,
        Self::HeldItemKind,
        Self::StocksRemaining,
        Self::OpponentStocksRemaining,
        Self::OpponentPresent,
        Self::OpponentDamage,
        Self::OpponentDx,
        Self::OpponentDxForward,
        Self::OpponentDy,
    ];

    const fn spec(self) -> (&'static str, &'static str, NativeKind, &'static str) {
        use NativeKind::{Flag, Float, Int};
        match self {
            Self::StatusChanging => (
                "status_changing",
                "Changing status",
                Flag,
                FIGHTER_STATUS_CHANGING_FEATURE,
            ),
            Self::Damage => (
                "fighter_damage",
                "Damage value",
                Float,
                FIGHTER_DAMAGE_FEATURE,
            ),
            Self::PartnerDamage => ("partner_damage", "Partner damage %", Float, PARTNER_FEATURE),
            Self::HoldingOpponent => (
                "holding_opponent",
                "Holding an opponent",
                Flag,
                HELD_OPPONENT_FEATURE,
            ),
            Self::HoldingItem => ("holding_item", "Holding an item", Flag, ITEMS_FEATURE),
            Self::PartnerPresent => ("partner_present", "Partner present", Flag, PARTNER_FEATURE),
            Self::HeldOpponentDamage => (
                "held_opponent_damage",
                "Held opponent damage %",
                Float,
                HELD_OPPONENT_FEATURE,
            ),
            Self::HeldOpponentWeight => (
                "held_opponent_weight",
                "Held opponent weight",
                Float,
                HELD_OPPONENT_FEATURE,
            ),
            Self::HeldOpponentEscapeTime => (
                "held_opponent_escape_time",
                "Held opponent escape time",
                Float,
                HELD_OPPONENT_FEATURE,
            ),
            Self::SpeedX => ("speed_x", "Speed X", Float, MOVEMENT_FEATURE),
            Self::SpeedY => ("speed_y", "Speed Y", Float, MOVEMENT_FEATURE),
            Self::PositionX => ("position_x", "Position X", Float, MOVEMENT_FEATURE),
            Self::PositionY => ("position_y", "Position Y", Float, MOVEMENT_FEATURE),
            Self::Facing => (
                "facing",
                "Facing (-1 left, 1 right)",
                Float,
                MOVEMENT_FEATURE,
            ),
            Self::HeldItemKind => (
                "held_item_kind",
                "Held item kind",
                Int,
                HELD_ITEM_KIND_FEATURE,
            ),
            Self::StocksRemaining => (
                "stocks_remaining",
                "Stocks remaining",
                Int,
                FIGHTER_STOCKS_FEATURE,
            ),
            Self::OpponentStocksRemaining => (
                "opponent_stocks_remaining",
                "Opponent stocks remaining",
                Int,
                FIGHTER_STOCKS_FEATURE,
            ),
            Self::OpponentPresent => (
                "opponent_present",
                "Opponent present",
                Flag,
                OPPONENT_FEATURE,
            ),
            Self::OpponentDamage => (
                "opponent_damage",
                "Opponent damage %",
                Float,
                OPPONENT_FEATURE,
            ),
            Self::OpponentDx => (
                "opponent_dx",
                "Opponent X distance (world)",
                Float,
                OPPONENT_FEATURE,
            ),
            Self::OpponentDxForward => (
                "opponent_dx_forward",
                "Opponent X distance (+ in front)",
                Float,
                OPPONENT_FEATURE,
            ),
            Self::OpponentDy => (
                "opponent_dy",
                "Opponent Y distance",
                Float,
                OPPONENT_FEATURE,
            ),
        }
    }
    pub const fn key(self) -> &'static str {
        self.spec().0
    }
    pub const fn label(self) -> &'static str {
        self.spec().1
    }
    pub const fn kind(self) -> NativeKind {
        self.spec().2
    }
    pub const fn feature(self) -> &'static str {
        self.spec().3
    }
    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.key() == key)
    }
    fn available_for(self, fighter: &str) -> bool {
        !matches!(self, Self::PartnerPresent | Self::PartnerDamage) || has_partner(fighter)
    }
    /// Counts and held-opponent readings; a negative sample is a sampler fault.
    fn non_negative(self) -> bool {
        matches!(
            self,
            Self::HeldOpponentDamage
                | Self::HeldOpponentWeight
                | Self::HeldOpponentEscapeTime
                | Self::StocksRemaining
                | Self::OpponentStocksRemaining
                | Self::OpponentDamage
        )
    }
}

/// A `FIGHTER_INSTANCE_WORK_ID_*` value every fighter has. The generated registry
/// holds only fighter-owned identifiers, so these reviewed rows are the whole
/// admission list; values are transcribed from the pinned `lua_const.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommonWork {
    pub symbol: &'static str,
    pub label: &'static str,
    pub kind: NativeKind,
    pub value: u32,
    /// Inclusive range a controller may write; `None` is read-only.
    pub write: Option<(i32, i32)>,
}

/// A sanity cap above the largest vanilla `jump_count_max`, not a game limit.
const MAX_AUTHORED_JUMPS: i32 = 10;
pub const COMMON_WORK: [CommonWork; 9] = {
    const fn row(
        symbol: &'static str,
        label: &'static str,
        kind: NativeKind,
        value: u32,
        write: Option<(i32, i32)>,
    ) -> CommonWork {
        CommonWork {
            symbol,
            label,
            kind,
            value,
            write,
        }
    }
    use NativeKind::{Flag, Float, Int};
    [
        row(
            "FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT",
            "Jumps used",
            Int,
            0x1000_0002,
            Some((0, MAX_AUTHORED_JUMPS)),
        ),
        row(
            "FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT_MAX",
            "Max jumps",
            Int,
            0x1000_0003,
            Some((0, MAX_AUTHORED_JUMPS)),
        ),
        row(
            "FIGHTER_INSTANCE_WORK_ID_INT_WALL_JUMP_COUNT",
            "Wall jumps used",
            Int,
            0x1000_0005,
            None,
        ),
        row(
            "FIGHTER_INSTANCE_WORK_ID_INT_CLIFF_COUNT",
            "Ledge grabs",
            Int,
            0x1000_00AB,
            None,
        ),
        row(
            "FIGHTER_INSTANCE_WORK_ID_INT_FRAME_IN_AIR",
            "Frames in air",
            Int,
            0x1000_0051,
            None,
        ),
        row(
            "FIGHTER_INSTANCE_WORK_ID_FLOAT_GUARD_SHIELD",
            "Shield health",
            Float,
            0x6,
            None,
        ),
        row(
            "FIGHTER_INSTANCE_WORK_ID_FLOAT_LANDING_FRAME",
            "Landing lag frames",
            Float,
            0x0,
            None,
        ),
        row(
            "FIGHTER_INSTANCE_WORK_ID_FLAG_FINAL",
            "Final Smash state",
            Flag,
            0x2000_0008,
            None,
        ),
        row(
            "FIGHTER_INSTANCE_WORK_ID_FLAG_METAL",
            "Metal state",
            Flag,
            0x2000_001E,
            None,
        ),
    ]
};

/// A fighter's own game value in plain words: `FLAG_ONE_WINGED_ACTIVATED` reads
/// "One winged activated". Each name is made once and kept.
pub fn plain_work_name(name: &str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let Ok(mut names) = NAMES.get_or_init(Default::default).lock() else {
        return "Game value";
    };
    if let Some(label) = names.get(name) {
        return label;
    }
    let words = name
        .trim_start_matches("INT_")
        .trim_start_matches("FLOAT_")
        .trim_start_matches("FLAG_")
        .replace('_', " ")
        .to_lowercase()
        .replace("special n", "neutral special")
        .replace("special s", "side special")
        .replace("special hi", "up special")
        .replace("special lw", "down special")
        .replace("attack air", "aerial")
        .replace(" num", " count");
    let mut letters = words.chars();
    let label: String = letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    });
    let label: &'static str = Box::leak(label.into_boxed_str());
    names.insert(name.to_owned(), label);
    label
}

pub fn common_work(symbol: &str) -> Option<&'static CommonWork> {
    COMMON_WORK.iter().find(|row| row.symbol == symbol)
}

/// The Work ID of a common row of exactly this registry kind.
#[allow(dead_code)] // Called by the plugin's registry only.
pub fn common_work_id(symbol: &str, kind: super::registry::IdentifierKind) -> Option<u32> {
    common_work(symbol)
        .filter(|row| row.kind.registry_kind() == kind)
        .map(|row| row.value)
}

/// What an `OwnedObject` input reads from the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObjectField {
    PosX,
    PosY,
    SpeedX,
    SpeedY,
    Alive,
}

impl ObjectField {
    pub const ALL: [Self; 5] = [
        Self::PosX,
        Self::PosY,
        Self::SpeedX,
        Self::SpeedY,
        Self::Alive,
    ];
    const fn spec(self) -> (&'static str, &'static str, NativeKind) {
        match self {
            Self::PosX => ("pos_x", "Object position X", NativeKind::Float),
            Self::PosY => ("pos_y", "Object position Y", NativeKind::Float),
            Self::SpeedX => ("speed_x", "Object speed X", NativeKind::Float),
            Self::SpeedY => ("speed_y", "Object speed Y", NativeKind::Float),
            Self::Alive => ("alive", "Object alive", NativeKind::Flag),
        }
    }
    pub const fn key(self) -> &'static str {
        self.spec().0
    }
    pub const fn label(self) -> &'static str {
        self.spec().1
    }
    pub const fn kind(self) -> NativeKind {
        self.spec().2
    }
    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.key() == key)
    }
}

/// Friendly unit inputs and their stable saved names.
const NAMED: [(NativeInput, &str, &str); 8] = [
    (
        NativeInput::CloudLimitGauge,
        "cloud_limit_gauge",
        "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE",
    ),
    (
        NativeInput::CloudLimitClearFrame,
        "cloud_limit_clear_frame",
        "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_BREAK_CLEAR_FRAME",
    ),
    (
        NativeInput::CloudLimitBreak,
        "cloud_limit_break",
        "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK",
    ),
    (
        NativeInput::SamusChargeCount,
        "samus_charge_count",
        "FIGHTER_SAMUS_INSTANCE_WORK_ID_INT_SPECIAL_N_COUNT",
    ),
    (
        NativeInput::SheikNeedleCount,
        "sheik_needle_count",
        "FIGHTER_SHEIK_INSTANCE_WORK_ID_INT_NEEDLE_COUNT",
    ),
    (
        NativeInput::RobotBeamEnergy,
        "robot_beam_energy",
        "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE",
    ),
    (
        NativeInput::RefletThunderSword,
        "reflet_thunder_sword",
        "FIGHTER_REFLET_INSTANCE_WORK_ID_FLAG_THUNDER_SWORD_ON",
    ),
    (
        NativeInput::HeroMenuSpell,
        "hero_menu_spell",
        "hero_menu_spell",
    ),
];

const ENERGIES: [super::status_commands::Energy; 4] = {
    use super::status_commands::Energy;
    [
        Energy::Stop,
        Energy::Gravity,
        Energy::Control,
        Energy::Motion,
    ]
};

fn text_ok(value: &str) -> bool {
    !value.is_empty() && !value.contains([':', '\0'])
}

impl NativeInput {
    pub const MAX_REFERENCE_BYTES: usize = 160;

    /// Article symbols carry their exact registry owner; no alias or numeric fallback.
    pub fn article_count(symbol: &str) -> Self {
        match super::registry::ENTRIES.iter().find(|entry| {
            entry.symbol == symbol && entry.kind == super::registry::IdentifierKind::Article
        }) {
            Some(entry) => Self::ArticleCount {
                fighter: entry.fighter.into(),
                symbol: symbol.into(),
            },
            None => Self::Unsupported(symbol.into()),
        }
    }

    pub fn fighter_value(fighter: &str, value: FighterValue) -> Self {
        Self::Fighter {
            fighter: fighter.into(),
            value,
        }
    }

    pub fn fighter(&self) -> Option<&str> {
        match self {
            Self::CloudLimitGauge | Self::CloudLimitClearFrame | Self::CloudLimitBreak => {
                Some("cloud")
            }
            Self::SamusChargeCount => Some("samus"),
            Self::SheikNeedleCount => Some("sheik"),
            Self::RobotBeamEnergy => Some("robot"),
            Self::RefletThunderSword => Some("reflet"),
            Self::HeroMenuSpell => Some("brave"),
            Self::AttackResultFlag { fighter, .. }
            | Self::ArticleExists { fighter, .. }
            | Self::ArticleCount { fighter, .. }
            | Self::ArticleDeflected { fighter, .. }
            | Self::FighterStatusIs { fighter, .. }
            | Self::Fighter { fighter, .. }
            | Self::OwnedObject { fighter, .. }
            | Self::Command { fighter, .. }
            | Self::ParamFloat { fighter, .. }
            | Self::ParamInt { fighter, .. }
            | Self::EnergySpeed { fighter, .. }
            | Self::Work { fighter, .. } => Some(fighter),
            Self::Unsupported(_) => None,
        }
    }

    pub fn symbol(&self) -> &str {
        match self {
            Self::AttackResultFlag { .. } => "attack_result_flag",
            Self::ArticleExists { symbol, .. }
            | Self::ArticleCount { symbol, .. }
            | Self::ArticleDeflected { symbol, .. }
            | Self::FighterStatusIs { symbol, .. }
            | Self::OwnedObject { symbol, .. }
            | Self::Command { symbol, .. }
            | Self::Work { symbol, .. }
            | Self::Unsupported(symbol) => symbol,
            Self::Fighter { value, .. } => value.key(),
            Self::ParamFloat { key, .. } | Self::ParamInt { key, .. } => key,
            Self::EnergySpeed { .. } => "energy_speed",
            named => NAMED
                .iter()
                .find(|(input, ..)| input == named)
                .map_or("", |(.., symbol)| symbol),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::CloudLimitGauge => "Limit gauge",
            Self::CloudLimitClearFrame => "Limit clear-frame value",
            Self::CloudLimitBreak => "Limit active",
            Self::SamusChargeCount => "Charge-shot counter",
            Self::SheikNeedleCount => "Needle count",
            Self::RobotBeamEnergy => "Beam energy value",
            Self::RefletThunderSword => "Thunder sword active",
            Self::HeroMenuSpell => "Hero menu spell",
            Self::Fighter { value, .. } => value.label(),
            Self::OwnedObject { field, .. } => field.label(),
            Self::ParamFloat { .. } => "Cape gravity acceleration",
            Self::ParamInt { .. } => "Reflector air stop frames",
            Self::EnergySpeed { energy, axis, .. } => match (energy, axis) {
                (super::status_commands::Energy::Stop, Axis::X) => "Stop speed X",
                (super::status_commands::Energy::Stop, Axis::Y) => "Stop speed Y",
                (super::status_commands::Energy::Gravity, Axis::X) => "Gravity speed X",
                (super::status_commands::Energy::Gravity, Axis::Y) => "Gravity speed Y",
                (super::status_commands::Energy::Control, Axis::X) => "Control speed X",
                (super::status_commands::Energy::Control, Axis::Y) => "Control speed Y",
                (super::status_commands::Energy::Motion, Axis::X) => "Motion speed X",
                (super::status_commands::Energy::Motion, Axis::Y) => "Motion speed Y",
            },
            Self::Command { symbol, .. } => command_label(symbol),
            Self::FighterStatusIs { symbol, .. } => symbol
                .split_once("_STATUS_KIND_")
                .map(|(_, suffix)| suffix)
                .unwrap_or(symbol),
            Self::AttackResultFlag { query, mask, .. } => match (query, mask) {
                (AttackResultQuery::DuringHitlag, AttackResultMask::Hit) => "Hit during hitlag",
                (AttackResultQuery::DuringHitlag, AttackResultMask::Shield) => {
                    "Shield during hitlag"
                }
                (AttackResultQuery::CurrentStatus, AttackResultMask::Hit) => {
                    "Hit in current status"
                }
                (AttackResultQuery::CurrentStatus, AttackResultMask::Shield) => {
                    "Shield in current status"
                }
            },
            Self::ArticleDeflected {
                absorbed: false, ..
            } => "Projectile reflected",
            Self::ArticleDeflected { absorbed: true, .. } => "Projectile absorbed",
            // Keep labels based on the exact reviewed symbolic selector.
            // Display prettification belongs to the authoring catalog.
            Self::ArticleExists { symbol, .. } | Self::ArticleCount { symbol, .. } => symbol,
            Self::Work {
                fighter,
                symbol,
                kind,
            } => {
                if let Some(common) = common_work(symbol) {
                    return common.label;
                }
                if fighter == "kirby" {
                    match symbol.as_str() {
                        KIRBY_COPY_CHARA => return "Copied fighter",
                        KIRBY_COPY_FLAG => return "Has copy ability",
                        _ => {}
                    }
                }
                if fighter == "pickel" && *kind == NativeKind::Int {
                    // Grade one is the shared dirt/sand/ice/wool inventory.
                    let material = match symbol.as_str() {
                        "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_GRADE_1" => Some("Dirt"),
                        "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_WOOD" => Some("Wood"),
                        "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_STONE" => Some("Stone"),
                        "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_IRON" => Some("Iron"),
                        "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_GOLD" => Some("Gold"),
                        "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_RED_STONE" => {
                            Some("Redstone")
                        }
                        "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_DIAMOND" => {
                            Some("Diamond")
                        }
                        _ => None,
                    };
                    if let Some(label) = material {
                        return label;
                    }
                }
                symbol
                    .split_once("_INSTANCE_WORK_ID_")
                    .map_or(symbol.as_str(), |(_, name)| plain_work_name(name))
            }
            Self::Unsupported(symbol) => symbol,
        }
    }

    pub const fn kind(&self) -> Option<NativeKind> {
        match self {
            Self::ParamFloat { .. }
            | Self::EnergySpeed { .. }
            | Self::CloudLimitGauge
            | Self::RobotBeamEnergy => Some(NativeKind::Float),
            Self::ParamInt { .. }
            | Self::CloudLimitClearFrame
            | Self::ArticleCount { .. }
            | Self::HeroMenuSpell
            | Self::SamusChargeCount
            | Self::SheikNeedleCount => Some(NativeKind::Int),
            Self::Command { .. }
            | Self::CloudLimitBreak
            | Self::RefletThunderSword
            | Self::AttackResultFlag { .. }
            | Self::FighterStatusIs { .. }
            | Self::ArticleExists { .. }
            | Self::ArticleDeflected { .. } => Some(NativeKind::Flag),
            Self::Fighter { value, .. } => Some(value.kind()),
            Self::OwnedObject { field, .. } => Some(field.kind()),
            Self::Work { kind, .. } => Some(*kind),
            Self::Unsupported(_) => None,
        }
    }

    pub const fn required_feature(&self) -> Option<&'static str> {
        match self {
            Self::ParamFloat { .. } | Self::ParamInt { .. } | Self::EnergySpeed { .. } => {
                Some(super::status_model::FEATURE)
            }
            Self::AttackResultFlag { .. } => Some(ATTACK_RESULT_FLAGS_FEATURE),
            Self::ArticleExists { .. } => Some(ARTICLE_PRESENCE_FEATURE),
            Self::ArticleCount { .. }
            | Self::ArticleDeflected { .. }
            | Self::OwnedObject { .. } => Some(OWNED_ARTICLES_FEATURE),
            Self::Fighter { value, .. } => Some(value.feature()),
            Self::Command { .. } => Some(COMMAND_INPUT_FEATURE),
            Self::HeroMenuSpell => Some(super::hero_menu_commands::FEATURE),
            Self::FighterStatusIs { .. } => Some(FIGHTER_STATUS_IS_FEATURE),
            _ => None,
        }
    }

    pub fn valid_for(&self, fighter: &str) -> bool {
        self.fighter() == Some(fighter) && self.is_supported()
    }

    pub fn is_supported(&self) -> bool {
        match self {
            Self::Command { fighter, symbol } => {
                self.reference_is_bounded() && command_symbols(fighter).contains(&symbol.as_str())
            }
            Self::ParamFloat {
                fighter,
                table,
                key,
            } => reviewed_param(fighter, table, key, NativeKind::Float),
            Self::ParamInt {
                fighter,
                table,
                key,
            } => reviewed_param(fighter, table, key, NativeKind::Int),
            Self::Work {
                fighter,
                symbol,
                kind,
            } => Self::work_descriptor_is_registered(fighter, symbol, *kind),
            Self::AttackResultFlag { fighter, .. } | Self::EnergySpeed { fighter, .. } => {
                self.reference_is_bounded()
                    && super::registry::fighter_kind_symbol(fighter).is_some()
            }
            Self::Fighter { fighter, value } => {
                self.reference_is_bounded()
                    && super::registry::fighter_kind_symbol(fighter).is_some()
                    && value.available_for(fighter)
            }
            Self::ArticleExists { fighter, symbol }
            | Self::ArticleCount { fighter, symbol }
            | Self::ArticleDeflected {
                fighter, symbol, ..
            } => Self::article_descriptor_is_registered(fighter, symbol),
            Self::OwnedObject {
                fighter, symbol, ..
            } => {
                self.reference_is_bounded()
                    && Self::article_descriptor_is_registered(fighter, symbol)
            }
            Self::FighterStatusIs { fighter, symbol } => {
                Self::status_descriptor_is_registered(fighter, symbol)
            }
            Self::Unsupported(_) => false,
            _ => self.kind().is_some(),
        }
    }

    /// Construct a typed reference for an exact fighter or scoped weapon Work
    /// identifier in the pinned registry.
    pub fn from_registry(fighter: &str, symbol: &str, kind: NativeKind) -> Result<Self, String> {
        if !Self::work_descriptor_is_registered(fighter, symbol, kind) {
            return Err(format!(
                "native Work reference {fighter:?}:{symbol:?}:{kind:?} is not registered"
            ));
        }
        Ok(Self::Work {
            fighter: fighter.to_owned(),
            symbol: symbol.to_owned(),
            kind,
        })
    }

    fn work_descriptor_is_registered(fighter: &str, symbol: &str, kind: NativeKind) -> bool {
        if fighter.is_empty()
            || symbol.is_empty()
            || fighter.bytes().any(|byte| byte == 0 || byte == b':')
            || symbol.bytes().any(|byte| byte == 0 || byte == b':')
            || !work_reference_is_bounded(fighter, symbol, kind)
        {
            return false;
        }
        if super::registry::projectile_work(fighter, symbol, kind.registry_kind()).is_some() {
            return true;
        }
        if let Some(common) = common_work(symbol) {
            return common.kind == kind && super::registry::fighter_kind_symbol(fighter).is_some();
        }
        let registry = registered_work_registry();
        let Some(Some(owner)) = registry.owners.get(fighter).copied() else {
            return false;
        };
        if work_symbol_suffix(symbol, kind, owner).is_none() {
            return false;
        }
        registry
            .descriptors
            .get(fighter)
            .and_then(|symbols| symbols.get(symbol))
            == Some(&kind)
    }

    fn article_descriptor_is_registered(fighter: &str, symbol: &str) -> bool {
        !fighter.is_empty()
            && !symbol.is_empty()
            && !fighter.bytes().any(|byte| byte == 0 || byte == b':')
            && !symbol.bytes().any(|byte| byte == 0 || byte == b':')
            && super::registry::fighter_kind_symbol(fighter).is_some()
            && article_reference_is_bounded(fighter, symbol)
            && super::registry::ENTRIES.iter().any(|entry| {
                entry.fighter == fighter
                    && entry.group.is_empty()
                    && entry.kind == super::registry::IdentifierKind::Article
                    && entry.symbol == symbol
                    && i32::try_from(entry.value).is_ok()
            })
    }

    fn status_descriptor_is_registered(fighter: &str, symbol: &str) -> bool {
        Self::status_value(fighter, symbol).is_some()
    }

    fn status_value(fighter: &str, symbol: &str) -> Option<i32> {
        if !named_reference_is_bounded("status_is::", fighter, symbol)
            || !registered_work_registry()
                .owners
                .get(fighter)
                .is_some_and(|owner| owner.is_some())
        {
            return None;
        }
        let entry = super::registry::resolve(
            fighter,
            symbol,
            super::registry::IdentifierKind::FighterStatus,
        )
        .ok()?;
        matches!(entry.group, "" | "common")
            .then(|| i32::try_from(entry.value).ok())
            .flatten()
    }

    /// Map registered references for old friendly slots back to their stable
    /// named variants. Other generic references keep their exact spelling.
    pub fn canonical(&self) -> Self {
        let Self::Work {
            fighter,
            symbol,
            kind,
        } = self
        else {
            return self.clone();
        };
        match (fighter.as_str(), symbol.as_str(), *kind) {
            ("cloud", "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE", NativeKind::Float) => {
                Self::CloudLimitGauge
            }
            (
                "cloud",
                "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_BREAK_CLEAR_FRAME",
                NativeKind::Int,
            ) => Self::CloudLimitClearFrame,
            ("cloud", "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK", NativeKind::Flag) => {
                Self::CloudLimitBreak
            }
            ("samus", "FIGHTER_SAMUS_INSTANCE_WORK_ID_INT_SPECIAL_N_COUNT", NativeKind::Int) => {
                Self::SamusChargeCount
            }
            ("sheik", "FIGHTER_SHEIK_INSTANCE_WORK_ID_INT_NEEDLE_COUNT", NativeKind::Int) => {
                Self::SheikNeedleCount
            }
            (
                "robot",
                "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE",
                NativeKind::Float,
            ) => Self::RobotBeamEnergy,
            (
                "reflet",
                "FIGHTER_REFLET_INSTANCE_WORK_ID_FLAG_THUNDER_SWORD_ON",
                NativeKind::Flag,
            ) => Self::RefletThunderSword,
            _ => self.clone(),
        }
    }

    /// Stable, bounded symbolic identity for persistence and diagnostics; this
    /// is also the saved JSON string. Numeric collision masks and engine
    /// identifiers are never represented.
    pub fn reference(&self) -> String {
        match self {
            Self::ParamFloat {
                fighter,
                table,
                key,
            } => format!("param_float:{fighter}:{table}:{key}"),
            Self::ParamInt {
                fighter,
                table,
                key,
            } => format!("param_int:{fighter}:{table}:{key}"),
            Self::EnergySpeed {
                fighter,
                energy,
                axis,
            } => format!("energy_speed:{fighter}:{energy:?}:{axis:?}"),
            Self::AttackResultFlag {
                fighter,
                query,
                mask,
            } => {
                format!(
                    "attack_result_flag:{fighter}:{}:{}",
                    query.as_str(),
                    mask.as_str()
                )
            }
            Self::ArticleExists { fighter, symbol } => format!("article_exists:{fighter}:{symbol}"),
            Self::ArticleCount { fighter, symbol } => format!("article_count:{fighter}:{symbol}"),
            Self::ArticleDeflected {
                fighter,
                symbol,
                absorbed,
            } => {
                format!(
                    "article_{}:{fighter}:{symbol}",
                    if *absorbed { "absorbed" } else { "reflected" }
                )
            }
            Self::FighterStatusIs { fighter, symbol } => format!("status_is:{fighter}:{symbol}"),
            Self::Fighter { fighter, value } => format!("{}:{fighter}", value.key()),
            Self::OwnedObject {
                fighter,
                symbol,
                field,
            } => format!("owned_object:{fighter}:{symbol}:{}", field.key()),
            Self::Command { fighter, symbol } => format!("command:{fighter}:{symbol}"),
            Self::Work {
                fighter,
                symbol,
                kind,
            } => format!("work_{}:{fighter}:{symbol}", kind.as_str()),
            Self::Unsupported(symbol) => symbol.clone(),
            named => NAMED
                .iter()
                .find(|(input, ..)| input == named)
                .map_or_else(String::new, |(_, key, _)| (*key).into()),
        }
    }

    /// Parse a saved reference. Anything not admitted by the closed catalog
    /// stays `Unsupported` with its exact spelling, so it survives a round trip.
    pub fn from_reference(value: &str) -> Self {
        if let Some((input, ..)) = NAMED.iter().find(|(_, key, _)| *key == value) {
            return input.clone();
        }
        let parts = value.split(':').collect::<Vec<_>>();
        let text = |index: usize| parts[index].to_owned();
        let source = match (parts[0], parts.len()) {
            (_, _) if value.contains('\0') => None,
            (key, 2) if FighterValue::from_key(key).is_some() => {
                FighterValue::from_key(key).map(|value| Self::Fighter {
                    fighter: text(1),
                    value,
                })
            }
            ("command", 3) => Some(Self::Command {
                fighter: text(1),
                symbol: text(2),
            }),
            ("param_float", 4) => Some(Self::ParamFloat {
                fighter: text(1),
                table: text(2),
                key: text(3),
            }),
            ("param_int", 4) => Some(Self::ParamInt {
                fighter: text(1),
                table: text(2),
                key: text(3),
            }),
            ("energy_speed", 4) => {
                let energy = ENERGIES
                    .into_iter()
                    .find(|energy| format!("{energy:?}") == parts[2]);
                let axis = [Axis::X, Axis::Y]
                    .into_iter()
                    .find(|axis| format!("{axis:?}") == parts[3]);
                energy.zip(axis).map(|(energy, axis)| Self::EnergySpeed {
                    fighter: text(1),
                    energy,
                    axis,
                })
            }
            ("attack_result_flag", 4) => {
                let query = [
                    AttackResultQuery::DuringHitlag,
                    AttackResultQuery::CurrentStatus,
                ]
                .into_iter()
                .find(|q| q.as_str() == parts[2]);
                let mask = [AttackResultMask::Hit, AttackResultMask::Shield]
                    .into_iter()
                    .find(|m| m.as_str() == parts[3]);
                query.zip(mask).map(|(query, mask)| Self::AttackResultFlag {
                    fighter: text(1),
                    query,
                    mask,
                })
            }
            (kind @ ("article_reflected" | "article_absorbed"), 3) => {
                Some(Self::ArticleDeflected {
                    fighter: text(1),
                    symbol: text(2),
                    absorbed: kind == "article_absorbed",
                })
            }
            ("owned_object", 4) => ObjectField::from_key(parts[3]).map(|field| Self::OwnedObject {
                fighter: text(1),
                symbol: text(2),
                field,
            }),
            ("article_count", 3) => Some(Self::ArticleCount {
                fighter: text(1),
                symbol: text(2),
            }),
            ("article_exists", 3) => Some(Self::ArticleExists {
                fighter: text(1),
                symbol: text(2),
            }),
            ("status_is", 3) => Some(Self::FighterStatusIs {
                fighter: text(1),
                symbol: text(2),
            }),
            (kind, 3) if kind.starts_with("work_") => {
                [NativeKind::Float, NativeKind::Int, NativeKind::Flag]
                    .into_iter()
                    .find(|native| kind == format!("work_{}", native.as_str()))
                    .map(|kind| Self::Work {
                        fighter: text(1),
                        symbol: text(2),
                        kind,
                    })
            }
            _ => None,
        };
        source
            .filter(Self::is_supported)
            .unwrap_or_else(|| Self::Unsupported(value.to_owned()))
    }

    /// Catalog for one exact owner and native type, with named-friendly entries
    /// before remaining registry-backed generic entries.
    pub fn catalog_for(fighter: &str, kind: NativeKind) -> Vec<Self> {
        let mut result = Vec::new();
        if kind == NativeKind::Flag {
            result.extend(command_symbols(fighter).iter().map(|symbol| Self::Command {
                fighter: fighter.into(),
                symbol: (*symbol).into(),
            }));
        }
        for &(owner, table, key, native_kind) in REVIEWED_PARAMS {
            if owner == fighter && native_kind == kind {
                result.push(if kind == NativeKind::Float {
                    Self::ParamFloat {
                        fighter: fighter.into(),
                        table: table.into(),
                        key: key.into(),
                    }
                } else {
                    Self::ParamInt {
                        fighter: fighter.into(),
                        table: table.into(),
                        key: key.into(),
                    }
                });
            }
        }
        let known_fighter = super::registry::fighter_kind_symbol(fighter).is_some();
        if kind == NativeKind::Float && known_fighter {
            for energy in ENERGIES {
                for axis in [Axis::X, Axis::Y] {
                    result.push(Self::EnergySpeed {
                        fighter: fighter.into(),
                        energy,
                        axis,
                    });
                }
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for source in Self::catalog()
            .iter()
            .filter(|source| source.fighter() == Some(fighter) && source.kind() == Some(kind))
        {
            let canonical = source.canonical();
            if seen.insert(canonical.clone()) {
                result.push(canonical);
            }
        }
        for entry in super::registry::ENTRIES.iter().filter(|entry| {
            entry.fighter == fighter
                && (entry.group.is_empty() || entry.symbol.starts_with("WEAPON_"))
        }) {
            if NativeKind::from_registry_kind(entry.kind) != Some(kind) {
                continue;
            }
            let Ok(source) = Self::from_registry(fighter, entry.symbol, kind) else {
                continue;
            };
            let source = source.canonical();
            if source.is_supported() && seen.insert(source.clone()) {
                result.push(source);
            }
        }
        if kind == NativeKind::Flag && known_fighter {
            for (query, mask) in [
                (AttackResultQuery::DuringHitlag, AttackResultMask::Hit),
                (AttackResultQuery::DuringHitlag, AttackResultMask::Shield),
                (AttackResultQuery::CurrentStatus, AttackResultMask::Hit),
                (AttackResultQuery::CurrentStatus, AttackResultMask::Shield),
            ] {
                result.push(Self::AttackResultFlag {
                    fighter: fighter.to_owned(),
                    query,
                    mask,
                });
            }
            for entry in super::registry::ENTRIES.iter().filter(|entry| {
                entry.fighter == fighter
                    && entry.group.is_empty()
                    && entry.kind == super::registry::IdentifierKind::Article
            }) {
                let source = Self::ArticleExists {
                    fighter: fighter.to_owned(),
                    symbol: entry.symbol.to_owned(),
                };
                if source.is_supported() && seen.insert(source.clone()) {
                    result.push(source);
                }
            }
        }
        for entry in super::registry::ENTRIES.iter().filter(|entry| {
            entry.fighter == fighter
                && entry.group.is_empty()
                && entry.kind == super::registry::IdentifierKind::Article
        }) {
            result.extend(
                ObjectField::ALL
                    .into_iter()
                    .filter(|field| field.kind() == kind)
                    .map(|field| Self::OwnedObject {
                        fighter: fighter.into(),
                        symbol: entry.symbol.into(),
                        field,
                    })
                    .filter(Self::is_supported),
            );
        }
        if known_fighter {
            result.extend(
                COMMON_WORK
                    .iter()
                    .filter(|row| row.kind == kind)
                    .map(|row| Self::Work {
                        fighter: fighter.into(),
                        symbol: row.symbol.into(),
                        kind,
                    }),
            );
            result.extend(
                FighterValue::ALL
                    .into_iter()
                    .filter(|value| value.kind() == kind && value.available_for(fighter))
                    .map(|value| Self::fighter_value(fighter, value)),
            );
        }
        if kind == NativeKind::Flag {
            for entry in super::registry::ENTRIES.iter().filter(|entry| {
                entry.kind == super::registry::IdentifierKind::FighterStatus
                    && super::registry::owned_by(entry, fighter)
            }) {
                let source = Self::FighterStatusIs {
                    fighter: fighter.to_owned(),
                    symbol: entry.symbol.to_owned(),
                };
                if source.is_supported() && seen.insert(source.clone()) {
                    result.push(source);
                }
            }
        }
        result
    }

    /// Original friendly aliases. Owner-dependent sources are added by
    /// `catalog_for`; validators check the complete closed selector model.
    pub fn catalog() -> &'static [NativeInput] {
        const CATALOG: &[NativeInput] = &[
            NativeInput::CloudLimitGauge,
            NativeInput::CloudLimitClearFrame,
            NativeInput::CloudLimitBreak,
            NativeInput::SamusChargeCount,
            NativeInput::SheikNeedleCount,
            NativeInput::RobotBeamEnergy,
            NativeInput::RefletThunderSword,
            NativeInput::HeroMenuSpell,
        ];
        CATALOG
    }

    pub fn reference_is_bounded(&self) -> bool {
        let short = || self.reference().len() <= Self::MAX_REFERENCE_BYTES;
        match self {
            Self::ParamFloat {
                fighter,
                table,
                key,
            }
            | Self::ParamInt {
                fighter,
                table,
                key,
            } => [fighter, table, key].iter().all(|v| text_ok(v)) && short(),
            Self::EnergySpeed { fighter, .. }
            | Self::AttackResultFlag { fighter, .. }
            | Self::Fighter { fighter, .. }
            | Self::Command { fighter, .. } => text_ok(fighter) && short(),
            Self::FighterStatusIs { fighter, symbol } => {
                named_reference_is_bounded("status_is::", fighter, symbol)
            }
            Self::Work {
                fighter,
                symbol,
                kind,
            } => {
                text_ok(fighter)
                    && text_ok(symbol)
                    && work_reference_is_bounded(fighter, symbol, *kind)
            }
            Self::ArticleExists { fighter, symbol }
            | Self::ArticleCount { fighter, symbol }
            | Self::ArticleDeflected {
                fighter, symbol, ..
            } => {
                text_ok(fighter) && text_ok(symbol) && article_reference_is_bounded(fighter, symbol)
            }
            Self::OwnedObject {
                fighter, symbol, ..
            } => text_ok(fighter) && text_ok(symbol) && short(),
            Self::Unsupported(value) => {
                !value.is_empty()
                    && value.len() <= Self::MAX_REFERENCE_BYTES
                    && !value.bytes().any(|byte| byte == 0)
            }
            _ => self.symbol().len() <= Self::MAX_REFERENCE_BYTES,
        }
    }
}

impl NativeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Float => "float",
            Self::Int => "int",
            Self::Flag => "flag",
        }
    }

    fn work_prefix(self) -> Option<&'static str> {
        match self {
            Self::Float => Some("FLOAT"),
            Self::Int => Some("INT"),
            Self::Flag => Some("FLAG"),
        }
    }

    fn from_registry_kind(kind: super::registry::IdentifierKind) -> Option<Self> {
        match kind {
            super::registry::IdentifierKind::WorkFloat => Some(Self::Float),
            super::registry::IdentifierKind::WorkInt => Some(Self::Int),
            super::registry::IdentifierKind::WorkFlag => Some(Self::Flag),
            _ => None,
        }
    }
}

/// Immutable index of native Work descriptors admitted by the generated table.
/// Its keys come only from static registry data; dynamic references never grow it.
struct NativeWorkRegistry<'a> {
    // Option preserves fighter_kind_symbol()'s first-match behavior even if its
    // first row has a malformed symbol prefix.
    owners: BTreeMap<&'a str, Option<&'a str>>,
    descriptors: BTreeMap<&'a str, BTreeMap<&'a str, NativeKind>>,
}

fn registered_work_registry() -> &'static NativeWorkRegistry<'static> {
    static REGISTRY: OnceLock<NativeWorkRegistry<'static>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        build_native_work_registry(super::registry::ENTRIES, super::registry::FIGHTER_KINDS)
    })
}

fn build_native_work_registry<'a>(
    entries: &'a [super::registry::RegistryEntry],
    fighter_kinds: &'a [super::registry::FighterKindEntry],
) -> NativeWorkRegistry<'a> {
    let mut owners = BTreeMap::new();
    for fighter in fighter_kinds {
        owners
            .entry(fighter.fighter)
            .or_insert_with(|| fighter.symbol.strip_prefix("FIGHTER_KIND_"));
    }

    // Count values in precisely the namespace checked by the former scan.
    let mut value_counts = BTreeMap::<(&str, u8, u32), usize>::new();
    for entry in entries.iter().filter(|entry| entry.group.is_empty()) {
        let Some(kind) = NativeKind::from_registry_kind(entry.kind) else {
            continue;
        };
        *value_counts
            .entry((entry.fighter, native_kind_index(kind), entry.value))
            .or_default() += 1;
    }

    // resolve() selects the first exact fighter/symbol row before checking kind.
    let mut first_by_name = BTreeMap::new();
    for entry in entries {
        first_by_name
            .entry((entry.fighter, entry.symbol))
            .or_insert(entry);
    }

    let mut descriptors = BTreeMap::<&str, BTreeMap<&str, NativeKind>>::new();
    for ((fighter, symbol), entry) in first_by_name {
        if !entry.group.is_empty() {
            continue;
        }
        let Some(kind) = NativeKind::from_registry_kind(entry.kind) else {
            continue;
        };
        let Some(Some(owner)) = owners.get(fighter).copied() else {
            continue;
        };
        if work_symbol_suffix(symbol, kind, owner).is_none()
            || value_counts.get(&(fighter, native_kind_index(kind), entry.value)) != Some(&1)
        {
            continue;
        }
        descriptors.entry(fighter).or_default().insert(symbol, kind);
    }

    NativeWorkRegistry {
        owners,
        descriptors,
    }
}

fn native_kind_index(kind: NativeKind) -> u8 {
    match kind {
        NativeKind::Float => 0,
        NativeKind::Int => 1,
        NativeKind::Flag => 2,
    }
}

fn work_symbol_suffix<'a>(symbol: &'a str, kind: NativeKind, owner: &str) -> Option<&'a str> {
    let suffix = symbol
        .strip_prefix("FIGHTER_")?
        .strip_prefix(owner)?
        .strip_prefix("_INSTANCE_WORK_ID_")?
        .strip_prefix(kind.work_prefix()?)?
        .strip_prefix('_')?;
    if suffix.is_empty()
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        || matches!(
            suffix,
            "START" | "TERM" | "END" | "MAX" | "INVALID" | "NONE"
        )
    {
        return None;
    }
    Some(suffix)
}

fn work_reference_is_bounded(fighter: &str, symbol: &str, kind: NativeKind) -> bool {
    let Some(length) = 5usize
        .checked_add(kind.as_str().len())
        .and_then(|length| length.checked_add(1))
        .and_then(|length| length.checked_add(fighter.len()))
        .and_then(|length| length.checked_add(1))
        .and_then(|length| length.checked_add(symbol.len()))
    else {
        return false;
    };
    length <= NativeInput::MAX_REFERENCE_BYTES
}

fn named_reference_is_bounded(prefix: &str, fighter: &str, symbol: &str) -> bool {
    !fighter.is_empty()
        && !symbol.is_empty()
        && !fighter.bytes().any(|byte| byte == 0 || byte == b':')
        && !symbol.bytes().any(|byte| byte == 0 || byte == b':')
        && prefix
            .len()
            .checked_add(fighter.len())
            .and_then(|length| length.checked_add(symbol.len()))
            .is_some_and(|length| length <= NativeInput::MAX_REFERENCE_BYTES)
}

/// Fighters whose partner is a separate live object: Nana (popo) and Luma (rosetta).
pub fn has_partner(fighter: &str) -> bool {
    matches!(fighter, "popo" | "rosetta")
}

fn article_reference_is_bounded(fighter: &str, symbol: &str) -> bool {
    "article_exists::"
        .len()
        .checked_add(fighter.len())
        .and_then(|length| length.checked_add(symbol.len()))
        .is_some_and(|length| length <= NativeInput::MAX_REFERENCE_BYTES)
}

impl Serialize for NativeInput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.reference())
    }
}

impl<'de> Deserialize<'de> for NativeInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = std::borrow::Cow::<str>::deserialize(deserializer)?;
        if value.is_empty() || value.len() > NativeInput::MAX_REFERENCE_BYTES {
            return Err(de::Error::custom(
                "native input reference is empty or too long",
            ));
        }
        Ok(NativeInput::from_reference(&value))
    }
}

/// An integer expression source: a saved reference, or the legacy article-count
/// object `{"symbol": ..}` whose owner the registry supplies.
pub fn int_source<'de, D: Deserializer<'de>>(deserializer: D) -> Result<NativeInput, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Reference(NativeInput),
        Article { symbol: String },
    }
    Ok(match Wire::deserialize(deserializer)? {
        Wire::Reference(input) => input,
        Wire::Article { symbol } => NativeInput::article_count(&symbol),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SituationKind {
    Ground,
    Air,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhaseInput {
    pub fighter: String,
    pub status: i32,
    pub status_frame: i32,
    pub motion_frame: f32,
    /// MotionModule::motion_kind (a Hash40).
    pub motion: u64,
    pub situation: Option<SituationKind>,
}

/// One immutable, per-dispatch set of native observations.
///
/// This value is never persisted with behavior state. An absent observation is
/// an evaluation error, not zero or a cached reading.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NativeSnapshot {
    values: BTreeMap<NativeInput, NativeObservation>,
    damage: Option<DamageInput>,
    pub controller: Option<ControllerInput>,
    pub phase: Option<PhaseInput>,
    pub cancel_enabled: Option<bool>,
    pub term_enabled: BTreeMap<String, bool>,
    pub motion_ended: Option<bool>,
    pub situation_changed: Option<bool>,
    pub stopped: bool,
    pub article_event: Option<(bool, String)>,
    pub(crate) random: std::rc::Rc<std::cell::Cell<u64>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)] // Constructed by the plugin sampler; offline host reads are unavailable.
pub enum NativeObservation {
    Float(f32),
    Int(i32),
    Flag(bool),
}

impl NativeSnapshot {
    pub fn empty() -> Self {
        Self {
            values: BTreeMap::new(),
            damage: None,
            controller: None,
            phase: None,
            cancel_enabled: None,
            term_enabled: BTreeMap::new(),
            motion_ended: None,
            situation_changed: None,
            stopped: false,
            article_event: None,
            random: Default::default(),
        }
    }

    /// Only interpreter root entry points install contact context. Named and
    /// unrelated hook roots strip it before evaluating any expressions.
    #[allow(dead_code)] // Plugin/pure interpreter root entry point.
    pub(crate) fn with_damage(&self, input: DamageInput) -> Self {
        Self {
            values: self.values.clone(),
            damage: Some(input),
            controller: self.controller.clone(),
            phase: self.phase.clone(),
            cancel_enabled: self.cancel_enabled,
            term_enabled: self.term_enabled.clone(),
            motion_ended: self.motion_ended,
            situation_changed: self.situation_changed,
            stopped: self.stopped,
            article_event: self.article_event.clone(),
            random: self.random.clone(),
        }
    }

    pub(crate) fn without_damage(&self) -> std::borrow::Cow<'_, Self> {
        if self.damage.is_none() {
            std::borrow::Cow::Borrowed(self)
        } else {
            std::borrow::Cow::Owned(Self {
                values: self.values.clone(),
                damage: None,
                controller: self.controller.clone(),
                phase: self.phase.clone(),
                cancel_enabled: self.cancel_enabled,
                term_enabled: self.term_enabled.clone(),
                motion_ended: self.motion_ended,
                situation_changed: self.situation_changed,
                stopped: self.stopped,
                article_event: self.article_event.clone(),
                random: self.random.clone(),
            })
        }
    }

    pub(crate) fn damage_is_projectile(&self) -> Result<bool, String> {
        self.damage.map(|input| input.projectile).ok_or_else(|| {
            "projectile classification is unavailable outside its owning damage root".into()
        })
    }

    pub(crate) fn pending_damage(&self) -> Result<f32, String> {
        self.damage.and_then(DamageInput::pending).ok_or_else(|| {
            "pending damage is unavailable for this contact or outside its owning damage root"
                .into()
        })
    }

    /// Compare a symbolic selector against the owning receiver's historical
    /// contact status. Current native observations are deliberately irrelevant.
    #[allow(dead_code)] // Scoped authored predicate integration follows the transient adapter.
    pub(crate) fn damage_status_is(&self, selector: &NativeInput) -> Result<bool, String> {
        let NativeInput::FighterStatusIs { fighter, symbol } = selector else {
            return Err(
                "damage status comparison requires an exact fighter-status selector".into(),
            );
        };
        let expected = NativeInput::status_value(fighter, symbol)
            .ok_or_else(|| "damage status selector is not registered for its fighter".to_owned())?;
        let (kind, status) = self
            .damage
            .and_then(|damage| damage.receiver_status)
            .ok_or_else(|| {
                "damage receiver status is unavailable outside its owning contact root".to_owned()
            })?;
        let expected_kind = super::registry::FIGHTER_KINDS
            .iter()
            .find(|entry| entry.fighter == fighter)
            .and_then(|entry| i32::try_from(entry.value).ok());
        if expected_kind != Some(kind) {
            return Err("damage status selector belongs to a different receiver fighter".into());
        }
        Ok(status == expected)
    }

    // Constructed by the plugin's game-thread sampler; the desktop normally
    // evaluates with no native observations rather than inventing a reading.

    /// Build an all-or-nothing typed snapshot from one complete sampled batch.
    #[allow(dead_code)] // Shared plugin API; the desktop never fabricates native observations.
    pub fn from_observations(
        sources: &[NativeInput],
        observations: &[NativeObservation],
    ) -> Result<Self, String> {
        if sources.len() > MAX_NATIVE_SNAPSHOT_SOURCES {
            return Err(format!(
                "native input sample exceeds {MAX_NATIVE_SNAPSHOT_SOURCES} sources"
            ));
        }
        if sources.len() != observations.len() {
            return Err("native input sample count does not match its declarations".into());
        }
        let mut snapshot = Self::empty();
        let mut seen = std::collections::BTreeSet::new();
        for (source, observation) in sources.iter().zip(observations) {
            let owner = source.fighter().unwrap_or_default();
            let canonical = source.canonical();
            if !source.valid_for(owner) || !seen.insert(canonical.clone()) {
                return Err(format!(
                    "native input {source:?} is unsupported or duplicated"
                ));
            }
            let non_negative = matches!(source, NativeInput::ArticleCount { .. })
                || matches!(source, NativeInput::Fighter { value, .. } if value.non_negative());
            match (source.kind(), observation) {
                (Some(NativeKind::Float), NativeObservation::Float(value)) => {
                    validate_float(*value, source)?;
                    if non_negative && *value < 0.0 {
                        return Err(format!("native {} cannot be negative", source.label()));
                    }
                }
                (Some(NativeKind::Int), NativeObservation::Int(value)) => {
                    if non_negative && *value < 0 {
                        return Err(format!("native {} cannot be negative", source.label()));
                    }
                    validate_int(*value, &format!("native {}", source.label()))?;
                }
                (Some(NativeKind::Flag), NativeObservation::Flag(_)) => {}
                _ => return Err(format!("native input {source:?} returned the wrong type")),
            }
            snapshot.values.insert(canonical, *observation);
        }
        Ok(snapshot)
    }

    pub fn read(&self, source: &NativeInput) -> Result<f32, String> {
        if source.kind() != Some(NativeKind::Float) || !source.is_supported() {
            return Err(format!("native input {source:?} is not a supported float"));
        }
        let Some(NativeObservation::Float(value)) = self.values.get(&source.canonical()) else {
            return Err(format!(
                "native input {source:?} is unavailable or mistyped"
            ));
        };
        validate_float(*value, source)?;
        Ok(*value)
    }

    pub fn read_int(&self, source: &NativeInput) -> Result<i32, String> {
        if source.kind() != Some(NativeKind::Int) || !source.is_supported() {
            return Err(format!(
                "native input {source:?} is not a supported integer"
            ));
        }
        let Some(NativeObservation::Int(value)) = self.values.get(&source.canonical()) else {
            return Err(format!(
                "native input {source:?} is unavailable or mistyped"
            ));
        };
        validate_int(*value, &format!("native {}", source.label()))?;
        Ok(*value)
    }

    pub fn read_flag(&self, source: &NativeInput) -> Result<bool, String> {
        if source.kind() != Some(NativeKind::Flag) || !source.is_supported() {
            return Err(format!("native input {source:?} is not a supported flag"));
        }
        match self.values.get(&source.canonical()) {
            Some(NativeObservation::Flag(value)) => Ok(*value),
            _ => Err(format!(
                "native input {source:?} is unavailable or mistyped"
            )),
        }
    }

    pub fn require(&self, source: &NativeInput) -> Result<(), String> {
        match source.kind() {
            Some(NativeKind::Float) => self.read(source).map(|_| ()),
            Some(NativeKind::Int) => self.read_int(source).map(|_| ()),
            Some(NativeKind::Flag) => self.read_flag(source).map(|_| ()),
            None => Err(format!("native input {source:?} is unsupported")),
        }
    }
}

fn validate_float(value: f32, source: &NativeInput) -> Result<(), String> {
    if matches!(
        source,
        NativeInput::Fighter {
            value: FighterValue::Facing,
            ..
        }
    ) && !matches!(value, -1.0 | 1.0)
    {
        return Err("fighter facing must be -1 or 1".into());
    }
    if !value.is_finite() {
        return Err(format!("native input {source:?} is not finite"));
    }
    if f64::from(value).abs() > MAX_ABS_NATIVE_VALUE {
        return Err(format!(
            "native input {source:?} is outside the live numeric range"
        ));
    }
    Ok(())
}

fn validate_int(value: i32, label: &str) -> Result<(), String> {
    if i64::from(value).abs() > MAX_ABS_NATIVE_VALUE as i64 {
        return Err(format!("{label} is outside the live numeric range"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputButton {
    Attack,
    Special,
    Jump,
    Guard,
    Grab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ButtonEdge {
    Held,
    Pressed,
    Released,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControllerInput {
    pub buttons: BTreeMap<(InputButton, ButtonEdge), bool>,
    pub stick_x: f32,
    pub stick_y: f32,
    pub facing: f32,
}

pub(crate) fn random_int_valid(min: i32, max: i32) -> bool {
    min <= max
        && i64::from(max) - i64::from(min) <= 1_000_000
        && i64::from(min).abs() <= MAX_ABS_NATIVE_VALUE as i64
        && i64::from(max).abs() <= MAX_ABS_NATIVE_VALUE as i64
}
pub(crate) fn random_float_valid(min: f32, max: f32) -> bool {
    min.is_finite()
        && max.is_finite()
        && min <= max
        && f64::from(min).abs() <= MAX_ABS_NATIVE_VALUE
        && f64::from(max).abs() <= MAX_ABS_NATIVE_VALUE
}

impl NativeInput {
    pub fn projectile_article(&self) -> Option<&'static str> {
        let Self::Work {
            fighter,
            symbol,
            kind,
        } = self
        else {
            return None;
        };
        super::registry::projectile_work(fighter, symbol, kind.registry_kind())
            .map(|(_, article)| article.symbol)
    }

    pub fn validate_write(&self, fighter: &str) -> Result<(), String> {
        let kind = self
            .kind()
            .ok_or("native write requires a typed Work field")?;
        if self.fighter() != Some(fighter) {
            return Err("native write field belongs to another fighter".into());
        }
        Self::from_registry(fighter, self.symbol(), kind)?;
        if let Some(common) = common_work(self.symbol()) {
            return common
                .write
                .map(|_| ())
                .ok_or_else(|| format!("{} is read-only", common.label));
        }
        use super::resource_commands::{Writer, RESOURCES};
        if RESOURCES
            .iter()
            .any(|r| r.writer != Writer::Work && r.fighter == fighter && r.symbol == self.symbol())
        {
            return Err("native resource requires a Resource helper action".into());
        }
        // Kirby's copy kind, slot and flag must change together with the hat,
        // model and neutral special; a bare Work write desyncs them.
        if fighter == "kirby"
            && matches!(
                self.symbol(),
                KIRBY_COPY_CHARA | KIRBY_COPY_FLAG | KIRBY_COPY_SLOT
            )
        {
            return Err("Kirby's copy ability is read-only".into());
        }
        // Limit's gauge, timer and active flag are one native resource lifecycle.
        if fighter == "cloud" && self.symbol().contains("LIMIT") {
            return Err("native Limit fields require the Limit resource action".into());
        }
        Ok(())
    }
}

impl NativeInput {
    /// Reject an authored constant outside a bounded write; the plugin clamps computed values.
    pub fn validate_write_int(&self, value: i32) -> Result<(), String> {
        match common_work(self.symbol()).and_then(|common| Some((common.label, common.write?))) {
            Some((label, (min, max))) if !(min..=max).contains(&value) => {
                Err(format!("{label} must be {min}..={max}"))
            }
            _ => Ok(()),
        }
    }
}

/// Kirby's copied fighter kind (FIGHTER_KIND_NONE, -1, without an ability).
const KIRBY_COPY_CHARA: &str = "FIGHTER_KIRBY_INSTANCE_WORK_ID_INT_COPY_CHARA";
const KIRBY_COPY_SLOT: &str = "FIGHTER_KIRBY_INSTANCE_WORK_ID_INT_COPY_CHARA_SLOT_NO";
const KIRBY_COPY_FLAG: &str = "FIGHTER_KIRBY_INSTANCE_WORK_ID_FLAG_COPY";

/// Smash's Hash40: CRC32 of the lowercase name with its length in the top byte.
pub fn hash40(name: &str) -> u64 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in name.bytes() {
        crc ^= u32::from(byte.to_ascii_lowercase());
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    u64::from(!crc) | (name.len() as u64) << 32
}

/// Mesh names are lowercase Hash40 labels of the model's mesh without its
/// `_VIS_O_OBJShape` suffix: `edge_wing_VIS_O_OBJShape` is `edge_wing`.
pub fn validate_mesh_name(mesh: &str) -> Result<(), String> {
    if motion_name_valid(mesh) {
        Ok(())
    } else {
        Err(format!(
            "mesh name {mesh:?} must be a lowercase name of up to 64 letters, digits or _"
        ))
    }
}

/// Motion names are short lowercase Hash40 labels like `throw_f`.
pub fn motion_name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl PhaseInput {
    pub fn validate(&self) -> Result<(), String> {
        if super::registry::fighter_kind_symbol(&self.fighter).is_none()
            || self.status_frame < 0
            || !self.motion_frame.is_finite()
            || self.motion_frame.abs() > 1_000_000.0
        {
            return Err("invalid current status/motion snapshot".into());
        }
        Ok(())
    }
    pub fn in_motion(&self, motion: &str) -> Result<bool, String> {
        self.validate()?;
        Ok(self.motion == hash40(motion))
    }
    pub fn in_status(&self, symbol: &str) -> Result<bool, String> {
        self.validate()?;
        let id = super::registry::resolve(
            &self.fighter,
            symbol,
            super::registry::IdentifierKind::FighterStatus,
        )
        .map_err(|e| e.to_string())?;
        Ok(i64::from(self.status) == i64::from(id.value))
    }
}

/// Main-hand presence sampled for one fighter lifetime and configuration.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct ItemState<I> {
    pub identity: I,
    pub revision: u64,
    pub holding: bool,
}
#[allow(dead_code)]
impl<I: PartialEq> ItemState<I> {
    /// Pickup includes creation; release includes throw, drop and deletion.
    /// A new lifetime/configuration establishes a baseline without events.
    pub fn edges(&self, next: &Self) -> [bool; 2] {
        if self.identity != next.identity || self.revision != next.revision {
            return [false; 2];
        }
        [!self.holding && next.holding, self.holding && !next.holding]
    }
}

/// Consecutive capture observations for one identity and configuration epoch.
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)] // Constructed by the plugin sampler and offline event fixtures.
pub struct CaptureState {
    pub holding: bool,
    pub captured: bool,
    pub throwing: bool,
}

#[allow(dead_code)]
impl CaptureState {
    /// An interrupted throw or ordinary grab escape is not a throw release.
    pub fn edges(self, next: Self) -> [bool; 3] {
        [
            !self.holding && next.holding,
            self.holding && !next.holding && self.throwing && next.throwing,
            !self.captured && next.captured,
        ]
    }
}

impl NativeKind {
    fn registry_kind(self) -> super::registry::IdentifierKind {
        match self {
            Self::Float => super::registry::IdentifierKind::WorkFloat,
            Self::Int => super::registry::IdentifierKind::WorkInt,
            Self::Flag => super::registry::IdentifierKind::WorkFlag,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_table_additions_are_fighter_specific() {
        assert!(
            super::command_symbols("ken").contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_ATTACK_COMMAND1")
        );
        assert!(super::command_symbols("dolly")
            .contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N2_COMMAND"));
        assert!(super::command_symbols("demon")
            .contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623STRICT"));
        assert!(
            super::command_symbols("demon").contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623ALONG")
        );
        assert!(
            super::command_symbols("demon").contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623BLONG")
        );
        assert!(
            super::command_symbols("demon").contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6AB")
        );
        assert!(
            super::command_symbols("demon").contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_323CATCH")
        );
        assert!(super::command_symbols("demon").contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_1"));
        assert!(super::command_symbols("demon").contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_3"));
        assert!(super::command_symbols("demon").contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_7"));
        assert!(super::command_symbols("demon").contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_9"));
        assert!(super::command_symbols("demon")
            .contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI_COMMAND"));
        assert!(
            !super::command_symbols("ryu").contains(&"FIGHTER_PAD_CMD_CAT4_FLAG_ATTACK_COMMAND1")
        );
        for fighter in ["ryu", "ken", "dolly", "demon"] {
            for symbol in super::command_symbols(fighter) {
                assert_ne!(super::command_label(symbol), *symbol);
                let plugin = include_str!(
                    "../../plugins/slight_replica/src/slight/gimmicks/native_values.rs"
                );
                assert!(
                    plugin
                        .split_whitespace()
                        .collect::<String>()
                        .contains(&format!("\"{symbol}\"=>")),
                    "the plugin cannot read {symbol}"
                );
            }
        }
    }

    use super::*;

    #[test]
    fn opponent_values_round_trip_and_stay_read_only() {
        for (value, reference, kind) in [
            (
                FighterValue::OpponentPresent,
                "opponent_present:ryu",
                NativeKind::Flag,
            ),
            (
                FighterValue::OpponentDamage,
                "opponent_damage:ryu",
                NativeKind::Float,
            ),
            (
                FighterValue::OpponentDx,
                "opponent_dx:ryu",
                NativeKind::Float,
            ),
            (
                FighterValue::OpponentDxForward,
                "opponent_dx_forward:ryu",
                NativeKind::Float,
            ),
            (
                FighterValue::OpponentDy,
                "opponent_dy:ryu",
                NativeKind::Float,
            ),
        ] {
            let input = NativeInput::fighter_value("ryu", value);
            assert_eq!(input.reference(), reference);
            assert_eq!(NativeInput::from_reference(reference), input);
            assert_eq!(
                serde_json::from_str::<NativeInput>(&format!("{reference:?}")).unwrap(),
                input
            );
            assert_eq!(input.required_feature(), Some(OPPONENT_FEATURE));
            assert!(NativeInput::catalog_for("ryu", kind).contains(&input));
            assert!(input.validate_write("ryu").is_err());
        }
        assert!(matches!(
            NativeInput::from_reference("opponent_dx:unknown"),
            NativeInput::Unsupported(_)
        ));
        assert!(matches!(
            NativeInput::from_reference("opponent_dx:ryu:extra"),
            NativeInput::Unsupported(_)
        ));
        assert_eq!(
            serde_json::to_string(&Facing::Opponent).unwrap(),
            "\"Opponent\""
        );
        assert!(MovementAction::SetFacing(Facing::Opponent)
            .validate()
            .is_ok());
        assert!(MovementAction::SetFacing(Facing::Opponent)
            .validate_for_article()
            .is_err());
        let source = NativeSnapshot::from_observations(
            &[NativeInput::fighter_value(
                "ryu",
                FighterValue::OpponentDamage,
            )],
            &[NativeObservation::Float(-1.0)],
        );
        assert!(source.is_err(), "negative damage is a sampler fault");
    }

    #[test]
    fn common_work_is_admitted_for_every_fighter_and_writes_only_the_allowlist() {
        for fighter in ["mario", "kirby"] {
            for row in COMMON_WORK {
                let reference = format!("work_{}:{fighter}:{}", row.kind.as_str(), row.symbol);
                let input = NativeInput::from_reference(&reference);
                assert_eq!(
                    input,
                    NativeInput::Work {
                        fighter: fighter.into(),
                        symbol: row.symbol.into(),
                        kind: row.kind
                    },
                    "{reference}"
                );
                assert_eq!(input.reference(), reference);
                assert_eq!(input.label(), row.label);
                assert!(NativeInput::catalog_for(fighter, row.kind).contains(&input));
                assert_eq!(
                    input.validate_write(fighter).is_ok(),
                    row.write.is_some(),
                    "{reference}"
                );
                assert!(
                    input.validate_write("cloud").is_err(),
                    "another fighter's value"
                );
            }
        }
        let writable: Vec<_> = COMMON_WORK
            .iter()
            .filter(|row| row.write.is_some())
            .map(|row| row.symbol)
            .collect();
        assert_eq!(
            writable,
            [
                "FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT",
                "FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT_MAX"
            ]
        );
        let max = NativeInput::from_reference(
            "work_int:mario:FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT_MAX",
        );
        assert!(max.validate_write_int(10).is_ok());
        assert!(max.validate_write_int(11).is_err());
        assert!(max.validate_write_int(-1).is_err());
        // Fighter-owned writes keep no bound, and a wrong kind or fighter is not admitted.
        assert!(NativeInput::from_reference(
            "work_int:cloud:FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_BREAK_CLEAR_FRAME"
        )
        .validate_write_int(99)
        .is_ok());
        for bad in [
            "work_float:mario:FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT",
            "work_int:unknown:FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT",
            "work_int:mario:FIGHTER_INSTANCE_WORK_ID_INT_COLOR",
        ] {
            assert!(
                matches!(
                    NativeInput::from_reference(bad),
                    NativeInput::Unsupported(_)
                ),
                "{bad}"
            );
        }
    }

    /// The table's values are hand-transcribed, so compare them with the source of truth.
    #[test]
    fn common_work_values_match_the_pinned_lua_const_table() {
        let path = std::path::Path::new(env!("HOME"))
            .join(".cargo/git/checkouts/skyline-smash-f820da3599566683/258a566/src/lua_const.rs");
        let Ok(source) = std::fs::read_to_string(&path) else {
            eprintln!("skipped: no pinned checkout at {}", path.display());
            return;
        };
        for row in COMMON_WORK {
            let expected = format!("    {}: 0x{:X},\n", row.symbol, row.value);
            assert!(
                source.contains(&expected),
                "{} is not {:#X} in {}",
                row.symbol,
                row.value,
                path.display()
            );
        }
    }

    #[test]
    fn stocks_remaining_round_trip_and_read_only_catalog() {
        for (input, reference) in [
            (
                NativeInput::fighter_value("mario", FighterValue::StocksRemaining),
                "stocks_remaining:mario",
            ),
            (
                NativeInput::fighter_value("mario", FighterValue::OpponentStocksRemaining),
                "opponent_stocks_remaining:mario",
            ),
        ] {
            let json = serde_json::to_string(&input).unwrap();
            assert_eq!(json, format!("\"{reference}\""));
            assert_eq!(serde_json::from_str::<NativeInput>(&json).unwrap(), input);
            assert_eq!(input.kind(), Some(NativeKind::Int));
            assert!(input.valid_for("mario"));
            assert!(!input.valid_for("cloud"));
            assert!(input.validate_write("mario").is_err());
            assert!(NativeInput::catalog_for("mario", NativeKind::Int).contains(&input));
            assert!(
                NativeSnapshot::from_observations(&[input], &[NativeObservation::Int(-1)]).is_err()
            );
        }
        for reference in [
            "stocks_remaining:unknown",
            "stocks_remaining:mario:extra",
            "opponent_stocks_remaining:",
        ] {
            let input: NativeInput = serde_json::from_str(&format!("\"{reference}\"")).unwrap();
            assert!(!input.is_supported());
        }
    }

    #[test]
    fn generic_references_round_trip_but_invalid_descriptors_fail_closed() {
        let symbol = "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BURNER_ENERGY_VALUE";
        let generic = NativeInput::from_registry("robot", symbol, NativeKind::Float).unwrap();
        let wire = format!("\"work_float:robot:{symbol}\"");
        assert_eq!(serde_json::to_string(&generic).unwrap(), wire);
        let decoded: NativeInput = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded, generic);
        assert_eq!(decoded.label(), "Burner energy value");
        assert_eq!(generic.canonical(), generic);

        let alias: NativeInput = serde_json::from_str(
            "\"work_float:robot:FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE\"",
        )
        .unwrap();
        assert_eq!(alias.canonical(), NativeInput::RobotBeamEnergy);
        for invalid in [
            "work_float:mario:FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BURNER_ENERGY_VALUE",
            "work_int:robot:FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BURNER_ENERGY_VALUE",
            "work_float:robot:FIGHTER_ROBOT_STATUS_KIND_ATTACK_AIR",
            "work_float:robot:FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_START",
            "work_float:robot:FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BURNER_ENERGY_VALUE:extra",
            "work_decimal:robot:FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BURNER_ENERGY_VALUE",
        ] {
            let encoded = serde_json::to_string(invalid).unwrap();
            let decoded: NativeInput = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, NativeInput::Unsupported(invalid.into()));
            assert!(!decoded.is_supported());
            assert!(!decoded.reference_is_bounded() || !decoded.valid_for("robot"));
            assert!(NativeInput::from_registry("robot", invalid, NativeKind::Float).is_err());
        }
        let nul: NativeInput = serde_json::from_str(
            "\"work_float:robot:FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BURNER\\u0000ENERGY_VALUE\"",
        )
        .unwrap();
        assert!(matches!(nul, NativeInput::Unsupported(_)));
        assert!(!nul.reference_is_bounded());
        let too_long = format!("\"{}\"", "x".repeat(NativeInput::MAX_REFERENCE_BYTES + 1));
        assert!(serde_json::from_str::<NativeInput>(&too_long).is_err());
        let malformed = NativeInput::Work {
            fighter: "robot".into(),
            symbol: symbol.into(),
            kind: NativeKind::Int,
        };
        assert!(!malformed.is_supported());
        assert!(!malformed.valid_for("robot"));
    }

    #[test]
    fn ink_tank_reads_round_trip_but_writes_require_the_resource_helper() {
        let symbol = "FIGHTER_INKLING_INSTANCE_WORK_ID_FLOAT_INK";
        let input = NativeInput::from_registry("inkling", symbol, NativeKind::Float).unwrap();
        assert!(NativeInput::catalog_for("inkling", NativeKind::Float).contains(&input));
        assert!(input.validate_write("inkling").is_err());
        assert!(NativeInput::from_registry("kirby", symbol, NativeKind::Float).is_err());
        assert!(NativeInput::from_registry("inkling", symbol, NativeKind::Int).is_err());
        let wire = serde_json::to_string(&input).unwrap();
        assert_eq!(serde_json::from_str::<NativeInput>(&wire).unwrap(), input);
        for value in [0.0, 12.5, 150.0] {
            let snapshot = NativeSnapshot::from_observations(
                std::slice::from_ref(&input),
                &[NativeObservation::Float(value)],
            )
            .unwrap();
            assert_eq!(snapshot.read(&input).unwrap(), value);
        }
    }

    #[test]
    fn pickel_materials_keep_named_reads_and_writes_require_the_resource_helper() {
        let catalog = NativeInput::catalog_for("pickel", NativeKind::Int);
        for (label, symbol, id) in [
            (
                "Dirt",
                "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_GRADE_1",
                0x100000BD,
            ),
            (
                "Wood",
                "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_WOOD",
                0x100000BE,
            ),
            (
                "Stone",
                "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_STONE",
                0x100000BF,
            ),
            (
                "Iron",
                "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_IRON",
                0x100000C0,
            ),
            (
                "Gold",
                "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_GOLD",
                0x100000C1,
            ),
            (
                "Redstone",
                "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_RED_STONE",
                0x100000C2,
            ),
            (
                "Diamond",
                "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_DIAMOND",
                0x100000C3,
            ),
        ] {
            let input = NativeInput::from_registry("pickel", symbol, NativeKind::Int).unwrap();
            assert_eq!(input.label(), label);
            assert_eq!(
                catalog
                    .iter()
                    .filter(|candidate| **candidate == input)
                    .count(),
                1
            );
            assert!(
                input.validate_write("pickel").is_err(),
                "{label} must go through the material helper"
            );
            assert!(input.validate_write("mario").is_err());
            assert!(NativeInput::from_registry("mario", symbol, NativeKind::Int).is_err());
            assert!(NativeInput::from_registry("pickel", symbol, NativeKind::Float).is_err());
            assert_eq!(
                super::super::registry::resolve(
                    "pickel",
                    symbol,
                    super::super::registry::IdentifierKind::WorkInt,
                )
                .unwrap()
                .value,
                id
            );
            let wire = serde_json::to_string(&input).unwrap();
            assert_eq!(wire, format!("\"work_int:pickel:{symbol}\""));
            assert_eq!(serde_json::from_str::<NativeInput>(&wire).unwrap(), input);
            let snapshot = NativeSnapshot::from_observations(
                std::slice::from_ref(&input),
                &[NativeObservation::Int(17)],
            )
            .unwrap();
            assert_eq!(snapshot.read_int(&input).unwrap(), 17);
        }
    }

    #[test]
    fn kirby_copy_ability_is_a_named_kirby_only_read() {
        for (label, symbol, kind, id, observation) in [
            (
                "Copied fighter",
                KIRBY_COPY_CHARA,
                NativeKind::Int,
                0x100000FC,
                NativeObservation::Int(-1),
            ),
            (
                "Has copy ability",
                KIRBY_COPY_FLAG,
                NativeKind::Flag,
                0x20000102,
                NativeObservation::Flag(true),
            ),
        ] {
            let input = NativeInput::from_registry("kirby", symbol, kind).unwrap();
            assert_eq!(input.label(), label);
            assert!(input.valid_for("kirby"));
            assert!(!input.valid_for("mario"));
            assert_eq!(
                NativeInput::catalog_for("kirby", kind)
                    .iter()
                    .filter(|c| **c == input)
                    .count(),
                1
            );
            assert!(NativeInput::catalog_for("mario", kind)
                .iter()
                .all(|c| c.symbol() != symbol));
            assert!(NativeInput::from_registry("mario", symbol, kind).is_err());
            assert_eq!(
                input.validate_write("kirby").unwrap_err(),
                "Kirby's copy ability is read-only"
            );
            assert_eq!(
                super::super::registry::resolve("kirby", symbol, kind.registry_kind())
                    .unwrap()
                    .value,
                id
            );
            let wire = serde_json::to_string(&input).unwrap();
            assert_eq!(serde_json::from_str::<NativeInput>(&wire).unwrap(), input);
            let snapshot = NativeSnapshot::from_observations(
                std::slice::from_ref(&input),
                std::slice::from_ref(&observation),
            )
            .unwrap();
            match observation {
                NativeObservation::Int(value) => {
                    assert_eq!(snapshot.read_int(&input).unwrap(), value)
                }
                _ => assert!(snapshot.read_flag(&input).unwrap()),
            }
        }
        // The slot is not offered as a named read but is equally unwritable.
        let slot = NativeInput::from_registry("kirby", KIRBY_COPY_SLOT, NativeKind::Int).unwrap();
        assert!(slot.validate_write("kirby").is_err());
        // Other Kirby Work fields stay writable: the guard is exact, not a prefix.
        let other = NativeInput::from_registry(
            "kirby",
            "FIGHTER_KIRBY_INSTANCE_WORK_ID_INT_SPECIAL_S_CHARGE",
            NativeKind::Int,
        )
        .unwrap();
        assert!(other.validate_write("kirby").is_ok());
    }

    #[test]
    fn canonical_snapshot_reads_aliases_through_one_typed_value() {
        let alias = NativeInput::from_registry(
            "robot",
            "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE",
            NativeKind::Float,
        )
        .unwrap();
        let snapshot = NativeSnapshot::from_observations(
            &[NativeInput::RobotBeamEnergy],
            &[NativeObservation::Float(22.5)],
        )
        .unwrap();
        assert_eq!(snapshot.read(&alias).unwrap(), 22.5);
        assert!(NativeSnapshot::from_observations(
            &[NativeInput::RobotBeamEnergy, alias],
            &[
                NativeObservation::Float(22.5),
                NativeObservation::Float(22.5)
            ],
        )
        .is_err());
    }

    #[test]
    fn hash40_matches_the_game() {
        // Values from the game's ParamLabels table.
        assert_eq!(hash40("throw_f"), 0x07_49f7_09a8);
        assert_eq!(hash40("top"), 0x03_1ed9_1fca);
        assert_eq!(hash40("invalid"), 0x07_fb99_7a80);
    }

    #[test]
    fn owned_object_references_round_trip_typed_and_owned() {
        const FIREBALL: &str = "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL";
        for field in ObjectField::ALL {
            let input = NativeInput::OwnedObject {
                fighter: "mario".into(),
                symbol: FIREBALL.into(),
                field,
            };
            let json = serde_json::to_string(&input).unwrap();
            assert_eq!(
                json,
                format!("\"owned_object:mario:{FIREBALL}:{}\"", field.key())
            );
            assert_eq!(serde_json::from_str::<NativeInput>(&json).unwrap(), input);
            assert!(input.valid_for("mario") && !input.valid_for("cloud"));
            assert_eq!(input.kind(), Some(field.kind()));
            assert_eq!(input.required_feature(), Some(OWNED_ARTICLES_FEATURE));
            assert!(NativeInput::catalog_for("mario", field.kind()).contains(&input));
        }
        for bad in [
            "owned_object:mario:FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE:speed_x",
            "owned_object:mario:FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL:spin",
            "owned_object:mario:FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL",
        ] {
            assert_eq!(
                NativeInput::from_reference(bad),
                NativeInput::Unsupported(bad.into())
            );
        }
    }

    #[test]
    fn deflected_article_references_round_trip() {
        for absorbed in [false, true] {
            let input = NativeInput::ArticleDeflected {
                fighter: "mario".into(),
                symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
                absorbed,
            };
            assert!(input.valid_for("mario"));
            let wire = serde_json::to_string(&input).unwrap();
            assert_eq!(serde_json::from_str::<NativeInput>(&wire).unwrap(), input);
        }
        let foreign: NativeInput = serde_json::from_str(
            "\"article_reflected:cloud:FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL\"",
        )
        .unwrap();
        assert!(matches!(foreign, NativeInput::Unsupported(_)));
    }

    #[test]
    fn attack_result_flag_references_are_closed_stable_and_fighter_owned() {
        use super::{AttackResultMask as Mask, AttackResultQuery as Query};
        let cases = [
            (Query::DuringHitlag, Mask::Hit, "during_hitlag:hit"),
            (Query::DuringHitlag, Mask::Shield, "during_hitlag:shield"),
            (Query::CurrentStatus, Mask::Hit, "current_status:hit"),
            (Query::CurrentStatus, Mask::Shield, "current_status:shield"),
        ];
        for (query, mask, suffix) in cases {
            let input = NativeInput::AttackResultFlag {
                fighter: "mario".into(),
                query,
                mask,
            };
            let wire = format!("\"attack_result_flag:mario:{suffix}\"");
            assert_eq!(serde_json::to_string(&input).unwrap(), wire);
            let decoded: NativeInput = serde_json::from_str(&wire).unwrap();
            assert_eq!(decoded, input);
            assert_eq!(decoded.kind(), Some(NativeKind::Flag));
            assert!(decoded.valid_for("mario"));
            assert!(!decoded.valid_for("cloud"));
        }
        for invalid in [
            "attack_result_flag:mario:during_hitlag:hit|shield",
            "attack_result_flag:mario:during_hitlag:2",
            "attack_result_flag:not_a_fighter:current_status:hit",
            "attack_result_flag:mario:unknown:hit",
            "attack_result_flag:mario:current_status:hit:extra",
        ] {
            let decoded: NativeInput =
                serde_json::from_str(&serde_json::to_string(invalid).unwrap()).unwrap();
            assert_eq!(decoded, NativeInput::Unsupported(invalid.into()));
            assert!(!decoded.is_supported());
        }
        assert_eq!(
            NativeInput::catalog_for("mario", NativeKind::Flag)
                .iter()
                .filter(|input| matches!(input, NativeInput::AttackResultFlag { .. }))
                .count(),
            4
        );
    }

    #[test]
    fn article_presence_references_are_exact_fighter_owned_articles() {
        let input = NativeInput::ArticleExists {
            fighter: "mario".into(),
            symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
        };
        let wire = "\"article_exists:mario:FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL\"";
        assert_eq!(serde_json::to_string(&input).unwrap(), wire);
        let decoded: NativeInput = serde_json::from_str(wire).unwrap();
        assert_eq!(decoded, input);
        assert_eq!(decoded.kind(), Some(NativeKind::Flag));
        assert_eq!(decoded.required_feature(), Some(ARTICLE_PRESENCE_FEATURE));
        assert!(decoded.valid_for("mario"));
        assert!(!decoded.valid_for("cloud"));
        for invalid in [
            "article_exists:mario:FIGHTER_MARIO_WORK_ID_INT_SPECIAL_N_COUNT",
            "article_exists:mario:FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP",
            "article_exists:mario:unknown",
            "article_exists:mario:FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL:extra",
        ] {
            let decoded: NativeInput =
                serde_json::from_str(&serde_json::to_string(invalid).unwrap()).unwrap();
            assert_eq!(decoded, NativeInput::Unsupported(invalid.into()));
            assert!(!decoded.is_supported());
        }
        assert!(NativeInput::catalog_for("mario", NativeKind::Flag)
            .iter()
            .any(|source| source == &input));
    }

    #[test]
    fn fighter_damage_snapshots_are_bounded_typed_and_immutable() {
        let damage = NativeInput::Fighter {
            value: FighterValue::Damage,
            fighter: "cloud".into(),
        };
        let sources = [damage.clone(), NativeInput::CloudLimitGauge];
        let values = [
            NativeObservation::Float(42.5),
            NativeObservation::Float(75.0),
        ];
        let snapshot = NativeSnapshot::from_observations(&sources, &values).unwrap();
        assert_eq!(snapshot.read(&damage).unwrap(), 42.5);
        assert_eq!(snapshot.read(&NativeInput::CloudLimitGauge).unwrap(), 75.0);
        assert!(snapshot.read_int(&damage).is_err());
        assert!(snapshot.read_flag(&damage).is_err());
        assert!(snapshot
            .read(&NativeInput::Fighter {
                value: FighterValue::Damage,
                fighter: "mario".into()
            })
            .is_err());
        for invalid in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            1_000_001.0,
            -1_000_001.0,
        ] {
            assert!(NativeSnapshot::from_observations(
                &sources,
                &[NativeObservation::Float(invalid), values[1]]
            )
            .is_err());
        }
        assert!(NativeSnapshot::from_observations(
            &sources,
            &[NativeObservation::Int(42), values[1]]
        )
        .is_err());
        assert_eq!(snapshot.read(&damage).unwrap(), 42.5);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
}
// Vanilla WorkModule access confirmed in HDR mario/status/special_s.rs:33 and
// fox/status/special_lw.rs:37. HDR ParamModule additions are deliberately excluded.
const REVIEWED_PARAMS: &[(&str, &str, &str, NativeKind)] = &[
    (
        "mario",
        "param_special_s",
        "special_s_attack_acl_y",
        NativeKind::Float,
    ),
    (
        "fox",
        "param_special_lw",
        "reflector_air_stop_y_frame",
        NativeKind::Int,
    ),
];
fn reviewed_param(fighter: &str, table: &str, key: &str, kind: NativeKind) -> bool {
    REVIEWED_PARAMS.contains(&(fighter, table, key, kind))
}

/// Standard grounded fighter throws. Command captures require their own native setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThrowDirection {
    Forward,
    Back,
    Up,
    Down,
}

#[cfg(test)]
mod projectile_tests {
    use super::*;
    #[test]
    fn projectile_work_is_typed_scoped_and_round_trips() {
        for (fighter, symbol, kind, article) in [
            (
                "mario",
                "WEAPON_MARIO_FIREBALL_INSTANCE_WORK_ID_INT_BOUND_COUNT",
                NativeKind::Int,
                "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL",
            ),
            (
                "samus",
                "WEAPON_SAMUS_CSHOT_INSTANCE_WORK_ID_FLOAT_CHARGE",
                NativeKind::Float,
                "FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT",
            ),
            (
                "cloud",
                "WEAPON_CLOUD_WAVE_INSTANCE_WORK_ID_FLAG_ATTACK_POWER_LOW",
                NativeKind::Flag,
                "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
            ),
        ] {
            let input = NativeInput::from_registry(fighter, symbol, kind).unwrap();
            assert_eq!(input.projectile_article(), Some(article));
            assert!(input.validate_write(fighter).is_ok());
            assert!(
                super::super::registry::resolve(fighter, symbol, kind.registry_kind()).is_err()
            );
            assert!(input.validate_write("link").is_err());
            assert_eq!(
                serde_json::from_str::<NativeInput>(&serde_json::to_string(&input).unwrap())
                    .unwrap(),
                input
            );
            assert!(NativeInput::catalog_for(fighter, kind).contains(&input));
            assert!(NativeInput::from_registry("link", symbol, kind).is_err());
            let wrong = if kind == NativeKind::Int {
                NativeKind::Float
            } else {
                NativeKind::Int
            };
            assert!(NativeInput::from_registry(fighter, symbol, wrong).is_err());
        }
        assert!(NativeInput::from_registry(
            "mario",
            "WEAPON_MARIO_FIREBALL_INSTANCE_WORK_ID_INT_TERM",
            NativeKind::Int
        )
        .is_err());
    }
}
