//! Fighter kind names from `slight_consts` rodata (fighters.rs).

use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FighterKind {
    Mario,
    Luigi,
    Lucina,
    Lucas,
    Lucario,
    LittleMac,
    Kirby,
    KingKRool,
    KingDedede,
    Ken,
    Kazuya,
    Joker,
    Jigglypuff,
    Ivysaur,
    Inkling,
    Incineroar,
    Ike,
    IceClimber,
    Ganondorf,
    GameWatch,
    Fox,
    Falco,
    Element,
    Donkey,
    DoctorMario,
    DiddyKong,
    DarkSamus,
    Daisy,
    Corrin,
    Cloud,
    Chrom,
    Charizard,
    CaptainFalcon,
    Byleth,
    BowserJr,
    BowserGiga,
    Bowser,
    Bayonetta,
    Banjo,
    ZeroSuitSamus,
    Zelda,
    YoungLink,
    Yoshi,
    WiiFitTrainer,
    Wario,
    Terry,
    Steve,
    Sonic,
    Snake,
    Simon,
    Shulk,
    Sheik,
    Samus,
    Ryu,
    Roy,
    Robot,
    Robin,
    Ridley,
    Richter,
    PokemonTrainer,
    Pit,
    PiranhaPlant,
    Pikachu,
    Pichu,
    Peach,
    Pacman,
    Olimar,
    Mythra,
    MinMin,
    MiiSwordsman,
    MiiGunner,
    MiiFighter,
    MiiEnemyS,
    MiiEnemyG,
    MiiEnemyF,
    Mewtwo,
    MetaKnight,
    MegaMan,
    Marth,
    All,
    Unknown,
}

const NAMES: &[(&str, FighterKind)] = &[
    ("ALL", FighterKind::All),
    ("MARIO", FighterKind::Mario),
    ("LUIGI", FighterKind::Luigi),
    ("LUCINA", FighterKind::Lucina),
    ("LUCAS", FighterKind::Lucas),
    ("LUCARIO", FighterKind::Lucario),
    ("LITTLEMAC", FighterKind::LittleMac),
    ("KIRBY", FighterKind::Kirby),
    ("KING_K_ROOL", FighterKind::KingKRool),
    ("KING_DEDEDE", FighterKind::KingDedede),
    ("KEN", FighterKind::Ken),
    ("KAZUYA", FighterKind::Kazuya),
    ("JOKER", FighterKind::Joker),
    ("JIGGLYPUFF", FighterKind::Jigglypuff),
    ("IVYSAUR", FighterKind::Ivysaur),
    ("INKLING", FighterKind::Inkling),
    ("INCINEROAR", FighterKind::Incineroar),
    ("IKE", FighterKind::Ike),
    ("ICECLIMBER", FighterKind::IceClimber),
    ("GANONDORF", FighterKind::Ganondorf),
    ("GAMEWATCH", FighterKind::GameWatch),
    ("FOX", FighterKind::Fox),
    ("FALCO", FighterKind::Falco),
    ("ELEMENT", FighterKind::Element),
    ("DONKEY", FighterKind::Donkey),
    ("DOCTOR_MARIO", FighterKind::DoctorMario),
    ("DIDDY_KONG", FighterKind::DiddyKong),
    ("DARK_SAMUS", FighterKind::DarkSamus),
    ("DAISY", FighterKind::Daisy),
    ("CORRIN", FighterKind::Corrin),
    ("CLOUD", FighterKind::Cloud),
    ("CHROM", FighterKind::Chrom),
    ("CHARIZARD", FighterKind::Charizard),
    ("CAPTAIN_FALCON", FighterKind::CaptainFalcon),
    ("BYLETH", FighterKind::Byleth),
    ("BOWSER_JR", FighterKind::BowserJr),
    ("BOWSER_GIGA", FighterKind::BowserGiga),
    ("BOWSER", FighterKind::Bowser),
    ("BAYONETTA", FighterKind::Bayonetta),
    ("BANJO", FighterKind::Banjo),
    ("ZERO_SUIT_SAMUS", FighterKind::ZeroSuitSamus),
    ("ZELDA", FighterKind::Zelda),
    ("YOUNGLINK", FighterKind::YoungLink),
    ("YOSHI", FighterKind::Yoshi),
    ("WIIFIT_TRAINER", FighterKind::WiiFitTrainer),
    ("WARIO", FighterKind::Wario),
    ("TERRY", FighterKind::Terry),
    ("STEVE", FighterKind::Steve),
    ("SONIC", FighterKind::Sonic),
    ("SNAKE", FighterKind::Snake),
    ("SIMON", FighterKind::Simon),
    ("SHULK", FighterKind::Shulk),
    ("SHEIK", FighterKind::Sheik),
    ("SAMUS", FighterKind::Samus),
    ("RYU", FighterKind::Ryu),
    ("ROY", FighterKind::Roy),
    ("ROBOT", FighterKind::Robot),
    ("ROBIN", FighterKind::Robin),
    ("RIDLEY", FighterKind::Ridley),
    ("RICHTER", FighterKind::Richter),
    ("POKEMON_TRAINER", FighterKind::PokemonTrainer),
    ("PIT", FighterKind::Pit),
    ("PIRANHA_PLANT", FighterKind::PiranhaPlant),
    ("PIKACHU", FighterKind::Pikachu),
    ("PICHU", FighterKind::Pichu),
    ("PEACH", FighterKind::Peach),
    ("PACMAN", FighterKind::Pacman),
    ("OLIMAR", FighterKind::Olimar),
    ("MYTHRA", FighterKind::Mythra),
    ("MINMIN", FighterKind::MinMin),
    ("MIISWORDSMAN", FighterKind::MiiSwordsman),
    ("MIIGUNNER", FighterKind::MiiGunner),
    ("MIIFIGHTER", FighterKind::MiiFighter),
    ("MIIENEMYS", FighterKind::MiiEnemyS),
    ("MIIENEMYG", FighterKind::MiiEnemyG),
    ("MIIENEMYF", FighterKind::MiiEnemyF),
    ("MEWTWO", FighterKind::Mewtwo),
    ("METAKNIGHT", FighterKind::MetaKnight),
    ("MEGAMAN", FighterKind::MegaMan),
    ("MARTH", FighterKind::Marth),
];

impl FromStr for FighterKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key = s.trim().to_ascii_uppercase().replace(' ', "_");
        for (name, kind) in NAMES {
            if *name == key {
                return Ok(*kind);
            }
        }
        Err(())
    }
}

pub fn parse_fighter(s: &str) -> Option<FighterKind> {
    s.parse().ok()
}

pub fn name(kind: FighterKind) -> &'static str {
    for (n, k) in NAMES {
        if *k == kind {
            return n;
        }
    }
    "UNKNOWN"
}

/// Number of fighter-kind entries in the game's lowercase-name table.
const FIGHTER_NAME_COUNT: usize = 118;

/// Number of object kinds in the game's lowercase weapon-name tables.
const WEAPON_COUNT: usize = 0x267;

/// Entry `index` of one of the game's name tables, an array of C-string pointers.
fn game_name(table: &crate::offsets::GameOffset, index: i32, count: usize) -> Option<&'static str> {
    let index = usize::try_from(index).ok().filter(|index| *index < count)?;
    unsafe {
        let text = skyline::hooks::getRegionAddress(skyline::hooks::Region::Text) as usize;
        if text == 0 {
            return None;
        }
        let ptr = *((text + table.get() + 8 * index) as *const *const u8);
        if ptr.is_null() {
            return None;
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        Some(std::str::from_utf8_unchecked(std::slice::from_raw_parts(
            ptr, len,
        )))
    }
}

/// Resolve a game fighter-kind id (`utility::get_kind`) to its lowercase name (e.g. "mario",
/// "donkey") by reading the game's own name table — the authoritative, version-correct source.
pub fn game_kind_name(kind: i32) -> Option<&'static str> {
    game_name(&crate::offsets::FIGHTER_NAMES, kind, FIGHTER_NAME_COUNT)
}

/// The fighter that owns an object kind and the object's name, from the game's own tables: the
/// names smashline builds its agents from ("mario", "hugeflame").
#[cfg(not(feature = "mod_plugin"))]
fn weapon_names(kind: i32) -> Option<(&'static str, &'static str)> {
    Some((
        game_name(&crate::offsets::WEAPON_OWNER_NAMES, kind, WEAPON_COUNT)?,
        game_name(&crate::offsets::WEAPON_NAMES, kind, WEAPON_COUNT)?,
    ))
}

/// Every object's agent name by kind, read once. Empty when the tables are not where this build
/// expects them, which the pinned kind of Mario's fireball tells: no object is named then, and
/// the editor is told its objects cannot be edited live.
///
/// Nana makes objects of Popo's kinds and runs her own scripts for them, so a kind of Popo's has
/// a second name.
#[cfg(not(feature = "mod_plugin"))]
static WEAPON_AGENTS: std::sync::LazyLock<Vec<Option<(String, Option<String>)>>> =
    std::sync::LazyLock::new(|| {
        let fireball = *smash::lib::lua_const::WEAPON_KIND_MARIO_FIREBALL;
        if weapon_names(fireball) != Some(("mario", "fireball")) {
            crate::slight::diag::note("object names: the game's name tables did not match");
            return Vec::new();
        }
        let agents: Vec<_> = (0..WEAPON_COUNT as i32)
            .map(|kind| {
                weapon_names(kind).map(|(owner, name)| {
                    let nana = (owner == "popo").then(|| format!("nana_{name}"));
                    (format!("{owner}_{name}"), nana)
                })
            })
            .collect();
        crate::slight::diag::note(format!(
            "object names: {} read from the game",
            agents.iter().flatten().count()
        ));
        agents
    });

/// The agent name ("mario_hugeflame") of an object kind made by a fighter of `maker`.
#[cfg(not(feature = "mod_plugin"))]
pub fn weapon_agent(kind: i32, maker: i32) -> Option<&'static str> {
    let (agent, nana) = WEAPON_AGENTS.get(usize::try_from(kind).ok()?)?.as_ref()?;
    match nana {
        Some(nana) if maker == *smash::lib::lua_const::FIGHTER_KIND_NANA => Some(nana),
        _ => Some(agent),
    }
}

/// The kind of the object an agent name belongs to, and the fighter that has to have made it
/// where the kind alone does not say.
#[cfg(not(feature = "mod_plugin"))]
pub fn weapon_object(agent: &str) -> Option<(i32, Option<i32>)> {
    WEAPON_AGENTS.iter().enumerate().find_map(|(kind, names)| {
        let (own, nana) = names.as_ref()?;
        if own == agent {
            return Some((kind as i32, None));
        }
        (nana.as_deref() == Some(agent))
            .then(|| (kind as i32, Some(*smash::lib::lua_const::FIGHTER_KIND_NANA)))
    })
}
