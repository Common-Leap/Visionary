//! One native resource family: a table of fighter gauges and counters.
//!
//! A row is read as its Work field through the shared `NativeInput` path, and
//! changed by Add or Set through the fighter's own native helper when the game
//! has one, otherwise by writing that Work field. Every symbol is a constant in
//! the pinned skyline_smash checkout; Work-written rows are fields HewDraw-Remix
//! writes with `WorkModule` in shipped code. `max` is the bound on an authored
//! value; helpers keep their own native cap. Amounts are native units.
use serde::{Deserialize, Serialize};

use super::native_inputs::NativeKind;

pub const FEATURE: &str = "native_resource_actions.v1";

/// How an Add or Set reaches the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    /// `WorkModule` set on the row's own field.
    Work,
    /// Cloud's reviewed Limit helper: an adjustment, or the full clear at zero.
    CloudLimit,
    /// `FighterSpecializer_Jack::add_rebel_gauge`.
    RebelGauge,
    /// `FighterSpecializer_Brave::add_sp`.
    HeroMp,
    /// `FighterSpecializer_Inkling::change_ink`, capped by `get_ink_max`.
    InklingInk,
    /// `FighterSpecializer_Pickel::add_material_num`/`sub_material_num` with this material kind.
    PickelMaterial(&'static str),
    /// `DamageModule::add_damage` on this fighter's partner (Nana, Luma). A missing partner is a no-op.
    PartnerDamage,
    /// Sends this fighter's partner to one action. It has no amount.
    PartnerAction(&'static str),
}

impl Writer {
    /// Partner rows live on the partner, so they have no Work field of this fighter behind them.
    #[allow(dead_code)] // Not used by every crate that includes this file.
    pub fn is_partner(self) -> bool {
        matches!(self, Self::PartnerDamage | Self::PartnerAction(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    /// Saved key; the four original rows keep their pre-table names.
    pub key: &'static str,
    pub fighter: &'static str,
    pub label: &'static str,
    pub symbol: &'static str,
    pub kind: NativeKind,
    pub max: f32,
    pub writer: Writer,
}

impl Entry {
    /// The read side: the row's Work field as an ordinary native input
    /// (`work:` reference); graphs read it like any other Work value.
    #[cfg(test)]
    pub fn input(&self) -> super::native_inputs::NativeInput {
        super::native_inputs::NativeInput::Work {
            fighter: self.fighter.into(),
            symbol: self.symbol.into(),
            kind: self.kind,
        }
        .canonical()
    }
}

macro_rules! resources {
    ($(($key:literal, $fighter:literal, $label:literal, $symbol:literal, $kind:ident, $max:literal, $writer:expr)),* $(,)?) => {
        pub static RESOURCES: &[Entry] = &[$(Entry { key: $key, fighter: $fighter, label: $label, symbol: $symbol, kind: NativeKind::$kind, max: $max, writer: $writer }),*];
    };
}

// Bounds: Cloud's helper clamps Limit to 0-100; Hero's MP is 100; Inkling's tank 150
// (native_catalog); HewDraw-Remix fills Joker's gauge with 100.0 and clamps Little
// Mac's KO meter to 0-100; Banjo has five Wonderwing feathers. Steve's materials use
// the former ±100 amount bound; the helper owns the real count cap.
resources! {
    ("cloudLimit", "cloud", "Limit", "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE", Float, 100.0, Writer::CloudLimit),
    ("heroMp", "brave", "MP", "FIGHTER_BRAVE_INSTANCE_WORK_ID_FLOAT_SP", Float, 100.0, Writer::HeroMp),
    ("jokerRebelGauge", "jack", "Rebel gauge", "FIGHTER_JACK_INSTANCE_WORK_ID_FLOAT_REBEL_GAUGE", Float, 100.0, Writer::RebelGauge),
    ("inklingInk", "inkling", "Ink", "FIGHTER_INKLING_INSTANCE_WORK_ID_FLOAT_INK", Float, 150.0, Writer::InklingInk),
    ("littleMacKoGauge", "littlemac", "KO meter", "FIGHTER_LITTLEMAC_INSTANCE_WORK_ID_FLOAT_KO_GAGE", Float, 100.0, Writer::Work),
    ("banjoFeathers", "buddy", "Wonderwing feathers", "FIGHTER_BUDDY_INSTANCE_WORK_ID_INT_SPECIAL_S_REMAIN", Int, 5.0, Writer::Work),
    ("nanaDamage", "popo", "Nana's damage", "FIGHTER_POPO_INSTANCE_WORK_ID_INT_PARTNER_OBJECT_ID", Float, 999.0, Writer::PartnerDamage),
    ("nanaWait", "popo", "Nana: stand still", "FIGHTER_POPO_INSTANCE_WORK_ID_INT_PARTNER_OBJECT_ID", Float, 1.0, Writer::PartnerAction("FIGHTER_STATUS_KIND_WAIT")),
    ("nanaJump", "popo", "Nana: jump", "FIGHTER_POPO_INSTANCE_WORK_ID_INT_PARTNER_OBJECT_ID", Float, 1.0, Writer::PartnerAction("FIGHTER_STATUS_KIND_JUMP_SQUAT")),
    ("nanaAttack", "popo", "Nana: attack", "FIGHTER_POPO_INSTANCE_WORK_ID_INT_PARTNER_OBJECT_ID", Float, 1.0, Writer::PartnerAction("FIGHTER_STATUS_KIND_ATTACK")),
    ("nanaSpecial", "popo", "Nana: neutral special", "FIGHTER_POPO_INSTANCE_WORK_ID_INT_PARTNER_OBJECT_ID", Float, 1.0, Writer::PartnerAction("FIGHTER_STATUS_KIND_SPECIAL_N")),
    ("nanaShield", "popo", "Nana: shield", "FIGHTER_POPO_INSTANCE_WORK_ID_INT_PARTNER_OBJECT_ID", Float, 1.0, Writer::PartnerAction("FIGHTER_STATUS_KIND_GUARD_ON")),
    ("lumaDamage", "rosetta", "Luma's damage", "FIGHTER_ROSETTA_INSTANCE_WORK_ID_INT_EXTEND_BUFFER", Float, 999.0, Writer::PartnerDamage),
    ("steveDirt", "pickel", "Dirt", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_GRADE_1", Int, 100.0, Writer::PickelMaterial("FIGHTER_PICKEL_MATERIAL_KIND_GRADE_1")),
    ("steveWood", "pickel", "Wood", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_WOOD", Int, 100.0, Writer::PickelMaterial("FIGHTER_PICKEL_MATERIAL_KIND_WOOD")),
    ("steveStone", "pickel", "Stone", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_STONE", Int, 100.0, Writer::PickelMaterial("FIGHTER_PICKEL_MATERIAL_KIND_STONE")),
    ("steveIron", "pickel", "Iron", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_IRON", Int, 100.0, Writer::PickelMaterial("FIGHTER_PICKEL_MATERIAL_KIND_IRON")),
    ("steveGold", "pickel", "Gold", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_GOLD", Int, 100.0, Writer::PickelMaterial("FIGHTER_PICKEL_MATERIAL_KIND_GOLD")),
    ("steveDiamonds", "pickel", "Diamonds", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_DIAMOND", Int, 100.0, Writer::PickelMaterial("FIGHTER_PICKEL_MATERIAL_KIND_DIAMOND")),
    ("steveRedstone", "pickel", "Redstone", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_RED_STONE", Int, 100.0, Writer::PickelMaterial("FIGHTER_PICKEL_MATERIAL_KIND_RED_STONE")),
}

/// A row of [`RESOURCES`], saved by its key.
#[derive(Clone, Copy)]
pub struct Resource(&'static Entry);

impl Resource {
    pub fn named(key: &str) -> Option<Self> {
        RESOURCES.iter().find(|entry| entry.key == key).map(Self)
    }
    pub fn for_fighter(fighter: &str) -> impl Iterator<Item = Self> + '_ {
        RESOURCES
            .iter()
            .filter(move |entry| entry.fighter == fighter)
            .map(Self)
    }
    pub fn entry(self) -> &'static Entry {
        self.0
    }
    pub fn fighter(self) -> &'static str {
        self.0.fighter
    }
    pub fn label(self) -> &'static str {
        self.0.label
    }
}

impl PartialEq for Resource {
    fn eq(&self, other: &Self) -> bool {
        self.0.key == other.0.key
    }
}
impl Eq for Resource {}
impl std::fmt::Debug for Resource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.key)
    }
}
impl Serialize for Resource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.key)
    }
}
impl<'de> Deserialize<'de> for Resource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let key = String::deserialize(deserializer)?;
        Self::named(&key)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown native resource {key:?}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    Add,
    Set,
}

/// Add `amount` to, or Set the resource to `amount`. `E` is an expression in a
/// graph and the resolved `f32` once the evaluator reaches the action.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command<E = f32> {
    pub op: Op,
    pub resource: Resource,
    pub amount: E,
}

impl<E> Command<E> {
    pub fn try_map<T, F>(&self, f: impl FnOnce(&E) -> Result<T, F>) -> Result<Command<T>, F> {
        Ok(Command {
            op: self.op,
            resource: self.resource,
            amount: f(&self.amount)?,
        })
    }
    pub fn label(&self) -> String {
        if matches!(self.resource.entry().writer, Writer::PartnerAction(_)) {
            return self.resource.label().to_owned();
        }
        match self.op {
            Op::Add => format!("Add {}", self.resource.label()),
            Op::Set => format!("Set {}", self.resource.label()),
        }
    }
    /// The owner check every graph, controller and runtime applies.
    pub fn check_owner(&self, fighter: &str) -> Result<(), String> {
        if self.resource.fighter() != fighter {
            return Err("resource belongs to another fighter".into());
        }
        Ok(())
    }
}

impl Command<f32> {
    /// The value after this command from `current`, within `0..=max`.
    pub fn target(self, current: f32) -> Result<f32, String> {
        let entry = self.resource.entry();
        if !current.is_finite() || current < 0.0 {
            return Err("native resource value must be finite and nonnegative".into());
        }
        if !self.amount.is_finite() || self.amount.abs() > entry.max {
            return Err(format!(
                "{} amount must be finite and within {} native units",
                entry.label, entry.max
            ));
        }
        if entry.kind == NativeKind::Int && self.amount.fract() != 0.0 {
            return Err(format!("{} amount must be an integer", entry.label));
        }
        Ok(match self.op {
            Op::Add => current + self.amount,
            Op::Set => self.amount,
        }
        .clamp(0.0, entry.max))
    }
    pub fn validate(self, fighter: &str) -> Result<(), String> {
        self.check_owner(fighter)?;
        self.target(0.0).map(|_| ())
    }
}
