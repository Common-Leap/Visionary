//! Decode-only boundary for symbolic gimmick identifiers (slice F1.4).
//!
//! This module deliberately performs no engine writes. A request is usable by
//! later operation adapters only after strict serde decoding and lookup in the
//! same generated allowlist the desktop uses.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierKind {
    WorkFloat,
    WorkInt,
    WorkFlag,
    Article,
    WeaponStatus,
    /// Exact fighter-owned statuses and the reviewed shared common statuses.
    /// Unlisted shared values, sentinels, and non-fighter helper owners are
    /// absent from the generated table by the pinned-registry policy.
    FighterStatus,
    TransitionTerm,
    TransitionGroup,
    /// An ITEM_KIND_* a fighter can be handed (owner "*").
    Item,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryEntry {
    pub fighter: &'static str,
    /// Weapon name for weapon statuses and Work fields, `common` for shared fighter statuses,
    /// and empty for fighter-owned identifiers.
    pub group: &'static str,
    pub kind: IdentifierKind,
    pub symbol: &'static str,
    pub value: u32,
}

/// Generated concrete weapon kind, used only as an object-type guard before
/// requesting a generated weapon status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponKindEntry {
    pub fighter: &'static str,
    pub group: &'static str,
    pub value: u32,
}

/// Fighter-kind mapping shared with the desktop registry. The numeric value is
/// pinned to the same skyline-smash source as the symbol and is used only to
/// identify a live object, never as an unvalidated write payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FighterKindEntry {
    pub fighter: &'static str,
    pub symbol: &'static str,
    pub value: u32,
}

#[allow(dead_code)]
mod generated {
    use super::{FighterKindEntry, IdentifierKind, RegistryEntry, WeaponKindEntry};
    include!("../../../../../src/gimmicks/registry_data.rs");
}

// The shared native-input model validates descriptors against the same exact
// generated entries as the desktop; this is not a numeric write API.
pub(crate) use generated::{ENTRIES, FIGHTER_KINDS};

pub fn fighter_kind_symbol(fighter: &str) -> Option<&'static str> {
    FIGHTER_KINDS
        .iter()
        .find(|entry| entry.fighter == fighter)
        .map(|entry| entry.symbol)
}

pub fn weapon_kind(fighter: &str, group: &str) -> Option<&'static WeaponKindEntry> {
    generated::WEAPON_KINDS
        .iter()
        .find(|entry| entry.fighter == fighter && entry.group == group)
}

/// Internal fighter name for an exact kind value from the pinned constants.
pub fn fighter_name(kind: i32) -> Option<&'static str> {
    let kind = u32::try_from(kind).ok()?;
    generated::FIGHTER_KINDS
        .iter()
        .find(|entry| entry.value == kind)
        .map(|entry| entry.fighter)
}

/// Exact engine kind for a pinned fighter name.
pub fn fighter_kind_value(fighter: &str) -> Option<i32> {
    generated::FIGHTER_KINDS
        .iter()
        .find(|entry| entry.fighter == fighter)
        .and_then(|entry| i32::try_from(entry.value).ok())
}

/// Shared fighter statuses are emitted once under this owner.
const COMMON_FIGHTER: &str = "*";

/// The pinned spelling of `fighter` when `entry` belongs to it, counting the
/// shared common statuses every supported fighter has.
fn owner(entry: &RegistryEntry, fighter: &str) -> Option<&'static str> {
    if entry.fighter == fighter {
        return Some(entry.fighter);
    }
    (entry.fighter == COMMON_FIGHTER)
        .then(|| {
            generated::FIGHTER_KINDS
                .iter()
                .find(|kind| kind.fighter == fighter)
        })
        .flatten()
        .map(|kind| kind.fighter)
}

/// Whether `entry` belongs to `fighter`, counting the shared common statuses.
pub fn owned_by(entry: &RegistryEntry, fighter: &str) -> bool {
    owner(entry, fighter).is_some()
}

/// Exact symbolic name for a pinned fighter/type/value tuple. Diagnostics use
/// this in the opt-in runtime matrix so logs retain source identity instead of
/// presenting a bare status number as evidence.
pub fn symbol_for_value(fighter: &str, kind: IdentifierKind, value: u32) -> Option<&'static str> {
    generated::ENTRIES
        .iter()
        .find(|entry| owner(entry, fighter).is_some() && entry.kind == kind && entry.value == value)
        .map(|entry| entry.symbol)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedIdentifier {
    pub fighter: &'static str,
    pub group: &'static str,
    pub kind: IdentifierKind,
    pub symbol: &'static str,
    pub value: u32,
}

/// Resolve a symbolic identifier in its exact fighter/type namespace.
pub fn resolve(
    fighter: &str,
    symbol: &str,
    expected: IdentifierKind,
) -> Result<ValidatedIdentifier, String> {
    let entry = generated::ENTRIES
        .iter()
        .find(|entry| owner(entry, fighter).is_some() && entry.symbol == symbol)
        .or_else(|| {
            generated::ENTRIES
                .iter()
                .find(|entry| entry.symbol == symbol)
        })
        .ok_or_else(|| format!("unknown symbolic identifier {symbol}"))?;
    let Some(owner) = owner(entry, fighter) else {
        return Err(format!(
            "identifier {symbol} belongs to {}, not {fighter}",
            entry.fighter
        ));
    };
    if !entry.group.is_empty()
        && matches!(
            entry.kind,
            IdentifierKind::WorkFloat | IdentifierKind::WorkInt | IdentifierKind::WorkFlag
        )
    {
        return Err(format!(
            "weapon Work field {symbol} requires projectile access"
        ));
    }
    if entry.kind != expected {
        return Err(format!(
            "identifier {symbol} is {:?}, not {:?}",
            entry.kind, expected
        ));
    }
    Ok(ValidatedIdentifier {
        fighter: owner,
        group: entry.group,
        kind: entry.kind,
        symbol: entry.symbol,
        value: entry.value,
    })
}

include!("../../../../../src/gimmicks/projectile_registry.rs");

/// The value of the first of this fighter's symbols that `matches` accepts, for node steps.
#[allow(dead_code)] // Called by the plugin only.
pub fn find_value(
    fighter: &str,
    kind: IdentifierKind,
    matches: impl Fn(&str) -> bool,
) -> Option<i32> {
    let entry = generated::ENTRIES.iter().find(|entry| {
        entry.kind == kind && owner(entry, fighter).is_some() && matches(entry.symbol)
    })?;
    Some(entry.value as i32)
}

/// Work IDs for the native sampler and writer, including scoped projectile fields.
pub fn work_value_id(fighter: &str, symbol: &str, kind: IdentifierKind) -> Result<u32, String> {
    if let Some(id) = super::native_inputs::common_work_id(symbol, kind) {
        return Ok(id);
    }
    if let Some((field, _)) = projectile_work(fighter, symbol, kind) {
        return Ok(field.value);
    }
    resolve(fighter, symbol, kind)
        .map(|entry| entry.value)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(
        fighter: &'a str,
        kind: &str,
        symbol: &'a str,
    ) -> (&'a str, &'a str, IdentifierKind) {
        let kind = match kind {
            "work_float" => IdentifierKind::WorkFloat,
            "work_int" => IdentifierKind::WorkInt,
            "work_flag" => IdentifierKind::WorkFlag,
            _ => IdentifierKind::FighterStatus,
        };
        (fighter, symbol, kind)
    }

    fn decode(
        (fighter, symbol, kind): (&str, &str, IdentifierKind),
    ) -> Result<ValidatedIdentifier, String> {
        resolve(fighter, symbol, kind)
    }

    #[test]
    fn work_range_boundaries_cannot_cross_the_wire_boundary() {
        for entry in generated::ENTRIES.iter().filter(|entry| {
            matches!(
                entry.kind,
                IdentifierKind::WorkFloat | IdentifierKind::WorkInt | IdentifierKind::WorkFlag
            )
        }) {
            assert!(
                !entry.symbol.ends_with("_START") && !entry.symbol.ends_with("_TERM"),
                "range marker in plugin registry: {}",
                entry.symbol
            );
        }
        for (fighter, kind, symbol) in [
            (
                "bayonetta",
                "work_float",
                "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLOAT_START",
            ),
            (
                "bayonetta",
                "work_float",
                "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLOAT_TERM",
            ),
            (
                "bayonetta",
                "work_int",
                "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_START",
            ),
            (
                "bayonetta",
                "work_int",
                "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_TERM",
            ),
            (
                "bayonetta",
                "work_flag",
                "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_START",
            ),
            (
                "bayonetta",
                "work_flag",
                "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_TERM",
            ),
            (
                "falco",
                "work_float",
                "FIGHTER_FALCO_INSTANCE_WORK_ID_FLOAT_POSTURE_LOG_POS_X_START",
            ),
        ] {
            assert!(
                decode(request(fighter, kind, symbol))
                    .unwrap_err()
                    .contains("unknown symbolic"),
                "range marker crossed the wire: {symbol}"
            );
        }
    }

    #[test]
    fn valid_symbol_resolves_only_in_its_typed_fighter_namespace() {
        let symbol = "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE";
        assert_eq!(
            decode(request("cloud", "work_float", symbol))
                .unwrap()
                .value,
            0x4C
        );
        assert!(decode(request("mario", "work_float", symbol))
            .unwrap_err()
            .contains("belongs to cloud"));
        assert!(decode(request("cloud", "work_int", symbol))
            .unwrap_err()
            .contains("not WorkInt"));
    }

    #[test]
    fn weapon_status_groups_have_a_concrete_weapon_type_guard() {
        let wave = weapon_kind("cloud", "wave").unwrap();
        assert_eq!(wave.value, 0x16C);
        assert!(weapon_kind("cloud", "not_a_weapon").is_none());
    }

    #[test]
    fn fighter_statuses_are_typed_and_reject_shared_sentinel_and_foreign_symbols() {
        let symbol = "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_END";
        assert_eq!(
            decode(request("cloud", "fighter_status", symbol))
                .unwrap()
                .value,
            0x1E7
        );
        assert_eq!(
            symbol_for_value("cloud", IdentifierKind::FighterStatus, 0x1E7),
            Some(symbol)
        );
        assert_eq!(
            symbol_for_value("mario", IdentifierKind::FighterStatus, 0x1E7),
            None
        );
        assert!(decode(request("mario", "fighter_status", symbol))
            .unwrap_err()
            .contains("belongs to cloud"));
        for unsafe_symbol in [
            "FIGHTER_STATUS_KIND_DEAD",
            "FIGHTER_CLOUD_STATUS_KIND_NUM",
            "FIGHTER_CLOUD_STATUS_KIND_PREV",
            "FIGHTER_DONKEY_STATUS_KIND_MAX",
            "FIGHTER_POKEMON_STATUS_KIND_SPECIAL_LW_OUT",
        ] {
            assert!(decode(request("cloud", "fighter_status", unsafe_symbol)).is_err());
        }
    }

    #[test]
    fn common_statuses_are_scoped_to_each_supported_fighter_in_the_plugin_decoder() {
        for fighter in ["mario", "cloud"] {
            let resolved = decode(request(
                fighter,
                "fighter_status",
                "FIGHTER_STATUS_KIND_WAIT",
            ))
            .unwrap();
            assert_eq!(resolved.fighter, fighter);
            assert_eq!(resolved.group, "common");
            assert_eq!(resolved.value, 0x0);
        }

        assert!(decode(request(
            "not_a_fighter",
            "fighter_status",
            "FIGHTER_STATUS_KIND_WAIT",
        ))
        .unwrap_err()
        .contains("not not_a_fighter"));
        assert!(
            decode(request("mario", "work_flag", "FIGHTER_STATUS_KIND_WAIT",))
                .unwrap_err()
                .contains("not WorkFlag")
        );
        for excluded in ["FIGHTER_STATUS_KIND_DEAD", "FIGHTER_STATUS_KIND_REBIRTH"] {
            assert!(decode(request("mario", "fighter_status", excluded))
                .unwrap_err()
                .contains("unknown symbolic"));
        }
    }

    #[test]
    fn generated_fighter_statuses_fit_the_signed_runtime_status_width() {
        assert!(generated::ENTRIES
            .iter()
            .filter(|entry| entry.kind == IdentifierKind::FighterStatus)
            .all(|entry| {
                entry.value <= i32::MAX as u32
                    && !matches!(
                        entry.symbol.rsplit("_STATUS_KIND_").next(),
                        Some("NONE" | "NUM" | "PREV" | "MAX")
                    )
            }));
    }
}
