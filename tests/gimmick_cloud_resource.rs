//! Native Limit command preflight and postconditions without a game process.

#[path = "../plugins/slight_replica/src/slight/gimmicks/cloud_resource_model.rs"]
mod model;

use model::{preflight, verify, Command, Snapshot};

type Mutation = fn(&mut Snapshot);

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

fn full() -> Snapshot {
    Snapshot {
        gauge: 100.0,
        previous_gauge: 100.0,
        full: true,
        set_custom: true,
        remaining: 780,
        ..empty()
    }
}

#[test]
fn adjustment_caps_floors_and_leaves_full_resources_unchanged() {
    for (gauge, delta, expected) in [
        (0.0, 40.0, 40.0),
        (40.0, -10.0, 30.0),
        (30.0, -80.0, 0.0),
        (30.0, 80.0, 100.0),
    ] {
        let before = Snapshot { gauge, ..empty() };
        let after = if expected == 100.0 {
            Snapshot {
                remaining: 900,
                previous_gauge: 0.0,
                ..full()
            }
        } else {
            Snapshot {
                gauge: expected,
                ..before
            }
        };
        assert!(verify(Command::Adjust(delta), before, after).is_ok());
    }
    assert!(verify(Command::Adjust(-25.0), full(), full()).is_ok());
    for after in [
        empty(),
        Snapshot {
            remaining: 779,
            ..full()
        },
    ] {
        assert!(verify(Command::Adjust(-25.0), full(), after).is_err());
    }
}

#[test]
fn clear_requires_every_lifecycle_field_to_be_reset() {
    for before in [
        full(),
        empty(),
        Snapshot {
            gauge: 25.0,
            previous_gauge: 25.0,
            ..empty()
        },
    ] {
        assert!(verify(Command::Clear, before, empty()).is_ok());
    }
    for change in [
        (|s| s.gauge = 1.0) as Mutation,
        |s| s.remaining = 1,
        |s| s.full = true,
        |s| s.set_custom = true,
        |s| s.special = true,
        |s| s.previous_gauge = 100.0,
    ] {
        let mut after = empty();
        change(&mut after);
        assert!(verify(Command::Clear, full(), after).is_err(), "{after:?}");
    }
}

#[test]
fn preflight_rejects_unsafe_phases_and_malformed_scalars() {
    for delta in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        1_000_001.0,
        -1_000_001.0,
    ] {
        assert!(preflight(Command::Adjust(delta), empty()).is_err());
    }
    for change in [
        (|s| s.status = 55) as Mutation,
        |s| s.changing = true,
        |s| s.gauge = f32::NAN,
        |s| s.gauge = 101.0,
        |s| s.gauge = -1.0,
        |s| s.previous_gauge = f32::INFINITY,
        |s| s.previous_gauge = -1.0,
        |s| s.remaining = -1,
        |s| s.full = true,
    ] {
        let mut before = empty();
        change(&mut before);
        assert!(preflight(Command::Clear, before).is_err(), "{before:?}");
    }
}

#[test]
fn reviewed_phases_preserve_identity_and_complete_charge_end_cleanup() {
    let phases = [0, 39, 14, 486, 487];
    for status in phases {
        let before = Snapshot { status, ..empty() };
        let after = Snapshot {
            gauge: 9.0,
            ..before
        };
        assert!(verify(Command::Adjust(9.0), before, after).is_ok());
        assert!(preflight(
            Command::Clear,
            Snapshot {
                changing: true,
                ..before
            }
        )
        .is_err());
        for other in phases.into_iter().filter(|other| *other != status) {
            for (command, expected) in [(Command::Adjust(9.0), after), (Command::Clear, before)] {
                assert!(verify(
                    command,
                    before,
                    Snapshot {
                        status: other,
                        ..expected
                    }
                )
                .is_err());
            }
        }
    }
    let before = Snapshot {
        status: 487,
        set_custom: false,
        special: true,
        ..full()
    };
    assert!(verify(
        Command::Clear,
        before,
        Snapshot {
            status: 487,
            ..empty()
        }
    )
    .is_ok());
}

#[test]
fn native_postconditions_reject_wrong_values_flags_and_phase_changes() {
    for change in [
        (|s| s.gauge = 41.0) as Mutation,
        |s| s.remaining = 1,
        |s| s.status = 486,
        |s| s.changing = true,
        |s| s.set_custom = true,
        |s| s.special = true,
        |s| s.previous_gauge = 40.0,
    ] {
        let mut after = Snapshot {
            gauge: 40.0,
            ..empty()
        };
        change(&mut after);
        assert!(verify(Command::Adjust(40.0), empty(), after).is_err());
    }
    for after in [
        Snapshot {
            gauge: 100.0,
            ..empty()
        },
        Snapshot {
            set_custom: false,
            ..full()
        },
    ] {
        assert!(verify(Command::Adjust(100.0), empty(), after).is_err());
    }
}
