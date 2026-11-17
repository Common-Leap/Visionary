//! Exercise the unchanged production registry, actor identity and native sampler.
//! Only native object/agent tables and the reviewed module APIs are stubbed.

#![allow(dead_code, clippy::missing_safety_doc)]

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

extern crate self as smash;

#[path = "../plugins/slight_replica/src/slight/gimmicks/actor_identity.rs"]
mod actor_identity;
#[path = "../plugins/slight_replica/src/slight/gimmicks/article_context.rs"]
mod article_context;
#[path = "../src/gimmicks/hero_menu_commands.rs"]
pub mod hero_menu_commands;
#[path = "../src/gimmicks/live_model.rs"]
mod live_model;
#[path = "../src/gimmicks/native_inputs.rs"]
mod native_inputs;
#[path = "../src/gimmicks/resource_commands.rs"]
pub mod resource_commands;
#[path = "../src/gimmicks/status_commands.rs"]
mod status_commands;
#[path = "../src/gimmicks/status_model.rs"]
mod status_model;
fn controller_random(seed: &std::cell::Cell<u64>, upper: i32) -> i32 {
    let next = seed
        .get()
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    seed.set(next);
    ((next >> 32) % upper as u64) as i32
}
#[path = "../plugins/slight_replica/src/slight/gimmicks/native_values.rs"]
mod native_values;
mod hero_menu_control {
    use super::actor_identity::Identity;
    pub(crate) fn sample(_: u32, _: &str, _: Identity) -> Result<i32, String> {
        Ok(-1)
    }
}
mod partner_control {
    use super::{
        actor_identity::Identity,
        native_inputs::{NativeInput, NativeObservation},
    };
    pub(crate) fn sample(
        _: u32,
        _: &str,
        _: Identity,
        _: &NativeInput,
    ) -> Result<NativeObservation, String> {
        Ok(NativeObservation::Flag(false))
    }
}
#[path = "../plugins/slight_replica/src/slight/gimmicks/registry.rs"]
mod registry;

// Capture-link engine calls are covered by controller primitive source checks.
// This sampler harness has one native actor and cannot fabricate a held fighter.
mod grab_control {
    pub(crate) fn sample(
        _: u32,
        _: &str,
        _: crate::actor_identity::Identity,
        _: &crate::native_inputs::NativeInput,
    ) -> Result<crate::native_inputs::NativeObservation, String> {
        Err("capture link unavailable in single-actor host harness".into())
    }
}

mod status_runtime {
    pub unsafe fn energy_id(_: crate::status_commands::Energy) -> i32 {
        0
    }
}

mod live_behavior {
    pub(crate) use crate::actor_identity::Identity;
}

mod behavior {
    #[derive(Clone, Copy)]
    pub(crate) struct NativeSource(pub &'static str);

    impl NativeSource {
        pub(crate) fn to_input(self) -> crate::native_inputs::NativeInput {
            crate::native_inputs::NativeInput::from_reference(self.0).canonical()
        }
    }
}

mod state {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ActorIdentity {
        pub fighter_kind: i32,
        pub entry_id: i32,
        pub module_accessor_addr: u64,
        pub stock_generation: u32,
    }
}

#[derive(Clone, Copy)]
struct Actor {
    boid: u32,
    accessor: u64,
    category: i32,
    kind: i32,
    entry: i32,
    active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Scalar {
    Float(f32),
    Int(i32),
    Flag(bool),
}

#[derive(Clone, Copy)]
enum Change {
    Stock,
    Accessor,
    Kind,
    Entry,
    Category,
    Inactive,
    NoActor,
    NoRecord,
    NoStock,
}

#[derive(Default)]
struct Engine {
    actor: Option<Actor>,
    other_actors: Vec<Actor>,
    owned_weapons: Vec<u32>,
    article_instances: Vec<u32>,
    weapon_values: BTreeMap<(u64, i32), Scalar>,
    stocks: BTreeMap<i32, u64>,
    stock_reads: Vec<i32>,
    record: Option<slight::agents::AgentRecord>,
    stock: Option<u32>,
    values: BTreeMap<i32, Scalar>,
    reads: Vec<(u64, i32)>,
    ordered_reads: Vec<(bool, u64, i32)>,
    after_read: Option<(usize, Change)>,
    attack_reads: Vec<(u64, i32, bool)>,
    attack_values: BTreeMap<(bool, i32), bool>,
    after_attack_read: Option<(usize, Change)>,
    article_reads: Vec<(u64, i32)>,
    article_values: BTreeMap<i32, i32>,
    after_article_read: Option<(usize, Change)>,
    damage_reads: Vec<(u64, i32)>,
    damage_value: f32,
    after_damage_read: Option<(usize, Change)>,
    status_reads: Vec<u64>,
    status_value: i32,
    /// Status and damage of `other_actors`, by accessor; the owner uses the fields above.
    other_status: BTreeMap<u64, i32>,
    other_damage: BTreeMap<u64, f32>,
    next_status: Option<i32>,
    after_status_read: Option<Change>,
    phase_reads: Vec<u64>,
    phase_value: bool,
    after_phase_read: Option<Change>,
}

impl Engine {
    fn read_status(&mut self, accessor: u64) -> i32 {
        if self
            .other_actors
            .iter()
            .any(|actor| actor.accessor == accessor)
        {
            return self.other_status.get(&accessor).copied().unwrap_or(0);
        }
        assert_eq!(self.actor.unwrap().accessor, accessor);
        self.status_reads.push(accessor);
        let value = self.status_value;
        if let Some(next) = self.next_status.take() {
            self.status_value = next;
        }
        if let Some(change) = self.after_status_read.take() {
            self.change(change);
        }
        value
    }
    fn read_damage(&mut self, accessor: u64, slot: i32) -> f32 {
        if self
            .other_actors
            .iter()
            .any(|actor| actor.accessor == accessor)
        {
            return self.other_damage[&accessor];
        }
        assert_eq!(self.actor.unwrap().accessor, accessor);
        assert_eq!(slot, 0, "only the reviewed fixed native slot is permitted");
        self.damage_reads.push((accessor, slot));
        let value = self.damage_value;
        if self
            .after_damage_read
            .is_some_and(|(at, _)| at == self.damage_reads.len())
        {
            let (_, change) = self.after_damage_read.take().unwrap();
            self.change(change);
        }
        value
    }

    fn change(&mut self, change: Change) {
        match change {
            Change::Stock => self.stock = Some(2),
            Change::Accessor => self.actor.as_mut().unwrap().accessor += 1,
            Change::Kind => self.actor.as_mut().unwrap().kind += 1,
            Change::Entry => self.actor.as_mut().unwrap().entry += 1,
            Change::Category => self.actor.as_mut().unwrap().category = 1,
            Change::Inactive => self.actor.as_mut().unwrap().active = false,
            Change::NoActor => self.actor = None,
            Change::NoRecord => self.record = None,
            Change::NoStock => self.stock = None,
        }
    }

    fn read(&mut self, accessor: u64, id: i32) -> Scalar {
        if let Some(value) = self.weapon_values.get(&(accessor, id)).copied() {
            self.reads.push((accessor, id));
            return value;
        }
        assert_eq!(self.actor.unwrap().accessor, accessor);
        self.reads.push((accessor, id));
        self.ordered_reads.push((false, accessor, id));
        let value = *self.values.get(&id).expect("unexpected native getter ID");
        if self
            .after_read
            .is_some_and(|(at, _)| at == self.reads.len())
        {
            let (_, change) = self.after_read.take().unwrap();
            self.change(change);
        }
        value
    }

    fn read_attack(&mut self, accessor: u64, mask: i32, status: bool) -> bool {
        assert_eq!(self.actor.unwrap().accessor, accessor);
        self.attack_reads.push((accessor, mask, status));
        self.ordered_reads.push((true, accessor, mask));
        let value = *self
            .attack_values
            .get(&(status, mask))
            .expect("unexpected AttackModule query/mask");
        if self
            .after_attack_read
            .is_some_and(|(at, _)| at == self.attack_reads.len())
        {
            let (_, change) = self.after_attack_read.take().unwrap();
            self.change(change);
        }
        value
    }

    fn read_article(&mut self, accessor: u64, article: i32) -> bool {
        assert_eq!(self.actor.unwrap().accessor, accessor);
        self.article_reads.push((accessor, article));
        self.ordered_reads.push((false, accessor, article));
        let value = *self
            .article_values
            .get(&article)
            .expect("unexpected ArticleModule article ID");
        if self
            .after_article_read
            .is_some_and(|(at, _)| at == self.article_reads.len())
        {
            let (_, change) = self.after_article_read.take().unwrap();
            self.change(change);
        }
        value > 0
    }
}

static ENGINE: LazyLock<Mutex<Engine>> = LazyLock::new(|| Mutex::new(Engine::default()));
static TEST_LOCK: Mutex<()> = Mutex::new(());

pub mod app {
    pub type KineticEnergy = BattleObjectModuleAccessor;
    pub struct BattleObjectModuleAccessor;
    pub struct FighterManager;
    pub struct FighterInformation;
    pub struct FighterEntryID(pub i32);

    pub mod sv_battle_object {
        fn actor(boid: u32) -> Option<crate::Actor> {
            let engine = crate::ENGINE.lock().unwrap();
            engine
                .actor
                .into_iter()
                .chain(engine.other_actors.iter().copied())
                .find(|a| a.boid == boid)
        }

        pub unsafe fn is_null(boid: u32) -> bool {
            actor(boid).is_none()
        }
        pub unsafe fn is_active(boid: u32) -> bool {
            actor(boid).is_some_and(|a| a.active)
        }
        pub unsafe fn module_accessor(boid: u32) -> *mut super::BattleObjectModuleAccessor {
            actor(boid).map_or(0, |a| a.accessor) as *mut super::BattleObjectModuleAccessor
        }
        pub unsafe fn category(boid: u32) -> i32 {
            actor(boid).map_or(-1, |a| a.category)
        }
        pub unsafe fn kind(boid: u32) -> i32 {
            actor(boid).map_or(-1, |a| a.kind)
        }
        pub unsafe fn entry_id(boid: u32) -> i32 {
            actor(boid).map_or(-1, |a| a.entry)
        }
    }

    pub mod lua_bind {
        pub struct FighterManager;
        impl FighterManager {
            pub unsafe fn get_fighter_information(
                _: *mut super::FighterManager,
                entry: super::FighterEntryID,
            ) -> *mut super::FighterInformation {
                if crate::ENGINE.lock().unwrap().stocks.contains_key(&entry.0) {
                    (entry.0 as usize + 1) as *mut super::FighterInformation
                } else {
                    std::ptr::null_mut()
                }
            }
        }
        pub struct FighterInformation;
        impl FighterInformation {
            pub unsafe fn stock_count(info: *mut super::FighterInformation) -> u64 {
                let entry = info as usize as i32 - 1;
                let mut engine = crate::ENGINE.lock().unwrap();
                engine.stock_reads.push(entry);
                engine.stocks[&entry]
            }
        }
        pub struct ItemModule;

        impl ItemModule {
            pub unsafe fn get_have_item_kind(
                m: *mut super::BattleObjectModuleAccessor,
                hand: i32,
            ) -> i32 {
                assert_eq!(hand, 0);
                WorkModule::get_int(m, -10)
            }
            pub unsafe fn is_have_item(_: *mut super::BattleObjectModuleAccessor, _: i32) -> bool {
                false
            }
        }

        pub struct DamageModule;

        impl DamageModule {
            pub unsafe fn damage(
                accessor: *mut super::BattleObjectModuleAccessor,
                slot: i32,
            ) -> f32 {
                crate::ENGINE
                    .lock()
                    .unwrap()
                    .read_damage(accessor as u64, slot)
            }
        }

        pub struct AttackModule;

        impl AttackModule {
            pub unsafe fn is_infliction(
                accessor: *mut super::BattleObjectModuleAccessor,
                mask: i32,
            ) -> bool {
                crate::ENGINE
                    .lock()
                    .unwrap()
                    .read_attack(accessor as u64, mask, false)
            }
            pub unsafe fn is_infliction_status(
                accessor: *mut super::BattleObjectModuleAccessor,
                mask: i32,
            ) -> bool {
                crate::ENGINE
                    .lock()
                    .unwrap()
                    .read_attack(accessor as u64, mask, true)
            }
        }

        pub struct Article;
        impl Article {
            pub unsafe fn get_battle_object_id(_: *mut Article) -> u64 {
                panic!("null article getter must not be dereferenced")
            }
        }
        pub struct ArticleModule;
        pub struct StatusModule;

        impl StatusModule {
            pub unsafe fn is_changing(accessor: *mut super::BattleObjectModuleAccessor) -> bool {
                let mut engine = crate::ENGINE.lock().unwrap();
                assert_eq!(engine.actor.unwrap().accessor, accessor as u64);
                engine.phase_reads.push(accessor as u64);
                let value = engine.phase_value;
                if let Some(change) = engine.after_phase_read.take() {
                    engine.change(change);
                }
                value
            }
            pub unsafe fn status_kind(accessor: *mut super::BattleObjectModuleAccessor) -> i32 {
                crate::ENGINE.lock().unwrap().read_status(accessor as u64)
            }
        }

        impl ArticleModule {
            // Live Mario fireballs are in the weapon registry while this getter returns null.
            pub unsafe fn get_article_from_no(
                _: *mut super::BattleObjectModuleAccessor,
                _: i32,
                _: i32,
            ) -> *mut Article {
                std::ptr::null_mut()
            }

            pub unsafe fn get_active_num(
                accessor: *mut super::BattleObjectModuleAccessor,
                article: i32,
            ) -> i32 {
                let mut engine = crate::ENGINE.lock().unwrap();
                let count = engine.article_values[&article];
                engine.read_article(accessor as u64, article);
                count
            }
            pub unsafe fn is_exist(
                accessor: *mut super::BattleObjectModuleAccessor,
                article: i32,
            ) -> bool {
                crate::ENGINE
                    .lock()
                    .unwrap()
                    .read_article(accessor as u64, article)
            }
        }

        pub struct ControlModule;
        impl ControlModule {
            pub unsafe fn get_command_flag_cat(
                m: *mut super::BattleObjectModuleAccessor,
                category: i32,
            ) -> i32 {
                assert_eq!(category, 3);
                WorkModule::get_int(m, -100)
            }

            pub unsafe fn check_button_on(
                m: *mut super::BattleObjectModuleAccessor,
                id: i32,
            ) -> bool {
                WorkModule::is_flag(m, id)
            }
            pub unsafe fn check_button_trigger(
                m: *mut super::BattleObjectModuleAccessor,
                id: i32,
            ) -> bool {
                WorkModule::is_flag(m, id + 10)
            }
            pub unsafe fn check_button_release(
                m: *mut super::BattleObjectModuleAccessor,
                id: i32,
            ) -> bool {
                WorkModule::is_flag(m, id + 20)
            }
            pub unsafe fn get_stick_x(m: *mut super::BattleObjectModuleAccessor) -> f32 {
                WorkModule::get_float(m, -1)
            }
            pub unsafe fn get_stick_y(m: *mut super::BattleObjectModuleAccessor) -> f32 {
                WorkModule::get_float(m, -2)
            }
        }
        pub struct KineticEnergy;
        impl KineticEnergy {
            pub unsafe fn get_speed_x(m: *mut super::KineticEnergy) -> f32 {
                WorkModule::get_float(m.cast(), -4)
            }
            pub unsafe fn get_speed_y(m: *mut super::KineticEnergy) -> f32 {
                WorkModule::get_float(m.cast(), -5)
            }
        }
        pub struct KineticModule;
        impl KineticModule {
            pub unsafe fn get_energy(m: *mut super::BattleObjectModuleAccessor, _: i32) -> u64 {
                m as u64
            }
            pub unsafe fn get_sum_speed_x(
                m: *mut super::BattleObjectModuleAccessor,
                attr: i32,
            ) -> f32 {
                assert_eq!(attr, 1);
                WorkModule::get_float(m, -4)
            }
            pub unsafe fn get_sum_speed_y(
                m: *mut super::BattleObjectModuleAccessor,
                attr: i32,
            ) -> f32 {
                assert_eq!(attr, 1);
                WorkModule::get_float(m, -5)
            }
        }
        pub struct PostureModule;
        impl PostureModule {
            pub unsafe fn pos_x(m: *mut super::BattleObjectModuleAccessor) -> f32 {
                WorkModule::get_float(m, -6)
            }
            pub unsafe fn pos_y(m: *mut super::BattleObjectModuleAccessor) -> f32 {
                WorkModule::get_float(m, -7)
            }

            pub unsafe fn lr(m: *mut super::BattleObjectModuleAccessor) -> f32 {
                WorkModule::get_float(m, -3)
            }
        }
        pub struct WorkModule;

        impl WorkModule {
            pub unsafe fn get_param_float(
                m: *mut super::BattleObjectModuleAccessor,
                _: u64,
                _: u64,
            ) -> f32 {
                Self::get_float(m, -8)
            }
            pub unsafe fn get_param_int(
                m: *mut super::BattleObjectModuleAccessor,
                _: u64,
                _: u64,
            ) -> i32 {
                Self::get_int(m, -9)
            }
            fn write(m: *mut super::BattleObjectModuleAccessor, id: i32, value: crate::Scalar) {
                let mut engine = crate::ENGINE.lock().unwrap();
                if let Some(slot) = engine.weapon_values.get_mut(&(m as u64, id)) {
                    *slot = value;
                } else {
                    assert_eq!(engine.actor.unwrap().accessor, m as u64);
                    engine.values.insert(id, value);
                }
            }
            pub unsafe fn set_float(
                m: *mut super::BattleObjectModuleAccessor,
                value: f32,
                id: i32,
            ) {
                Self::write(m, id, crate::Scalar::Float(value));
            }
            pub unsafe fn set_int(m: *mut super::BattleObjectModuleAccessor, value: i32, id: i32) {
                Self::write(m, id, crate::Scalar::Int(value));
            }
            pub unsafe fn on_flag(m: *mut super::BattleObjectModuleAccessor, id: i32) {
                Self::write(m, id, crate::Scalar::Flag(true));
            }
            pub unsafe fn off_flag(m: *mut super::BattleObjectModuleAccessor, id: i32) {
                Self::write(m, id, crate::Scalar::Flag(false));
            }
            pub unsafe fn get_float(
                accessor: *mut super::BattleObjectModuleAccessor,
                id: i32,
            ) -> f32 {
                let crate::Scalar::Float(value) =
                    crate::ENGINE.lock().unwrap().read(accessor as u64, id)
                else {
                    panic!("wrong native getter type")
                };
                value
            }
            pub unsafe fn get_int(
                accessor: *mut super::BattleObjectModuleAccessor,
                id: i32,
            ) -> i32 {
                let crate::Scalar::Int(value) =
                    crate::ENGINE.lock().unwrap().read(accessor as u64, id)
                else {
                    panic!("wrong native getter type")
                };
                value
            }
            pub unsafe fn is_flag(
                accessor: *mut super::BattleObjectModuleAccessor,
                id: i32,
            ) -> bool {
                let crate::Scalar::Flag(value) =
                    crate::ENGINE.lock().unwrap().read(accessor as u64, id)
                else {
                    panic!("wrong native getter type")
                };
                value
            }
        }
    }
}

pub mod lib {
    pub mod lua_const {
        pub static FIGHTER_STATUS_KIND_DEAD: &i32 = &0xB5;
        pub static FIGHTER_STATUS_KIND_STANDBY: &i32 = &0x1D6;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND: &i32 = &0x1;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N2_COMMAND: &i32 = &0x2;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_S_COMMAND: &i32 = &0x4;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI_COMMAND: &i32 = &0x8;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI2_COMMAND: &i32 = &0x80;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_SUPER_SPECIAL_COMMAND: &i32 = &0x100;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_SUPER_SPECIAL2_COMMAND: &i32 = &0x400;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6: &i32 = &0x10;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_4N4: &i32 = &0x20;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623NB: &i32 = &0x1000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623A: &i32 = &0x10000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_ATTACK_COMMAND1: &i32 = &0x100000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623STRICT: &i32 = &0x200000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623ALONG: &i32 = &0x400000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623BLONG: &i32 = &0x800000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6AB: &i32 = &0x1000000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_323CATCH: &i32 = &0x2000000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_1: &i32 = &0x4000000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_3: &i32 = &0x8000000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_7: &i32 = &0x10000000;
        pub static FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_9: &i32 = &0x20000000;

        pub static KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN: &i32 = &1;
        pub static CONTROL_PAD_BUTTON_ATTACK: &i32 = &101;
        pub static CONTROL_PAD_BUTTON_SPECIAL: &i32 = &102;
        pub static CONTROL_PAD_BUTTON_JUMP: &i32 = &103;
        pub static CONTROL_PAD_BUTTON_GUARD: &i32 = &104;
        pub static CONTROL_PAD_BUTTON_CATCH: &i32 = &105;
        pub static COLLISION_KIND_MASK_HIT: &i32 = &0x2;
        pub static COLLISION_KIND_MASK_SHIELD: &i32 = &0x4;
        pub static COLLISION_KIND_MASK_REFLECTOR: &i32 = &0x8;
        pub static COLLISION_KIND_MASK_ABSORBER: &i32 = &0x10;
    }
}

mod slight {
    pub mod systems {
        pub mod article_notifier {
            pub fn owned_weapons(_: u32, _: &str, _: &str) -> Vec<u32> {
                crate::ENGINE.lock().unwrap().owned_weapons.clone()
            }
        }
    }
    pub mod diag {
        pub fn note(_: impl Into<String>) {}
    }
    pub mod agents {
        pub fn newest_registered(candidates: &[u32]) -> Option<u32> {
            crate::ENGINE
                .lock()
                .unwrap()
                .article_instances
                .iter()
                .rev()
                .find(|id| candidates.contains(id))
                .copied()
        }
        #[derive(Clone, PartialEq, Eq)]
        pub struct AgentRecord {
            pub boid: u32,
            pub module_accessor_addr: u64,
            pub category: i32,
            pub kind: i32,
            pub entry_id: i32,
        }

        pub fn all_records() -> Vec<AgentRecord> {
            let engine = crate::ENGINE.lock().unwrap();
            engine
                .record
                .clone()
                .into_iter()
                .chain(engine.other_actors.iter().map(|actor| AgentRecord {
                    boid: actor.boid,
                    module_accessor_addr: actor.accessor,
                    category: actor.category,
                    kind: actor.kind,
                    entry_id: actor.entry,
                }))
                .collect()
        }
        pub fn lookup(boid: u32) -> Option<AgentRecord> {
            all_records().into_iter().find(|record| record.boid == boid)
        }
        pub fn stock_generation(record: &AgentRecord) -> Option<u32> {
            let engine = crate::ENGINE.lock().unwrap();
            if engine.record.as_ref() == Some(record) {
                engine.stock
            } else if engine
                .other_actors
                .iter()
                .any(|actor| actor.boid == record.boid)
            {
                Some(1)
            } else {
                None
            }
        }
    }
}

const BOID: u32 = 0xAB51;
const ACCESSOR: u64 = 0x1234_5678;
const GAUGE: native_values::WorkSource<'static> = native_values::WorkSource {
    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE",
    kind: registry::IdentifierKind::WorkFloat,
};
const CLEAR_FRAME: native_values::WorkSource<'static> = native_values::WorkSource {
    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_BREAK_CLEAR_FRAME",
    kind: registry::IdentifierKind::WorkInt,
};
const POWERED: native_values::WorkSource<'static> = native_values::WorkSource {
    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK",
    kind: registry::IdentifierKind::WorkFlag,
};

fn setup() -> actor_identity::Identity {
    setup_as("cloud", 0x3E)
}

fn status_source(symbol: &str) -> native_inputs::NativeInput {
    native_inputs::NativeInput::FighterStatusIs {
        fighter: "cloud".into(),
        symbol: symbol.into(),
    }
}

#[test]
fn changing_status_samples_exact_native_boolean_once_with_fresh_identity_and_context_guards() {
    use native_inputs::{FighterValue, NativeInput};
    let _serial = TEST_LOCK.lock().unwrap();
    let source = NativeInput::Fighter {
        value: FighterValue::StatusChanging,
        fighter: "cloud".into(),
    };
    for value in [true, false] {
        let identity = setup();
        ENGINE.lock().unwrap().phase_value = value;
        let snapshot = native_values::sample_behavior_inputs(
            BOID,
            "cloud",
            identity,
            &[source.clone(), source.clone()],
        )
        .unwrap();
        assert_eq!(snapshot.read_flag(&source).unwrap(), value);
        assert_eq!(ENGINE.lock().unwrap().phase_reads, [ACCESSOR]);
        assert!(ENGINE.lock().unwrap().status_reads.is_empty());
        assert!(ENGINE.lock().unwrap().reads.is_empty());
    }
    for change in [
        Change::Stock,
        Change::Accessor,
        Change::Kind,
        Change::Entry,
        Change::Category,
        Change::Inactive,
        Change::NoActor,
        Change::NoRecord,
        Change::NoStock,
    ] {
        let identity = setup();
        ENGINE.lock().unwrap().after_phase_read = Some(change);
        assert!(native_values::sample_behavior_inputs(
            BOID,
            "cloud",
            identity,
            &[source.clone(), NativeInput::CloudLimitGauge]
        )
        .is_err());
        assert!(ENGINE.lock().unwrap().reads.is_empty());
    }
    let identity = setup();
    assert!(native_values::sample_behavior_inputs(
        BOID,
        "cloud",
        identity,
        &[
            source.clone(),
            NativeInput::Fighter {
                value: FighterValue::StatusChanging,
                fighter: "mario".into()
            }
        ]
    )
    .is_err());
    assert!(ENGINE.lock().unwrap().phase_reads.is_empty());
    let mut checks = 0;
    assert!(native_values::sample_behavior_inputs_guarded(
        BOID,
        "cloud",
        identity,
        &[source],
        || {
            checks += 1;
            if checks >= 2 {
                Err("contact was invalidated".into())
            } else {
                Ok(())
            }
        }
    )
    .is_err());
}

#[test]
fn current_status_conditions_share_one_native_sample_and_refresh_next_dispatch() {
    let _serial = TEST_LOCK.lock().unwrap();
    let identity = setup();
    let charge = status_source("FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE");
    let ending = status_source("FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_END");
    assert!(charge.valid_for("cloud"));
    assert!(ending.valid_for("cloud"));
    {
        let mut engine = ENGINE.lock().unwrap();
        engine.status_value = 0x1e6;
        engine.next_status = Some(0x1e7);
    }
    let inputs = [charge.clone(), ending.clone(), charge.clone()];
    let first = native_values::sample_behavior_inputs(BOID, "cloud", identity, &inputs).unwrap();
    assert!(first.read_flag(&charge).unwrap());
    assert!(!first.read_flag(&ending).unwrap());
    assert_eq!(ENGINE.lock().unwrap().status_reads, [ACCESSOR]);
    assert!(ENGINE.lock().unwrap().reads.is_empty());
    let next = native_values::sample_behavior_inputs(BOID, "cloud", identity, &inputs).unwrap();
    assert!(!next.read_flag(&charge).unwrap());
    assert!(next.read_flag(&ending).unwrap());
    assert!(first.read_flag(&charge).unwrap());
    assert_eq!(ENGINE.lock().unwrap().status_reads.len(), 2);
}

#[test]
fn common_and_owned_statuses_share_one_checked_status_read() {
    let _serial = TEST_LOCK.lock().unwrap();
    let identity = setup();
    let sources = [
        status_source("FIGHTER_STATUS_KIND_WAIT"),
        status_source("FIGHTER_STATUS_KIND_SPECIAL_N"),
        status_source("FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE"),
    ];
    for (index, status) in [0, 476, 486].into_iter().enumerate() {
        ENGINE.lock().unwrap().status_value = status;
        let snapshot =
            native_values::sample_behavior_inputs(BOID, "cloud", identity, &sources).unwrap();
        for (choice, source) in sources.iter().enumerate() {
            assert_eq!(snapshot.read_flag(source).unwrap(), choice == index);
        }
    }
    assert_eq!(ENGINE.lock().unwrap().status_reads, [ACCESSOR; 3]);
    assert!(ENGINE.lock().unwrap().reads.is_empty());
}

#[test]
fn current_status_invalid_selectors_prevalidate_without_native_reads() {
    let _serial = TEST_LOCK.lock().unwrap();
    let valid = status_source("FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE");
    for invalid in [
        status_source("FIGHTER_MARIO_STATUS_KIND_SPECIAL_N"),
        status_source("FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK"),
        status_source("WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR"),
        status_source("486"),
        native_inputs::NativeInput::FighterStatusIs {
            fighter: "mario".into(),
            symbol: "FIGHTER_STATUS_KIND_SPECIAL_N".into(),
        },
    ] {
        let identity = setup();
        assert!(native_values::sample_behavior_inputs(
            BOID,
            "cloud",
            identity,
            &[
                valid.clone(),
                native_inputs::NativeInput::CloudLimitGauge,
                invalid
            ]
        )
        .is_err());
        assert!(ENGINE.lock().unwrap().status_reads.is_empty());
        assert!(ENGINE.lock().unwrap().reads.is_empty());
    }
}

fn setup_as(fighter: &str, kind: i32) -> actor_identity::Identity {
    *ENGINE.lock().unwrap() = Engine {
        actor: Some(Actor {
            boid: BOID,
            accessor: ACCESSOR,
            category: 0,
            kind,
            entry: 1,
            active: true,
        }),
        record: Some(slight::agents::AgentRecord {
            boid: BOID,
            module_accessor_addr: ACCESSOR,
            category: 0,
            kind,
            entry_id: 1,
        }),
        stock: Some(1),
        values: BTreeMap::from([
            (0x4C, Scalar::Float(75.0)),
            (0x4E, Scalar::Float(6.5)),
            (0x100000C5, Scalar::Int(9)),
            (0x100000C1, Scalar::Int(12)),
            (0x200000E0, Scalar::Flag(true)),
            (0x100000BE, Scalar::Int(112)),
            (0x100000BD, Scalar::Int(6)),
            (0x200000E3, Scalar::Flag(true)),
        ]),
        ..Default::default()
    };
    actor_identity::Identity::from_live_fighter(BOID, fighter).unwrap()
}

fn attack_source(
    query: native_inputs::AttackResultQuery,
    mask: native_inputs::AttackResultMask,
) -> native_inputs::NativeInput {
    native_inputs::NativeInput::AttackResultFlag {
        fighter: "cloud".into(),
        query,
        mask,
    }
}

#[test]
fn fighter_damage_uses_only_slot_zero_and_shares_the_dispatch_snapshot() {
    use native_inputs::{FighterValue, NativeInput};
    let _serial = TEST_LOCK.lock().unwrap();
    let identity = setup();
    ENGINE.lock().unwrap().damage_value = 42.5;
    let damage = NativeInput::Fighter {
        value: FighterValue::Damage,
        fighter: "cloud".into(),
    };
    let snapshot = native_values::sample_behavior_inputs(
        BOID,
        "cloud",
        identity,
        &[damage.clone(), NativeInput::CloudLimitGauge, damage.clone()],
    )
    .unwrap();
    assert_eq!(snapshot.read(&damage).unwrap(), 42.5);
    assert_eq!(snapshot.read(&NativeInput::CloudLimitGauge).unwrap(), 75.0);
    assert_eq!(ENGINE.lock().unwrap().damage_reads, [(ACCESSOR, 0)]);
    assert_eq!(ENGINE.lock().unwrap().reads, [(ACCESSOR, 0x4C)]);
    assert!(ENGINE.lock().unwrap().attack_reads.is_empty());
    assert!(ENGINE.lock().unwrap().article_reads.is_empty());
    ENGINE.lock().unwrap().damage_value = 99.0;
    assert_eq!(
        snapshot.read(&damage).unwrap(),
        42.5,
        "an existing snapshot must not reread native state"
    );
    let static_snapshot = native_values::sample_static_inputs(
        BOID,
        "cloud",
        identity,
        &[behavior::NativeSource("fighter_damage:cloud")],
    )
    .unwrap();
    assert_eq!(static_snapshot.read(&damage).unwrap(), 99.0);
}

#[test]
fn snapshots_are_typed_ordered_and_read_each_native_source_once() {
    let _serial = TEST_LOCK.lock().unwrap();
    let identity = setup();
    let values =
        native_values::sample_work_values(BOID, "cloud", identity, &[GAUGE, CLEAR_FRAME, POWERED])
            .unwrap();
    assert_eq!(
        values,
        vec![
            native_values::WorkValue::Float(75.0),
            native_values::WorkValue::Int(9),
            native_values::WorkValue::Flag(true)
        ]
    );
    assert_eq!(
        ENGINE.lock().unwrap().reads,
        [
            (ACCESSOR, 0x4C),
            (ACCESSOR, 0x100000C5),
            (ACCESSOR, 0x200000E0)
        ]
    );
    ENGINE
        .lock()
        .unwrap()
        .values
        .insert(0x4C, Scalar::Float(88.0));
    assert_eq!(values[0], native_values::WorkValue::Float(75.0));
    assert_eq!(
        native_values::sample_work_values(BOID, "cloud", identity, &[GAUGE]).unwrap(),
        [native_values::WorkValue::Float(88.0)]
    );
}

#[test]
fn mixed_work_attack_article_sources_keep_order_dedupe_and_all_or_none() {
    use native_inputs::{AttackResultMask as M, AttackResultQuery as Q, NativeInput};
    let _serial = TEST_LOCK.lock().unwrap();
    let identity = setup();
    let article = NativeInput::ArticleExists {
        fighter: "cloud".into(),
        symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_FUSIONSWORD".into(),
    };
    let attack = attack_source(Q::DuringHitlag, M::Hit);
    let article_id = i32::try_from(
        registry::resolve(
            "cloud",
            "FIGHTER_CLOUD_GENERATE_ARTICLE_FUSIONSWORD",
            registry::IdentifierKind::Article,
        )
        .unwrap()
        .value,
    )
    .unwrap();
    ENGINE.lock().unwrap().article_values.insert(article_id, 1);
    ENGINE
        .lock()
        .unwrap()
        .attack_values
        .insert((false, 2), false);
    let sources = [
        article.clone(),
        NativeInput::CloudLimitGauge,
        attack.clone(),
        article.clone(),
        NativeInput::CloudLimitGauge,
        attack.clone(),
    ];
    let snapshot =
        native_values::sample_behavior_inputs(BOID, "cloud", identity, &sources).unwrap();
    assert!(snapshot.read_flag(&article).unwrap());
    assert_eq!(snapshot.read(&NativeInput::CloudLimitGauge).unwrap(), 75.0);
    assert!(!snapshot.read_flag(&attack).unwrap());
    assert_eq!(
        ENGINE.lock().unwrap().ordered_reads,
        [
            (false, ACCESSOR, article_id),
            (false, ACCESSOR, 0x4C),
            (true, ACCESSOR, 2),
        ]
    );

    // A later Work getter can invalidate the fighter identity after the first
    // article sample; the batch returns no snapshot and never reaches AttackModule.
    let identity = setup();
    ENGINE.lock().unwrap().article_values.insert(article_id, 1);
    ENGINE.lock().unwrap().after_read = Some((1, Change::Stock));
    assert!(native_values::sample_behavior_inputs(
        BOID,
        "cloud",
        identity,
        &[article, NativeInput::CloudLimitGauge, attack],
    )
    .is_err());
    assert_eq!(
        ENGINE.lock().unwrap().article_reads,
        [(ACCESSOR, article_id)]
    );
    assert_eq!(ENGINE.lock().unwrap().reads, [(ACCESSOR, 0x4C)]);
    assert!(ENGINE.lock().unwrap().attack_reads.is_empty());
}

#[test]
fn generic_registered_inputs_share_alias_reads_and_static_mapping() {
    let _serial = TEST_LOCK.lock().unwrap();
    let identity = setup_as("robot", 0x2D);
    let generic_alias = native_inputs::NativeInput::from_registry(
        "robot",
        "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE",
        native_inputs::NativeKind::Float,
    )
    .unwrap();
    let sources = [
        native_inputs::NativeInput::RobotBeamEnergy,
        generic_alias.clone(),
    ];
    let snapshot =
        native_values::sample_behavior_inputs(BOID, "robot", identity, &sources).unwrap();
    assert_eq!(snapshot.read(&sources[0]).unwrap(), 75.0);
    assert_eq!(snapshot.read(&sources[1]).unwrap(), 75.0);
    assert_eq!(
        ENGINE.lock().unwrap().reads,
        [(ACCESSOR, 0x4C)],
        "named and generic aliases of a slot must cause one physical getter"
    );

    ENGINE.lock().unwrap().reads.clear();
    let burner = native_inputs::NativeInput::from_registry(
        "robot",
        "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BURNER_ENERGY_VALUE",
        native_inputs::NativeKind::Float,
    )
    .unwrap();
    let integer = native_inputs::NativeInput::from_registry(
        "robot",
        "FIGHTER_ROBOT_INSTANCE_WORK_ID_INT_BEAM_GLOW_ATTACK",
        native_inputs::NativeKind::Int,
    )
    .unwrap();
    let flag = native_inputs::NativeInput::from_registry(
        "robot",
        "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLAG_ARMSPIN_PUSH_B",
        native_inputs::NativeKind::Flag,
    )
    .unwrap();
    let generic_sources = [burner.clone(), integer.clone(), flag.clone()];
    let generic_snapshot =
        native_values::sample_behavior_inputs(BOID, "robot", identity, &generic_sources).unwrap();
    assert_eq!(generic_snapshot.read(&burner).unwrap(), 6.5);
    assert_eq!(generic_snapshot.read_int(&integer).unwrap(), 12);
    assert!(generic_snapshot.read_flag(&flag).unwrap());
    assert_eq!(
        ENGINE.lock().unwrap().reads,
        [
            (ACCESSOR, 0x4E),
            (ACCESSOR, 0x100000C1),
            (ACCESSOR, 0x200000E0)
        ]
    );

    ENGINE.lock().unwrap().reads.clear();
    let static_snapshot = native_values::sample_static_inputs(
        BOID,
        "robot",
        identity,
        &[
            behavior::NativeSource(
                "work_float:robot:FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BURNER_ENERGY_VALUE",
            ),
            behavior::NativeSource(
                "work_int:robot:FIGHTER_ROBOT_INSTANCE_WORK_ID_INT_BEAM_GLOW_ATTACK",
            ),
            behavior::NativeSource(
                "work_flag:robot:FIGHTER_ROBOT_INSTANCE_WORK_ID_FLAG_ARMSPIN_PUSH_B",
            ),
        ],
    )
    .unwrap();
    assert_eq!(static_snapshot.read(&burner).unwrap(), 6.5);
    assert_eq!(static_snapshot.read_int(&integer).unwrap(), 12);
    assert!(static_snapshot.read_flag(&flag).unwrap());
    assert_eq!(
        ENGINE.lock().unwrap().reads,
        [
            (ACCESSOR, 0x4E),
            (ACCESSOR, 0x100000C1),
            (ACCESSOR, 0x200000E0)
        ]
    );
}

#[test]
fn ownership_changes_during_getter_discard_the_batch_and_stop_reads() {
    let _serial = TEST_LOCK.lock().unwrap();
    for change in [
        Change::Stock,
        Change::Accessor,
        Change::Kind,
        Change::Entry,
        Change::Category,
        Change::Inactive,
        Change::NoActor,
        Change::NoRecord,
        Change::NoStock,
    ] {
        for at in 1..=3 {
            let identity = setup();
            ENGINE.lock().unwrap().after_read = Some((at, change));
            assert!(native_values::sample_work_values(
                BOID,
                "cloud",
                identity,
                &[GAUGE, CLEAR_FRAME, POWERED]
            )
            .is_err());
            assert_eq!(
                ENGINE.lock().unwrap().reads,
                [
                    (ACCESSOR, 0x4C),
                    (ACCESSOR, 0x100000C5),
                    (ACCESSOR, 0x200000E0)
                ][..at]
            );
        }
    }
}

#[test]
fn pending_damage_external_context_guard_stops_each_sampling_boundary_without_partial_snapshot() {
    use native_inputs::NativeInput;
    let _serial = TEST_LOCK.lock().unwrap();
    for stop_at in 1..=5 {
        let identity = setup();
        let mut checks = 0;
        let result = native_values::sample_behavior_inputs_guarded(
            BOID,
            "cloud",
            identity,
            &[
                NativeInput::CloudLimitGauge,
                NativeInput::CloudLimitClearFrame,
            ],
            || {
                checks += 1;
                if checks == stop_at {
                    Err("contact participant changed".into())
                } else {
                    Ok(())
                }
            },
        );
        assert!(result.is_err(), "guard boundary {stop_at} escaped");
        assert_eq!(ENGINE.lock().unwrap().reads.len(), (stop_at - 1) / 2);
    }
    let identity = setup();
    assert!(
        native_values::sample_behavior_inputs_guarded(BOID, "cloud", identity, &[], || Err(
            "stale contact".into()
        ))
        .is_err()
    );
    assert!(ENGINE.lock().unwrap().reads.is_empty());
}

#[test]
fn actual_zero_and_false_are_valid_values_not_unavailable_samples() {
    let _serial = TEST_LOCK.lock().unwrap();
    let identity = setup();
    ENGINE.lock().unwrap().values = BTreeMap::from([
        (0x4C, Scalar::Float(0.0)),
        (0x100000C5, Scalar::Int(0)),
        (0x200000E0, Scalar::Flag(false)),
    ]);
    assert_eq!(
        native_values::sample_work_values(BOID, "cloud", identity, &[GAUGE, CLEAR_FRAME, POWERED])
            .unwrap(),
        [
            native_values::WorkValue::Float(0.0),
            native_values::WorkValue::Int(0),
            native_values::WorkValue::Flag(false)
        ]
    );
}

#[test]
fn controller_input_uses_distinct_edges_axes_and_rejects_identity_changes_between_getters() {
    use native_inputs::{ButtonEdge, InputButton};
    let _serial = TEST_LOCK.lock().unwrap();
    for change_at in [None, Some(1), Some(16), Some(17)] {
        let identity = setup_as("robot", 0x2D);
        {
            let mut engine = ENGINE.lock().unwrap();
            for id in 101..=105 {
                engine.values.insert(id, Scalar::Flag(true));
                engine.values.insert(id + 10, Scalar::Flag(false));
                engine.values.insert(id + 20, Scalar::Flag(id == 102));
            }
            for (id, value) in [(-1, 0.75), (-2, -0.5), (-3, -1.0)] {
                engine.values.insert(id, Scalar::Float(value));
            }
            engine.after_read = change_at.map(|at| (at, Change::Stock));
        }
        let result = native_values::sample_controller_input(BOID, "robot", identity);
        if let Some(at) = change_at {
            assert!(result.is_err());
            assert_eq!(ENGINE.lock().unwrap().reads.len(), at);
        } else {
            let input = result.unwrap();
            assert_eq!(
                (input.stick_x, input.stick_y, input.facing),
                (0.75, -0.5, -1.0)
            );
            assert!(input.buttons[&(InputButton::Special, ButtonEdge::Held)]);
            assert!(!input.buttons[&(InputButton::Special, ButtonEdge::Pressed)]);
            assert!(input.buttons[&(InputButton::Special, ButtonEdge::Released)]);
            assert!(!input.buttons[&(InputButton::Attack, ButtonEdge::Released)]);
        }
    }
}

#[test]
fn native_work_writes_use_exact_setter_and_add_rechecks_identity_after_the_read() {
    use live_model::{Action, CmpOp, Expr, Predicate, Value};
    use native_inputs::{NativeInput, NativeKind};
    let _serial = TEST_LOCK.lock().unwrap();
    for (symbol, kind, value, initial) in [
        (
            "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE",
            NativeKind::Float,
            Value::Float(2.5),
            Scalar::Float(4.0),
        ),
        (
            "FIGHTER_ROBOT_INSTANCE_WORK_ID_INT_BEAM_GLOW_ATTACK",
            NativeKind::Int,
            Value::Int(2),
            Scalar::Int(4),
        ),
    ] {
        let input = NativeInput::from_registry("robot", symbol, kind).unwrap();
        let registry_kind = if kind == NativeKind::Float {
            registry::IdentifierKind::WorkFloat
        } else {
            registry::IdentifierKind::WorkInt
        };
        let id = registry::resolve("robot", symbol, registry_kind)
            .unwrap()
            .value as i32;
        let identity = setup_as("robot", 0x2D);
        ENGINE.lock().unwrap().values.insert(id, initial);
        let action = Action::AddNative {
            input: input.clone(),
            value: Expr::Const(value),
        };
        native_values::execute_write(BOID, "robot", identity, &action).unwrap();
        assert_eq!(
            ENGINE.lock().unwrap().values[&id],
            if kind == NativeKind::Float {
                Scalar::Float(6.5)
            } else {
                Scalar::Int(6)
            }
        );
        let action = Action::SetNative {
            input: input.clone(),
            value: Expr::Const(value),
        };
        native_values::execute_write(BOID, "robot", identity, &action).unwrap();
        assert_eq!(
            ENGINE.lock().unwrap().values[&id],
            if kind == NativeKind::Float {
                Scalar::Float(2.5)
            } else {
                Scalar::Int(2)
            }
        );
        for change in [Change::Stock, Change::Accessor, Change::Inactive] {
            let identity = setup_as("robot", 0x2D);
            {
                let mut engine = ENGINE.lock().unwrap();
                engine.values.insert(id, initial);
                engine.after_read = Some((1, change));
            }
            assert!(native_values::execute_write(
                BOID,
                "robot",
                identity,
                &Action::AddNative {
                    input: input.clone(),
                    value: Expr::Const(value)
                }
            )
            .is_err());
            assert_eq!(ENGINE.lock().unwrap().values[&id], initial);
        }
    }
    let identity = setup_as("robot", 0x2D);
    let symbol = "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLAG_ARMSPIN_PUSH_B";
    let input = NativeInput::from_registry("robot", symbol, NativeKind::Flag).unwrap();
    let id = registry::resolve("robot", symbol, registry::IdentifierKind::WorkFlag)
        .unwrap()
        .value as i32;
    for value in [1, 0] {
        let action = Action::SetNativeFlag {
            input: input.clone(),
            value: Predicate::Compare {
                op: CmpOp::Eq,
                left: Expr::Const(Value::Int(value)),
                right: Expr::Const(Value::Int(1)),
            },
        };
        native_values::execute_write(BOID, "robot", identity, &action).unwrap();
        assert_eq!(ENGINE.lock().unwrap().values[&id], Scalar::Flag(value == 1));
    }
    assert!(native_values::execute_write(
        BOID,
        "robot",
        identity,
        &Action::SetNative {
            input,
            value: Expr::Const(Value::Float(1.0))
        }
    )
    .is_err());
    let identity = setup();
    assert!(native_values::execute_write(
        BOID,
        "cloud",
        identity,
        &Action::SetNative {
            input: NativeInput::CloudLimitGauge,
            value: Expr::Const(Value::Float(50.0))
        }
    )
    .is_err());
    assert!(ENGINE.lock().unwrap().reads.is_empty());
}

#[test]
fn owned_article_count_rejects_foreign_symbols_and_identity_changes_during_read() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let source = native_inputs::NativeInput::article_count("FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE");
    for count in [0, 1, 4] {
        let identity = setup();
        ENGINE.lock().unwrap().article_values.insert(0, count);
        let snapshot = native_values::sample_behavior_inputs(
            BOID,
            "cloud",
            identity,
            std::slice::from_ref(&source),
        )
        .unwrap();
        assert_eq!(snapshot.read_int(&source).unwrap(), count);
        assert_eq!(ENGINE.lock().unwrap().article_reads, [(ACCESSOR, 0)]);
    }
    let identity = setup();
    ENGINE.lock().unwrap().article_values.insert(0, 3);
    ENGINE.lock().unwrap().after_article_read = Some((1, Change::Stock));
    assert!(native_values::sample_behavior_inputs(
        BOID,
        "cloud",
        identity,
        std::slice::from_ref(&source)
    )
    .is_err());
    for symbol in [
        "FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO",
        "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE",
        "missing",
    ] {
        let identity = setup();
        assert!(native_values::sample_behavior_inputs(
            BOID,
            "cloud",
            identity,
            &[native_inputs::NativeInput::article_count(symbol)]
        )
        .is_err());
        assert!(ENGINE.lock().unwrap().article_reads.is_empty());
    }
}

#[export_name = "_ZN3lib9SingletonIN3app14FighterManagerEE9instance_E"]
static mut FIGHTER_MANAGER_STUB: *mut app::FighterManager = std::ptr::dangling_mut();

#[test]
fn stocks_sample_self_and_highest_active_opponent_once_per_snapshot() {
    use native_inputs::{FighterValue, NativeInput};
    let _guard = TEST_LOCK.lock().unwrap();
    let identity = setup();
    let own = NativeInput::Fighter {
        value: FighterValue::StocksRemaining,
        fighter: "cloud".into(),
    };
    let opponent = NativeInput::Fighter {
        value: FighterValue::OpponentStocksRemaining,
        fighter: "cloud".into(),
    };
    {
        let mut engine = ENGINE.lock().unwrap();
        engine.stocks = BTreeMap::from([(1, 2), (2, 1), (3, 4), (4, 9)]);
        let actor = engine.actor.unwrap();
        engine.other_actors = vec![Actor {
            boid: BOID + 1,
            accessor: ACCESSOR + 1,
            entry: 2,
            ..actor
        }];
    }
    let snapshot = native_values::sample_behavior_inputs(
        BOID,
        "cloud",
        identity,
        &[own.clone(), opponent.clone(), own.clone()],
    )
    .unwrap();
    assert_eq!(snapshot.read_int(&own).unwrap(), 2);
    assert_eq!(snapshot.read_int(&opponent).unwrap(), 1);
    assert_eq!(ENGINE.lock().unwrap().stock_reads, [1, 2]);
    {
        let mut engine = ENGINE.lock().unwrap();
        let actor = engine.actor.unwrap();
        engine.other_actors.extend([
            Actor {
                boid: BOID + 2,
                accessor: ACCESSOR + 2,
                entry: 3,
                ..actor
            },
            Actor {
                boid: BOID + 3,
                accessor: ACCESSOR + 3,
                entry: 4,
                active: false,
                ..actor
            },
        ]);
    }
    let snapshot = native_values::sample_behavior_inputs(
        BOID,
        "cloud",
        identity,
        std::slice::from_ref(&opponent),
    )
    .unwrap();
    assert_eq!(snapshot.read_int(&opponent).unwrap(), 4);
    ENGINE.lock().unwrap().other_actors.clear();
    assert!(native_values::sample_behavior_inputs(BOID, "cloud", identity, &[opponent]).is_err());
    ENGINE.lock().unwrap().stocks.clear();
    assert!(native_values::sample_behavior_inputs(BOID, "cloud", identity, &[own]).is_err());
}

#[test]
fn opponent_values_read_the_nearest_live_other_player_and_are_defined_without_one() {
    use native_inputs::{FighterValue as V, NativeInput};
    let _guard = TEST_LOCK.lock().unwrap();
    let source = |value| NativeInput::Fighter {
        value,
        fighter: "cloud".into(),
    };
    let sources = [
        V::OpponentPresent,
        V::OpponentDamage,
        V::OpponentDx,
        V::OpponentDxForward,
        V::OpponentDy,
    ]
    .map(source);
    let identity = setup();
    {
        let mut engine = ENGINE.lock().unwrap();
        let actor = engine.actor.unwrap();
        // I stand at (-30, 5) facing left.
        engine.values.extend([
            (-6, Scalar::Float(-30.0)),
            (-7, Scalar::Float(5.0)),
            (-3, Scalar::Float(-1.0)),
        ]);
        let at = |offset: u32, entry, x: f32| {
            (
                Actor {
                    boid: BOID + offset,
                    accessor: ACCESSOR + u64::from(offset),
                    entry,
                    ..actor
                },
                x,
            )
        };
        let actors = [
            at(1, 2, 200.0), // far: not chosen
            at(2, 3, -50.0), // the nearest live opponent
            at(3, 4, -31.0), // nearer, but KO'd
            at(4, 5, -31.0), // nearer, but in standby
            at(5, 1, -31.0), // nearer, but my own player entry
            (
                Actor {
                    active: false,
                    ..at(6, 6, -31.0).0
                },
                -31.0,
            ),
        ];
        for (actor, x) in actors {
            engine.other_actors.push(actor);
            engine
                .weapon_values
                .insert((actor.accessor, -6), Scalar::Float(x));
            engine
                .weapon_values
                .insert((actor.accessor, -7), Scalar::Float(15.0));
            engine.other_damage.insert(actor.accessor, 42.5);
        }
        engine.other_damage.insert(ACCESSOR + 1, 7.0);
        engine.other_status.insert(ACCESSOR + 3, 0xB5);
        engine.other_status.insert(ACCESSOR + 4, 0x1D6);
    }
    let snapshot =
        native_values::sample_behavior_inputs(BOID, "cloud", identity, &sources).unwrap();
    assert!(snapshot.read_flag(&sources[0]).unwrap());
    assert_eq!(snapshot.read(&sources[1]).unwrap(), 42.5);
    assert_eq!(
        snapshot.read(&sources[2]).unwrap(),
        -20.0,
        "raw world distance"
    );
    assert_eq!(
        snapshot.read(&sources[3]).unwrap(),
        20.0,
        "in front of a fighter facing left"
    );
    assert_eq!(snapshot.read(&sources[4]).unwrap(), 10.0);
    // The sole live opponent is KO'd: a defined reading, not a failed batch.
    ENGINE
        .lock()
        .unwrap()
        .other_actors
        .retain(|actor| actor.boid == BOID + 3);
    let snapshot =
        native_values::sample_behavior_inputs(BOID, "cloud", identity, &sources).unwrap();
    assert!(!snapshot.read_flag(&sources[0]).unwrap());
    for reading in &sources[1..] {
        assert_eq!(snapshot.read(reading).unwrap(), 0.0);
    }
}

#[test]
fn common_work_samples_its_own_id_and_writes_only_the_allowlist_within_bounds() {
    use live_model::{Action, Expr, Value};
    use native_inputs::{NativeInput, NativeKind};
    let _guard = TEST_LOCK.lock().unwrap();
    let identity = setup();
    let max =
        NativeInput::from_reference("work_int:cloud:FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT_MAX");
    let shield =
        NativeInput::from_reference("work_float:cloud:FIGHTER_INSTANCE_WORK_ID_FLOAT_GUARD_SHIELD");
    assert_eq!(max.kind(), Some(NativeKind::Int));
    {
        let mut engine = ENGINE.lock().unwrap();
        engine.values.insert(0x1000_0003, Scalar::Int(2));
        engine.values.insert(0x6, Scalar::Float(50.0));
    }
    let snapshot = native_values::sample_behavior_inputs(
        BOID,
        "cloud",
        identity,
        &[max.clone(), shield.clone()],
    )
    .unwrap();
    assert_eq!(snapshot.read_int(&max).unwrap(), 2);
    assert_eq!(snapshot.read(&shield).unwrap(), 50.0);
    let stored = || ENGINE.lock().unwrap().values[&0x1000_0003];
    let set = |value| Action::SetNative {
        input: max.clone(),
        value: Expr::Const(Value::Int(value)),
    };
    let add = |value| Action::AddNative {
        input: max.clone(),
        value: Expr::Const(Value::Int(value)),
    };
    native_values::execute_write(BOID, "cloud", identity, &set(3)).unwrap();
    assert_eq!(stored(), Scalar::Int(3));
    native_values::execute_write(BOID, "cloud", identity, &add(1)).unwrap();
    assert_eq!(stored(), Scalar::Int(4));
    // A computed value past the range clamps instead of reaching the engine.
    native_values::execute_write(BOID, "cloud", identity, &set(99)).unwrap();
    assert_eq!(stored(), Scalar::Int(10));
    native_values::execute_write(BOID, "cloud", identity, &add(-99)).unwrap();
    assert_eq!(stored(), Scalar::Int(0));
    let wall =
        NativeInput::from_reference("work_int:cloud:FIGHTER_INSTANCE_WORK_ID_INT_WALL_JUMP_COUNT");
    assert!(wall.is_supported());
    let refused = Action::SetNative {
        input: wall,
        value: Expr::Const(Value::Int(0)),
    };
    assert!(native_values::execute_write(BOID, "cloud", identity, &refused).is_err());
}

#[test]
fn movement_reads_use_main_speed_and_posture_with_identity_fences() {
    use native_inputs::{FighterValue, NativeInput};
    let _serial = TEST_LOCK.lock().unwrap();
    for (source, id, value) in [
        (
            NativeInput::Fighter {
                value: FighterValue::SpeedX,
                fighter: "cloud".into(),
            },
            -4,
            -2.5,
        ),
        (
            NativeInput::Fighter {
                value: FighterValue::SpeedY,
                fighter: "cloud".into(),
            },
            -5,
            1.5,
        ),
        (
            NativeInput::Fighter {
                value: FighterValue::PositionX,
                fighter: "cloud".into(),
            },
            -6,
            -30.0,
        ),
        (
            NativeInput::Fighter {
                value: FighterValue::PositionY,
                fighter: "cloud".into(),
            },
            -7,
            15.0,
        ),
        (
            NativeInput::Fighter {
                value: FighterValue::Facing,
                fighter: "cloud".into(),
            },
            -3,
            -1.0,
        ),
    ] {
        let identity = setup();
        ENGINE
            .lock()
            .unwrap()
            .values
            .insert(id, Scalar::Float(value));
        let snapshot = native_values::sample_behavior_inputs(
            BOID,
            "cloud",
            identity,
            &[source.clone(), source.clone()],
        )
        .unwrap();
        assert_eq!(snapshot.read(&source).unwrap(), value);
        assert_eq!(ENGINE.lock().unwrap().reads, [(ACCESSOR, id)]);
        for change in [
            Change::Stock,
            Change::Accessor,
            Change::Inactive,
            Change::Kind,
            Change::Entry,
        ] {
            let identity = setup();
            {
                let mut engine = ENGINE.lock().unwrap();
                engine.values.insert(id, Scalar::Float(value));
                engine.after_read = Some((1, change));
            }
            assert!(native_values::sample_behavior_inputs(
                BOID,
                "cloud",
                identity,
                std::slice::from_ref(&source)
            )
            .is_err());
        }
        for value in [f32::NAN, f32::INFINITY, 1_000_001.0] {
            let identity = setup();
            ENGINE
                .lock()
                .unwrap()
                .values
                .insert(id, Scalar::Float(value));
            assert!(native_values::sample_behavior_inputs(
                BOID,
                "cloud",
                identity,
                std::slice::from_ref(&source)
            )
            .is_err());
        }
    }
}

#[test]
fn reviewed_status_values_are_immutable_and_guard_each_read() {
    use native_inputs::{Axis, NativeInput};
    use status_commands::Energy;
    let _serial = TEST_LOCK.lock().unwrap();
    let parameter = NativeInput::ParamFloat {
        fighter: "mario".into(),
        table: "param_special_s".into(),
        key: "special_s_attack_acl_y".into(),
    };
    let speed = NativeInput::EnergySpeed {
        fighter: "mario".into(),
        energy: Energy::Stop,
        axis: Axis::X,
    };
    for source in [&parameter, &speed] {
        let encoded = serde_json::to_string(source).unwrap();
        assert_eq!(
            serde_json::from_str::<NativeInput>(&encoded).unwrap(),
            *source
        );
        assert!(!source.valid_for("fox"));
    }
    let identity = setup_as("mario", 0);
    {
        let mut e = ENGINE.lock().unwrap();
        e.values.insert(-8, Scalar::Float(0.025));
        e.values.insert(-4, Scalar::Float(1.25));
    }
    let snapshot = native_values::sample_behavior_inputs(
        BOID,
        "mario",
        identity,
        &[parameter.clone(), speed.clone()],
    )
    .unwrap();
    ENGINE.lock().unwrap().values.insert(-4, Scalar::Float(8.0));
    assert_eq!(snapshot.read(&parameter).unwrap(), 0.025);
    assert_eq!(snapshot.read(&speed).unwrap(), 1.25);
    for change in [
        Change::Stock,
        Change::Accessor,
        Change::Kind,
        Change::Entry,
        Change::Inactive,
    ] {
        let identity = setup_as("mario", 0);
        {
            let mut e = ENGINE.lock().unwrap();
            e.values.insert(-8, Scalar::Float(0.025));
            e.after_read = Some((1, change));
        }
        assert!(native_values::sample_behavior_inputs(
            BOID,
            "mario",
            identity,
            std::slice::from_ref(&parameter)
        )
        .is_err());
    }
    let invalid = NativeInput::ParamFloat {
        fighter: "mario".into(),
        table: "param_special_s".into(),
        key: "invented".into(),
    };
    let identity = setup_as("mario", 0);
    assert!(native_values::sample_behavior_inputs(BOID, "mario", identity, &[invalid]).is_err());
    assert!(ENGINE.lock().unwrap().reads.is_empty());
    let int = NativeInput::ParamInt {
        fighter: "fox".into(),
        table: "param_special_lw".into(),
        key: "reflector_air_stop_y_frame".into(),
    };
    let identity = setup_as("fox", 7);
    ENGINE.lock().unwrap().values.insert(-9, Scalar::Int(4));
    assert_eq!(
        native_values::sample_behavior_inputs(BOID, "fox", identity, std::slice::from_ref(&int))
            .unwrap()
            .read_int(&int)
            .unwrap(),
        4
    );
    let wrong_type = NativeInput::ParamFloat {
        fighter: "fox".into(),
        table: "param_special_lw".into(),
        key: "reflector_air_stop_y_frame".into(),
    };
    assert!(!wrong_type.is_supported());
    let mut checks = 0;
    assert!(
        native_values::sample_behavior_inputs_guarded(BOID, "fox", identity, &[int], || {
            checks += 1;
            if checks == 3 {
                Err("entry epoch changed".into())
            } else {
                Ok(())
            }
        })
        .is_err()
    );
}

#[test]
fn held_item_kind_samples_main_hand_once_and_rejects_changed_identity() {
    use native_inputs::{FighterValue, ItemAction, NativeInput};
    let _serial = TEST_LOCK.lock().unwrap();
    let source = NativeInput::Fighter {
        value: FighterValue::HeldItemKind,
        fighter: "cloud".into(),
    };
    for value in [-1, ItemAction::item_kind("ITEM_KIND_PEACHDAIKON").unwrap()] {
        let identity = setup();
        ENGINE
            .lock()
            .unwrap()
            .values
            .insert(-10, Scalar::Int(value));
        let snapshot = native_values::sample_behavior_inputs(
            BOID,
            "cloud",
            identity,
            &[source.clone(), source.clone()],
        )
        .unwrap();
        assert_eq!(snapshot.read_int(&source).unwrap(), value);
        assert_eq!(ENGINE.lock().unwrap().reads, [(ACCESSOR, -10)]);
    }
    for change in [
        Change::Stock,
        Change::Accessor,
        Change::Inactive,
        Change::Kind,
        Change::Entry,
    ] {
        let identity = setup();
        {
            let mut engine = ENGINE.lock().unwrap();
            engine.values.insert(-10, Scalar::Int(121));
            engine.after_read = Some((1, change));
        }
        assert!(native_values::sample_behavior_inputs(
            BOID,
            "cloud",
            identity,
            std::slice::from_ref(&source)
        )
        .is_err());
    }
}

#[test]
fn command_flags_use_category_four_mask_and_identity_fences() {
    use native_inputs::NativeInput;
    let _serial = TEST_LOCK.lock().unwrap();
    let source = NativeInput::Command {
        fighter: "ryu".into(),
        symbol: "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND".into(),
    };
    for (bits, expected) in [(0, false), (2, false), (3, true)] {
        let identity = setup_as("ryu", 0x3c);
        ENGINE
            .lock()
            .unwrap()
            .values
            .insert(-100, Scalar::Int(bits));
        let snapshot = native_values::sample_behavior_inputs(
            BOID,
            "ryu",
            identity,
            &[source.clone(), source.clone()],
        )
        .unwrap();
        assert_eq!(snapshot.read_flag(&source).unwrap(), expected);
        assert_eq!(ENGINE.lock().unwrap().reads, [(ACCESSOR, -100)]);
    }
    for change in [
        Change::Stock,
        Change::Accessor,
        Change::Inactive,
        Change::Kind,
        Change::Entry,
    ] {
        let identity = setup_as("ryu", 0x3c);
        {
            let mut engine = ENGINE.lock().unwrap();
            engine.values.insert(-100, Scalar::Int(1));
            engine.after_read = Some((1, change));
        }
        assert!(native_values::sample_behavior_inputs(
            BOID,
            "ryu",
            identity,
            std::slice::from_ref(&source)
        )
        .is_err());
    }
}

#[test]
fn projectile_reads_newest_owned_and_writes_all_owned_without_touching_fighter_or_foreign_weapon() {
    use live_model::{Action, Expr, Value};
    use native_inputs::{NativeInput, NativeKind};
    let _serial = TEST_LOCK.lock().unwrap();
    let identity = setup_as("mario", registry::fighter_kind_value("mario").unwrap());
    let source = NativeInput::from_registry(
        "mario",
        "WEAPON_MARIO_FIREBALL_INSTANCE_WORK_ID_INT_BOUND_COUNT",
        NativeKind::Int,
    )
    .unwrap();
    let snapshot = native_values::sample_behavior_inputs(
        BOID,
        "mario",
        identity,
        std::slice::from_ref(&source),
    )
    .unwrap();
    assert_eq!(snapshot.read_int(&source).unwrap(), 0);
    assert!(ENGINE.lock().unwrap().reads.is_empty());
    let id = 0x1000000A;
    let article = registry::resolve(
        "mario",
        "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL",
        registry::IdentifierKind::Article,
    )
    .unwrap()
    .value as i32;
    {
        let mut engine = ENGINE.lock().unwrap();
        engine.article_instances = vec![10, 11, 12];
        engine.owned_weapons = vec![10, 11];
        engine.article_values.insert(article, 3);
        engine.values.insert(id, Scalar::Int(777));
        for (boid, value) in [(10, 4), (11, 8), (12, 12)] {
            engine.other_actors.push(Actor {
                boid,
                accessor: boid as u64 + 100,
                category: 1,
                kind: 0,
                entry: -1,
                active: true,
            });
            engine
                .weapon_values
                .insert((boid as u64 + 100, id), Scalar::Int(value));
        }
    }
    let snapshot = native_values::sample_behavior_inputs(
        BOID,
        "mario",
        identity,
        std::slice::from_ref(&source),
    )
    .unwrap();
    assert_eq!(snapshot.read_int(&source).unwrap(), 8);
    let static_values = native_values::sample_work_values(
        BOID,
        "mario",
        identity,
        &[native_values::WorkSource {
            symbol: "WEAPON_MARIO_FIREBALL_INSTANCE_WORK_ID_INT_BOUND_COUNT",
            kind: registry::IdentifierKind::WorkInt,
        }],
    )
    .unwrap();
    assert!(matches!(
        static_values.as_slice(),
        [native_values::WorkValue::Int(8)]
    ));
    native_values::execute_write(
        BOID,
        "mario",
        identity,
        &Action::SetNative {
            input: source.clone(),
            value: Expr::Const(Value::Int(99)),
        },
    )
    .unwrap();
    let engine = ENGINE.lock().unwrap();
    assert_eq!(engine.weapon_values[&(110, id)], Scalar::Int(99));
    assert_eq!(engine.weapon_values[&(111, id)], Scalar::Int(99));
    assert_eq!(engine.weapon_values[&(112, id)], Scalar::Int(12));
    assert_eq!(engine.values[&id], Scalar::Int(777));
    assert_eq!(snapshot.read_int(&source).unwrap(), 8);
}

#[test]
fn owned_object_reads_the_newest_live_owned_weapon_and_absence_reads_zero() {
    use native_inputs::{NativeInput, ObjectField};
    let _serial = TEST_LOCK.lock().unwrap();
    let identity = setup_as("mario", registry::fighter_kind_value("mario").unwrap());
    let input = |field| NativeInput::OwnedObject {
        fighter: "mario".into(),
        symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
        field,
    };
    let sources: Vec<_> = ObjectField::ALL.into_iter().map(input).collect();
    let absent = native_values::sample_behavior_inputs(BOID, "mario", identity, &sources).unwrap();
    assert!(!absent.read_flag(&input(ObjectField::Alive)).unwrap());
    assert_eq!(absent.read(&input(ObjectField::SpeedX)).unwrap(), 0.0);
    {
        let mut engine = ENGINE.lock().unwrap();
        engine.article_instances = vec![10, 11, 12];
        engine.owned_weapons = vec![10, 11];
        for (boid, base) in [(10, 1.0), (11, 2.0), (12, 3.0)] {
            let accessor = boid as u64 + 100;
            engine.other_actors.push(Actor {
                boid,
                accessor,
                category: 1,
                kind: 0,
                entry: -1,
                active: true,
            });
            for (id, offset) in [(-6, 0.0), (-7, 0.5), (-4, 0.25), (-5, 0.75)] {
                engine
                    .weapon_values
                    .insert((accessor, id), Scalar::Float(base + offset));
            }
        }
    }
    let live = native_values::sample_behavior_inputs(BOID, "mario", identity, &sources).unwrap();
    // 12 is newer but not owned; 11 is the newest owned weapon.
    assert!(live.read_flag(&input(ObjectField::Alive)).unwrap());
    assert_eq!(live.read(&input(ObjectField::PosX)).unwrap(), 2.0);
    assert_eq!(live.read(&input(ObjectField::PosY)).unwrap(), 2.5);
    assert_eq!(live.read(&input(ObjectField::SpeedX)).unwrap(), 2.25);
    assert_eq!(live.read(&input(ObjectField::SpeedY)).unwrap(), 2.75);
}
