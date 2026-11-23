//! Community-verified sheet of which weapon kinds can be cloned with
//! smashline's `clone_weapon`. The sheet's verifiers are credited per row in
//! `verified_by`; the raw export lives in `assets/clone_compat.csv`.

use std::sync::LazyLock;

/// The public sheet this data was exported from.
pub const SOURCE_URL: &str =
    "https://docs.google.com/spreadsheets/d/16SEU3MibrzTJHTjxJb7c5e7JzGgrfWY_c_hqNJtGvNY/edit?gid=0";

#[derive(Debug)]
pub struct CompatEntry {
    /// Weapon kind without the `WEAPON_KIND_` prefix, e.g. `MARIO_FIREBALL`.
    pub weapon_kind: &'static str,
    /// `Some(true)` for "Yes", `Some(false)` for "No", `None` when unknown.
    pub can_clone: Option<bool>,
    pub verified_by: &'static str,
    pub on_hit: &'static str,
    pub on_shield: &'static str,
    pub notes: &'static str,
}

struct Sheet {
    #[cfg_attr(not(test), allow(dead_code))]
    advice: &'static str,
    entries: Vec<CompatEntry>,
}

/// Splits CSV text into records of cells. Handles quoted cells containing
/// commas, newlines and `""` escapes.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    quoted = false;
                }
            } else {
                cell.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            _ => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.trim().to_owned().into_boxed_str())
}

static SHEET: LazyLock<Sheet> = LazyLock::new(|| {
    let rows = parse_csv(include_str!("../../assets/clone_compat.csv"));
    let advice = rows.first().and_then(|r| r.first()).map_or("", |s| leak(s));
    // Row 0 is the advice note, row 1 the header.
    let entries = rows
        .iter()
        .skip(2)
        .filter(|r| !r.first().is_none_or(|k| k.trim().is_empty()))
        .map(|r| {
            let cell = |i: usize| r.get(i).map_or("", |s| leak(s));
            CompatEntry {
                weapon_kind: cell(0),
                can_clone: match cell(1).to_ascii_lowercase().as_str() {
                    "yes" => Some(true),
                    "no" => Some(false),
                    _ => None,
                },
                verified_by: cell(2),
                on_hit: cell(3),
                on_shield: cell(4),
                notes: cell(5),
            }
        })
        .collect();
    Sheet { advice, entries }
});

/// The sheet's top note about making educated guesses (row 1 of the sheet).
pub const ADVICE: &str = "NOTE: We're not charting every single article:fighter combination, so please make educated guesses before cloning articles. A good rule of thumb is that any article that would need to communicate with the owner in a unique way (pkthunder, gordo, blade beam), utilizes level geometry (hydrant), comes from a DLC fighter (ray of punishment), or comes from a semiclone character (young link arrows) is most likely not going to work. Self-destruct behavior can often be changed by either hooking the common weapon on hit vtable (0x33bdc30 [13.0.3]), common weapon on shield vtable (0x346c8d0 [13.0.3]) or the weapon's specific vtable entries.";

/// Looks a weapon kind up. Accepts `WEAPON_KIND_MARIO_FIREBALL`,
/// `*WEAPON_KIND_MARIO_FIREBALL` or `MARIO_FIREBALL`.
pub fn lookup(weapon_kind: &str) -> Option<&'static CompatEntry> {
    let name = weapon_kind.trim().trim_start_matches('*');
    let name = name.strip_prefix("WEAPON_KIND_").unwrap_or(name);
    SHEET.entries.iter().find(|e| e.weapon_kind == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mario_fireball_is_cloneable_with_notes() {
        let e = lookup("*WEAPON_KIND_MARIO_FIREBALL").unwrap();
        assert_eq!(e.can_clone, Some(true));
        assert_eq!(e.on_hit, "Depends on vl.prc");
        assert!(e.notes.contains("vl parameters"));
    }

    #[test]
    fn brave_crash_cannot_clone() {
        assert_eq!(lookup("BRAVE_CRASH").unwrap().can_clone, Some(false));
    }

    #[test]
    fn samus_bomb_notes_mention_active_num() {
        assert!(lookup("SAMUS_BOMB")
            .unwrap()
            .notes
            .contains("get_active_num"));
    }

    #[test]
    fn unknown_kind_is_none() {
        assert!(lookup("NOT_A_KIND").is_none());
    }

    #[test]
    fn sheet_has_all_rows_and_advice() {
        assert_eq!(SHEET.entries.len(), 67);
        assert!(ADVICE.contains("educated guesses"));
        assert_eq!(
            ADVICE, SHEET.advice,
            "ADVICE must match the CSV's first row"
        );
    }
}
