//! Host harness for the exact live behavior plugin module.
//!
//! The harness stubs only the engine object table, agent sampler, dispatcher,
//! and allowlisted operations. It includes the production `live_model.rs` and
//! `live_behavior.rs` sources unchanged, so protocol/replacement/routing/state
//! behavior runs on the host without linking Skyline.

#![allow(dead_code, clippy::missing_safety_doc)]

#[path = "slight/controller_runtime_tests.rs"]
mod controller_adapter_tests;

#[path = "slight/article_lifetime.rs"]
mod article_lifetime_tests;

#[path = "slight/damage_expression_tests.rs"]
mod damage_expression_tests;

use std::sync::LazyLock;

static TEST_LOCK: LazyLock<std::sync::Mutex<()>> = LazyLock::new(|| std::sync::Mutex::new(()));

fn mock_fighter_kind(boid: u32) -> i32 {
    match boid {
        0xF0AA_0009 | 0xF0AA_000B => 62,
        0xF0AA_00A4 => 0x2C,
        0xF0AA_00A5 => 0xD,
        _ => 0x7F,
    }
}

extern crate self as parking_lot;
extern crate self as smash;

pub mod lib {
    pub mod lua_const {
        pub static FIGHTER_STATUS_KIND_SPECIAL_LW: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 0x1DF);
        pub static FIGHTER_STATUS_KIND_DEAD: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 181);
        pub static FIGHTER_STATUS_KIND_REBIRTH: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 182);
        pub static FIGHTER_EFLAME_STATUS_KIND_SPECIAL_LW_STANDBY: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 481);
        pub static FIGHTER_EFLAME_STATUS_KIND_SPECIAL_LW_OUT: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 482);
        pub static FIGHTER_ELIGHT_STATUS_KIND_SPECIAL_LW_STANDBY: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 481);
        pub static FIGHTER_ELIGHT_STATUS_KIND_SPECIAL_LW_OUT: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 482);
        pub static FIGHTER_PZENIGAME_STATUS_KIND_SPECIAL_LW_STANDBY: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 481);
        pub static FIGHTER_PZENIGAME_STATUS_KIND_SPECIAL_LW_OUT: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 482);
        pub static FIGHTER_PFUSHIGISOU_STATUS_KIND_SPECIAL_LW_STANDBY: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 481);
        pub static FIGHTER_PFUSHIGISOU_STATUS_KIND_SPECIAL_LW_OUT: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 482);
        pub static FIGHTER_PLIZARDON_STATUS_KIND_SPECIAL_LW_STANDBY: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 481);
        pub static FIGHTER_PLIZARDON_STATUS_KIND_SPECIAL_LW_OUT: std::sync::LazyLock<i32> =
            std::sync::LazyLock::new(|| 482);

        pub static FIGHTER_KIND_CLOUD: std::sync::LazyLock<i32> = std::sync::LazyLock::new(|| 62);
    }
}

pub struct Mutex<T>(std::sync::Mutex<T>);

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
}

pub mod app {
    pub mod lua_bind {
        pub struct StatusModule;
        impl StatusModule {
            pub unsafe fn status_kind(module: *mut super::BattleObjectModuleAccessor) -> i32 {
                crate::slight::agents::all_records()
                    .iter()
                    .find(|r| r.module_accessor_addr == module as u64)
                    .map_or(0, |r| r.status_kind)
            }
        }
    }
    pub struct BattleObjectModuleAccessor;

    pub mod sv_battle_object {
        use std::collections::BTreeMap;
        use std::sync::{LazyLock, Mutex};

        #[derive(Clone, Copy)]
        struct EngineObject {
            accessor: u64,
            category: i32,
            kind: i32,
            entry_id: i32,
            active: bool,
        }

        static OBJECTS: LazyLock<Mutex<BTreeMap<u32, EngineObject>>> =
            LazyLock::new(|| Mutex::new(BTreeMap::new()));

        fn test_object(boid: u32) -> Option<EngineObject> {
            (boid >= 0xF000_0000).then_some(EngineObject {
                accessor: 0xF000_0000_0000_0000u64 | u64::from(boid),
                category: 0,
                kind: crate::mock_fighter_kind(boid),
                entry_id: 0,
                active: true,
            })
        }

        pub fn set(boid: u32, accessor: u64, category: i32, kind: i32, entry_id: i32) {
            OBJECTS.lock().unwrap().insert(
                boid,
                EngineObject {
                    accessor,
                    category,
                    kind,
                    entry_id,
                    active: true,
                },
            );
        }

        pub fn clear() {
            OBJECTS.lock().unwrap().clear();
        }

        pub unsafe fn is_null(boid: u32) -> bool {
            !OBJECTS.lock().unwrap().contains_key(&boid) && test_object(boid).is_none()
        }

        pub unsafe fn is_active(boid: u32) -> bool {
            OBJECTS
                .lock()
                .unwrap()
                .get(&boid)
                .copied()
                .or_else(|| test_object(boid))
                .is_some_and(|object| object.active)
        }

        pub unsafe fn module_accessor(boid: u32) -> *mut super::BattleObjectModuleAccessor {
            OBJECTS
                .lock()
                .unwrap()
                .get(&boid)
                .copied()
                .or_else(|| test_object(boid))
                .map_or(std::ptr::null_mut(), |object| {
                    object.accessor as *mut super::BattleObjectModuleAccessor
                })
        }

        pub unsafe fn category(boid: u32) -> i32 {
            OBJECTS
                .lock()
                .unwrap()
                .get(&boid)
                .copied()
                .or_else(|| test_object(boid))
                .map_or(-1, |o| o.category)
        }

        pub unsafe fn kind(boid: u32) -> i32 {
            OBJECTS
                .lock()
                .unwrap()
                .get(&boid)
                .copied()
                .or_else(|| test_object(boid))
                .map_or(-1, |o| o.kind)
        }

        pub unsafe fn entry_id(boid: u32) -> i32 {
            OBJECTS
                .lock()
                .unwrap()
                .get(&boid)
                .copied()
                .or_else(|| test_object(boid))
                .map_or(-1, |o| o.entry_id)
        }
    }
}

mod slight {
    pub mod agents {
        pub fn newest_registered(_objects: &[u32]) -> Option<u32> {
            None
        }
        use std::collections::BTreeMap;
        use std::sync::{LazyLock, Mutex};

        #[derive(Clone, Debug)]
        pub struct AgentRecord {
            pub boid: u32,
            pub module_accessor_addr: u64,
            pub category: i32,
            pub kind: i32,
            pub status_kind: i32,
            pub entry_id: i32,
        }

        static RECORDS: LazyLock<Mutex<BTreeMap<u32, AgentRecord>>> =
            LazyLock::new(|| Mutex::new(BTreeMap::new()));

        pub fn set(record: AgentRecord) {
            RECORDS.lock().unwrap().insert(record.boid, record);
        }

        pub fn lookup(boid: u32) -> Option<AgentRecord> {
            RECORDS.lock().unwrap().get(&boid).cloned().or_else(|| {
                (boid >= 0xF000_0000).then_some(AgentRecord {
                    boid,
                    module_accessor_addr: 0xF000_0000_0000_0000u64 | u64::from(boid),
                    category: 0,
                    kind: crate::mock_fighter_kind(boid),
                    status_kind: 0,
                    entry_id: 0,
                })
            })
        }

        pub fn all_records() -> Vec<AgentRecord> {
            RECORDS.lock().unwrap().values().cloned().collect()
        }

        pub fn stock_generation(record: &AgentRecord) -> Option<u32> {
            lookup(record.boid).filter(|known| {
                known.module_accessor_addr == record.module_accessor_addr
                    && known.kind == record.kind
                    && known.entry_id == record.entry_id
            })?;
            Some(1)
        }

        pub fn status_transition_is_current(
            boid: u32,
            module_accessor_addr: u64,
            fighter_kind: i32,
            entry_id: i32,
            expected_generation: u32,
        ) -> bool {
            let Some(record) = lookup(boid) else {
                return false;
            };
            if record.category != 0
                || record.module_accessor_addr != module_accessor_addr
                || record.kind != fighter_kind
                || record.entry_id != entry_id
                || stock_generation(&record) != Some(expected_generation)
            {
                return false;
            }
            unsafe {
                crate::app::sv_battle_object::is_active(boid)
                    && crate::app::sv_battle_object::category(boid) == 0
                    && crate::app::sv_battle_object::module_accessor(boid) as u64
                        == module_accessor_addr
                    && crate::app::sv_battle_object::kind(boid) == fighter_kind
                    && crate::app::sv_battle_object::entry_id(boid) == entry_id
            }
        }

        pub fn clear() {
            RECORDS.lock().unwrap().clear();
        }
    }

    pub mod diag {
        static NOTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        pub fn note(message: impl Into<String>) {
            NOTES.lock().unwrap().push(message.into());
        }
        pub fn take_notes() -> Vec<String> {
            std::mem::take(&mut *NOTES.lock().unwrap())
        }
    }

    pub mod systems {
        pub mod article_notifier {
            pub fn owned_weapons(_owner: u32, _fighter: &str, _symbol: &str) -> Vec<u32> {
                Vec::new()
            }
        }
        pub mod event_system {
            #[derive(Clone, Debug)]
            pub enum GameEvent {
                RealAnimationChange {
                    boid: u32,
                },
                Spawn {
                    boid: u32,
                },
                OwnedArticle {
                    owner_boid: u32,
                    fighter_kind: i32,
                    entry_id: i32,
                    module_accessor_addr: u64,
                    stock_generation: u32,
                    symbol: &'static str,
                    spawned: bool,
                },
                ArticleSpawn {
                    owner_boid: u32,
                    article_boid: u32,
                },
                Win {
                    entry_id: i32,
                },
                Kills {
                    boid: u32,
                },
                Die {
                    boid: u32,
                },
                Suicide {
                    boid: u32,
                },
                Respawn {
                    boid: u32,
                },
                StatusChange {
                    boid: u32,
                    module_accessor_addr: u64,
                    fighter_kind: i32,
                    entry_id: i32,
                    previous_status: i32,
                    status: i32,
                    stock_generation: u32,
                },
                FighterDownObserved {
                    boid: u32,
                    fighter_kind: i32,
                    entry_id: i32,
                    dead_reason: i32,
                    ko_entry_id: i32,
                    stock_generation: u32,
                },
                FighterRebirthObserved {
                    boid: u32,
                    fighter_kind: i32,
                    entry_id: i32,
                    stock_generation: u32,
                },
                CloudLimitGroundedNeutralSpent {
                    boid: u32,
                    fighter_kind: i32,
                    entry_id: i32,
                    stock_generation: u32,
                    module_accessor_addr: u64,
                },
                CloudLimitIdleExpired {
                    boid: u32,
                    fighter_kind: i32,
                    entry_id: i32,
                    stock_generation: u32,
                    module_accessor_addr: u64,
                },
                StartsFinalAnim {
                    boid: u32,
                },
                Loops {
                    boid: u32,
                },
                Point {
                    host: u16,
                    port: u16,
                },
            }
        }

        pub mod damage_manager {
            pub struct CollisionHitRecord {
                pub attacker_boid: u32,
                pub defender_boid: u32,
            }

            pub fn drain_collision_hits() -> Vec<CollisionHitRecord> {
                Vec::new()
            }

            pub fn drain_defender_hits() -> Vec<CollisionHitRecord> {
                Vec::new()
            }
        }
    }

    pub mod gimmicks {
        // Native target delivery is covered by the shared queue tests and
        // source assertions; this harness stubs engine operations.
        pub(crate) mod incoming_hit {
            pub fn set_enabled(_: bool) {}
        }
        pub(crate) mod grab_control {
            pub fn preflight(
                boid: u32,
                fighter: &str,
                identity: super::actor_identity::Identity,
                count: usize,
            ) -> Result<(), String> {
                if super::actor_identity::Identity::from_live_fighter(boid, fighter)
                    != Some(identity)
                    || !(0..8).contains(&identity.entry_id)
                    || count > 64
                {
                    return Err("invalid grab command owner or queue count".into());
                }
                Ok(())
            }
        }
        /// Records controller object ops; the real queue runs on weapon Main lines.
        pub(crate) mod deflect_volume {
            pub type ObjectOp = (u32, String, String, Option<(f32, f32)>);
            pub static OPS: std::sync::Mutex<Vec<ObjectOp>> = std::sync::Mutex::new(Vec::new());
            pub fn object_op(
                owner: u32,
                fighter: &str,
                symbol: &str,
                speed: Option<(f32, f32)>,
            ) -> Result<(), String> {
                OPS.lock()
                    .unwrap()
                    .push((owner, fighter.into(), symbol.into(), speed));
                Ok(())
            }
        }
        pub(crate) mod damage_control {
            pub fn configuration_changed() {}
            pub fn preflight(
                _: u32,
                _: &str,
                _: super::actor_identity::Identity,
                _: usize,
            ) -> Result<(), String> {
                Ok(())
            }
        }
        pub(crate) use crate::TEST_LOCK;
        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/article_context.rs"]
        pub(crate) mod article_context;

        #[path = "../../../src/gimmicks/native_inputs.rs"]
        pub mod native_inputs;

        #[path = "../../../tests/slight/cloud_resource_mock.rs"]
        pub(crate) mod cloud_resource;
        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/cloud_resource_model.rs"]
        pub(crate) mod cloud_resource_model;

        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/dispatcher.rs"]
        pub mod dispatcher;

        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/contact_snapshot_model.rs"]
        pub mod contact_snapshot_model;
        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/damage_capture_model.rs"]
        pub mod damage_capture_model;
        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/damage_commit_model.rs"]
        pub mod damage_commit_model;
        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/damage_delivery_model.rs"]
        pub mod damage_delivery_model;
        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/damage_dispatch.rs"]
        pub mod damage_dispatch;
        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/damage_feed_model.rs"]
        pub mod damage_feed_model;

        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/damage_context.rs"]
        pub(crate) mod damage_context;

        // The native feed is Skyline-specific. This host fence checks the
        // logical delivery stamp and freshly resolves both mock engine actors.
        pub(crate) mod damage_feed {
            use super::{
                actor_identity::Identity, damage_dispatch::Event, damage_feed_model::Stamp,
            };
            static STAMP: std::sync::Mutex<Option<Stamp>> = std::sync::Mutex::new(None);
            static CONFIGURATION_CHANGES: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);

            pub(crate) fn configuration_changed() {
                CONFIGURATION_CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                set_stamp(None);
            }

            pub(crate) fn configuration_changes() -> u64 {
                CONFIGURATION_CHANGES.load(std::sync::atomic::Ordering::SeqCst)
            }

            pub(crate) fn set_stamp(stamp: Option<Stamp>) {
                *STAMP.lock().unwrap() = stamp;
            }

            pub(crate) fn event_is_current(event: Event) -> bool {
                *STAMP.lock().unwrap() == Some(event.delivery_stamp())
                    && [event.source(), event.target()].into_iter().all(|actor| {
                        let Some(fighter) = super::registry::fighter_name(actor.identity.kind)
                        else {
                            return false;
                        };
                        Identity::from_live_fighter(actor.identity.object_id, fighter)
                            == Some(Identity {
                                fighter_kind: actor.identity.kind,
                                entry_id: actor.identity.entry,
                                module_accessor_addr: actor.identity.accessor,
                                stock_generation: actor.stock_generation,
                            })
                    })
            }
        }

        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/registry.rs"]
        mod pinned_registry;

        pub mod registry {
            use super::pinned_registry as pinned;
            pub use pinned::{owned_by, projectile_work, resolve, weapon_kind, IdentifierKind};
            pub(crate) use pinned::{FighterKindEntry, RegistryEntry, ENTRIES, FIGHTER_KINDS};

            pub fn fighter_name(kind: i32) -> Option<&'static str> {
                if kind == 0x7F {
                    Some("test")
                } else {
                    pinned::fighter_name(kind)
                }
            }

            pub fn fighter_kind_value(fighter: &str) -> Option<i32> {
                if fighter == "test" {
                    Some(0x7F)
                } else {
                    pinned::fighter_kind_value(fighter)
                }
            }

            pub fn fighter_kind_symbol(fighter: &str) -> Option<&'static str> {
                if fighter == "test" {
                    Some("FIGHTER_KIND_TEST")
                } else {
                    pinned::fighter_kind_symbol(fighter)
                }
            }
        }

        pub mod operations {
            use std::sync::atomic::{AtomicUsize, Ordering};
            use std::sync::{LazyLock, Mutex};

            #[derive(Debug, Clone, PartialEq, Eq)]
            pub enum ObservedAction {
                FighterStatus(u32, String),
                Generate(u32, String),
                Remove(u32, String),
                RemoveNewest(u32, String),
                ArticleStatus(u32, String, String),
                Weapon(u32, u32, String),
            }

            static ACTIONS: LazyLock<Mutex<Vec<ObservedAction>>> =
                LazyLock::new(|| Mutex::new(Vec::new()));
            static ACTION_CALLS: AtomicUsize = AtomicUsize::new(0);
            static FAIL_ACTION_AT: AtomicUsize = AtomicUsize::new(0);

            enum ActionHook {
                ClearBoi(u32),
                ResetStaticBoi(u32),
                ResetStaticAll,
                DispatchChargeTick(u32),
                DispatchStaticChargeTick(u32),
                ResetStaticBoiAndDispatch(u32),
                Restart {
                    fighter: String,
                    boid: u32,
                    revision: u64,
                },
                ReplaceSnapshot(String),
                ClearCompiledBaseline,
                SetIdentity {
                    boid: u32,
                    accessor: u64,
                    kind: i32,
                    entry_id: i32,
                },
            }

            static ACTION_HOOK: LazyLock<Mutex<Option<ActionHook>>> =
                LazyLock::new(|| Mutex::new(None));

            fn record(action: ObservedAction) -> Result<(), String> {
                ACTIONS.lock().unwrap().push(action);
                let call = ACTION_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
                // Native callbacks may re-enter this observer. Release its
                // control mutex before invoking any runtime hook.
                let hook = ACTION_HOOK.lock().unwrap().take();
                if let Some(hook) = hook {
                    match hook {
                        ActionHook::ClearBoi(boid) => {
                            super::live_behavior::clear_boid(boid);
                        }
                        ActionHook::ResetStaticBoi(boid) => {
                            super::live_behavior::clear_boid(boid);
                            super::state::reset_boid(boid);
                        }
                        ActionHook::ResetStaticAll => {
                            super::live_behavior::clear_all();
                            super::state::reset_all();
                        }
                        ActionHook::DispatchChargeTick(boid) => {
                            super::live_behavior::dispatch(
                                super::dispatcher::GimmickHook::ChargeTick,
                                boid,
                            );
                        }
                        ActionHook::DispatchStaticChargeTick(boid) => {
                            let _ = super::behavior::execute(
                                &crate::STATIC_PROGRAM,
                                super::dispatcher::GimmickHook::ChargeTick,
                                boid,
                            );
                        }
                        ActionHook::ResetStaticBoiAndDispatch(boid) => {
                            super::live_behavior::clear_boid(boid);
                            super::state::reset_boid(boid);
                            let _ = super::behavior::execute(
                                &crate::STATIC_PROGRAM,
                                super::dispatcher::GimmickHook::ChargeTick,
                                boid,
                            );
                        }
                        ActionHook::Restart {
                            fighter,
                            boid,
                            revision,
                        } => {
                            let ack = super::live_behavior::restart(&fighter, boid, revision);
                            assert!(ack.accepted, "{}", ack.message);
                        }
                        ActionHook::ReplaceSnapshot(raw) => {
                            let ack = super::live_behavior::handle(&raw);
                            assert!(ack.accepted, "{}", ack.message);
                        }
                        ActionHook::ClearCompiledBaseline => {
                            super::live_behavior::clear_compiled_baseline();
                        }
                        ActionHook::SetIdentity {
                            boid,
                            accessor,
                            kind,
                            entry_id,
                        } => {
                            super::super::agents::set(super::super::agents::AgentRecord {
                                boid,
                                module_accessor_addr: accessor,
                                category: 0,
                                kind,
                                status_kind: 0,
                                entry_id,
                            });
                            crate::app::sv_battle_object::set(boid, accessor, 0, kind, entry_id);
                        }
                    }
                }
                if FAIL_ACTION_AT.load(Ordering::SeqCst) == call {
                    Err(format!("injected action failure at call {call}"))
                } else {
                    Ok(())
                }
            }

            pub fn fail_action_at(call: usize) {
                ACTION_CALLS.store(0, Ordering::SeqCst);
                FAIL_ACTION_AT.store(call, Ordering::SeqCst);
            }

            pub fn hook_next_action_clear_boid(boid: u32) {
                *ACTION_HOOK.lock().unwrap() = Some(ActionHook::ClearBoi(boid));
            }

            pub fn hook_next_action_reset_static_boid(boid: u32) {
                *ACTION_HOOK.lock().unwrap() = Some(ActionHook::ResetStaticBoi(boid));
            }

            pub fn hook_next_action_reset_static_all() {
                *ACTION_HOOK.lock().unwrap() = Some(ActionHook::ResetStaticAll);
            }

            pub fn hook_next_action_dispatch_charge_tick(boid: u32) {
                *ACTION_HOOK.lock().unwrap() = Some(ActionHook::DispatchChargeTick(boid));
            }

            pub fn hook_next_action_replace_snapshot(raw: String) {
                *ACTION_HOOK.lock().unwrap() = Some(ActionHook::ReplaceSnapshot(raw));
            }

            pub fn hook_next_action_clear_compiled_baseline() {
                *ACTION_HOOK.lock().unwrap() = Some(ActionHook::ClearCompiledBaseline);
            }

            pub fn hook_next_action_change_identity(
                boid: u32,
                accessor: u64,
                kind: i32,
                entry_id: i32,
            ) {
                *ACTION_HOOK.lock().unwrap() = Some(ActionHook::SetIdentity {
                    boid,
                    accessor,
                    kind,
                    entry_id,
                });
            }

            pub fn reset_controls() {
                ACTION_CALLS.store(0, Ordering::SeqCst);
                FAIL_ACTION_AT.store(0, Ordering::SeqCst);
                ACTION_HOOK.lock().unwrap().take();
            }

            pub fn validate_behavior_article(fighter: &str, symbol: &str) -> Result<(), String> {
                if fighter == "cloud" && symbol == "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE" {
                    Ok(())
                } else {
                    Err("article symbol is not allowlisted".into())
                }
            }

            pub fn validate_behavior_article_status(
                fighter: &str,
                symbol: &str,
                status: &str,
            ) -> Result<(), String> {
                validate_behavior_article(fighter, symbol)?;
                validate_behavior_weapon_status(fighter, 1, status)
            }
            pub fn remove_newest_article_from_behavior(
                fighter: &str,
                boid: u32,
                symbol: &str,
            ) -> Result<(), String> {
                validate_behavior_article(fighter, symbol)?;
                record(ObservedAction::RemoveNewest(boid, symbol.into()))
            }
            pub fn move_articles_from_behavior(
                fighter: &str,
                boid: u32,
                symbol: &str,
                op: super::native_inputs::MovementAction,
            ) -> Result<(), String> {
                validate_behavior_article(fighter, symbol)?;
                op.validate_for_article()?;
                record(ObservedAction::RemoveNewest(
                    boid,
                    format!("{symbol}:{op:?}"),
                ))
            }
            pub fn request_article_status_from_behavior(
                fighter: &str,
                boid: u32,
                symbol: &str,
                status: &str,
            ) -> Result<(), String> {
                validate_behavior_article_status(fighter, symbol, status)?;
                record(ObservedAction::ArticleStatus(
                    boid,
                    symbol.into(),
                    status.into(),
                ))
            }
            pub fn validate_behavior_fighter_status(
                fighter: &str,
                symbol: &str,
            ) -> Result<(), String> {
                super::registry::resolve(
                    fighter,
                    symbol,
                    super::registry::IdentifierKind::FighterStatus,
                )
                .map(|_| ())
            }

            pub(crate) fn preflight_behavior_operation(
                fighter: &str,
                boid: u32,
                action: &super::live_model::Action,
            ) -> Result<(), String> {
                use super::live_model::Action;
                match action {
                    Action::RequestFighterStatus { symbol } => {
                        validate_behavior_fighter_status(fighter, symbol)
                    }
                    Action::GenerateArticle { symbol } => {
                        validate_behavior_article(fighter, symbol)?;
                        if boid == 0xF0AA_0009 {
                            return Err("injected native article generation rejection".into());
                        }
                        Ok(())
                    }
                    Action::RequestWeaponStatus { target, symbol } => {
                        validate_behavior_weapon_status(fighter, target.battle_object_id, symbol)?;
                        super::super::agents::lookup(target.battle_object_id)
                            .ok_or("weapon is not live")?;
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }

            pub fn request_resolved_status(
                _fighter: &str,
                boid: u32,
                status: u32,
            ) -> Result<(), String> {
                record(ObservedAction::FighterStatus(boid, status.to_string()))
            }

            pub fn request_fighter_status_from_behavior(
                fighter: &str,
                boid: u32,
                symbol: &str,
            ) -> Result<(), String> {
                validate_behavior_fighter_status(fighter, symbol)?;
                record(ObservedAction::FighterStatus(boid, symbol.into()))
            }

            pub fn validate_behavior_weapon_status(
                fighter: &str,
                weapon_boid: u32,
                symbol: &str,
            ) -> Result<(), String> {
                if weapon_boid == 0 {
                    return Err("nonzero explicit weapon battle-object id required".into());
                }
                if fighter == "cloud" && symbol == "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR" {
                    Ok(())
                } else {
                    Err("weapon status target is not allowlisted".into())
                }
            }

            pub fn generate_article_from_behavior(
                fighter: &str,
                boid: u32,
                symbol: &str,
            ) -> Result<(), String> {
                validate_behavior_article(fighter, symbol)?;
                if boid == 0xF0AA_0009 {
                    return Err("injected native article generation rejection".into());
                }
                record(ObservedAction::Generate(boid, symbol.into()))
            }

            pub fn remove_article_from_behavior(
                fighter: &str,
                boid: u32,
                symbol: &str,
            ) -> Result<(), String> {
                validate_behavior_article(fighter, symbol)?;
                record(ObservedAction::Remove(boid, symbol.into()))
            }

            pub fn request_weapon_status_from_behavior(
                fighter: &str,
                boid: u32,
                weapon: u32,
                symbol: &str,
            ) -> Result<(), String> {
                validate_behavior_weapon_status(fighter, weapon, symbol)?;
                if super::super::agents::lookup(weapon).is_none() {
                    return Err(format!(
                        "weapon battle object 0x{weapon:X} is not live and registered"
                    ));
                }
                record(ObservedAction::Weapon(boid, weapon, symbol.into()))
            }

            pub fn take_actions() -> Vec<ObservedAction> {
                std::mem::take(&mut *ACTIONS.lock().unwrap())
            }
        }

        fn controller_random(seed: &std::cell::Cell<u64>, upper: i32) -> i32 {
            let next = seed
                .get()
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed.set(next);
            ((next >> 32) % upper as u64) as i32
        }
        #[path = "../../../src/gimmicks/hero_menu_commands.rs"]
        pub mod hero_menu_commands;
        #[path = "live_model.rs"]
        pub mod live_model;
        #[path = "../../../src/gimmicks/resource_commands.rs"]
        pub mod resource_commands;
        #[path = "../../../src/gimmicks/status_commands.rs"]
        pub mod status_commands;
        #[path = "../../../src/gimmicks/status_model.rs"]
        pub mod status_model;

        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/fighter_status_queue.rs"]
        pub(crate) mod fighter_status_queue;

        #[path = "../../../src/gimmicks/controller_runtime.rs"]
        pub mod controller_runtime;

        #[path = "../../../src/gimmicks/controller_protocol.rs"]
        pub mod controller_protocol;

        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/state.rs"]
        pub mod state;

        #[path = "live_behavior.rs"]
        pub mod live_behavior;

        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/actor_identity.rs"]
        pub(crate) mod actor_identity;

        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/chart_event_bridge.rs"]
        pub(crate) mod chart_event_bridge;

        #[path = "../../../src/gimmicks/damage_commands.rs"]
        pub(crate) mod damage_commands;

        #[path = "../../../src/gimmicks/phase_commands.rs"]
        pub(crate) mod phase_commands;

        pub(crate) mod hero_menu_control {
            use super::actor_identity::Identity;
            pub fn preflight(_: u32, _: &str, _: Identity, _: u8) -> Result<(), String> {
                Ok(())
            }
            pub fn execute(_: u32, _: &str, _: Identity, _: u8) -> Result<(), String> {
                Ok(())
            }
        }

        pub(crate) mod resource_control {
            use super::{actor_identity::Identity, resource_commands::Command};
            static CALLS: std::sync::Mutex<Vec<(u32, Command)>> = std::sync::Mutex::new(Vec::new());
            pub fn take_calls() -> Vec<(u32, Command)> {
                std::mem::take(&mut *CALLS.lock().unwrap())
            }
            pub fn resolved(c: &Command<super::live_model::Expr>) -> Result<Command, String> {
                c.try_map(|e| match e {
                    super::live_model::Expr::Const(super::live_model::Value::Float(v)) => Ok(*v),
                    _ => Err("unresolved".to_string()),
                })
            }
            pub fn preflight(b: u32, i: Identity, c: Command) -> Result<(), String> {
                c.validate(c.resource.fighter())?;
                if Identity::from_live_fighter(b, c.resource.fighter()) != Some(i) {
                    return Err("resource actor identity changed".into());
                }
                Ok(())
            }
            pub fn execute(b: u32, i: Identity, c: Command) -> Result<(), String> {
                preflight(b, i, c)?;
                CALLS.lock().unwrap().push((b, c));
                Ok(())
            }
        }
        pub(crate) mod phase_control {
            use super::{
                actor_identity::Identity, live_model as model, native_inputs::PhaseInput,
                phase_commands,
            };
            use std::sync::Mutex;
            static PHASE: Mutex<Option<PhaseInput>> = Mutex::new(None);
            static COMMANDS: Mutex<Vec<phase_commands::Command>> = Mutex::new(Vec::new());
            pub fn set_sample(value: Option<PhaseInput>) {
                *PHASE.lock().unwrap() = value;
            }
            pub fn take_commands() -> Vec<phase_commands::Command> {
                std::mem::take(&mut *COMMANDS.lock().unwrap())
            }
            fn check(boid: u32, fighter: &str, expected: Identity) -> Result<(), String> {
                if Identity::from_live_fighter(boid, fighter) != Some(expected) {
                    return Err("stale phase identity".into());
                }
                Ok(())
            }
            pub fn sample(
                boid: u32,
                fighter: &str,
                expected: Identity,
            ) -> Result<PhaseInput, String> {
                check(boid, fighter, expected)?;
                PHASE
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| "phase sample unavailable".into())
            }
            pub fn preflight(
                boid: u32,
                fighter: &str,
                expected: Identity,
                action: &model::Action,
            ) -> Result<(), String> {
                phase_commands::resolve(action)?;
                check(boid, fighter, expected)
            }
            pub fn execute(
                boid: u32,
                fighter: &str,
                expected: Identity,
                action: &model::Action,
            ) -> Result<(), String> {
                preflight(boid, fighter, expected, action)?;
                phase_commands::execute_guarded(
                    phase_commands::resolve(action)?,
                    || check(boid, fighter, expected),
                    || -1.0,
                    |c| COMMANDS.lock().unwrap().push(c),
                )
            }
        }

        pub(crate) mod native_values {
            use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
            use std::sync::{LazyLock, Mutex};

            use super::{
                actor_identity::Identity,
                behavior::NativeSource,
                native_inputs::{FighterValue, NativeInput, NativeObservation, NativeSnapshot},
            };

            static CONTROLLER: Mutex<Option<super::native_inputs::ControllerInput>> =
                Mutex::new(None);

            pub(crate) fn set_controller_input(input: super::native_inputs::ControllerInput) {
                *CONTROLLER.lock().unwrap() = Some(input);
            }

            pub(crate) fn sample_controller_input(
                boid: u32,
                fighter: &str,
                expected: Identity,
            ) -> Result<super::native_inputs::ControllerInput, String> {
                if Identity::from_live_fighter(boid, fighter) != Some(expected) {
                    return Err("stale controller input identity".into());
                }
                Ok(CONTROLLER.lock().unwrap().clone().unwrap_or(
                    super::native_inputs::ControllerInput {
                        facing: 1.0,
                        ..Default::default()
                    },
                ))
            }
            pub(crate) fn preflight_write(
                boid: u32,
                fighter: &str,
                expected: Identity,
                action: &super::live_model::Action,
            ) -> Result<(), String> {
                if Identity::from_live_fighter(boid, fighter) != Some(expected) {
                    return Err("stale native write identity".into());
                }
                match action {
                    super::live_model::Action::SetNative { input, .. }
                    | super::live_model::Action::AddNative { input, .. }
                    | super::live_model::Action::SetNativeFlag { input, .. } => {
                        input.validate_write(fighter)
                    }
                    _ => Err("expected native write".into()),
                }
            }
            pub(crate) fn execute_write(
                boid: u32,
                fighter: &str,
                expected: Identity,
                action: &super::live_model::Action,
            ) -> Result<(), String> {
                preflight_write(boid, fighter, expected, action)
            }
            pub(crate) const MAX_WORK_SAMPLES: usize = 64;
            static SAMPLE_BITS: AtomicU32 = AtomicU32::new(75.0f32.to_bits());
            static SAMPLE_COUNT: AtomicUsize = AtomicUsize::new(0);
            static SAMPLE_INT: AtomicI32 = AtomicI32::new(9);
            static CHANGE_INT_AFTER_SAMPLE: AtomicBool = AtomicBool::new(false);
            static NEXT_SAMPLE_INT: AtomicI32 = AtomicI32::new(0);
            static SAMPLE_FLAG: AtomicBool = AtomicBool::new(true);
            static SAMPLE_STATUS_CHANGING: AtomicBool = AtomicBool::new(false);
            static SAMPLE_FIGHTER_STATUS: AtomicI32 = AtomicI32::new(i32::MIN);
            static FAIL_SAMPLE: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            static RECYCLE_ACTOR: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            static AFTER_SAMPLE_SNAPSHOT: LazyLock<Mutex<Option<String>>> =
                LazyLock::new(|| Mutex::new(None));
            static AFTER_SAMPLE_CALLBACK: Mutex<Option<fn()>> = Mutex::new(None);

            pub(crate) fn set_test_sample(value: Option<f32>) {
                FAIL_SAMPLE.store(value.is_none(), Ordering::SeqCst);
                if let Some(value) = value {
                    SAMPLE_BITS.store(value.to_bits(), Ordering::SeqCst);
                }
            }

            pub(crate) fn set_test_int(value: i32) {
                SAMPLE_INT.store(value, Ordering::SeqCst);
            }

            pub(crate) fn clear_int_sample_change() {
                CHANGE_INT_AFTER_SAMPLE.store(false, Ordering::SeqCst);
            }

            pub(crate) fn set_test_flag(value: bool) {
                SAMPLE_FLAG.store(value, Ordering::SeqCst);
            }

            pub(crate) fn set_test_status_changing(value: bool) {
                SAMPLE_STATUS_CHANGING.store(value, Ordering::SeqCst);
            }

            pub(crate) fn set_test_fighter_status(value: Option<i32>) {
                SAMPLE_FIGHTER_STATUS.store(value.unwrap_or(i32::MIN), Ordering::SeqCst);
            }

            pub(crate) fn take_sample_count() -> usize {
                SAMPLE_COUNT.swap(0, Ordering::SeqCst)
            }

            pub(crate) fn recycle_actor_after_sample() {
                RECYCLE_ACTOR.store(true, Ordering::SeqCst);
            }

            pub(crate) fn replace_snapshot_after_next_sample(raw: String) {
                *AFTER_SAMPLE_SNAPSHOT.lock().unwrap() = Some(raw);
            }

            pub(crate) fn clear_after_sample_hook() {
                AFTER_SAMPLE_SNAPSHOT.lock().unwrap().take();
                AFTER_SAMPLE_CALLBACK.lock().unwrap().take();
            }

            pub(crate) fn after_next_sample(callback: fn()) {
                *AFTER_SAMPLE_CALLBACK.lock().unwrap() = Some(callback);
            }

            pub(crate) fn sample_behavior_inputs(
                boid: u32,
                fighter: &str,
                expected: Identity,
                sources: &[NativeInput],
            ) -> Result<NativeSnapshot, String> {
                SAMPLE_COUNT.fetch_add(1, Ordering::SeqCst);
                if Identity::from_live_fighter(boid, fighter) != Some(expected) {
                    return Err("test sampler observed stale actor identity".into());
                }
                if sources.is_empty() {
                    let mut snapshot = NativeSnapshot::empty();
                    snapshot.article_event = super::article_context::sample(boid, expected);
                    return Ok(snapshot);
                }
                if sources
                    .iter()
                    .any(|source| !source.is_supported() || !source.valid_for(fighter))
                {
                    return Err("test sampler rejected unreviewed native input".into());
                }
                if FAIL_SAMPLE.load(Ordering::SeqCst) {
                    return Err("injected unavailable native input".into());
                }
                let observations = sources
                    .iter()
                    .map(|source| match source.kind() {
                        Some(super::native_inputs::NativeKind::Float) => NativeObservation::Float(
                            f32::from_bits(SAMPLE_BITS.load(Ordering::SeqCst)),
                        ),
                        Some(super::native_inputs::NativeKind::Int) => {
                            NativeObservation::Int(SAMPLE_INT.load(Ordering::SeqCst))
                        }
                        Some(super::native_inputs::NativeKind::Flag) => NativeObservation::Flag(
                            if matches!(
                                source,
                                NativeInput::Fighter {
                                    value: FighterValue::StatusChanging,
                                    ..
                                }
                            ) {
                                SAMPLE_STATUS_CHANGING.load(Ordering::SeqCst)
                            } else if let NativeInput::FighterStatusIs { fighter, symbol } = source
                            {
                                let status = SAMPLE_FIGHTER_STATUS.load(Ordering::SeqCst);
                                if status == i32::MIN {
                                    SAMPLE_FLAG.load(Ordering::SeqCst)
                                } else {
                                    super::registry::resolve(
                                        fighter,
                                        symbol,
                                        super::registry::IdentifierKind::FighterStatus,
                                    )
                                    .is_ok_and(|entry| entry.value as i32 == status)
                                }
                            } else {
                                SAMPLE_FLAG.load(Ordering::SeqCst)
                            },
                        ),
                        None => unreachable!("unsupported source rejected above"),
                    })
                    .collect::<Vec<_>>();
                let mut snapshot = NativeSnapshot::from_observations(sources, &observations)?;
                snapshot.article_event = super::article_context::sample(boid, expected);
                let callback = AFTER_SAMPLE_CALLBACK.lock().unwrap().take();
                if let Some(callback) = callback {
                    callback();
                }
                if let Some(raw) = AFTER_SAMPLE_SNAPSHOT.lock().unwrap().take() {
                    let ack = super::live_behavior::handle(&raw);
                    if !ack.accepted {
                        return Err(format!("injected snapshot rejected: {}", ack.message));
                    }
                }
                if CHANGE_INT_AFTER_SAMPLE.swap(false, Ordering::SeqCst) {
                    SAMPLE_INT.store(NEXT_SAMPLE_INT.load(Ordering::SeqCst), Ordering::SeqCst);
                }
                if RECYCLE_ACTOR.swap(false, Ordering::SeqCst) {
                    let record = super::super::agents::lookup(boid)
                        .expect("test actor is present during native sampling");
                    super::super::agents::set(super::super::agents::AgentRecord {
                        module_accessor_addr: record.module_accessor_addr + 1,
                        ..record.clone()
                    });
                    crate::app::sv_battle_object::set(
                        boid,
                        record.module_accessor_addr + 1,
                        record.category,
                        record.kind,
                        record.entry_id,
                    );
                }
                Ok(snapshot)
            }

            pub(crate) fn sample_static_inputs(
                boid: u32,
                fighter: &str,
                expected: Identity,
                sources: &[NativeSource],
            ) -> Result<NativeSnapshot, String> {
                let sources = sources
                    .iter()
                    .map(|source| source.to_input())
                    .collect::<Vec<_>>();
                sample_behavior_inputs(boid, fighter, expected, &sources)
            }

            pub(crate) fn sample_behavior_inputs_guarded(
                boid: u32,
                fighter: &str,
                expected: Identity,
                sources: &[NativeInput],
                mut check: impl FnMut() -> Result<(), String>,
            ) -> Result<NativeSnapshot, String> {
                check()?;
                let snapshot = sample_behavior_inputs(boid, fighter, expected, sources)?;
                check()?;
                Ok(snapshot)
            }

            pub(crate) fn sample_static_inputs_guarded(
                boid: u32,
                fighter: &str,
                expected: Identity,
                sources: &[NativeSource],
                check: impl FnMut() -> Result<(), String>,
            ) -> Result<NativeSnapshot, String> {
                let sources = sources
                    .iter()
                    .map(|source| source.to_input())
                    .collect::<Vec<_>>();
                sample_behavior_inputs_guarded(boid, fighter, expected, &sources, check)
            }
        }

        #[path = "../../../plugins/slight_replica/src/slight/gimmicks/behavior.rs"]
        pub mod behavior;

        pub mod meter_snapshot {
            // The emitted fixture has zero bindings; no HUD getter is mocked.
            pub struct MeterBinding;
        }

        pub fn on_reset_fighter(boid: u32) {
            live_behavior::clear_boid(boid);
            state::reset_boid(boid);
        }
    }
}

use slight::gimmicks::{live_behavior as runtime, live_model as model};

fn static_matches(boid: u32) -> bool {
    runtime::actor_matches(boid, "cloud")
}

static STATIC_ONE: slight::gimmicks::behavior::Expr =
    slight::gimmicks::behavior::Expr::Const(slight::gimmicks::behavior::Value::Int(1));
static STATIC_NEGATIVE: slight::gimmicks::behavior::Expr =
    slight::gimmicks::behavior::Expr::Const(slight::gimmicks::behavior::Value::Int(-1_000_000));
static STATIC_COUNT: slight::gimmicks::behavior::Expr =
    slight::gimmicks::behavior::Expr::Var("count");
static STATIC_GUARD: slight::gimmicks::behavior::Predicate =
    slight::gimmicks::behavior::Predicate::Compare {
        op: slight::gimmicks::behavior::CmpOp::Gte,
        left: &STATIC_COUNT,
        right: &STATIC_NEGATIVE,
    };
static STATIC_FRAME_ACTIONS: [slight::gimmicks::behavior::Action; 4] = [
    slight::gimmicks::behavior::Action::Add {
        var: "count",
        value: &STATIC_ONE,
    },
    slight::gimmicks::behavior::Action::GenerateArticle {
        symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
    },
    slight::gimmicks::behavior::Action::GenerateArticle {
        symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
    },
    slight::gimmicks::behavior::Action::GenerateArticle {
        symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
    },
];
static STATIC_EXPIRY_ACTIONS: [slight::gimmicks::behavior::Action; 2] = [
    slight::gimmicks::behavior::Action::Add {
        var: "count",
        value: &STATIC_ONE,
    },
    slight::gimmicks::behavior::Action::GenerateArticle {
        symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
    },
];
static STATIC_VARS: [slight::gimmicks::behavior::VarDef; 2] = [
    slight::gimmicks::behavior::VarDef {
        name: "count",
        kind: slight::gimmicks::behavior::VarKind::Int,
        initial: slight::gimmicks::behavior::Value::Int(0),
    },
    slight::gimmicks::behavior::VarDef {
        name: "timer",
        kind: slight::gimmicks::behavior::VarKind::Timer,
        initial: slight::gimmicks::behavior::Value::Int(1),
    },
];
static STATIC_TRANSITIONS: [slight::gimmicks::behavior::Transition; 2] = [
    slight::gimmicks::behavior::Transition {
        from: 0,
        to: 0,
        guard: &STATIC_GUARD,
        actions: &STATIC_FRAME_ACTIONS,
    },
    slight::gimmicks::behavior::Transition {
        from: 0,
        to: 0,
        guard: &slight::gimmicks::behavior::Predicate::Expired("timer"),
        actions: &STATIC_EXPIRY_ACTIONS,
    },
];
static STATIC_FRAME_BINDING: [usize; 1] = [0];
static STATIC_TIMEOUT_BINDING: [usize; 1] = [1];
static STATIC_BINDINGS: [slight::gimmicks::behavior::Binding; 2] = [
    slight::gimmicks::behavior::Binding {
        hook: slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        article: None,
        transitions: &STATIC_FRAME_BINDING,
    },
    slight::gimmicks::behavior::Binding {
        hook: slight::gimmicks::dispatcher::GimmickHook::Timeout,
        article: None,
        transitions: &STATIC_TIMEOUT_BINDING,
    },
];
static STATIC_PROGRAM: slight::gimmicks::behavior::Program = slight::gimmicks::behavior::Program {
    id: "static_guard_test",
    fighter: "cloud",
    matches_fighter: static_matches,
    initial_state: 0,
    vars: &STATIC_VARS,
    native_inputs: &[],
    transitions: &STATIC_TRANSITIONS,
    bindings: &STATIC_BINDINGS,
    event_bindings: &[],
};

// Paired static/live named-event parity fixture. The root sender mutates
// variables both before and after enqueuing alpha; alpha forwards only its
// owned incoming payload to beta. The trace records FIFO receiver order.
fn reset() {
    slight::gimmicks::resource_control::take_calls();
    slight::diag::take_notes();
    slight::gimmicks::damage_feed::set_stamp(None);
    runtime::clear_all();
    runtime::clear_overrides();
    runtime::clear_compiled_baseline();
    slight::gimmicks::state::reset_all();
    slight::gimmicks::dispatcher::clear();
    slight::agents::clear();
    smash::app::sv_battle_object::clear();
    slight::gimmicks::operations::take_actions();
    slight::gimmicks::operations::reset_controls();
    slight::gimmicks::phase_control::set_sample(None);
    slight::gimmicks::phase_control::take_commands();
    slight::gimmicks::native_values::set_controller_input(
        slight::gimmicks::native_inputs::ControllerInput {
            facing: 1.0,
            ..Default::default()
        },
    );
    slight::gimmicks::native_values::set_test_sample(Some(75.0));
    slight::gimmicks::native_values::clear_after_sample_hook();
    slight::gimmicks::native_values::set_test_int(9);
    slight::gimmicks::native_values::set_test_flag(true);
    slight::gimmicks::native_values::set_test_status_changing(false);
    slight::gimmicks::native_values::set_test_fighter_status(None);
    slight::gimmicks::native_values::clear_int_sample_change();
    slight::gimmicks::native_values::take_sample_count();
}

fn cloud(boid: u32, accessor: u64, entry_id: i32) {
    slight::agents::set(slight::agents::AgentRecord {
        boid,
        module_accessor_addr: accessor,
        category: 0,
        kind: 62,
        status_kind: 0,
        entry_id,
    });
    smash::app::sv_battle_object::set(boid, accessor, 0, 62, entry_id);
}

fn fighter(boid: u32, accessor: u64, entry_id: i32, name: &str) {
    let kind = match name {
        "jack" => 0x52,
        "brave" => 0x53,
        "pickel" => 0x58,
        "samus" => 3,
        "sheik" => 0x10,
        "robot" => 0x2D,
        "reflet" => 0x38,
        "lucario" => 0x2C,
        "peach" => 0xD,
        _ => slight::gimmicks::registry::fighter_kind_value(name).expect("pinned fighter kind"),
    };
    slight::agents::set(slight::agents::AgentRecord {
        boid,
        module_accessor_addr: accessor,
        category: 0,
        kind,
        status_kind: 0,
        entry_id,
    });
    smash::app::sv_battle_object::set(boid, accessor, 0, kind, entry_id);
}

fn observe_captured_status_change(
    boid: u32,
    module_accessor_addr: u64,
    fighter_kind: i32,
    entry_id: i32,
    stock_generation: u32,
    previous_status: i32,
    status: i32,
) {
    slight::gimmicks::dispatcher::observe_event(
        &slight::systems::event_system::GameEvent::StatusChange {
            boid,
            module_accessor_addr,
            fighter_kind,
            entry_id,
            previous_status,
            status,
            stock_generation,
        },
    );
}

fn program(
    initial_count: i32,
    status_trigger: bool,
    resource: Option<model::ResourceSource>,
    timer: Option<i32>,
) -> model::LiveProgram {
    let mut vars = vec![model::VarDef {
        keep_across_form_switch: false,
        name: "count".into(),
        kind: model::VarKind::Int,
        initial: model::Value::Int(initial_count),
    }];
    if let Some(timer) = timer {
        vars.push(model::VarDef {
            keep_across_form_switch: false,
            name: "timer".into(),
            kind: model::VarKind::Timer,
            initial: model::Value::Int(timer),
        });
    }
    let mut transitions = Vec::new();
    let mut bindings = Vec::new();
    let mut status_triggers = Vec::new();
    let mut resource_triggers = Vec::new();

    transitions.push(model::Transition {
        from: "active".into(),
        to: "active".into(),
        guard: model::Predicate::Compare {
            op: model::CmpOp::Gte,
            left: model::Expr::Var("count".into()),
            right: model::Expr::Const(model::Value::Int(-1_000_000)),
        },
        on_frame: true,
        actions: vec![model::Action::Add {
            var: "count".into(),
            value: model::Expr::Const(model::Value::Int(1)),
        }],
    });
    bindings.push(model::HookBinding {
        hook: model::HookName::OnChargeTick,
        article: None,
        transitions: vec![0],
    });
    let stock_start_index = transitions.len();
    transitions.push(model::Transition {
        from: "active".into(),
        to: "active".into(),
        guard: model::Predicate::Compare {
            op: model::CmpOp::Gte,
            left: model::Expr::Var("count".into()),
            right: model::Expr::Const(model::Value::Int(-1_000_000)),
        },
        on_frame: true,
        actions: vec![model::Action::Reset {
            var: "count".into(),
        }],
    });
    bindings.push(model::HookBinding {
        hook: model::HookName::OnStockStart,
        article: None,
        transitions: vec![stock_start_index],
    });

    if status_trigger {
        let transition_index = transitions.len();
        transitions.push(model::Transition {
            from: "active".into(),
            to: "active".into(),
            guard: model::Predicate::Flag("armed".into()),
            on_frame: true,
            actions: vec![model::Action::GenerateArticle {
                symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
            }],
        });
        bindings.push(model::HookBinding {
            hook: model::HookName::OnSpend,
            article: None,
            transitions: vec![transition_index],
        });
        vars.push(model::VarDef {
            keep_across_form_switch: false,
            name: "armed".into(),
            kind: model::VarKind::Flag,
            initial: model::Value::Flag(true),
        });
        status_triggers.push(model::StatusTrigger {
            hook: model::HookName::OnSpend,
            symbol: "FIGHTER_CLOUD_STATUS_KIND_FINAL2".into(),
            transitions: Vec::new(),
        });
    }
    if let Some(source) = resource {
        let hook = source.hook();
        let transition_index = transitions.len();
        transitions.push(model::Transition {
            from: "active".into(),
            to: "active".into(),
            guard: model::Predicate::Compare {
                op: model::CmpOp::Gte,
                left: model::Expr::Var("count".into()),
                right: model::Expr::Const(model::Value::Int(-1_000_000)),
            },
            on_frame: true,
            actions: vec![model::Action::Add {
                var: "count".into(),
                value: model::Expr::Const(model::Value::Int(10)),
            }],
        });
        bindings.push(model::HookBinding {
            hook,
            article: None,
            transitions: vec![transition_index],
        });
        resource_triggers.push(model::ResourceTrigger { hook, source });
    }
    if timer.is_some() {
        let transition_index = transitions.len();
        transitions.push(model::Transition {
            from: "active".into(),
            to: "active".into(),
            guard: model::Predicate::Expired("timer".into()),
            on_frame: true,
            actions: vec![model::Action::Add {
                var: "count".into(),
                value: model::Expr::Const(model::Value::Int(100)),
            }],
        });
        bindings[0].transitions.push(transition_index);
    }
    model::LiveProgram {
        custom_statuses: Default::default(),
        status_bindings: Vec::new(),
        event_bindings: Vec::new(),
        version: model::MODEL_VERSION,
        native_inputs: Vec::new(),
        id: "live:cloud".into(),
        fighter: "cloud".into(),
        initial: "active".into(),
        states: vec!["active".into()],
        vars,
        transitions,
        bindings,
        status_triggers,
        resource_triggers,
    }
}

fn named_action_program(
    initial_count: i32,
    timer: Option<i32>,
    actions: Vec<model::Action>,
) -> model::LiveProgram {
    let mut authored = program(initial_count, false, None, timer);
    let index = authored.transitions.len();
    authored.transitions.push(model::Transition {
        from: "active".into(),
        to: "active".into(),
        guard: model::Predicate::Compare {
            op: model::CmpOp::Eq,
            left: model::Expr::Const(model::Value::Int(1)),
            right: model::Expr::Const(model::Value::Int(1)),
        },
        on_frame: true,
        actions,
    });
    authored.event_bindings = vec![model::EventBinding {
        name: "external_action".into(),
        transitions: vec![index],
        fields: vec![],
    }];
    authored
}

fn ordinary_action_program(initial_count: i32, actions: Vec<model::Action>) -> model::LiveProgram {
    let mut authored = program(initial_count, false, None, None);
    authored.transitions[0].actions = actions;
    authored
}

fn ordinary_expiry_action_program(
    initial_count: i32,
    actions: Vec<model::Action>,
) -> model::LiveProgram {
    let mut authored = program(initial_count, false, None, Some(1));
    let expiry_index = authored
        .transitions
        .iter()
        .position(|transition| matches!(transition.guard, model::Predicate::Expired(_)))
        .expect("timer fixture declares an expiry transition");
    let mut expiry_actions = vec![model::Action::Add {
        var: "count".into(),
        value: model::Expr::Const(model::Value::Int(100)),
    }];
    expiry_actions.extend(actions);
    authored.transitions[expiry_index].actions = expiry_actions;
    // Model a TimerExpiry trigger, not an Every-frame condition which is
    // intentionally true on every tick while its timer remains at zero.
    authored.bindings[0]
        .transitions
        .retain(|index| *index != expiry_index);
    authored.bindings.push(model::HookBinding {
        hook: model::HookName::OnTimeout,
        article: None,
        transitions: vec![expiry_index],
    });
    authored
}

fn request(
    revision: u64,
    programs: Vec<model::LiveProgram>,
    disabled_fighters: Vec<String>,
) -> String {
    serde_json::json!({"gimmick_behavior": {
        "version": slight::gimmicks::controller_protocol::REQUEST_VERSION,
        "revision": revision,
        "disabled_fighters": disabled_fighters,
        "charts": programs.into_iter().map(|program| serde_json::json!({"chart_id":"main","program":program})).collect::<Vec<_>>(),
    }})
    .to_string()
}

fn count(boid: u32) -> i32 {
    runtime::wire_rows()
        .into_iter()
        .find(|row| row.boid == boid)
        .and_then(|row| row.vars.get("count").copied())
        .and_then(|value| match value {
            model::Value::Int(value) => Some(value),
            _ => None,
        })
        .unwrap()
}

fn int_value(boid: u32, name: &str) -> i32 {
    runtime::wire_rows()
        .into_iter()
        .find(|row| row.boid == boid)
        .and_then(|row| row.vars.get(name).copied())
        .and_then(|value| match value {
            model::Value::Int(value) => Some(value),
            _ => None,
        })
        .unwrap()
}

fn float_value(boid: u32, name: &str) -> f32 {
    runtime::wire_rows()
        .into_iter()
        .find(|row| row.boid == boid)
        .and_then(|row| row.vars.get(name).copied())
        .and_then(|value| match value {
            model::Value::Float(value) => Some(value),
            _ => None,
        })
        .unwrap()
}

fn cloud_native_cascade_program() -> model::LiveProgram {
    let mut authored = program(0, false, None, None);
    authored.vars.push(model::VarDef {
        keep_across_form_switch: false,
        name: "observed".into(),
        kind: model::VarKind::Float,
        initial: model::Value::Float(0.0),
    });
    authored.vars.push(model::VarDef {
        keep_across_form_switch: false,
        name: "clear_frame".into(),
        kind: model::VarKind::Int,
        initial: model::Value::Int(0),
    });
    authored.native_inputs = vec![
        slight::gimmicks::native_inputs::NativeInput::CloudLimitGauge,
        slight::gimmicks::native_inputs::NativeInput::CloudLimitClearFrame,
        slight::gimmicks::native_inputs::NativeInput::CloudLimitBreak,
    ];
    authored.transitions[0].guard = model::Predicate::And(
        Box::new(model::Predicate::NativeFlag(
            slight::gimmicks::native_inputs::NativeInput::CloudLimitBreak,
        )),
        Box::new(model::Predicate::Compare {
            op: model::CmpOp::Gte,
            left: model::Expr::NativeFloat(
                slight::gimmicks::native_inputs::NativeInput::CloudLimitGauge,
            ),
            right: model::Expr::Const(model::Value::Float(50.0)),
        }),
    );
    authored.transitions[0].actions = vec![
        model::Action::Set {
            var: "observed".into(),
            value: model::Expr::NativeFloat(
                slight::gimmicks::native_inputs::NativeInput::CloudLimitGauge,
            ),
        },
        model::Action::Set {
            var: "clear_frame".into(),
            value: model::Expr::NativeInt(
                slight::gimmicks::native_inputs::NativeInput::CloudLimitClearFrame,
            ),
        },
        model::Action::EmitEvent {
            name: "cascade".into(),
        },
    ];
    let receiver = authored.transitions.len();
    authored.transitions.push(model::Transition {
        from: "active".into(),
        to: "active".into(),
        guard: model::Predicate::Compare {
            op: model::CmpOp::Gte,
            left: model::Expr::NativeFloat(
                slight::gimmicks::native_inputs::NativeInput::CloudLimitGauge,
            ),
            right: model::Expr::Const(model::Value::Float(70.0)),
        },
        on_frame: false,
        actions: vec![model::Action::Set {
            var: "observed".into(),
            value: model::Expr::NativeFloat(
                slight::gimmicks::native_inputs::NativeInput::CloudLimitGauge,
            ),
        }],
    });
    authored.event_bindings.push(model::EventBinding {
        name: "cascade".into(),
        transitions: vec![receiver],
        fields: vec![],
    });
    authored
}

#[test]
fn live_guard_action_and_named_event_share_one_native_snapshot() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(0xF0AA_0091, 0x9100, 0);
    slight::gimmicks::native_values::set_test_sample(Some(75.0));
    let ack = runtime::handle(&request(
        1,
        vec![cloud_native_cascade_program()],
        Vec::new(),
    ));
    assert!(ack.accepted, "{}", ack.message);

    runtime::dispatch(
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        0xF0AA_0091,
    );

    assert_eq!(float_value(0xF0AA_0091, "observed"), 75.0);
    assert_eq!(int_value(0xF0AA_0091, "clear_frame"), 9);
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 1);
}

fn static_count(boid: u32) -> Option<i32> {
    match slight::gimmicks::state::get(boid, "gimmick/static_guard_test/count") {
        Some(slight::gimmicks::state::VarValue::Int(value)) => Some(value),
        _ => None,
    }
}

fn static_actions(count: usize) -> Vec<slight::gimmicks::operations::ObservedAction> {
    vec![
        slight::gimmicks::operations::ObservedAction::Generate(
            200,
            "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
        );
        count
    ]
}

#[test]
fn accepted_named_action_failure_stops_delivery_without_rolling_back_authored_state() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    reset();
    cloud(32, 0x8200, 4);

    let authored = named_action_program(
        7,
        None,
        vec![
            model::Action::Add {
                var: "count".into(),
                value: model::Expr::Const(model::Value::Int(50)),
            },
            model::Action::GenerateArticle {
                symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
            },
            model::Action::RemoveArticle {
                symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
            },
            model::Action::GenerateArticle {
                symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
            },
        ],
    );
    assert!(authored.validate().is_ok(), "{:?}", authored.validate());
    assert!(runtime::handle(&request(1, vec![authored], vec![])).accepted);
    slight::gimmicks::operations::fail_action_at(2);

    runtime::dispatch_named(32, "cloud", "external_action")
        .unwrap()
        .unwrap();
    assert_eq!(
        count(32),
        57,
        "accepted state must not roll back after native delivery"
    );
    assert!(slight::diag::take_notes()
        .iter()
        .any(|note| note.contains("native delivery stopped: injected action failure at call 2")));
    assert_eq!(
        slight::gimmicks::operations::take_actions(),
        vec![
            slight::gimmicks::operations::ObservedAction::Generate(
                32,
                "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into()
            ),
            slight::gimmicks::operations::ObservedAction::Remove(
                32,
                "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into()
            ),
        ],
        "later actions must not be attempted after native delivery fails"
    );
    reset();
}

#[test]
fn status_exit_uses_previous_status_and_remains_independent_from_entry() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    reset();
    cloud(22, 0x6200, 0);

    let mut authored = program(0, false, None, None);
    let exit_index = authored.transitions.len();
    authored.transitions.push(model::Transition {
        from: "active".into(),
        to: "active".into(),
        guard: model::Predicate::Compare {
            op: model::CmpOp::Eq,
            left: model::Expr::Const(model::Value::Int(0)),
            right: model::Expr::Const(model::Value::Int(0)),
        },
        on_frame: true,
        actions: vec![model::Action::Add {
            var: "count".into(),
            value: model::Expr::Const(model::Value::Int(1)),
        }],
    });
    let enter_index = authored.transitions.len();
    authored.transitions.push(model::Transition {
        from: "active".into(),
        to: "active".into(),
        guard: model::Predicate::Compare {
            op: model::CmpOp::Eq,
            left: model::Expr::Const(model::Value::Int(0)),
            right: model::Expr::Const(model::Value::Int(0)),
        },
        on_frame: true,
        actions: vec![model::Action::Add {
            var: "count".into(),
            value: model::Expr::Const(model::Value::Int(10)),
        }],
    });
    authored.bindings.extend([
        model::HookBinding {
            hook: model::HookName::OnStatusExit,
            article: None,
            transitions: vec![exit_index],
        },
        model::HookBinding {
            hook: model::HookName::OnStatusEnter,
            article: None,
            transitions: vec![enter_index],
        },
    ]);
    authored.status_triggers.extend([
        model::StatusTrigger {
            hook: model::HookName::OnStatusExit,
            symbol: "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE".into(),
            transitions: vec![exit_index],
        },
        model::StatusTrigger {
            hook: model::HookName::OnStatusEnter,
            symbol: "FIGHTER_CLOUD_STATUS_KIND_FINAL2".into(),
            transitions: vec![enter_index],
        },
    ]);
    assert!(authored.validate().is_ok());
    assert!(runtime::handle(&request(1, vec![authored], vec![])).accepted);

    // Generic dispatch and a nonmatching previous/current filter do nothing.
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::StatusExit, 22);
    observe_captured_status_change(22, 0x6200, 62, 0, 1, 0x1ef, 0x1e6);
    assert_eq!(count(22), 0);

    // One current-A -> B edge selects exit-A and enter-B independently while
    // both routes remain bound to the same captured actor identity.
    observe_captured_status_change(22, 0x6200, 62, 0, 1, 0x1e6, 0x1ef);
    assert_eq!(count(22), 11);
    observe_captured_status_change(22, 0x6200, 62, 0, 1, 0x1e6, 0x1ef);
    assert_eq!(count(22), 22);
}

#[test]
fn engine_actions_execute_only_after_exact_allowlisted_transition() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    reset();
    cloud(15, 0x6000, 0);
    let ack = runtime::handle(&request(1, vec![program(0, true, None, None)], vec![]));
    assert!(ack.accepted, "{}", ack.message);
    runtime::dispatch_status(0x1ef, 15);
    assert_eq!(
        slight::gimmicks::operations::take_actions(),
        vec![slight::gimmicks::operations::ObservedAction::Generate(
            15,
            "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into()
        ),]
    );

    let mut wrong_owner = program(0, false, None, None);
    wrong_owner.transitions[0].actions = vec![model::Action::GenerateArticle {
        symbol: "FIGHTER_BRAVE_GENERATE_ARTICLE_FIREBALL".into(),
    }];
    let rejected = runtime::handle(&request(2, vec![wrong_owner], vec![]));
    assert!(!rejected.accepted);
    assert_eq!(rejected.revision, 2);
    assert!(slight::gimmicks::operations::take_actions().is_empty());
}

#[test]
fn compiled_dispatch_commits_atomically_in_action_order_and_retries_failed_timer() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    reset();
    cloud(200, 0x2000, 0);
    slight::gimmicks::behavior::execute(
        &STATIC_PROGRAM,
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        200,
    )
    .unwrap();
    assert_eq!(
        slight::gimmicks::operations::take_actions(),
        static_actions(4)
    );
    assert_eq!(static_count(200), Some(2));
    assert_eq!(
        slight::gimmicks::state::get(200, "gimmick/static_guard_test/timer"),
        Some(slight::gimmicks::state::VarValue::Int(0))
    );

    slight::gimmicks::state::reset_boid(200);
    slight::gimmicks::operations::fail_action_at(4);
    let error = slight::gimmicks::behavior::execute(
        &STATIC_PROGRAM,
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        200,
    )
    .unwrap_err();
    assert!(error.contains("injected action failure at call 4"));
    assert_eq!(
        slight::gimmicks::operations::take_actions(),
        static_actions(4)
    );
    assert_eq!(static_count(200), None);
    slight::gimmicks::operations::reset_controls();
    slight::gimmicks::operations::take_actions();
    slight::gimmicks::behavior::execute(
        &STATIC_PROGRAM,
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        200,
    )
    .unwrap();
    assert_eq!(
        slight::gimmicks::operations::take_actions(),
        static_actions(4)
    );
    assert_eq!(static_count(200), Some(2));
}

#[test]
fn compiled_dispatch_lifecycle_and_native_identity_changes_cancel_suffix_and_commit() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    reset();
    cloud(200, 0x2000, 0);
    slight::gimmicks::state::set(
        200,
        "gimmick/static_guard_test/count",
        slight::gimmicks::state::VarValue::Int(40),
    );
    slight::gimmicks::operations::hook_next_action_reset_static_boid(200);
    let error = slight::gimmicks::behavior::execute(
        &STATIC_PROGRAM,
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        200,
    )
    .unwrap_err();
    assert!(error.contains("static_guard_test for actor 200"), "{error}");
    assert_eq!(
        slight::gimmicks::operations::take_actions(),
        static_actions(1)
    );
    assert_eq!(static_count(200), None);

    cloud(200, 0x2000, 0);
    slight::gimmicks::operations::hook_next_action_reset_static_all();
    let error = slight::gimmicks::behavior::execute(
        &STATIC_PROGRAM,
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        200,
    )
    .unwrap_err();
    assert!(error.contains("static_guard_test for actor 200"), "{error}");
    assert_eq!(
        slight::gimmicks::operations::take_actions(),
        static_actions(1)
    );
    assert_eq!(static_count(200), None);

    slight::gimmicks::operations::hook_next_action_change_identity(200, 0x3000, 62, 1);
    let error = slight::gimmicks::behavior::execute(
        &STATIC_PROGRAM,
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        200,
    )
    .unwrap_err();
    assert!(error.contains("static_guard_test for actor 200"), "{error}");
    assert_eq!(
        slight::gimmicks::operations::take_actions(),
        static_actions(1)
    );
    assert_eq!(static_count(200), None);

    slight::gimmicks::behavior::execute(
        &STATIC_PROGRAM,
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        200,
    )
    .unwrap();
    assert_eq!(static_count(200), Some(2));
}

#[test]
fn owned_article_events_actions_and_count_match_static_and_live_with_identity_fences() {
    use slight::gimmicks::{
        behavior as b,
        dispatcher::{self, GimmickHook as H},
        operations::ObservedAction as A,
    };
    const ARTICLE: &str = "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE";
    const STATUS: &str = "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR";
    const ARTICLE_COUNT: &str = "article_count:cloud:FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE";
    static SOURCE: [b::NativeSource; 1] = [b::NativeSource(ARTICLE_COUNT)];
    static COUNT: b::Expr = b::Expr::NativeInt(b::NativeSource(ARTICLE_COUNT));
    static ACTIONS: [b::Action; 3] = [
        b::Action::Set {
            var: "count",
            value: &COUNT,
        },
        b::Action::RequestArticleStatus {
            symbol: ARTICLE,
            status: STATUS,
        },
        b::Action::RemoveNewestArticle { symbol: ARTICLE },
    ];
    static TRANSITIONS: [b::Transition; 2] = [
        b::Transition {
            from: 0,
            to: 0,
            guard: &STATIC_GUARD,
            actions: &ACTIONS,
        },
        b::Transition {
            from: 0,
            to: 0,
            guard: &STATIC_GUARD,
            actions: &ACTIONS,
        },
    ];
    static VARS: [b::VarDef; 1] = [b::VarDef {
        name: "count",
        kind: b::VarKind::Int,
        initial: b::Value::Int(0),
    }];
    static BINDINGS: [b::Binding; 2] = [
        b::Binding {
            hook: H::ArticleSpawn,
            article: Some(ARTICLE),
            transitions: &[0],
        },
        b::Binding {
            hook: H::ArticleRemoved,
            article: Some(ARTICLE),
            transitions: &[1],
        },
    ];
    static PROGRAM: b::Program = b::Program {
        id: "owned_article_test",
        fighter: "cloud",
        matches_fighter: static_matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &SOURCE,
        transitions: &TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &[],
    };
    fn spawned(boid: u32) {
        b::execute(&PROGRAM, H::ArticleSpawn, boid).unwrap();
    }
    fn removed(boid: u32) {
        b::execute(&PROGRAM, H::ArticleRemoved, boid).unwrap();
    }
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    for mode in 0..3 {
        reset();
        cloud(777, 0x77700, 0);
        if mode == 0 {
            b::register(
                &PROGRAM,
                &[
                    b::Registration {
                        hook: H::ArticleSpawn,
                        handler: spawned,
                        status: None,
                        transitions: None,
                    },
                    b::Registration {
                        hook: H::ArticleRemoved,
                        handler: removed,
                        status: None,
                        transitions: None,
                    },
                ],
            )
            .unwrap();
        } else {
            let mut program = ordinary_action_program(
                0,
                vec![
                    model::Action::Set {
                        var: "count".into(),
                        value: model::Expr::NativeInt(
                            slight::gimmicks::native_inputs::NativeInput::article_count(ARTICLE),
                        ),
                    },
                    model::Action::RequestArticleStatus {
                        symbol: ARTICLE.into(),
                        status: STATUS.into(),
                    },
                    model::Action::RemoveNewestArticle {
                        symbol: ARTICLE.into(),
                    },
                ],
            );
            program.native_inputs =
                vec![slight::gimmicks::native_inputs::NativeInput::article_count(
                    ARTICLE,
                )];
            program.transitions.truncate(1);
            program.transitions.push(program.transitions[0].clone());
            program.bindings = vec![
                model::HookBinding {
                    hook: model::HookName::OnArticleSpawn,
                    article: Some(ARTICLE.into()),
                    transitions: vec![0],
                },
                model::HookBinding {
                    hook: model::HookName::OnArticleRemoved,
                    article: Some(ARTICLE.into()),
                    transitions: vec![1],
                },
            ];
            if mode == 1 {
                controller_adapter_tests::install_compiled(vec![("main", program)]);
            } else {
                controller_adapter_tests::install_controller(1, vec![("main", program)]);
            }
        }
        let event =
            |spawned, accessor, symbol| slight::systems::event_system::GameEvent::OwnedArticle {
                owner_boid: 777,
                fighter_kind: 62,
                entry_id: 0,
                module_accessor_addr: accessor,
                stock_generation: 1,
                symbol,
                spawned,
            };
        // Wrong owner lifetime cannot be delivered even though the BOID is live.
        dispatcher::observe_event(&event(true, 0x77701, ARTICLE));
        assert!(slight::gimmicks::operations::take_actions().is_empty());
        // Same fighter, different kind must not reach an exact filter.
        dispatcher::observe_event(&event(
            true,
            0x77700,
            "FIGHTER_CLOUD_GENERATE_ARTICLE_FUSIONSWORD",
        ));
        assert!(slight::gimmicks::operations::take_actions().is_empty());
        for spawned in [true, false] {
            dispatcher::observe_event(&event(spawned, 0x77700, ARTICLE));
            assert_eq!(
                slight::gimmicks::operations::take_actions(),
                vec![
                    A::ArticleStatus(777, ARTICLE.into(), STATUS.into()),
                    A::RemoveNewest(777, ARTICLE.into())
                ],
                "mode {mode}"
            );
            if mode == 0 {
                assert_eq!(b::read(&PROGRAM, 777, "count").unwrap(), b::Value::Int(9));
            } else {
                assert_eq!(
                    runtime::read_controller_value(777, "cloud", "main", "count"),
                    Some(model::Value::Int(9))
                );
            }
        }
        fn recycle() {
            cloud(777, 0x77701, 0);
        }
        slight::gimmicks::native_values::after_next_sample(recycle);
        // Dispatch fails before any native action or authored commit.
        if mode == 0 {
            let identity =
                slight::gimmicks::actor_identity::Identity::from_live_fighter(777, "cloud")
                    .unwrap();
            let result =
                slight::gimmicks::article_context::with_event(777, identity, true, ARTICLE, || {
                    b::execute(&PROGRAM, H::ArticleSpawn, 777)
                });
            assert!(result.is_err());
        } else {
            dispatcher::observe_event(&event(true, 0x77700, ARTICLE));
        }
        assert!(slight::gimmicks::operations::take_actions().is_empty());
    }
    reset();
}
