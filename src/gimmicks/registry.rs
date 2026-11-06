//! Typed, fail-closed lookup for engine identifiers used by gimmick behavior.

include!("projectile_registry.rs");

#[cfg(test)]
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdentifierKind {
    WorkFloat,
    WorkInt,
    WorkFlag,
    Article,
    WeaponStatus,
    /// An exact fighter-owned status or one of the reviewed shared
    /// `FIGHTER_STATUS_KIND_*` entries scoped to a supported fighter.
    /// Helper owners are never admitted.
    FighterStatus,
    TransitionTerm,
    TransitionGroup,
    /// An ITEM_KIND_* a fighter can be handed (owner "*").
    Item,
}

/// Weapon statuses belonging to one article, matched by its weapon name
/// (`…_GENERATE_ARTICLE_WAVE` owns the `wave` statuses).
pub fn article_statuses<'a>(
    fighter: &'a str,
    article: &str,
) -> impl Iterator<Item = &'static RegistryEntry> + 'a {
    let group = article
        .rsplit_once("_GENERATE_ARTICLE_")
        .map_or_else(String::new, |(_, name)| name.to_ascii_lowercase());
    entries_for(fighter, IdentifierKind::WeaponStatus)
        .filter(move |entry| !group.is_empty() && entry.group == group)
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

/// Concrete weapon kind used only to verify a live object before a generated
/// weapon-status identifier can reach `StatusModule`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponKindEntry {
    pub fighter: &'static str,
    pub group: &'static str,
    pub value: u32,
}

/// A fighter internal name paired with its exact pinned `lua_const.rs`
/// `FIGHTER_KIND_*` spelling and value. Source adapters still import the
/// symbolic constant; the value lets the plugin identify live objects without
/// relying on a version-sensitive address in the game's text segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FighterKindEntry {
    pub fighter: &'static str,
    pub symbol: &'static str,
    pub value: u32,
}

include!("registry_data.rs");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupError {
    UnknownSymbol {
        fighter: String,
        symbol: String,
    },
    WrongKind {
        symbol: String,
        expected: IdentifierKind,
        actual: IdentifierKind,
    },
    WrongFighter {
        symbol: String,
        expected: String,
        actual: String,
    },
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSymbol { fighter, symbol } => write!(
                formatter,
                "unknown safe identifier {symbol:?} for fighter {fighter:?}"
            ),
            Self::WrongKind {
                symbol,
                expected,
                actual,
            } => write!(
                formatter,
                "identifier {symbol:?} is {actual:?}, not the required {expected:?}"
            ),
            Self::WrongFighter {
                symbol,
                expected,
                actual,
            } => write!(
                formatter,
                "identifier {symbol:?} belongs to fighter {actual:?}, not {expected:?}"
            ),
        }
    }
}

impl std::error::Error for LookupError {}

/// Owner of the shared fighter statuses, which every supported fighter has.
pub const COMMON_FIGHTER: &str = "*";

/// Whether `entry` belongs to `fighter`, counting shared common statuses.
pub fn owned_by(entry: &RegistryEntry, fighter: &str) -> bool {
    entry.fighter == fighter
        || entry.fighter == COMMON_FIGHTER
            && FIGHTER_KINDS.iter().any(|kind| kind.fighter == fighter)
}

pub fn resolve(
    fighter: &str,
    symbol: &str,
    expected: IdentifierKind,
) -> Result<&'static RegistryEntry, LookupError> {
    let Some(entry) = ENTRIES
        .iter()
        .find(|entry| owned_by(entry, fighter) && entry.symbol == symbol)
        .or_else(|| ENTRIES.iter().find(|entry| entry.symbol == symbol))
    else {
        return Err(LookupError::UnknownSymbol {
            fighter: fighter.to_owned(),
            symbol: symbol.to_owned(),
        });
    };
    if !owned_by(entry, fighter) {
        return Err(LookupError::WrongFighter {
            symbol: symbol.to_owned(),
            expected: fighter.to_owned(),
            actual: entry.fighter.to_owned(),
        });
    }
    if !entry.group.is_empty()
        && matches!(
            entry.kind,
            IdentifierKind::WorkFloat | IdentifierKind::WorkInt | IdentifierKind::WorkFlag
        )
    {
        return Err(LookupError::UnknownSymbol {
            fighter: fighter.into(),
            symbol: symbol.into(),
        });
    }
    if entry.kind != expected {
        return Err(LookupError::WrongKind {
            symbol: symbol.to_owned(),
            expected,
            actual: entry.kind,
        });
    }
    Ok(entry)
}

/// The concrete weapon kind bound to a generated weapon-status group.
///
/// This is deliberately separate from [`resolve`]: callers cannot use it as
/// a generic numeric identifier, only as the live-object type guard paired
/// with an already resolved status entry.
pub fn weapon_kind(fighter: &str, group: &str) -> Option<&'static WeaponKindEntry> {
    WEAPON_KINDS
        .iter()
        .find(|entry| entry.fighter == fighter && entry.group == group)
}

/// The exact `FIGHTER_KIND_*` symbol that an adapter may import for `fighter`.
///
/// Names without a direct mapping in the pinned source return `None`; callers
/// must refuse emission instead of deriving an alias or a numeric fallback.
pub fn fighter_kind_symbol(fighter: &str) -> Option<&'static str> {
    FIGHTER_KINDS
        .iter()
        .find(|entry| entry.fighter == fighter)
        .map(|entry| entry.symbol)
}

/// Exact engine kind for a pinned fighter name.
pub fn fighter_kind_value(fighter: &str) -> Option<i32> {
    FIGHTER_KINDS
        .iter()
        .find(|entry| entry.fighter == fighter)
        .and_then(|entry| i32::try_from(entry.value).ok())
}

#[cfg(test)]
fn validate(entries: &[RegistryEntry]) -> Result<(), String> {
    let mut names = HashSet::new();
    let mut values = HashSet::new();
    for entry in entries {
        if !names.insert((entry.fighter, entry.symbol)) {
            return Err(format!("duplicate registry symbol {}", entry.symbol));
        }
        let namespace = (entry.fighter, entry.group, entry.kind, entry.value);
        if !values.insert(namespace) {
            return Err(format!(
                "duplicate value 0x{:X} in {:?}/{}/{}",
                entry.value, entry.kind, entry.fighter, entry.group
            ));
        }
    }
    Ok(())
}

/// Revision metadata carried alongside every generated entry.
#[allow(dead_code)] // Keeps the generated provenance constants without editing the generated file.
pub fn source_evidence() -> (&'static str, &'static str, usize) {
    (
        SKYLINE_SMASH_REVISION,
        LUA_CONST_SHA256,
        OMITTED_AMBIGUOUS_ALIASES,
    )
}

/// Allowlisted identifiers for one fighter and category, in generated stable
/// order. Authoring UIs use this instead of accepting an unbounded numeric ID.
pub fn entries_for(
    fighter: &str,
    kind: IdentifierKind,
) -> impl Iterator<Item = &'static RegistryEntry> + '_ {
    ENTRIES.iter().filter(move |entry| {
        entry.kind == kind
            && owned_by(entry, fighter)
            && (entry.group.is_empty()
                || !matches!(
                    kind,
                    IdentifierKind::WorkFloat | IdentifierKind::WorkInt | IdentifierKind::WorkFlag
                ))
    })
}

/// Every allowlisted article identifier in stable generated order.
///
/// Resource auditing uses this closed registry instead of scraping symbol
/// spellings from source or accepting caller-provided names. The returned
/// identifiers are still engine constants; finding a same-named asset is
/// evidence of co-location, not proof of an `ArticleModule` runtime binding.
pub fn article_entries() -> impl Iterator<Item = &'static RegistryEntry> {
    ENTRIES
        .iter()
        .filter(|entry| entry.kind == IdentifierKind::Article)
}

pub(crate) fn fighter_status_picker(ui: &mut egui::Ui, fighter: &str, symbol: &mut String) {
    search_picker(
        ui,
        fighter,
        IdentifierKind::FighterStatus,
        symbol,
        "Search statuses",
        super::field_canvas::status_name,
    );
}

pub(crate) fn transition_picker(
    ui: &mut egui::Ui,
    fighter: &str,
    symbol: &mut String,
    group: bool,
) {
    search_picker(
        ui,
        fighter,
        if group {
            IdentifierKind::TransitionGroup
        } else {
            IdentifierKind::TransitionTerm
        },
        symbol,
        "Search transitions",
        |symbol| {
            symbol
                .trim_start_matches("FIGHTER_STATUS_TRANSITION_")
                .trim_start_matches("TERM_ID_CONT_")
                .trim_start_matches("GROUP_CHK_")
                .to_lowercase()
                .replace('_', " ")
        },
    );
}

pub(crate) fn item_picker(ui: &mut egui::Ui, symbol: &mut String) {
    search_picker(
        ui,
        COMMON_FIGHTER,
        IdentifierKind::Item,
        symbol,
        "Search items",
        super::graph_view::item_name,
    );
}

fn search_picker(
    ui: &mut egui::Ui,
    fighter: &str,
    kind: IdentifierKind,
    symbol: &mut String,
    hint: &str,
    name: fn(&str) -> String,
) {
    let id = ui.make_persistent_id((hint, "search"));
    let mut query = ui.data_mut(|data| data.get_temp::<String>(id).unwrap_or_default());
    egui::ComboBox::from_id_salt(hint)
        .selected_text(name(symbol))
        .height(420.0)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut query).hint_text(hint));
            let needle = query.to_lowercase();
            for entry in entries_for(fighter, kind) {
                let label = name(entry.symbol);
                if label.to_lowercase().contains(&needle)
                    || entry.symbol.to_lowercase().contains(&needle)
                {
                    ui.selectable_value(symbol, entry.symbol.into(), label)
                        .on_hover_text(entry.symbol);
                }
            }
        })
        .response
        .on_hover_text(symbol.as_str());
    ui.data_mut(|data| data.insert_temp(id, query));
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn exact_lookup_requires_the_right_fighter_and_category() {
        for (symbol, kind) in [
            (
                "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE",
                IdentifierKind::WorkFloat,
            ),
            (
                "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
                IdentifierKind::Article,
            ),
            (
                "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR",
                IdentifierKind::WeaponStatus,
            ),
            (
                "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_END",
                IdentifierKind::FighterStatus,
            ),
            ("FIGHTER_STATUS_KIND_WAIT", IdentifierKind::FighterStatus),
        ] {
            assert!(resolve("cloud", symbol, kind).is_ok());
            assert!(matches!(
                resolve("cloud", symbol, IdentifierKind::WorkInt),
                Err(LookupError::WrongKind { .. })
            ));
        }
        assert!(matches!(
            resolve(
                "mario",
                "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
                IdentifierKind::Article
            ),
            Err(LookupError::WrongFighter { .. })
        ));
        assert_eq!(weapon_kind("cloud", "wave").unwrap().group, "wave");
        assert_eq!(weapon_kind("cloud", "wave").unwrap().value, 0x16C);
        assert_eq!(
            resolve(
                "cloud",
                "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE",
                IdentifierKind::WorkFloat
            )
            .unwrap()
            .value,
            0x4C
        );
        assert_eq!(
            resolve(
                "cloud",
                "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_END",
                IdentifierKind::FighterStatus
            )
            .unwrap()
            .value,
            0x1E7
        );
        assert!(weapon_kind("cloud", "missing").is_none());
        assert_eq!(fighter_kind_symbol("cloud"), Some("FIGHTER_KIND_CLOUD"));
        for name in ["Cloud", "unknown", "robin", "pokemon", "shield"] {
            assert!(fighter_kind_symbol(name).is_none(), "{name}");
        }
    }

    #[test]
    fn sentinels_and_unreviewed_spellings_are_never_resolved() {
        for entry in ENTRIES.iter().filter(|entry| {
            matches!(
                entry.kind,
                IdentifierKind::WorkFloat | IdentifierKind::WorkInt | IdentifierKind::WorkFlag
            )
        }) {
            assert!(
                !entry.symbol.ends_with("_START") && !entry.symbol.ends_with("_TERM"),
                "{}",
                entry.symbol
            );
        }
        for symbol in [
            "NOT_A_SYMBOL",
            "FIGHTER_STATUS_KIND_NONE",
            "FIGHTER_STATUS_KIND_NUM",
            "FIGHTER_STATUS_KIND_DEAD",
            "FIGHTER_CLOUD_STATUS_KIND_PREV",
            "FIGHTER_DONKEY_STATUS_KIND_MAX",
            "FIGHTER_POKEMON_STATUS_KIND_SPECIAL_LW_OUT",
            "FIGHTER_CLOUD_STATUS_KIND_NUM",
            "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_START",
        ] {
            assert!(
                resolve("cloud", symbol, IdentifierKind::FighterStatus).is_err(),
                "{symbol}"
            );
        }
        for suffix in [
            "FLOAT_START",
            "FLOAT_TERM",
            "INT_START",
            "INT_TERM",
            "FLAG_START",
            "FLAG_TERM",
        ] {
            let symbol = format!("FIGHTER_BAYONETTA_INSTANCE_WORK_ID_{suffix}");
            for kind in [
                IdentifierKind::WorkFloat,
                IdentifierKind::WorkInt,
                IdentifierKind::WorkFlag,
            ] {
                assert!(resolve("bayonetta", &symbol, kind).is_err(), "{symbol}");
            }
        }
    }

    #[test]
    fn generated_registry_has_unique_names_and_namespaced_values() {
        validate(ENTRIES).unwrap();
        assert!(ENTRIES.len() > 3_900);
    }

    #[test]
    fn reviewed_common_statuses_resolve_for_every_supported_fighter_only_as_statuses() {
        // These values are generated from the pinned skyline-smash lua_const.rs;
        // the generator's --check mode verifies every generated value against it.
        let common = [
            ("FIGHTER_STATUS_KIND_WAIT", 0x0),
            ("FIGHTER_STATUS_KIND_ATTACK", 0x27),
            ("FIGHTER_STATUS_KIND_ATTACK_AIR", 0x36),
            ("FIGHTER_STATUS_KIND_SPECIAL_N", 0x1DC),
            ("FIGHTER_STATUS_KIND_SPECIAL_S", 0x1DD),
            ("FIGHTER_STATUS_KIND_SPECIAL_LW", 0x1DF),
            ("FIGHTER_STATUS_KIND_SPECIAL_HI", 0x1DE),
            ("FIGHTER_STATUS_KIND_FALL", 0xE),
            ("FIGHTER_STATUS_KIND_JUMP", 0xB),
            ("FIGHTER_STATUS_KIND_SQUAT", 0x11),
            ("FIGHTER_STATUS_KIND_GUARD_ON", 0x1B),
            ("FIGHTER_STATUS_KIND_GUARD", 0x1C),
            ("FIGHTER_STATUS_KIND_GUARD_OFF", 0x1D),
        ];

        for fighter in FIGHTER_KINDS {
            for (symbol, value) in common {
                let entry = resolve(fighter.fighter, symbol, IdentifierKind::FighterStatus)
                    .unwrap_or_else(|error| panic!("{}: {error}", fighter.fighter));
                assert_eq!(entry.value, value, "{} / {symbol}", fighter.fighter);
                assert_eq!(entry.group, "common");
                assert!(entries_for(fighter.fighter, IdentifierKind::WorkFlag)
                    .all(|entry| entry.symbol != symbol));
            }
        }

        assert!(matches!(
            resolve(
                "not_a_fighter",
                "FIGHTER_STATUS_KIND_WAIT",
                IdentifierKind::FighterStatus
            ),
            Err(LookupError::WrongFighter { .. })
        ));
        for excluded in [
            "FIGHTER_STATUS_KIND_DEAD",
            "FIGHTER_STATUS_KIND_REBIRTH",
            "FIGHTER_STATUS_KIND_GUARD_WAIT",
        ] {
            assert!(matches!(
                resolve("mario", excluded, IdentifierKind::FighterStatus),
                Err(LookupError::UnknownSymbol { .. })
            ));
        }
        assert!(matches!(
            resolve(
                "mario",
                "FIGHTER_STATUS_KIND_WAIT",
                IdentifierKind::WorkFlag
            ),
            Err(LookupError::WrongKind { .. })
        ));
    }

    #[test]
    fn every_generated_fighter_status_has_valid_ownership_and_no_range_marker() {
        let statuses = ENTRIES
            .iter()
            .filter(|entry| entry.kind == IdentifierKind::FighterStatus)
            .collect::<Vec<_>>();
        let common = statuses
            .iter()
            .filter(|entry| entry.group == "common")
            .count();
        assert_eq!(common, 101);
        assert_eq!(statuses.len(), 1_918 + common);
        for entry in statuses {
            match entry.group {
                "" => {
                    assert!(fighter_kind_symbol(entry.fighter).is_some());
                    let prefix = format!(
                        "FIGHTER_{}_STATUS_KIND_",
                        entry.fighter.to_ascii_uppercase()
                    );
                    assert!(entry.symbol.starts_with(&prefix), "{}", entry.symbol);
                }
                "common" => assert!(
                    entry.fighter == COMMON_FIGHTER
                        && entry.symbol.starts_with("FIGHTER_STATUS_KIND_"),
                    "{}",
                    entry.symbol
                ),
                group => panic!("unexpected status group {group:?}"),
            }
            assert!(
                !matches!(
                    entry.symbol.rsplit("_STATUS_KIND_").next(),
                    Some("NONE" | "NUM" | "PREV" | "MAX")
                ) && entry.value <= i32::MAX as u32,
                "{}",
                entry.symbol
            );
        }
    }
}
