//! Named fighter-owned native resources and persistent special-move state.
//!
//! Fighter keys are exact registry owners, not display names or echo aliases.
//! Labels identify useful fields; symbols and types come from the pinned registry.
//! A label does not establish native write side effects, lifecycle events, or
//! HUD ownership. Bounds describe evidenced native units, never display scaling.

use super::native_inputs::NativeKind;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NativeGimmick {
    pub fighter: &'static str,
    pub label: &'static str,
    pub symbol: &'static str,
    pub kind: NativeKind,
    /// Inclusive native bounds, when known. Enum encodings and unverified
    /// counter capacities deliberately have no range.
    pub range: Option<(f64, f64)>,
    pub note: &'static str,
}

/// Named native values for an exact internal fighter name, in display order.
/// Unknown names and owners without evidenced fields yield no entries.
pub fn for_fighter(fighter: &str) -> impl Iterator<Item = &'static NativeGimmick> + '_ {
    GIMMICKS
        .iter()
        .filter(move |entry| entry.fighter == fighter)
}

/// Marks a value listed straight from the game's own names, with no hand-written label.
pub const LISTED_FROM_GAME: &str = "Listed from the game's own names.";

/// Every value of this fighter a block can use: the named ones first, then the rest of what the
/// game keeps per fighter, such as Steve's materials or Villager's pocket.
pub fn all_for_fighter(fighter: &str) -> &'static [NativeGimmick] {
    use super::registry::{entries_for, IdentifierKind as Id};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static LISTS: OnceLock<Mutex<HashMap<String, &'static [NativeGimmick]>>> = OnceLock::new();
    let mut lists = LISTS.get_or_init(Default::default).lock().unwrap();
    if let Some(list) = lists.get(fighter) {
        return list;
    }
    let mut list: Vec<NativeGimmick> = for_fighter(fighter).copied().collect();
    for (id, kind) in [
        (Id::WorkInt, NativeKind::Int),
        (Id::WorkFloat, NativeKind::Float),
        (Id::WorkFlag, NativeKind::Flag),
    ] {
        for entry in entries_for(fighter, id) {
            let Some((_, name)) = entry.symbol.split_once("_INSTANCE_WORK_ID_") else {
                continue;
            };
            let internal = ["HANDLE", "OBJECT_ID", "HASH", "EXTEND_BUFFER", "_TASK_"];
            if !entry.group.is_empty()
                || internal.iter().any(|word| name.contains(word))
                || list.iter().any(|known| known.symbol == entry.symbol)
                || super::native_inputs::NativeInput::from_registry(fighter, entry.symbol, kind)
                    .is_err()
            {
                continue;
            }
            let words = name
                .trim_start_matches("INT_")
                .trim_start_matches("FLOAT_")
                .trim_start_matches("FLAG_")
                .replace('_', " ")
                .to_lowercase();
            let mut letters = words.chars();
            let label: String = letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            });
            list.push(NativeGimmick {
                fighter: entry.fighter,
                label: Box::leak(label.into_boxed_str()),
                symbol: entry.symbol,
                kind,
                range: None,
                note: LISTED_FROM_GAME,
            });
        }
    }
    let list: &'static [NativeGimmick] = Box::leak(list.into_boxed_slice());
    lists.insert(fighter.to_owned(), list);
    list
}

// Grouping by owner keeps the data compact without deriving symbol spellings.
macro_rules! gimmicks {
    ($($fighter:literal => [$(($label:literal, $symbol:literal, $kind:ident, $range:expr, $note:literal)),* $(,)?]),* $(,)?) => {
        static GIMMICKS: &[NativeGimmick] = &[$($(
            NativeGimmick {
                fighter: $fighter,
                label: $label,
                symbol: $symbol,
                kind: NativeKind::$kind,
                range: $range,
                note: $note,
            },
        )*)*];
    };
}

// Ranges: Cloud's native helper clamps to 0–100 (native-resource evidence,
// 2026-10-18); Sheik's counter was sampled at 0–6 (charge-counter evidence,
// 2026-10-09). The task board records Hero's 100 MP, Inkling's 150 ink and
// Banjo's five feathers. Other capacities and enum encodings remain unspecified.
gimmicks! {
    "bayonetta" => [
        ("Witch Time frame", "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLOAT_WITCH_TIME_FRAME", Float, None, "Raw Witch Time frame field; repeated-use penalty and duration are separate behavior."),
        ("Witch Time active", "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_WITCH_TIME", Flag, None, "Native Witch Time flag."),
    ],
    "brave" => [
        ("MP", "FIGHTER_BRAVE_INSTANCE_WORK_ID_FLOAT_SP", Float, Some((0.0, 100.0)), "Hero's native MP resource."),
        ("MP capacity", "FIGHTER_BRAVE_INSTANCE_WORK_ID_FLOAT_MAX_SP", Float, None, "Native maximum MP field."),
        ("Attack buff frame", "FIGHTER_BRAVE_INSTANCE_WORK_ID_INT_SPECIAL_LW_ATTACK_UP_FRAME", Int, None, "Raw Command Selection attack-buff frame field."),
        ("Speed buff frame", "FIGHTER_BRAVE_INSTANCE_WORK_ID_INT_SPECIAL_LW_SPEED_UP_FRAME", Int, None, "Raw Command Selection speed-buff frame field."),
        ("Bounce frame", "FIGHTER_BRAVE_INSTANCE_WORK_ID_INT_SPECIAL_LW_REFLECT_FRAME", Int, None, "Raw Command Selection reflection-buff frame field."),
    ],
    "buddy" => [
        ("Wonderwing feathers", "FIGHTER_BUDDY_INSTANCE_WORK_ID_INT_SPECIAL_S_REMAIN", Int, Some((0.0, 5.0)), "Remaining Wonderwing uses; the native stock allotment is five."),
        ("Breegull Blaster level", "FIGHTER_BUDDY_INSTANCE_WORK_ID_INT_SPECIAL_N_BAKYUN_BULLET_LEVEL", Int, None, "Native bullet level used by Blaster decay; level encoding is unspecified."),
        ("Breegull Blaster recovery count", "FIGHTER_BUDDY_INSTANCE_WORK_ID_INT_SPECIAL_N_BAKYUN_BULLET_RECOVERY_COUNT", Int, None, "Raw Blaster recovery counter."),
    ],
    "cloud" => [
        ("Limit gauge", "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE", Float, Some((0.0, 100.0)), "Native Limit resource; the native adjustment helper clamps to this range."),
        ("Limit Break active", "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK", Flag, None, "Native full-Limit state; a transition alone does not identify a spend."),
        ("Limit clear frame", "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_BREAK_CLEAR_FRAME", Int, None, "Raw native clear-frame counter; not an authored timer."),
    ],
    "dedede" => [
        ("Jet Hammer charge", "FIGHTER_DEDEDE_INSTANCE_WORK_ID_INT_SPECIAL_LW_CHARGE", Int, None, "Raw Jet Hammer charge field; native units are unspecified."),
    ],
    "demon" => [
        ("Rage enabled", "FIGHTER_DEMON_INSTANCE_WORK_ID_FLAG_ENABLE_RAGE_SYSTEM", Flag, None, "Kazuya's native Rage-system enable flag."),
        ("Rage already enabled", "FIGHTER_DEMON_INSTANCE_WORK_ID_FLAG_ENABLE_RAGE_SYSTEM_DONE", Flag, None, "Native Rage-system activation-history flag."),
        ("Rage count", "FIGHTER_DEMON_INSTANCE_WORK_ID_INT_RAGE_SYSTEM_COUNT", Int, None, "Raw Rage-system counter; not a damage percentage."),
    ],
    "diddy" => [
        ("Banana pull interval", "FIGHTER_DIDDY_INSTANCE_WORK_ID_INT_SPECIAL_LW_INTERVAL_FRAME", Int, None, "Raw down-special interval frame field; not banana article presence."),
    ],
    "dolly" => [
        ("GO enabled", "FIGHTER_DOLLY_INSTANCE_WORK_ID_FLAG_ENABLE_SUPER_SPECIAL", Flag, None, "Terry's native super-special availability flag."),
    ],
    "donkey" => [
        ("Giant Punch charge", "FIGHTER_DONKEY_INSTANCE_WORK_ID_INT_SPECIAL_N_COUNT", Int, None, "Stored native punch counter; no conversion to percent or frames is assumed."),
    ],
    "duckhunt" => [
        ("Wild Gunman random table", "FIGHTER_DUCKHUNT_INSTANCE_WORK_ID_INT_SPECIAL_LW_RANDOM_TABLE", Int, None, "Raw gunman selection table field; gunman encoding is unspecified."),
        ("Wild Gunman ready", "FIGHTER_DUCKHUNT_INSTANCE_WORK_ID_FLAG_GUNMAN_OK", Flag, None, "Native gunman readiness flag."),
    ],
    "edge" => [
        ("One-Winged Angel active", "FIGHTER_EDGE_INSTANCE_WORK_ID_FLAG_ONE_WINGED_ACTIVATED", Flag, None, "Sephiroth's native wing activation state."),
        ("Wing activation points", "FIGHTER_EDGE_INSTANCE_WORK_ID_FLOAT_ONE_WINGED_ACTIVATE_POINT", Float, None, "Native activation points; not the fighter's damage percentage."),
        ("Wing deactivation points", "FIGHTER_EDGE_INSTANCE_WORK_ID_FLOAT_ONE_WINGED_DEACTIVATE_POINT", Float, None, "Native deactivation points; no threshold formula is assumed."),
    ],
    "eflame" => [
        ("Sword available", "FIGHTER_EFLAME_INSTANCE_WORK_ID_FLAG_HAS_ESWORD", Flag, None, "Pyra's native sword-possession flag for Blazing End."),
    ],
    "elight" => [
        ("Foresight state", "FIGHTER_ELIGHT_INSTANCE_WORK_ID_INT_FORESIGHT_STATE", Int, None, "Mythra's raw Foresight state; enum encoding is unspecified."),
        ("Foresight reuse", "FIGHTER_ELIGHT_INSTANCE_WORK_ID_INT_FORESIGHT_REUSE", Int, None, "Raw native reuse field; no cooldown units are assumed."),
        ("Foresight disabled", "FIGHTER_ELIGHT_INSTANCE_WORK_ID_FLAG_FORESIGHT_DISABLE", Flag, None, "Native Foresight-disable flag."),
    ],
    "gamewatch" => [
        ("Oil Panic gauge", "FIGHTER_GAMEWATCH_INSTANCE_WORK_ID_FLOAT_SPECIAL_LW_GAUGE", Float, None, "Native bucket gauge; its float units are not assumed to equal filled segments."),
        ("Oil Panic attack power", "FIGHTER_GAMEWATCH_INSTANCE_WORK_ID_FLOAT_SPECIAL_LW_ATTACK", Float, None, "Native stored bucket attack-power field."),
        ("Judge result", "FIGHTER_GAMEWATCH_INSTANCE_WORK_ID_INT_SPECIAL_S_KIND", Int, None, "Raw Judge kind; no mapping to displayed numbers is assumed."),
    ],
    "gaogaen" => [
        ("Revenge active", "FIGHTER_GAOGAEN_INSTANCE_WORK_ID_FLAG_IS_REVENGE", Flag, None, "Incineroar's native Revenge state."),
        ("Revenge multiplier", "FIGHTER_GAOGAEN_INSTANCE_WORK_ID_FLOAT_REVENGE_RATE", Float, None, "Native Revenge rate; bounds and stacking remain unspecified."),
        ("Revenge timer", "FIGHTER_GAOGAEN_INSTANCE_WORK_ID_INT_REVENGE_TIMER", Int, None, "Raw Revenge timer field."),
        ("Revenge damage total", "FIGHTER_GAOGAEN_INSTANCE_WORK_ID_FLOAT_REVENGE_TOTAL_DAMAGE", Float, None, "Native Revenge damage accumulator."),
    ],
    "gekkouga" => [
        ("Water Shuriken charge", "FIGHTER_GEKKOUGA_INSTANCE_WORK_ID_FLOAT_SPECIAL_N_CHARGE_RATE", Float, None, "Native Water Shuriken charge rate; units and storage behavior are unspecified."),
        ("Shadow Sneak hold count", "FIGHTER_GEKKOUGA_INSTANCE_WORK_ID_FLOAT_SPECIAL_S_HOLD_COUNT", Float, None, "Raw Shadow Sneak hold counter in native float units."),
        ("Shadow Sneak disabled", "FIGHTER_GEKKOUGA_INSTANCE_WORK_ID_FLAG_SPECIAL_S_IS_DISABLE", Flag, None, "Native side-special disable flag; reset conditions are unspecified."),
        ("Hydro Pump disabled", "FIGHTER_GEKKOUGA_INSTANCE_WORK_ID_FLAG_SPECIAL_HI_IS_DISABLE", Flag, None, "Native up-special disable flag; reset conditions are unspecified."),
    ],
    "inkling" => [
        ("Ink tank", "FIGHTER_INKLING_INSTANCE_WORK_ID_FLOAT_INK", Float, Some((0.0, 150.0)), "Ink tank level; use Add ink to refill or spend ink. Opponent ink coating is separate."),
        ("Splat Bomb available", "FIGHTER_INKLING_INSTANCE_WORK_ID_FLAG_CAN_SPECIAL_LW_BOMB", Flag, None, "Native bomb-availability flag; not bomb article presence."),
    ],
    "jack" => [
        ("Rebellion gauge", "FIGHTER_JACK_INSTANCE_WORK_ID_FLOAT_REBEL_GAUGE", Float, None, "Joker's native gauge; native scale is not inferred from the HUD."),
        ("Arsène mode", "FIGHTER_JACK_INSTANCE_WORK_ID_FLAG_DOYLE", Flag, None, "Native Doyle mode flag; distinct from article existence."),
        ("Arsène exists", "FIGHTER_JACK_INSTANCE_WORK_ID_FLAG_DOYLE_EXIST", Flag, None, "Native Doyle-existence flag; not an ArticleModule query."),
    ],
    "kamui" => [
        ("Dragon Lunge disabled", "FIGHTER_KAMUI_INSTANCE_WORK_ID_FLAG_SPECIAL_S_DISABLE", Flag, None, "Native Dragon Lunge disable flag."),
        ("Dragon Lunge disabled for jump", "FIGHTER_KAMUI_INSTANCE_WORK_ID_FLAG_SPECIAL_S_DISABLE_FOR_JUMP", Flag, None, "Native Dragon Lunge disable-for-jump flag."),
    ],
    "kirby" => [
        ("Copy ability present", "FIGHTER_KIRBY_INSTANCE_WORK_ID_FLAG_COPY", Flag, None, "Native copy-ability flag."),
        ("Copied fighter kind", "FIGHTER_KIRBY_INSTANCE_WORK_ID_INT_COPY_CHARA", Int, None, "Raw copied fighter kind; not a roster index."),
        ("Copied Inkling ink", "FIGHTER_KIRBY_INSTANCE_WORK_ID_FLOAT_INKLING_SPECIAL_N_INK", Float, None, "Kirby's separate copied ink field; Inkling's capacity is not assumed."),
    ],
    "koopa" => [
        ("Fire Breath speed multiplier", "FIGHTER_KOOPA_INSTANCE_WORK_ID_FLOAT_BREATH_SPEED_MUL", Float, None, "Native breath speed multiplier used by Fire Breath; not a normalized fuel meter."),
    ],
    "koopajr" => [
        ("Clown Kart mode", "FIGHTER_KOOPAJR_INSTANCE_WORK_ID_FLAG_SPECIAL_S_KART", Flag, None, "Native Clown Kart mode flag."),
        ("Clown Kart Dash disabled", "FIGHTER_KOOPAJR_INSTANCE_WORK_ID_FLAG_SPECIAL_S_DISABLE", Flag, None, "Native side-special disable flag; reset conditions are unspecified."),
    ],
    "krool" => [
        ("Belly armor life", "FIGHTER_KROOL_INSTANCE_WORK_ID_FLOAT_WAIST_LIFE", Float, None, "King K. Rool's native waist-armor life; damage splitting is separate behavior."),
        ("Crown dropped", "FIGHTER_KROOL_INSTANCE_WORK_ID_FLAG_DROP_CROWN", Flag, None, "Native crown-drop flag; not crown article presence."),
    ],
    "link" => [
        ("Boomerang state", "FIGHTER_LINK_INSTANCE_WORK_ID_FLAG_BOOMERANG", Flag, None, "Native boomerang state flag; article presence requires a separate query."),
    ],
    "littlemac" => [
        ("KO meter", "FIGHTER_LITTLEMAC_INSTANCE_WORK_ID_FLOAT_KO_GAGE", Float, None, "Native Power Meter; HUD arrows do not establish the native float scale."),
        ("KO meter maximum", "FIGHTER_LITTLEMAC_INSTANCE_WORK_ID_FLOAT_KO_GAGE_MAX_VALUE", Float, None, "Native maximum-gauge field."),
        ("KO meter keep frame", "FIGHTER_LITTLEMAC_INSTANCE_WORK_ID_INT_KO_GAGE_MAX_KEEP_FRAME", Int, None, "Raw full-meter keep-frame field; not KO Punch readiness by itself."),
    ],
    "lucario" => [
        ("Aura power", "FIGHTER_LUCARIO_INSTANCE_WORK_ID_FLOAT_CURR_AURAPOWER", Float, None, "Native current aura power; percent and stock scaling are separate behavior."),
        ("Aura Sphere charge", "FIGHTER_LUCARIO_INSTANCE_WORK_ID_INT_AURABALL_CHARGE_FRAME", Int, None, "Raw stored Aura Sphere charge-frame field."),
    ],
    "luigi" => [
        ("Cyclone buoyancy", "FIGHTER_LUIGI_INSTANCE_WORK_ID_FLAG_SPECIAL_LW_BUOYANCY", Flag, None, "Native Luigi Cyclone buoyancy flag; reset conditions are unspecified."),
    ],
    "mario" => [
        ("F.L.U.D.D. charge", "FIGHTER_MARIO_INSTANCE_WORK_ID_INT_SPECIAL_LW_CHARGE", Int, None, "Native integer charge field; not the number of water globs."),
    ],
    "master" => [
        ("Failnaught hop used", "FIGHTER_MASTER_INSTANCE_WORK_ID_FLAG_SPECIAL_N_HOP_DONE", Flag, None, "Native neutral-special hop-done flag; reset conditions are unspecified."),
    ],
    "mewtwo" => [
        ("Shadow Ball charge", "FIGHTER_MEWTWO_INSTANCE_WORK_ID_INT_SHADOWBALL_CHARGE_FRAME", Int, None, "Raw stored Shadow Ball charge-frame field."),
        ("Shadow Ball held", "FIGHTER_MEWTWO_INSTANCE_WORK_ID_FLAG_SHADOWBALL_HAD", Flag, None, "Native held-ball flag."),
    ],
    "miifighter" => [
        ("Burning Dropkick charge", "FIGHTER_MIIFIGHTER_INSTANCE_WORK_ID_FLOAT_NENSYO_KICK_CHARGE_RATE", Float, None, "Native Burning Dropkick charge-rate field; units and storage behavior are unspecified."),
    ],
    "miigunner" => [
        ("Charge Blast charge", "FIGHTER_MIIGUNNER_INSTANCE_WORK_ID_INT_GUNNER_CHARGE_COUNT", Int, None, "Native charge counter for the selected Charge Blast special."),
    ],
    "miiswordsman" => [
        ("Gale Stab charge", "FIGHTER_MIISWORDSMAN_INSTANCE_WORK_ID_FLOAT_HENSOKU_SLASH_CHARGE_RATE", Float, None, "Native Gale Stab charge-rate field; units and storage behavior are unspecified."),
    ],
    "murabito" => [
        ("Pocket object kind", "FIGHTER_MURABITO_INSTANCE_WORK_ID_INT_SPECIAL_N_OBJECT_KIND", Int, None, "Raw pocketed-object kind; validity and category require separate checks."),
        ("Pocket object category", "FIGHTER_MURABITO_INSTANCE_WORK_ID_INT_SPECIAL_N_OBJECT_CATEGORY", Int, None, "Raw pocketed-object category; no enum decoding is assumed."),
        ("Balloon count", "FIGHTER_MURABITO_INSTANCE_WORK_ID_INT_SPECIAL_HI_BALLOON_NUM", Int, None, "Native Balloon Trip count."),
    ],
    "pacman" => [
        ("Bonus Fruit rank", "FIGHTER_PACMAN_INSTANCE_WORK_ID_INT_SPECIAL_N_CHARGE_RANK", Int, None, "Native charge rank; fruit-kind encoding is unspecified."),
        ("Bonus Fruit charge", "FIGHTER_PACMAN_INSTANCE_WORK_ID_FLOAT_SPECIAL_N_CHARGE_RATE", Float, None, "Raw native charge rate; no percentage conversion is assumed."),
    ],
    "packun" => [
        ("Poison Breath charge", "FIGHTER_PACKUN_INSTANCE_WORK_ID_INT_SPECIAL_S_COUNT", Int, None, "Piranha Plant's native side-special counter; native capacity is unspecified."),
    ],
    "peach" => [
        ("Float active", "FIGHTER_PEACH_INSTANCE_WORK_ID_FLAG_UNIQ_FLOAT", Flag, None, "Peach's native float state; not a remaining-duration field."),
    ],
    "pickel" => [
        ("Dirt", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_GRADE_1", Int, None, "Native grade-one material count; appearance depends on the stage."),
        ("Wood", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_WOOD", Int, None, "Native wood inventory count."),
        ("Stone", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_STONE", Int, None, "Native stone inventory count."),
        ("Iron", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_IRON", Int, None, "Native iron inventory count."),
        ("Gold", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_GOLD", Int, None, "Native gold inventory count."),
        ("Diamond", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_DIAMOND", Int, None, "Native diamond inventory count."),
        ("Redstone", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_MATERIAL_NUM_RED_STONE", Int, None, "Native redstone inventory count."),
        ("Attack durability", "FIGHTER_PICKEL_INSTANCE_WORK_ID_FLOAT_ATTACK_DURABILITY", Float, None, "Native attack-durability field; not an inventory of every tool's remaining uses."),
        ("Held tool kind", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_HAVE_CRAFT_WEAPON_KIND", Int, None, "Native held crafted-tool kind; enum encoding is unspecified."),
        ("Held tool material", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_HAVE_CRAFT_WEAPON_MATERIAL_KIND", Int, None, "Native held tool material kind; enum encoding is unspecified."),
        ("Crafting table recreate frame", "FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_TABLE_RECREATE_FRAME", Int, None, "Raw table recreation frame field; not crafting table HP."),
    ],
    "pikmin" => [
        ("Winged Pikmin flight frame", "FIGHTER_PIKMIN_INSTANCE_WORK_ID_FLOAT_SPECIAL_HI_FRAME", Float, None, "Raw up-special frame field in native float units; fatigue scaling is unspecified."),
    ],
    "plizardon" => [
        ("Flamethrower speed multiplier", "FIGHTER_PLIZARDON_INSTANCE_WORK_ID_FLOAT_BREATH_SPEED_MUL", Float, None, "Native Flamethrower breath speed multiplier."),
        ("Flamethrower scale", "FIGHTER_PLIZARDON_INSTANCE_WORK_ID_FLOAT_BREATH_SCALE", Float, None, "Native Flamethrower breath scale field; bounds are unspecified."),
    ],
    "popo" => [
        ("Partner object ID", "FIGHTER_POPO_INSTANCE_WORK_ID_INT_PARTNER_OBJECT_ID", Int, None, "Native partner identifier; not the partner's damage or AI state."),
        ("Nana is leader", "FIGHTER_POPO_INSTANCE_WORK_ID_FLAG_MAIN_FIGHTER_NANA", Flag, None, "Native leader-selection flag; no separate Nana-owner slot is inferred."),
    ],
    "pzenigame" => [
        ("Water Gun charge", "FIGHTER_PZENIGAME_INSTANCE_WORK_ID_INT_SPECIAL_N_CHARGE", Int, None, "Squirtle's native stored charge; not Pokémon Trainer's switch cooldown."),
    ],
    "reflet" => [
        ("Levin Sword equipped", "FIGHTER_REFLET_INSTANCE_WORK_ID_FLAG_THUNDER_SWORD_ON", Flag, None, "Native Thunder Sword selection flag; durability is separate."),
        ("Levin Sword durability", "FIGHTER_REFLET_INSTANCE_WORK_ID_INT_THUNDER_SWORD_CURRENT_POINT", Int, None, "Native sword durability points."),
        ("Thunder tome durability", "FIGHTER_REFLET_INSTANCE_WORK_ID_INT_SPECIAL_N_CURRENT_POINT", Int, None, "Native neutral-special tome points."),
        ("Arcfire tome durability", "FIGHTER_REFLET_INSTANCE_WORK_ID_FLOAT_SPECIAL_S_CURRENT_POINT", Float, None, "Native side-special tome points use a float slot."),
        ("Elwind tome durability", "FIGHTER_REFLET_INSTANCE_WORK_ID_INT_SPECIAL_HI_CURRENT_POINT", Int, None, "Native up-special tome points."),
        ("Nosferatu tome durability", "FIGHTER_REFLET_INSTANCE_WORK_ID_INT_SPECIAL_LW_CURRENT_POINT", Int, None, "Native down-special tome points."),
        ("Thunder charge kind", "FIGHTER_REFLET_INSTANCE_WORK_ID_INT_SPECIAL_N_THUNDER_KIND", Int, None, "Native stored spell stage; enum encoding is unspecified."),
        ("Levin Sword revival count", "FIGHTER_REFLET_INSTANCE_WORK_ID_INT_THUNDER_SWORD_REVIVAL_COUNT", Int, None, "Raw sword revival counter; no timing conversion is assumed."),
    ],
    "ridley" => [
        ("Space Pirate Rush unavailable", "FIGHTER_RIDLEY_INSTANCE_WORK_ID_FLAG_UNABLE_SPECIAL_S", Flag, None, "Native side-special unavailability flag; reset conditions are unspecified."),
        ("Skewer critical disabled", "FIGHTER_RIDLEY_INSTANCE_WORK_ID_FLAG_DISABLE_CRITICAL_SPECIAL_LW", Flag, None, "Native Skewer critical-disable flag."),
        ("Skewer finish disable count", "FIGHTER_RIDLEY_INSTANCE_WORK_ID_INT_DISABLE_SPECIAL_LW_FINISH_COUNT", Int, None, "Raw Skewer finish-disable counter; units and reset conditions are unspecified."),
    ],
    "robot" => [
        ("Robo Beam energy", "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE", Float, None, "Native beam energy; HUD brightness does not establish capacity."),
        ("Robo Burner fuel", "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BURNER_ENERGY_VALUE", Float, None, "Native burner energy; no conversion to flight frames is assumed."),
        ("Gyro charge", "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_GYRO_CHARGE_VALUE", Float, None, "Native stored gyro charge value."),
    ],
    "rockman" => [
        ("Leaf Shield active", "FIGHTER_ROCKMAN_INSTANCE_WORK_ID_FLAG_SPECIAL_LW_LEAFSHIELD", Flag, None, "Mega Man's native Leaf Shield state; not a remaining-leaf count."),
        ("Leaf Shield hold frame", "FIGHTER_ROCKMAN_INSTANCE_WORK_ID_INT_SPECIAL_LW_HOLD_FRAME", Int, None, "Raw native Leaf Shield hold-frame field."),
    ],
    "ryu" => [
        ("Aerial Tatsumaki disabled", "FIGHTER_RYU_INSTANCE_WORK_ID_FLAG_DISABLE_AIR_SPECIAL_S", Flag, None, "Native aerial Tatsumaki disable flag; reset conditions are unspecified."),
    ],
    "samus" => [
        ("Charge Shot charge", "FIGHTER_SAMUS_INSTANCE_WORK_ID_INT_SPECIAL_N_COUNT", Int, None, "Raw stored charge counter; the sampled 112 plateau is not a universal capacity."),
    ],
    "sheik" => [
        ("Stored needles", "FIGHTER_SHEIK_INSTANCE_WORK_ID_INT_NEEDLE_COUNT", Int, Some((0.0, 6.0)), "Native needle count; sampled partial stores, six-charge volleys and zero."),
    ],
    "shizue" => [
        ("Lloid Trap ready", "FIGHTER_SHIZUE_INSTANCE_WORK_ID_FLAG_CLAYROCKET_IS_READY", Flag, None, "Isabelle's native trap-readiness flag; not trap HP or lifetime."),
    ],
    "shulk" => [
        ("Monado Art active", "FIGHTER_SHULK_INSTANCE_WORK_ID_FLAG_SPECIAL_N_ACTIVE", Flag, None, "Native art activation state."),
        ("Monado Art kind", "FIGHTER_SHULK_INSTANCE_WORK_ID_INT_SPECIAL_N_TYPE", Int, None, "Native art kind; enum encoding is unspecified."),
        ("Jump Art cooldown frame", "FIGHTER_SHULK_INSTANCE_WORK_ID_INT_SPECIAL_N_UNAVAILABLE_FRAME_JUMP", Int, None, "Raw Jump Art unavailable-frame field."),
        ("Speed Art cooldown frame", "FIGHTER_SHULK_INSTANCE_WORK_ID_INT_SPECIAL_N_UNAVAILABLE_FRAME_SPEED", Int, None, "Raw Speed Art unavailable-frame field."),
        ("Shield Art cooldown frame", "FIGHTER_SHULK_INSTANCE_WORK_ID_INT_SPECIAL_N_UNAVAILABLE_FRAME_SHIELD", Int, None, "Raw Shield Art unavailable-frame field."),
        ("Buster Art cooldown frame", "FIGHTER_SHULK_INSTANCE_WORK_ID_INT_SPECIAL_N_UNAVAILABLE_FRAME_BUSTER", Int, None, "Raw Buster Art unavailable-frame field."),
        ("Smash Art cooldown frame", "FIGHTER_SHULK_INSTANCE_WORK_ID_INT_SPECIAL_N_UNAVAILABLE_FRAME_SMASH", Int, None, "Raw Smash Art unavailable-frame field."),
    ],
    "sonic" => [
        ("Homing Attack added power", "FIGHTER_SONIC_INSTANCE_WORK_ID_FLOAT_SPECIAL_N_ADD_ATTACK_POWER", Float, None, "Native neutral-special additional attack-power field; bounds are unspecified."),
    ],
    "szerosuit" => [
        ("Flip Jump state", "FIGHTER_SZEROSUIT_INSTANCE_WORK_ID_FLAG_FLIP_JUMP", Flag, None, "Native Flip Jump state flag; reset conditions are unspecified."),
    ],
    "tantan" => [
        ("Right ARM kind", "FIGHTER_TANTAN_INSTANCE_WORK_ID_INT_PUNCH_KIND_R", Int, None, "Native right-arm selection; ARM kind encoding is unspecified."),
        ("Left Dragon powered", "FIGHTER_TANTAN_INSTANCE_WORK_ID_FLAG_DRAGONIZE_L", Flag, None, "Native left-arm Dragonization flag."),
        ("Left ARM enlargement frame", "FIGHTER_TANTAN_INSTANCE_WORK_ID_INT_ARM_L_BIG_FRAME", Int, None, "Raw left-arm enlargement frame field; direction and capacity are unspecified."),
    ],
    "trail" => [
        ("Queued magic kind", "FIGHTER_TRAIL_INSTANCE_WORK_ID_INT_SPECIAL_N_MAGIC_KIND", Int, None, "Sora's native magic kind; spell enum encoding is unspecified."),
    ],
    "wario" => [
        ("Waft charge count", "FIGHTER_WARIO_INSTANCE_WORK_ID_INT_GASS_COUNT", Int, None, "Native gas counter; no conversion to elapsed seconds is assumed."),
        ("Waft level", "FIGHTER_WARIO_INSTANCE_WORK_ID_INT_GASS_LEVEL", Int, None, "Native gas level; level encoding is unspecified."),
        ("Bike HP", "FIGHTER_WARIO_INSTANCE_WORK_ID_FLOAT_SPECIAL_S_BIKE_HP", Float, None, "Native bike HP field; not article presence."),
        ("Bike time", "FIGHTER_WARIO_INSTANCE_WORK_ID_INT_BIKE_TIME", Int, None, "Raw native bike-time field; no cooldown direction is assumed."),
    ],
    "wiifit" => [
        ("Deep Breathing effective frame", "FIGHTER_WIIFIT_INSTANCE_WORK_ID_INT_SPECIAL_LW_WAZA_EFFECTIVE_FRAME", Int, None, "Raw native buff effective-frame field; freshness affects behavior separately."),
        ("Deep Breathing restore frame", "FIGHTER_WIIFIT_INSTANCE_WORK_ID_INT_SPECIAL_LW_WAZA_RESTORE_FRAME", Int, None, "Raw native restore-frame field."),
        ("Sun Salutation charge ratio", "FIGHTER_WIIFIT_INSTANCE_WORK_ID_FLOAT_SPECIAL_N_CHARGE_LEVEL_RATIO", Float, None, "Native charge ratio; its bounds have not been established."),
    ],
    "yoshi" => [
        ("Aerial jump armor", "FIGHTER_YOSHI_INSTANCE_WORK_ID_FLAG_JUMP_AERIAL_ARMOR", Flag, None, "Native aerial-jump armor flag; strength and clearing are separate behavior."),
        ("Aerial damage reaction", "FIGHTER_YOSHI_INSTANCE_WORK_ID_FLOAT_AERIAL_DAMAGE_REACTION", Float, None, "Raw native aerial damage-reaction field; units are unspecified."),
    ],
    "zelda" => [
        ("Phantom building", "FIGHTER_ZELDA_INSTANCE_WORK_ID_FLAG_SPECIAL_LW_BUILD_PHANTOM", Flag, None, "Native Phantom-building flag; not the Phantom's charge stage or HP."),
    ],
}

#[cfg(test)]
impl NativeGimmick {
    /// Build a controller input through the existing exact-owner/type gate.
    pub fn native_input(&self) -> Result<super::native_inputs::NativeInput, String> {
        super::native_inputs::NativeInput::from_registry(self.fighter, self.symbol, self.kind)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::gimmicks::registry::{self, IdentifierKind};

    #[test]
    fn entries_resolve_with_matching_kinds_and_unique_fighter_labels() {
        let mut labels = HashSet::new();
        for entry in GIMMICKS {
            let kind = match entry.kind {
                NativeKind::Float => IdentifierKind::WorkFloat,
                NativeKind::Int => IdentifierKind::WorkInt,
                NativeKind::Flag => IdentifierKind::WorkFlag,
            };
            registry::resolve(entry.fighter, entry.symbol, kind)
                .unwrap_or_else(|error| panic!("{} / {}: {error}", entry.fighter, entry.label));
            assert!(
                labels.insert((entry.fighter, entry.label)),
                "duplicate label {} / {}",
                entry.fighter,
                entry.label
            );
            let input = entry.native_input().unwrap();
            assert_eq!(input.fighter(), Some(entry.fighter));
            assert_eq!(input.symbol(), entry.symbol);
            assert_eq!(input.kind(), Some(entry.kind));
            let wrong_owner = NativeGimmick {
                fighter: if entry.fighter == "cloud" {
                    "mario"
                } else {
                    "cloud"
                },
                ..*entry
            };
            assert!(
                wrong_owner.native_input().is_err(),
                "{} was accepted for wrong owner {}",
                entry.symbol,
                wrong_owner.fighter
            );
            assert!(for_fighter(entry.fighter).any(|candidate| std::ptr::eq(candidate, entry)));
            assert!(for_fighter(entry.fighter).all(|candidate| candidate.fighter == entry.fighter));
        }
        assert!(for_fighter("not_a_fighter").next().is_none());
    }

    #[test]
    fn shared_move_names_do_not_alias_registry_owners() {
        for (owner, other) in [
            ("samus", "samusd"),
            ("peach", "daisy"),
            ("ryu", "ken"),
            ("link", "toonlink"),
            ("link", "younglink"),
            ("gekkouga", "greninja"),
        ] {
            let entries: Vec<_> = for_fighter(owner).collect();
            assert!(!entries.is_empty(), "missing catalog owner {owner}");
            for entry in entries {
                assert!(NativeGimmick {
                    fighter: other,
                    ..*entry
                }
                .native_input()
                .is_err());
            }
            assert!(for_fighter(other).next().is_none());
        }
    }
}
