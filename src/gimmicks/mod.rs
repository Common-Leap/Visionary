pub mod acmd_adapter;
pub mod adapter;
pub mod behavior;
pub mod blueprint;
pub mod catalog;
pub mod cli;
pub mod codegen;
pub mod controller;
pub mod controller_asset;
pub mod controller_asset_io;
pub mod controller_edit;
pub(crate) mod controller_lower;
pub mod controller_project;
pub mod controller_protocol;
pub mod controller_runtime;
pub mod controls;
pub mod editor;
pub mod event_payload;
pub mod export;
pub mod field_canvas;
mod field_formula;
pub mod field_projection;
pub mod field_values;
pub mod graph_edit;
pub mod graph_view;
pub mod hooks;
pub mod live_model;
pub mod live_params;
pub mod live_protocol;
pub mod live_sync;
pub mod meter;
pub mod meter_view;
pub mod native_catalog;
pub mod native_inputs;
pub mod operations_view;
pub mod package;
pub mod params;
pub mod paths;
pub mod registry;
pub mod templates;
pub(crate) mod value_edit;
pub mod verifier;
pub mod view;

#[allow(dead_code)] // Native plugin delivery; desktop exercises the same command guard in tests.
mod phase_commands;

#[cfg(test)]
pub(crate) mod fixtures;

#[cfg(test)]
mod resource_actions_tests;

#[cfg(test)]
mod fighter_status_tests;

fn controller_random(seed: &std::cell::Cell<u64>, upper: i32) -> i32 {
    let next = seed
        .get()
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    seed.set(next);
    ((next >> 32) % upper as u64) as i32
}

#[cfg(test)]
mod controller_primitives_tests;

#[cfg(test)]
mod damage_events_tests;

#[allow(dead_code)] // Shared with native Main-boundary delivery.
mod damage_commands;
#[allow(dead_code)] // Shared with native capture-link delivery.
mod grab_commands;
pub mod hero_menu_commands;

pub mod status_model;

pub mod resource_commands;
pub mod status_commands;

pub mod status_recipes;
