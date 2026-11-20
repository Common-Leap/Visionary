//! Which character an edit belongs to.
//!
//! A base fighter ("mario") and a character made in Visionary on top of it ("mario#c08", shown as
//! Vision) are separate characters. A made character starts as a copy of its donor and from then
//! on owns its own edits: the donor's later edits do not reach it, and its edits never reach the
//! donor. The engine still only knows the donor fighter, so every store keyed by character also
//! needs the engine fighter, which [`CharacterKey::engine_fighter`] gives.
//!
//! The key is spelled exactly as [`RosterKey`] spells it, so a base fighter's key is its fighter
//! name as before and projects with no made characters need no migration.

use std::fmt;

use crate::roster::RosterKey;

/// One character the editor can open: a base fighter or a character made in Visionary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CharacterKey(RosterKey);

impl CharacterKey {
    /// The key of a base fighter, spelled as its engine fighter name ("mario").
    pub fn base(fighter: &str) -> Self {
        Self(RosterKey::fighter(fighter))
    }

    /// The key of a character made in Visionary, by its roster key ("mario#c08").
    pub fn made(key: RosterKey) -> Self {
        Self(key)
    }

    /// Read a key from its stored spelling. Anything with a `#` is a made character; anything
    /// else is a base fighter, which keeps every fighter-keyed store of an older project valid.
    pub fn parse(spelling: &str) -> Self {
        match spelling.split_once('#') {
            Some((donor, slot)) => match slot.strip_prefix('c').and_then(|n| n.parse().ok()) {
                Some(slot) if !donor.is_empty() => Self(RosterKey::slot(donor, slot)),
                _ => Self(RosterKey::fighter(spelling)),
            },
            None => Self::base(spelling),
        }
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn roster_key(&self) -> &RosterKey {
        &self.0
    }

    /// The fighter the game runs for this character.
    pub fn engine_fighter(&self) -> &str {
        let spelling = self.0.as_str();
        spelling
            .split_once('#')
            .map_or(spelling, |(donor, _)| donor)
    }

    /// True for a character made in Visionary.
    pub fn is_made(&self) -> bool {
        self.0.as_str().contains('#')
    }

    /// The made character's entry in the project, when it is one.
    pub fn authored<'a>(
        &self,
        roster: &'a crate::mod_project::RosterMod,
    ) -> Option<&'a crate::mod_project::AuthoredEntry> {
        if !self.is_made() {
            return None;
        }
        roster.authored.iter().find(|entry| entry.key == self.0)
    }

    /// The costumes this character owns. Empty for a base fighter: see [`base_costumes`] for
    /// what is left of a donor once its made characters take theirs.
    pub fn made_slots(&self, roster: &crate::mod_project::RosterMod) -> Vec<u8> {
        self.authored(roster)
            .map(crate::mod_project::AuthoredEntry::all_slots)
            .unwrap_or_default()
    }

    /// The name people know this character by: its select-screen name when the project set
    /// one, the name it was made with, or the fighter's own display name.
    pub fn display_name(
        &self,
        roster: &crate::mod_project::RosterMod,
        fighter_display: Option<&str>,
    ) -> String {
        if let Some(name) = roster.names.get(&self.0).filter(|name| !name.is_empty()) {
            return name.clone();
        }
        if let Some(entry) = self.authored(roster) {
            if !entry.display_name.is_empty() {
                return entry.display_name.clone();
            }
        }
        fighter_display
            .filter(|name| !name.is_empty())
            .map_or_else(|| self.engine_fighter().to_owned(), str::to_owned)
    }

    /// "c08–c15" for a made character, or the given base range for a base fighter.
    pub fn costume_range(&self, roster: &crate::mod_project::RosterMod) -> Option<String> {
        let slots = self.made_slots(roster);
        slot_range(&slots)
    }
}

impl fmt::Display for CharacterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// "c08", "c08–c15", or "c08, c10" for slots that do not run on.
pub fn slot_range(slots: &[u8]) -> Option<String> {
    let mut sorted = slots.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let (first, last) = (*sorted.first()?, *sorted.last()?);
    if first == last {
        return Some(format!("c{first:02}"));
    }
    if usize::from(last - first) + 1 == sorted.len() {
        return Some(format!("c{first:02}\u{2013}c{last:02}"));
    }
    Some(
        sorted
            .iter()
            .map(|slot| format!("c{slot:02}"))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// The donor's own costumes: every slot it has that none of its made characters own.
pub fn base_costumes(
    fighter: &str,
    fighter_slots: &[u8],
    roster: &crate::mod_project::RosterMod,
) -> Vec<u8> {
    let taken: Vec<u8> = roster
        .authored
        .iter()
        .filter(|entry| entry.donor.eq_ignore_ascii_case(fighter))
        .flat_map(crate::mod_project::AuthoredEntry::all_slots)
        .collect();
    fighter_slots
        .iter()
        .copied()
        .filter(|slot| !taken.contains(slot))
        .collect()
}

/// The characters made on `fighter`, in roster order.
pub fn made_on<'a>(
    fighter: &'a str,
    roster: &'a crate::mod_project::RosterMod,
) -> impl Iterator<Item = &'a crate::mod_project::AuthoredEntry> + 'a {
    roster
        .authored
        .iter()
        .filter(move |entry| entry.donor.eq_ignore_ascii_case(fighter))
}

/// A page a character can be opened on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterPage {
    /// Classic editor, hitbox tab.
    ClassicCollisions,
    /// Classic editor, visual effects tab (which effects a move spawns).
    ClassicEffects,
    /// Classic editor, sound and feedback tab.
    ClassicAudio,
    MoveNodes,
    Gimmicks,
    /// The effect file editor.
    Effects,
}

/// The key every per-move store uses: `"<character>/<move>"`.
pub fn move_key(character: &CharacterKey, move_name: &str) -> String {
    format!("{}/{move_name}", character.as_str())
}

/// Split a per-move store key into its character and move. Move names never hold a `/`, and a
/// character key never does either, so the first `/` is the boundary.
pub fn parse_move_key(key: &str) -> Option<(CharacterKey, &str)> {
    let (character, move_name) = split_move_key(key)?;
    Some((CharacterKey::parse(character), move_name))
}

/// [`parse_move_key`] without parsing the character: its stored spelling and the move. For
/// grouping and lookups in stores keyed by that spelling, never for asking the game anything —
/// that needs [`CharacterKey::engine_fighter`].
pub fn split_move_key(key: &str) -> Option<(&str, &str)> {
    key.split_once('/')
}

/// The edits one character owns, counted per family. Every count is of that character's own
/// store only: a made character's edits are never counted on its donor, nor the reverse.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CharacterEditCounts {
    /// Moves whose game script (hitboxes and other collision calls) changed.
    pub game_moves: usize,
    /// Moves edited as nodes.
    pub node_graphs: usize,
    pub sound_moves: usize,
    pub expression_moves: usize,
    /// Moves whose effect calls changed (the effects a move spawns).
    pub effect_call_moves: usize,
    /// Effects whose look was changed in the effect file.
    pub authored_effects: usize,
    /// Effects borrowed from another effect file.
    pub borrowed_effects: usize,
    pub gimmicks: usize,
    pub meters: usize,
}

impl CharacterEditCounts {
    /// True when the character owns no edit of any family.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_keys_are_spelled_as_fighter_names() {
        let mario = CharacterKey::base("Mario");
        assert_eq!(mario.as_str(), "mario");
        assert_eq!(mario.engine_fighter(), "mario");
        assert!(!mario.is_made());
        assert_eq!(CharacterKey::parse("mario"), mario);
    }

    #[test]
    fn made_keys_are_spelled_as_roster_keys() {
        let vision = CharacterKey::made(RosterKey::slot("mario", 8));
        assert_eq!(vision.as_str(), "mario#c08");
        assert_eq!(vision.engine_fighter(), "mario");
        assert!(vision.is_made());
        assert_eq!(CharacterKey::parse("mario#c08"), vision);
        assert_eq!(
            CharacterKey::parse("mario#c112").roster_key(),
            &RosterKey::slot("mario", 112)
        );
    }

    #[test]
    fn move_keys_round_trip_for_base_and_made_characters() {
        for (character, move_name) in [
            (CharacterKey::base("mario"), "attack_11"),
            (CharacterKey::made(RosterKey::slot("mario", 8)), "attack_11"),
            (
                CharacterKey::made(RosterKey::slot("pzenigame", 14)),
                "special_n",
            ),
            (CharacterKey::base("popo"), "attack_air_n"),
        ] {
            let key = move_key(&character, move_name);
            let (parsed, parsed_move) = parse_move_key(&key).unwrap();
            assert_eq!(parsed, character);
            assert_eq!(parsed_move, move_name);
        }
        // Spelled exactly as before for base fighters, so older projects need nothing.
        assert_eq!(
            move_key(&CharacterKey::base("mario"), "attack_11"),
            "mario/attack_11"
        );
        assert_eq!(parse_move_key("no_slash"), None);
    }

    fn roster_with_vision() -> crate::mod_project::RosterMod {
        let mut roster = crate::mod_project::RosterMod::default();
        roster.authored.push(crate::mod_project::AuthoredEntry {
            key: RosterKey::slot("mario", 8),
            donor: "mario".into(),
            slot: 8,
            slots: (9..=15).collect(),
            display_name: "Vision".into(),
            name_id: "vision".into(),
            moveset_scaffolded: false,
            files_root: None,
        });
        roster
    }

    #[test]
    fn names_and_costumes_come_from_the_project() {
        let mut roster = roster_with_vision();
        let vision = CharacterKey::parse("mario#c08");
        assert_eq!(vision.display_name(&roster, Some("Mario")), "Vision");
        assert_eq!(vision.made_slots(&roster), (8..=15).collect::<Vec<_>>());
        assert_eq!(
            vision.costume_range(&roster).as_deref(),
            Some("c08\u{2013}c15")
        );
        roster
            .names
            .insert(RosterKey::slot("mario", 8), "Visionne".into());
        assert_eq!(vision.display_name(&roster, Some("Mario")), "Visionne");
        let mario = CharacterKey::base("mario");
        assert_eq!(mario.display_name(&roster, Some("Mario")), "Mario");
        assert!(mario.made_slots(&roster).is_empty());
        assert_eq!(
            base_costumes("mario", &(0..16).collect::<Vec<_>>(), &roster),
            (0..8).collect::<Vec<_>>()
        );
    }

    #[test]
    fn slot_ranges_read_plainly() {
        assert_eq!(slot_range(&[]), None);
        assert_eq!(slot_range(&[8]).as_deref(), Some("c08"));
        assert_eq!(slot_range(&[9, 8, 10]).as_deref(), Some("c08\u{2013}c10"));
        assert_eq!(slot_range(&[8, 10]).as_deref(), Some("c08, c10"));
    }
}
