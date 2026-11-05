use super::Facade;

use crate::slight::systems::init_frame::InitAction;

pub struct MainModuleFacade;

impl Facade for MainModuleFacade {
    fn name(&self) -> &'static str {
        "Main module"
    }
    fn install(&mut self) {
        skyline::println!("[SLight] Installing facade Main module");
        crate::slight::systems::main_module::install();
    }
    fn init_frame(&mut self) {
        if let Some(rec) = crate::slight::frame_context::current_agent() {
            if crate::slight::frame_context::should_skip_agent_init(rec.boid) {
                return;
            }
            if rec.category == 0 {
                match crate::slight::systems::init_frame::poll(rec.boid) {
                    InitAction::FirstInit if !crate::slight::agents::has_initialized(rec.boid) => {
                        crate::slight::systems::main_module::on_init_fighter(rec.boid);
                    }
                    InitAction::Reinit {
                        previous_status,
                        status,
                    } => match fighter_lifecycle_action(previous_status, status) {
                        FighterLifecycleAction::Reset => {
                            crate::slight::systems::main_module::on_reset_fighter(rec.boid);
                        }
                        FighterLifecycleAction::Respawn => {
                            crate::slight::systems::main_module::on_reinit_fighter(rec.boid);
                        }
                        FighterLifecycleAction::Refresh => {
                            crate::slight::systems::main_module::refresh_fighter(rec.boid);
                        }
                    },
                    InitAction::None | InitAction::FirstInit => {}
                }
            } else if rec.category == 1 {
                match crate::slight::systems::init_frame::poll(rec.boid) {
                    InitAction::FirstInit if !crate::slight::agents::has_initialized(rec.boid) => {
                        crate::slight::systems::main_module::on_init_weapon(rec.boid);
                    }
                    InitAction::Reinit { .. } => {
                        crate::slight::systems::main_module::on_reset_weapon(rec.boid);
                        crate::slight::systems::main_module::on_init_weapon(rec.boid);
                    }
                    InitAction::None | InitAction::FirstInit => {}
                }
            }
        }
    }
    fn per_agent_init(&self) -> bool {
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FighterLifecycleAction {
    /// A negative status is a dead/reset sentinel, never a move transition.
    Reset,
    /// Leaving a negative sentinel begins a fresh stock lifecycle.
    Respawn,
    /// Normal statuses/motions only refresh framework views.
    Refresh,
}

/// `init_frame` reports every status or motion delta as a reinit. Gimmick
/// state is stock-scoped, so only a signed reset sentinel may wipe it; all
/// ordinary action changes keep the status baseline and generated behavior
/// state intact.
fn fighter_lifecycle_action(previous_status: i32, status: i32) -> FighterLifecycleAction {
    if status < 0 {
        FighterLifecycleAction::Reset
    } else if previous_status < 0 {
        FighterLifecycleAction::Respawn
    } else {
        FighterLifecycleAction::Refresh
    }
}

#[cfg(test)]
mod tests {
    use super::{fighter_lifecycle_action, FighterLifecycleAction};

    #[test]
    fn ordinary_status_changes_do_not_reset_stock_scoped_gimmicks() {
        assert_eq!(
            fighter_lifecycle_action(0x1E6, 0x1E7),
            FighterLifecycleAction::Refresh
        );
    }

    #[test]
    fn signed_reset_sentinels_have_one_way_lifecycle_semantics() {
        assert_eq!(
            fighter_lifecycle_action(10, -1),
            FighterLifecycleAction::Reset
        );
        assert_eq!(
            fighter_lifecycle_action(-1, 10),
            FighterLifecycleAction::Respawn
        );
        assert_eq!(
            fighter_lifecycle_action(-2, -1),
            FighterLifecycleAction::Reset
        );
    }
}
