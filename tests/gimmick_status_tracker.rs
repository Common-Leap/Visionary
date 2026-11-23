//! Host harness for the exact production fighter-status tracker.
//!
//! `agents.rs` is compiled unchanged below; only the Smash API, event sink,
//! and adjacent data type are stubbed so its embedded tracker tests execute
//! without the Nintendo Switch runtime.

#![allow(dead_code, non_snake_case, clippy::missing_safety_doc)]

extern crate self as parking_lot;
extern crate self as smash;

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
    pub struct BattleObjectModuleAccessor {
        pub battle_object_id: u32,
    }

    pub mod lua_bind {
        use super::BattleObjectModuleAccessor;

        pub struct StatusModule;
        pub struct WorkModule;

        impl StatusModule {
            pub unsafe fn status_kind(_: *mut BattleObjectModuleAccessor) -> i32 {
                0
            }
        }

        impl WorkModule {
            pub unsafe fn get_int(_: *mut BattleObjectModuleAccessor, _: i32) -> i32 {
                0
            }
        }
    }

    pub mod sv_battle_object {
        use super::BattleObjectModuleAccessor;

        pub unsafe fn is_null(_: u32) -> bool {
            true
        }

        pub unsafe fn is_active(_: u32) -> bool {
            false
        }

        pub unsafe fn module_accessor(_: u32) -> *mut BattleObjectModuleAccessor {
            std::ptr::null_mut()
        }

        pub unsafe fn category(_: u32) -> i32 {
            -1
        }

        pub unsafe fn kind(_: u32) -> i32 {
            -1
        }

        pub unsafe fn entry_id(_: u32) -> i32 {
            -1
        }

        pub unsafe fn get_founder_id(_: u32) -> i32 {
            -1
        }
    }
}

pub mod lib {
    pub mod lua_const {
        pub static FIGHTER_INSTANCE_WORK_ID_INT_DEAD_REASON: &i32 = &1;
        pub static FIGHTER_INSTANCE_WORK_ID_INT_KO_ENTRY_ID: &i32 = &2;
        pub static FIGHTER_STATUS_KIND_DEAD: &i32 = &0x0b;
        pub static FIGHTER_STATUS_KIND_REBIRTH: &i32 = &0x0a;
    }
}

mod slight {
    pub mod effect_viewer {
        pub mod effect_data {
            #[derive(Clone, Debug, Default)]
            pub struct EffectData {
                pub effect_name: String,
            }
        }
    }

    pub mod systems {
        pub mod dynamic_memory {
            pub fn has_agent(_: u32) -> bool {
                false
            }
        }

        pub mod event_system {
            #[allow(dead_code)]
            pub enum GameEvent {
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
            }

            pub fn emit(_: GameEvent) {}

            pub fn discard_status_changes(_: u32) {}
        }
    }

    // Inline-module `#[path]` resolution starts from tests/slight/, hence the
    // two parent components before reaching the repository's plugins tree.
    #[path = "../../plugins/slight_replica/src/slight/agents.rs"]
    pub mod agents;
}
