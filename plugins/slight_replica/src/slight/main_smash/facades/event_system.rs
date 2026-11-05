use super::Facade;

pub struct EventSystemFacade;

impl Facade for EventSystemFacade {
    fn name(&self) -> &'static str {
        "Event System"
    }
    fn install(&mut self) {
        skyline::println!("[SLight] Installing facade Event System");
        crate::slight::systems::event_system::install();
    }
    fn on_frame(&mut self) {
        crate::slight::systems::event_system::on_frame();
        // Run once per global frame, after lifecycle events have been observed.
        // The gimmick dispatcher owns only the collision queues it consumes;
        // it never drains the shared event queue.
        crate::slight::gimmicks::on_frame();
    }
    fn fighter_frame(&self) -> bool {
        false
    }
    fn weapon_frame(&self) -> bool {
        false
    }
}
