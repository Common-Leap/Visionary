pub mod agent_extender;
pub mod agents;
pub mod csv;
pub mod diag;
#[cfg(not(feature = "mod_plugin"))]
pub mod effect_viewer;
/// A mod's plugin keeps only the effect records the framework's systems read.
#[cfg(feature = "mod_plugin")]
pub mod effect_viewer {
    pub mod effect_data;
    pub mod effect_names;
    pub mod tracker;
    /// Nothing feeds the tracker in a mod's plugin, so effect edits have nothing to act on.
    pub mod apply {
        #[derive(Default)]
        pub struct ParsedEdit {
            pub id: u64,
            pub scale: Option<f32>,
            pub rate: Option<f32>,
            pub pos: Option<super::effect_data::Point3D>,
            pub rot: Option<super::effect_data::Point3D>,
            pub visible: Option<bool>,
            pub is_follow: Option<bool>,
            pub frame: Option<f32>,
            pub color: Option<super::effect_data::Color>,
            pub movement_state: Option<f32>,
        }
        pub fn apply_rpm_edit(_edit: &ParsedEdit) -> bool {
            false
        }
    }
    pub fn init_fighter(boid: u32) {
        crate::slight::systems::main_module::on_init_fighter(boid);
    }
}
pub mod extras;
pub mod frame_context;
pub mod gimmicks;
#[cfg(not(feature = "mod_plugin"))]
pub mod hitbox_viewer;
pub mod main_smash;
pub mod math;
#[cfg(not(feature = "mod_plugin"))]
pub mod pending;
#[cfg(not(feature = "mod_plugin"))]
pub mod roster_heap;
#[cfg(not(feature = "mod_plugin"))]
pub mod roster_pin;
#[cfg(not(feature = "mod_plugin"))]
pub mod sd_poll;
pub mod slight_consts;
pub mod smash_utils;
pub mod systems;
