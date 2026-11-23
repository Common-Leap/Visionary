//! Read-only fighter capability catalog for the gimmicks editor.
//!
//! Mechanics are task-board concepts, not claims that a mechanic is currently
//! implemented or that a native runtime observation exists. Slot counts come
//! only from the pinned identifier registry.

use super::codegen::ResourceSource;
use super::registry::{self, FighterKindEntry, IdentifierKind};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FighterProfile {
    pub fighter: &'static str,
    pub title: &'static str,
    pub mechanics: &'static [&'static str],
    pub work_floats: usize,
    pub work_ints: usize,
    pub work_flags: usize,
    pub articles: usize,
    pub weapon_statuses: usize,
    pub fighter_statuses: usize,
    pub native_sources: Vec<ResourceSource>,
}

/// Return the pinned fighter profile for an exact internal engine name.
/// Unknown names are rejected; aliases and case folding are not inferred.
pub fn profile(fighter: &str) -> Option<FighterProfile> {
    let kind = registry::FIGHTER_KINDS
        .iter()
        .find(|entry| entry.fighter == fighter)?;
    Some(make_profile(kind))
}

fn make_profile(kind: &FighterKindEntry) -> FighterProfile {
    let fighter = kind.fighter;
    FighterProfile {
        fighter,
        title: title(fighter),
        mechanics: mechanics(fighter),
        work_floats: count(fighter, IdentifierKind::WorkFloat),
        work_ints: count(fighter, IdentifierKind::WorkInt),
        work_flags: count(fighter, IdentifierKind::WorkFlag),
        articles: count(fighter, IdentifierKind::Article),
        weapon_statuses: count(fighter, IdentifierKind::WeaponStatus),
        fighter_statuses: count(fighter, IdentifierKind::FighterStatus),
        native_sources: if fighter == "cloud" {
            vec![
                ResourceSource::CloudLimitGroundedNeutral,
                ResourceSource::CloudLimitIdleExpiry,
            ]
        } else {
            Vec::new()
        },
    }
}

fn count(fighter: &str, kind: IdentifierKind) -> usize {
    registry::ENTRIES
        .iter()
        .filter(|entry| entry.kind == kind && registry::owned_by(entry, fighter))
        .count()
}

fn title(fighter: &'static str) -> &'static str {
    match fighter {
        "bayonetta" => "Bayonetta",
        "brave" => "Hero",
        "buddy" => "Banjo & Kazooie",
        "captain" => "Captain Falcon",
        "chrom" => "Chrom",
        "cloud" => "Cloud",
        "daisy" => "Daisy",
        "dedede" => "King Dedede",
        "demon" => "Kazuya",
        "diddy" => "Diddy Kong",
        "dolly" => "Terry",
        "donkey" => "Donkey Kong",
        "duckhunt" => "Duck Hunt",
        "edge" => "Sephiroth",
        "eflame" => "Pyra",
        "element" => "Pyra / Mythra",
        "elight" => "Mythra",
        "falco" => "Falco",
        "fox" => "Fox",
        "gamewatch" => "Mr. Game & Watch",
        "ganon" => "Ganondorf",
        "gaogaen" => "Incineroar",
        "gekkouga" => "Greninja",
        "ike" => "Ike",
        "inkling" => "Inkling",
        "jack" => "Joker",
        "kamui" => "Corrin",
        "ken" => "Ken",
        "kirby" => "Kirby",
        "koopa" => "Bowser",
        "koopag" => "Giga Bowser",
        "koopajr" => "Bowser Jr.",
        "krool" => "King K. Rool",
        "link" => "Link",
        "littlemac" => "Little Mac",
        "lucario" => "Lucario",
        "lucas" => "Lucas",
        "lucina" => "Lucina",
        "luigi" => "Luigi",
        "mario" => "Mario",
        "mariod" => "Dr. Mario",
        "marth" => "Marth",
        "master" => "Byleth",
        "metaknight" => "Meta Knight",
        "mewtwo" => "Mewtwo",
        "miienemyf" => "Mii Enemy (Brawler)",
        "miienemyg" => "Mii Enemy (Gunner)",
        "miienemys" => "Mii Enemy (Swordfighter)",
        "miifighter" => "Mii Brawler",
        "miigunner" => "Mii Gunner",
        "miiswordsman" => "Mii Swordfighter",
        "murabito" => "Villager",
        "ness" => "Ness",
        "packun" => "Piranha Plant",
        "pacman" => "Pac-Man",
        "palutena" => "Palutena",
        "peach" => "Peach",
        "pfushigisou" => "Ivysaur",
        "pichu" => "Pichu",
        "pickel" => "Steve",
        "pikachu" => "Pikachu",
        "pikmin" => "Olimar",
        "pit" => "Pit",
        "pitb" => "Dark Pit",
        "plizardon" => "Charizard",
        "popo" => "Ice Climbers",
        "ptrainer" => "Pokémon Trainer",
        "purin" => "Jigglypuff",
        "pzenigame" => "Squirtle",
        "reflet" => "Robin",
        "richter" => "Richter",
        "ridley" => "Ridley",
        "robot" => "R.O.B.",
        "rockman" => "Mega Man",
        "rosetta" => "Rosalina & Luma",
        "roy" => "Roy",
        "ryu" => "Ryu",
        "samus" => "Samus",
        "samusd" => "Dark Samus",
        "sheik" => "Sheik",
        "shizue" => "Isabelle",
        "shulk" => "Shulk",
        "simon" => "Simon",
        "snake" => "Snake",
        "sonic" => "Sonic",
        "szerosuit" => "Zero Suit Samus",
        "tantan" => "Min Min",
        "toonlink" => "Toon Link",
        "trail" => "Sora",
        "wario" => "Wario",
        "wiifit" => "Wii Fit Trainer",
        "wolf" => "Wolf",
        "yoshi" => "Yoshi",
        "younglink" => "Young Link",
        "zelda" => "Zelda",
        // This is only reachable for a future pinned kind without a display name.
        other => other,
    }
}

fn mechanics(fighter: &str) -> &'static [&'static str] {
    match fighter {
        "buddy" => &["Wonderwing feathers", "Egg inventory", "Blaster decay"],
        "bayonetta" => &["Witch Time", "Bat Within", "Bullet Arts"],
        "koopa" => &["Tough Guy", "Fire Breath"],
        "koopajr" => &["Clown Car", "Cannon and Mecha Koopa"],
        "master" => &["Failnaught charge", "Areadbhar tipper", "Aymr armor"],
        "cloud" => &["Limit gauge"],
        "kamui" => &["Fang Shot charge", "Lunge pin"],
        "donkey" => &["Giant Punch charge", "Cargo"],
        "diddy" => &["Banana", "Peanut overcharge", "Rocketbarrel detach"],
        "mariod" => &[],
        "duckhunt" => &["Can", "Clay Pigeon", "Gunmen"],
        "gekkouga" => &["Shuriken charge", "Sneak", "Substitute"],
        "brave" => &["MP", "Command menu", "Critical hits"],
        "popo" => &["Nana partner"],
        "ike" => &["Eruption pillars", "Quick Draw hold"],
        "gaogaen" => &["Revenge stacks"],
        "inkling" => &["Ink tank", "Inked state"],
        "shizue" => &["Lloid Trap", "Fishing Rod", "Pocket"],
        "purin" => &["Rollout charge", "Rest", "Shield-break fall"],
        "jack" => &["Rebellion gauge", "Arsène"],
        "demon" => &["Rage", "Tough Body", "Crouch Dash and EWGF"],
        "ryu" | "ken" => &["Strength states", "Command inputs", "Focus"],
        "dedede" => &["Gordo", "Jet Hammer charge", "Inhale"],
        "krool" => &["Belly armor", "Crown", "Blunderbuss"],
        "kirby" => &["Copy Ability"],
        "link" => &["Remote bomb", "Shield", "Bow"],
        "toonlink" => &["Bomb", "Shield", "Bow"],
        "younglink" => &["Bomb", "Shield", "Fire Bow"],
        "littlemac" => &["Power Meter", "KO Punch"],
        "lucario" => &["Aura"],
        "lucas" => &["Freeze", "Thunder pierce", "Magnet"],
        "luigi" => &["Missile misfire", "Cyclone mash"],
        "mario" => &["F.L.U.D.D. tank", "Cape"],
        "rockman" => &["Blade", "Bomber", "Leaf Shield"],
        "mewtwo" => &["Shadow Ball storage", "Disable", "Confusion"],
        "miifighter" | "miiswordsman" | "miigunner" => {
            &["Custom move selection", "Class-specific attributes"]
        }
        "tantan" => &["ARM cycle", "Dragon arm buff"],
        "ness" => &["Magnet", "Flash", "Thunder 2", "Yo-yo"],
        "pikmin" => &["Pikmin queue", "Maturity", "Elements", "Pikmin HP"],
        "pacman" => &["Fruit cycle", "Hydrant", "Trampoline"],
        "peach" | "daisy" => &["Turnip RNG", "Float", "Toad"],
        "pikachu" => &["Skull Bash storage", "Thunder"],
        "pichu" => &["Skull Bash storage", "Recoil", "Thunder"],
        "ptrainer" | "pfushigisou" | "plizardon" | "pzenigame" => {
            &["Trainer switch cooldown", "Three-Pokémon team"]
        }
        "packun" => &["Ptooie hold", "Poison charge", "Long-Stem armor"],
        "eflame" => &["Pyra / Mythra swap", "Sword loss"],
        "elight" => &["Pyra / Mythra swap", "Foresight"],
        "robot" => &["Beam", "Gyro", "Fuel"],
        "rosetta" => &["Luma HP", "Luma respawn", "Detether"],
        "samus" | "samusd" => &["Charge storage", "Missile selection"],
        "edge" => &["Wing", "Flare stages"],
        "sheik" => &["Needle storage"],
        "shulk" => &["Arts dial", "Arts cooldowns"],
        "snake" => &["Grenade", "C4", "Nikita inventory"],
        "sonic" => &["Spin levels", "Homing Attack", "Spring"],
        "trail" => &["Magic cycle"],
        "pickel" => &[
            "Mining",
            "Durability and tiers",
            "Crafting",
            "Blocks and Redstone",
        ],
        "dolly" => &["GO meter", "Super specials", "Auto-facing"],
        "murabito" => &["Pocket", "Timber stages", "Lloid"],
        "wario" => &["Waft", "Bike", "Chomp"],
        "wiifit" => &["Sun storage", "Deep Breathing buff"],
        "yoshi" => &["Egg Lay", "Egg throw", "Double-jump armor"],
        "zelda" => &["Phantom stages"],
        "szerosuit" => &["Paralyzer charge", "Flip Jump"],
        "ridley" => &["Plasma storage"],
        // The task board records these families as having no persistent
        // resource mechanic; ACMD and shared parameters already cover them.
        "captain" | "chrom" | "falco" | "fox" | "ganon" | "lucina" | "marth" | "metaknight"
        | "palutena" | "pit" | "pitb" | "richter" | "roy" | "simon" | "wolf" => &[],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles() -> Vec<FighterProfile> {
        let mut result: Vec<_> = registry::FIGHTER_KINDS.iter().map(make_profile).collect();
        result.sort_by_key(|entry| entry.fighter);
        result
    }

    #[test]
    fn catalog_covers_every_pinned_fighter_and_is_sorted_deterministically() {
        let first = profiles();
        let second = profiles();
        assert_eq!(first, second);
        assert_eq!(first.len(), registry::FIGHTER_KINDS.len());
        assert!(first
            .windows(2)
            .all(|window| window[0].fighter < window[1].fighter));
        for kind in registry::FIGHTER_KINDS {
            assert!(profile(kind.fighter).is_some(), "missing {}", kind.fighter);
        }
    }

    #[test]
    fn counts_match_exact_registry_categories_for_every_fighter() {
        for profile in profiles() {
            let registry_count = |kind| {
                registry::ENTRIES
                    .iter()
                    .filter(|entry| {
                        entry.kind == kind && registry::owned_by(entry, profile.fighter)
                    })
                    .count()
            };
            assert_eq!(
                profile.work_floats,
                registry_count(IdentifierKind::WorkFloat)
            );
            assert_eq!(profile.work_ints, registry_count(IdentifierKind::WorkInt));
            assert_eq!(profile.work_flags, registry_count(IdentifierKind::WorkFlag));
            assert_eq!(profile.articles, registry_count(IdentifierKind::Article));
            assert_eq!(
                profile.weapon_statuses,
                registry_count(IdentifierKind::WeaponStatus)
            );
            assert_eq!(
                profile.fighter_statuses,
                registry_count(IdentifierKind::FighterStatus)
            );
        }
    }

    #[test]
    fn native_sources_are_cloud_only_and_echo_families_are_present() {
        let cloud = profile("cloud").unwrap();
        assert_eq!(cloud.native_sources.len(), 2);
        assert_eq!(
            cloud.native_sources,
            vec![
                ResourceSource::CloudLimitGroundedNeutral,
                ResourceSource::CloudLimitIdleExpiry
            ]
        );
        assert!(profiles()
            .iter()
            .filter(|entry| entry.fighter != "cloud")
            .all(|entry| entry.native_sources.is_empty()));

        assert_eq!(
            profile("buddy").unwrap().mechanics[0],
            "Wonderwing feathers"
        );
        assert_eq!(profile("reflet").unwrap().title, "Robin");
        assert_eq!(profile("koopa").unwrap().mechanics[0], "Tough Guy");
        assert_eq!(
            profile("donkey").unwrap().mechanics[0],
            "Giant Punch charge"
        );
        assert_eq!(profile("gekkouga").unwrap().mechanics[0], "Shuriken charge");
        assert!(profile("pitb").unwrap().mechanics.is_empty());
        assert_eq!(profile("ridley").unwrap().mechanics, &["Plasma storage"]);
        assert!(profile("mariod").unwrap().mechanics.is_empty());
        assert!(profile("koopag").unwrap().mechanics.is_empty());
        assert!(profile("element").unwrap().mechanics.is_empty());
    }

    #[test]
    fn rejects_unknown_or_non_exact_names() {
        assert!(profile("unknown fighter").is_none());
        assert!(profile("Cloud").is_none());
        assert!(profile("robin").is_none());
    }

    #[test]
    fn variant_specific_concepts_do_not_leak_between_family_members() {
        assert!(!profile("pikachu").unwrap().mechanics.contains(&"Recoil"));
        assert!(profile("pichu").unwrap().mechanics.contains(&"Recoil"));
        assert!(profile("link").unwrap().mechanics.contains(&"Remote bomb"));
        assert!(!profile("toonlink")
            .unwrap()
            .mechanics
            .contains(&"Remote bomb"));
        assert!(profile("eflame").unwrap().mechanics.contains(&"Sword loss"));
        assert!(!profile("elight").unwrap().mechanics.contains(&"Sword loss"));
    }
}
