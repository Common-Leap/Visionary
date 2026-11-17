//! Typed, non-rendering meter snapshots (slice F3.3/F3.4 foundation).
//!
//! This module deliberately stops before `ui2d`: it proves which *live
//! fighter* owns each meter value and provides a read-only, typed snapshot for
//! a later generic renderer. A binding cannot contain a numeric work id. Work
//! reads first resolve the generated
//! symbolic allowlist; generated behavior values use the closed interpreter's
//! read-only API. Invalid, stale, duplicate, or recycled fighter identities
//! produce no snapshot rather than a value attributed to the wrong player.
//! Work-source snapshots may be copied to the desktop for its read-only live
//! authoring preview; TCP never supplies a source or value to this module.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use parking_lot::Mutex;
use smash::app::sv_battle_object;

use super::{behavior, registry};

/// One typed source a future renderer may display.  The source is static
/// plugin code, never desktop/TCP input at runtime.
#[derive(Clone, Copy)]
pub enum MeterSource {
    /// A generated, allowlisted fighter work identifier.
    Work {
        symbol: &'static str,
        kind: registry::IdentifierKind,
    },
    /// A generated behavior variable.  This is intentionally read-only.
    Behavior {
        program: &'static behavior::Program,
        variable: &'static str,
    },
}

/// A static meter declaration. `id` must be unique across all declarations;
/// duplicates are rejected for the whole synchronization pass.
#[derive(Clone, Copy)]
pub struct MeterBinding {
    pub id: &'static str,
    pub fighter: &'static str,
    pub source: MeterSource,
    pub presentation: MeterPresentation,
}

/// Renderer-independent display semantics retained from the validated desktop
/// binding.  This is static generated data, not an instruction to touch ui2d.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MeterPresentation {
    FloatBar { min: f32, max: f32 },
    IntBar { min: i32, max: i32 },
    FlagIcon,
    CycleIcon { labels: &'static [&'static str] },
    Pips { count: u16 },
    StockArrows { min: i32, max: i32 },
}

/// The raw, typed value before any presentation range/clamping policy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MeterValue {
    Float(f32),
    Int(i32),
    Flag(bool),
}

impl From<behavior::Value> for MeterValue {
    fn from(value: behavior::Value) -> Self {
        match value {
            behavior::Value::Float(value) => Self::Float(value),
            behavior::Value::Int(value) => Self::Int(value),
            behavior::Value::Flag(value) => Self::Flag(value),
        }
    }
}

/// Stable player entry plus the complete current object identity.  Entry owns
/// the display slot; BOID, fighter kind, and module address prevent an old
/// fighter object (or a transformed/recycled one) from leaking into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FighterIdentity {
    pub entry_id: i32,
    pub boid: u32,
    pub fighter_kind: i32,
    module_accessor_addr: u64,
}

/// One synchronized value.  It is intentionally presentation-free.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterSnapshot {
    pub binding_id: &'static str,
    pub fighter: &'static str,
    pub identity: FighterIdentity,
    pub value: MeterValue,
}

/// A validated value ready for a future renderer.  Numeric displays are
/// clamped to their authored display range; malformed declarations, non-finite
/// floats, type mismatches, and invalid cycle indices produce no value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PreparedValue {
    Bar { fraction: f32 },
    FlagIcon { active: bool },
    CycleIcon { index: usize, label: &'static str },
    Pips { filled: u16, total: u16 },
    StockArrows { value: i32, min: i32, max: i32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedMeter {
    pub binding_id: &'static str,
    pub fighter: &'static str,
    pub identity: FighterIdentity,
    pub value: PreparedValue,
}

#[derive(Clone)]
struct EntrySnapshots {
    identity: FighterIdentity,
    values: BTreeMap<&'static str, MeterSnapshot>,
}

static SNAPSHOTS: LazyLock<Mutex<BTreeMap<i32, EntrySnapshots>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Rebuild snapshots from the current agent registry.  The rebuild has no
/// engine writes and replaces the whole store, so registry pruning also clears
/// despawned fighters even when their reset callback was missed.
pub fn on_frame(bindings: &'static [MeterBinding]) {
    let live = crate::slight::agents::all_records()
        .into_iter()
        .filter_map(LiveFighter::from_record)
        .collect::<Vec<_>>();
    synchronize(bindings, live, read_live);
}

/// Remove an object's snapshots immediately on spawn/reset/despawn.  The next
/// frame can only reintroduce values after it re-proves the live identity.
pub fn clear_boid(boid: u32) {
    SNAPSHOTS
        .lock()
        .retain(|_, entry| entry.identity.boid != boid);
}

/// Match teardown, plugin teardown, and reconnect boundary.
pub fn clear() {
    SNAPSHOTS.lock().clear();
}

/// Return a copy for a specific player entry and binding.  A later renderer
/// must query by entry, never by fighter kind, so same-kind multiplayer stays
/// separate.
pub fn snapshot(entry_id: i32, binding_id: &str) -> Option<MeterSnapshot> {
    SNAPSHOTS
        .lock()
        .get(&entry_id)
        .and_then(|entry| entry.values.get(binding_id).cloned())
}

/// Project one current snapshot through its static presentation contract.
/// The id and fighter checks prevent callers from pairing unrelated static
/// declarations and live values.
pub fn prepare(binding: &MeterBinding, snapshot: &MeterSnapshot) -> Option<PreparedMeter> {
    if snapshot.binding_id != binding.id || snapshot.fighter != binding.fighter {
        return None;
    }
    let value = match (binding.presentation, snapshot.value) {
        (MeterPresentation::FloatBar { min, max }, MeterValue::Float(value)) => {
            let span = max - min;
            if !min.is_finite()
                || !max.is_finite()
                || !value.is_finite()
                || !span.is_finite()
                || span <= 0.0
            {
                return None;
            }
            PreparedValue::Bar {
                fraction: ((value.clamp(min, max) - min) / span).clamp(0.0, 1.0),
            }
        }
        (MeterPresentation::IntBar { min, max }, MeterValue::Int(value)) if min < max => {
            let clamped = value.clamp(min, max) as f64;
            PreparedValue::Bar {
                fraction: ((clamped - min as f64) / (max as f64 - min as f64)) as f32,
            }
        }
        (MeterPresentation::FlagIcon, MeterValue::Flag(active)) => {
            PreparedValue::FlagIcon { active }
        }
        (MeterPresentation::CycleIcon { labels }, MeterValue::Int(value)) => {
            let index = usize::try_from(value).ok()?;
            let label = *labels.get(index)?;
            if label.trim().is_empty() {
                return None;
            }
            PreparedValue::CycleIcon { index, label }
        }
        (MeterPresentation::Pips { count }, MeterValue::Int(value)) if count > 0 => {
            PreparedValue::Pips {
                filled: value.clamp(0, i32::from(count)) as u16,
                total: count,
            }
        }
        (MeterPresentation::StockArrows { min, max }, MeterValue::Int(value)) if min < max => {
            PreparedValue::StockArrows {
                value: value.clamp(min, max),
                min,
                max,
            }
        }
        _ => return None,
    };
    Some(PreparedMeter {
        binding_id: binding.id,
        fighter: binding.fighter,
        identity: snapshot.identity,
        value,
    })
}

/// Fetch and prepare one current player-entry value.
pub fn prepared(entry_id: i32, binding: &MeterBinding) -> Option<PreparedMeter> {
    let snapshot = snapshot(entry_id, binding.id)?;
    prepare(binding, &snapshot)
}

/// Count currently published player entries.  Diagnostics/tests only.
pub fn entry_count() -> usize {
    SNAPSHOTS.lock().len()
}

/// Read-only telemetry for Visionary's live meter preview. Only generated
/// Work sources carry a stable public symbol; behavior variables stay inside
/// their compiled program and are not exposed over TCP.
pub fn wire_rows(bindings: &[MeterBinding]) -> Vec<serde_json::Value> {
    let snapshots = SNAPSHOTS.lock();
    let mut rows = Vec::new();
    for binding in bindings {
        let MeterSource::Work { symbol, kind } = binding.source else {
            continue;
        };
        for entry in snapshots.values() {
            let Some(snapshot) = entry.values.get(binding.id) else {
                continue;
            };
            let (kind, value) = match (kind, snapshot.value) {
                (registry::IdentifierKind::WorkFloat, MeterValue::Float(value))
                    if value.is_finite() =>
                {
                    ("work_float", serde_json::json!({ "float": value }))
                }
                (registry::IdentifierKind::WorkInt, MeterValue::Int(value)) => {
                    ("work_int", serde_json::json!({ "int": value }))
                }
                (registry::IdentifierKind::WorkFlag, MeterValue::Flag(value)) => {
                    ("work_flag", serde_json::json!({ "flag": value }))
                }
                _ => continue,
            };
            rows.push(serde_json::json!({
                "binding_id": binding.id,
                "fighter": binding.fighter,
                "entry_id": snapshot.identity.entry_id,
                "boid": snapshot.identity.boid,
                "symbol": symbol,
                "kind": kind,
                "value": value,
            }));
        }
    }
    rows.sort_by(|left, right| {
        left["entry_id"]
            .as_i64()
            .unwrap_or(-1)
            .cmp(&right["entry_id"].as_i64().unwrap_or(-1))
            .then_with(|| {
                left["binding_id"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["binding_id"].as_str().unwrap_or_default())
            })
    });
    rows
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LiveFighter {
    identity: FighterIdentity,
    fighter: &'static str,
}

impl LiveFighter {
    fn from_record(record: crate::slight::agents::AgentRecord) -> Option<Self> {
        if record.category != 0 || record.entry_id < 0 || record.module_accessor_addr == 0 {
            return None;
        }
        let fighter = crate::slight::gimmicks::registry::fighter_name(record.kind)?;
        Some(Self {
            identity: FighterIdentity {
                entry_id: record.entry_id,
                boid: record.boid,
                fighter_kind: record.kind,
                module_accessor_addr: record.module_accessor_addr,
            },
            fighter,
        })
    }
}

/// Pure reconciliation used by host tests.  Reject duplicate player entries
/// rather than choosing whichever duplicate happened to arrive first.
fn synchronize<F>(bindings: &[MeterBinding], live: Vec<LiveFighter>, mut read: F)
where
    F: FnMut(&MeterBinding, LiveFighter) -> Result<MeterValue, String>,
{
    let mut ids = BTreeSet::new();
    if bindings
        .iter()
        .any(|binding| binding.id.is_empty() || !ids.insert(binding.id))
    {
        clear();
        return;
    }

    let mut counts = BTreeMap::<i32, usize>::new();
    for fighter in &live {
        *counts.entry(fighter.identity.entry_id).or_default() += 1;
    }

    let mut next = BTreeMap::new();
    for fighter in live {
        // A duplicate player entry makes ownership ambiguous.  Do not retain a
        // stale previous snapshot for it.
        if counts[&fighter.identity.entry_id] != 1 {
            continue;
        }
        let mut values = BTreeMap::new();
        for binding in bindings
            .iter()
            .filter(|binding| binding.fighter == fighter.fighter)
        {
            if let Ok(value) = read(binding, fighter) {
                values.insert(
                    binding.id,
                    MeterSnapshot {
                        binding_id: binding.id,
                        fighter: binding.fighter,
                        identity: fighter.identity,
                        value,
                    },
                );
            }
        }
        if !values.is_empty() {
            next.insert(
                fighter.identity.entry_id,
                EntrySnapshots {
                    identity: fighter.identity,
                    values,
                },
            );
        }
    }
    *SNAPSHOTS.lock() = next;
}

/// Prove the record is still the very object that was registered before
/// touching its WorkModule.  This repeats the important live checks instead
/// of trusting a possibly one-frame-old agent registry record.
fn read_live(binding: &MeterBinding, fighter: LiveFighter) -> Result<MeterValue, String> {
    unsafe {
        let boid = fighter.identity.boid;
        if sv_battle_object::is_null(boid)
            || !sv_battle_object::is_active(boid)
            || sv_battle_object::category(boid) != 0
            || sv_battle_object::entry_id(boid) != fighter.identity.entry_id
            || sv_battle_object::kind(boid) != fighter.identity.fighter_kind
        {
            return Err("fighter identity is no longer live".to_string());
        }
        let accessor = sv_battle_object::module_accessor(boid);
        if accessor.is_null() || accessor as u64 != fighter.identity.module_accessor_addr {
            return Err("fighter module accessor changed".to_string());
        }
        match binding.source {
            MeterSource::Work { symbol, kind } => {
                let identity =
                    super::live_behavior::Identity::from_live_fighter(boid, binding.fighter)
                        .ok_or_else(|| "meter fighter has no current stock identity".to_string())?;
                let values = super::native_values::sample_work_values(
                    boid,
                    binding.fighter,
                    identity,
                    &[super::native_values::WorkSource { symbol, kind }],
                )?;
                match values.as_slice() {
                    [super::native_values::WorkValue::Float(value)] => {
                        Ok(MeterValue::Float(*value))
                    }
                    [super::native_values::WorkValue::Int(value)] => Ok(MeterValue::Int(*value)),
                    [super::native_values::WorkValue::Flag(value)] => Ok(MeterValue::Flag(*value)),
                    _ => Err("meter native source returned no exact value".to_string()),
                }
            }
            MeterSource::Behavior { program, variable } => {
                if program.fighter != binding.fighter || !(program.matches_fighter)(boid) {
                    return Err("behavior meter does not match live fighter".to_string());
                }
                behavior::read(program, boid, variable).map(MeterValue::from)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOUD: MeterBinding = MeterBinding {
        id: "cloud.limit",
        fighter: "cloud",
        source: MeterSource::Work {
            symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE",
            kind: registry::IdentifierKind::WorkFloat,
        },
        presentation: MeterPresentation::FloatBar {
            min: 0.0,
            max: 100.0,
        },
    };

    fn live(entry_id: i32, boid: u32, fighter: &'static str) -> LiveFighter {
        LiveFighter {
            identity: FighterIdentity {
                entry_id,
                boid,
                fighter_kind: 0x1234,
                module_accessor_addr: boid as u64 + 1,
            },
            fighter,
        }
    }

    fn sync(bindings: &[MeterBinding], fighters: Vec<LiveFighter>) {
        synchronize(bindings, fighters, |binding, fighter| {
            if binding.id == "reject" {
                Err("simulated invalid read".into())
            } else {
                Ok(MeterValue::Int(
                    (fighter.identity.entry_id * 10) + fighter.identity.boid as i32,
                ))
            }
        });
    }

    #[test]
    fn same_kind_multiplayer_is_owned_by_entry_not_fighter_kind() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        clear();
        sync(&[CLOUD], vec![live(0, 10, "cloud"), live(1, 11, "cloud")]);
        assert_eq!(entry_count(), 2);
        assert_eq!(
            snapshot(0, "cloud.limit").unwrap().value,
            MeterValue::Int(10)
        );
        assert_eq!(
            snapshot(1, "cloud.limit").unwrap().value,
            MeterValue::Int(21)
        );
        clear();
    }

    #[test]
    fn respawn_or_transformation_replaces_old_identity_at_same_entry() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        clear();
        sync(&[CLOUD], vec![live(2, 20, "cloud")]);
        assert_eq!(snapshot(2, "cloud.limit").unwrap().identity.boid, 20);
        // A new object in the same player slot replaces, rather than merges
        // with, the old entry.
        sync(&[CLOUD], vec![live(2, 21, "cloud")]);
        let current = snapshot(2, "cloud.limit").unwrap();
        assert_eq!(current.identity.boid, 21);
        assert_eq!(current.value, MeterValue::Int(41));
        clear();
    }

    #[test]
    fn reset_despawn_and_match_reconcile_clear_values() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        clear();
        sync(&[CLOUD], vec![live(0, 30, "cloud")]);
        clear_boid(30);
        assert!(snapshot(0, "cloud.limit").is_none());
        sync(&[CLOUD], vec![live(0, 31, "cloud")]);
        sync(&[CLOUD], vec![]);
        assert_eq!(entry_count(), 0);
        clear();
    }

    #[test]
    fn ambiguous_or_failed_sources_fail_closed_without_stale_values() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        clear();
        sync(&[CLOUD], vec![live(0, 40, "cloud")]);
        // Duplicate entry ownership is rejected as a whole.
        sync(&[CLOUD], vec![live(0, 40, "cloud"), live(0, 41, "cloud")]);
        assert!(snapshot(0, "cloud.limit").is_none());
        let reject = MeterBinding {
            id: "reject",
            ..CLOUD
        };
        sync(&[reject], vec![live(0, 42, "cloud")]);
        assert!(snapshot(0, "reject").is_none());
        // Duplicate binding ids are a static configuration error, so clear
        // the full store rather than guessing which declaration won.
        sync(&[CLOUD, CLOUD], vec![live(0, 42, "cloud")]);
        assert_eq!(entry_count(), 0);
        clear();
    }

    #[test]
    fn wire_rows_preserve_zero_boid_and_typed_work_values() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        clear();
        synchronize(&[CLOUD], vec![live(0, 0, "cloud")], |_, _| {
            Ok(MeterValue::Float(47.5))
        });

        let rows = wire_rows(&[CLOUD]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["binding_id"], CLOUD.id);
        assert_eq!(rows[0]["fighter"], CLOUD.fighter);
        assert_eq!(rows[0]["entry_id"], 0);
        assert_eq!(rows[0]["boid"], 0);
        assert_eq!(rows[0]["kind"], "work_float");
        assert_eq!(rows[0]["value"]["float"], 47.5);
        clear();
    }

    fn snapshot_with(value: MeterValue) -> MeterSnapshot {
        MeterSnapshot {
            binding_id: CLOUD.id,
            fighter: CLOUD.fighter,
            identity: live(0, 50, "cloud").identity,
            value,
        }
    }

    #[test]
    fn prepared_bars_clamp_and_reject_non_finite_or_mismatched_values() {
        assert_eq!(
            prepare(&CLOUD, &snapshot_with(MeterValue::Float(125.0)))
                .unwrap()
                .value,
            PreparedValue::Bar { fraction: 1.0 }
        );
        assert!(prepare(&CLOUD, &snapshot_with(MeterValue::Float(f32::NAN))).is_none());
        assert!(prepare(&CLOUD, &snapshot_with(MeterValue::Int(50))).is_none());
    }

    #[test]
    fn prepared_discrete_presentations_are_bounded_and_fail_closed() {
        let pips = MeterBinding {
            presentation: MeterPresentation::Pips { count: 3 },
            ..CLOUD
        };
        assert_eq!(
            prepare(&pips, &snapshot_with(MeterValue::Int(9)))
                .unwrap()
                .value,
            PreparedValue::Pips {
                filled: 3,
                total: 3
            }
        );
        const LABELS: &[&str] = &["one", "two"];
        let cycle = MeterBinding {
            presentation: MeterPresentation::CycleIcon { labels: LABELS },
            ..CLOUD
        };
        assert_eq!(
            prepare(&cycle, &snapshot_with(MeterValue::Int(1)))
                .unwrap()
                .value,
            PreparedValue::CycleIcon {
                index: 1,
                label: "two"
            }
        );
        assert!(prepare(&cycle, &snapshot_with(MeterValue::Int(2))).is_none());
    }
}
