use super::*;
use slight::gimmicks::meter_snapshot::{self, MeterBinding, MeterPresentation, MeterSource};
use slight::gimmicks::registry::IdentifierKind;

const LIMIT: MeterSource = MeterSource::Work {
    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE",
    kind: IdentifierKind::WorkFloat,
};
const BAR: MeterPresentation = MeterPresentation::FloatBar {
    min: 0.0,
    max: 100.0,
};
const MADE: &[u8] = &[8, 9, 10, 11, 12, 13, 14, 15];

/// Cloud's own meter and the meter of a character made on Cloud (costumes c08-c15), as the
/// exported adapter declares them.
static BINDINGS: [MeterBinding; 2] = [
    MeterBinding {
        id: "meter.cloud.0",
        fighter: "cloud",
        costumes: &[],
        except_costumes: MADE,
        source: LIMIT,
        presentation: BAR,
    },
    MeterBinding {
        id: "meter.cloud#c08.0",
        fighter: "cloud",
        costumes: MADE,
        except_costumes: &[],
        source: LIMIT,
        presentation: BAR,
    },
];

/// Each player sees the meter of the character whose costume it wears: Cloud's on c00, the made
/// character's on c08, and neither on the other's costume.
#[test]
fn a_meter_shows_only_on_its_own_characters_costumes() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    meter_snapshot::clear();
    cloud(301, 0x30100, 0);
    cloud(302, 0x30200, 1);
    smash::app::lua_bind::dress(0x30100, 0);
    smash::app::lua_bind::dress(0x30200, 8);
    meter_snapshot::on_frame(&BINDINGS);
    assert!(meter_snapshot::snapshot(0, "meter.cloud.0").is_some());
    assert!(meter_snapshot::snapshot(0, "meter.cloud#c08.0").is_none());
    assert!(meter_snapshot::snapshot(1, "meter.cloud#c08.0").is_some());
    assert!(meter_snapshot::snapshot(1, "meter.cloud.0").is_none());

    // Changing costume between matches moves the player to the other character's meter.
    smash::app::lua_bind::dress(0x30100, 12);
    meter_snapshot::on_frame(&BINDINGS);
    assert!(meter_snapshot::snapshot(0, "meter.cloud.0").is_none());
    assert!(meter_snapshot::snapshot(0, "meter.cloud#c08.0").is_some());
    meter_snapshot::clear();
}

/// A fighter with no made characters keeps its meter on every costume.
#[test]
fn an_unlimited_meter_shows_on_every_costume() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    meter_snapshot::clear();
    static OWN: [MeterBinding; 1] = [MeterBinding {
        id: "meter.cloud.0",
        fighter: "cloud",
        costumes: &[],
        except_costumes: &[],
        source: LIMIT,
        presentation: BAR,
    }];
    for (boid, accessor, entry, costume) in [(311, 0x31100, 0, 0), (312, 0x31200, 1, 9)] {
        cloud(boid, accessor, entry);
        smash::app::lua_bind::dress(accessor, costume);
    }
    meter_snapshot::on_frame(&OWN);
    assert!(meter_snapshot::snapshot(0, "meter.cloud.0").is_some());
    assert!(meter_snapshot::snapshot(1, "meter.cloud.0").is_some());
    meter_snapshot::clear();
}
