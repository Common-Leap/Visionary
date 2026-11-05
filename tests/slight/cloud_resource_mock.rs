//! Host engine double for graph resource effects; no native binding is simulated.
use super::{
    actor_identity::Identity,
    cloud_resource_model::{self as model, Snapshot},
};
pub(crate) use model::Command;
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};
#[derive(Default)]
struct Store {
    values: BTreeMap<u32, Snapshot>,
    calls: Vec<(u32, Identity, Command)>,
    unready: bool,
}
static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));
fn empty() -> Snapshot {
    Snapshot {
        gauge: 0.0,
        remaining: 0,
        full: false,
        set_custom: false,
        special: false,
        previous_gauge: 0.0,
        status: 0,
        changing: false,
    }
}
pub(crate) fn preflight(boid: u32, expected: Identity, command: Command) -> Result<(), String> {
    command.validate().map_err(str::to_owned)?;
    if Identity::from_live_fighter(boid, "cloud") != Some(expected) {
        return Err("native Limit actor identity changed".into());
    }
    let store = STORE.lock().unwrap();
    if store.unready {
        return Err("resource context is not ready".into());
    }
    model::preflight(
        command,
        store.values.get(&boid).copied().unwrap_or_else(empty),
    )
    .map_err(str::to_owned)
}
pub(crate) fn execute(boid: u32, expected: Identity, command: Command) -> Result<Snapshot, String> {
    preflight(boid, expected, command)?;
    let mut store = STORE.lock().unwrap();
    let before = store.values.get(&boid).copied().unwrap_or_else(empty);
    let after = match command {
        Command::Adjust(_) if before.full => before,
        Command::Adjust(delta) => {
            let gauge = (before.gauge + delta).clamp(0.0, 100.0);
            Snapshot {
                gauge,
                full: gauge == 100.0,
                set_custom: gauge == 100.0,
                remaining: if gauge == 100.0 {
                    900
                } else {
                    before.remaining
                },
                ..before
            }
        }
        Command::Clear => Snapshot {
            status: before.status,
            changing: before.changing,
            ..empty()
        },
    };
    model::verify(command, before, after).map_err(str::to_owned)?;
    store.calls.push((boid, expected, command));
    store.values.insert(boid, after);
    Ok(after)
}
pub(crate) fn reset() {
    *STORE.lock().unwrap() = Store::default();
}
pub(crate) fn set_phase(boid: u32, status: i32, changing: bool) {
    let mut store = STORE.lock().unwrap();
    let state = store.values.entry(boid).or_insert_with(empty);
    state.status = status;
    state.changing = changing;
}
pub(crate) fn calls() -> Vec<(u32, Identity, Command)> {
    STORE.lock().unwrap().calls.clone()
}
