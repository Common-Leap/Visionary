//! Exact fighter-owned WorkModule flags admitted only for native source editing.
//!
//! This catalogue intentionally has no numeric values and remains separate
//! from the shared live gimmick registry. Entries were checked against
//! `skyline-smash` revision `258a566618eb5e5540df572d6cbacf48728a1b98`
//! (`lua_const.rs` SHA-256
//! `ac8aa7e32e87a440e8b1f8f99d53270a29a0ee2e9a9bbcd17eb8d71248e1963c`), its
//! exact `FIGHTER_KIND_*` mappings, and the cached native Game sources.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub fighter: &'static str,
    pub symbol: &'static str,
}

include!("native_status_flags_data.rs");

pub fn entries() -> &'static [Entry] {
    ENTRIES
}

pub fn resolve(fighter: &str, symbol: &str) -> Option<&'static Entry> {
    ENTRIES
        .iter()
        .find(|entry| entry.fighter == fighter && entry.symbol == symbol)
}

pub fn owner_of(symbol: &str) -> Option<&'static str> {
    ENTRIES
        .iter()
        .find(|entry| entry.symbol == symbol)
        .map(|entry| entry.fighter)
}

pub fn contains_symbol(symbol: &str) -> bool {
    owner_of(symbol).is_some()
}

pub fn choices(fighter: &str) -> impl Iterator<Item = &'static str> + '_ {
    entries()
        .iter()
        .filter(move |entry| entry.fighter == fighter)
        .map(|entry| entry.symbol)
}
