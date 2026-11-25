//! The joint each fighter object is kept on.
//!
//! The game ties an object a fighter holds or wears to a joint: one of the object's own joints is
//! held at one of the fighter's, place and turn, every frame. Both names are in the object's
//! entry of the program's own table (what 13.0.5's `main` returns from 0x33aa1e0), read here for
//! every object of every fighter. Objects kept at the fighter's origin, or not kept on anything,
//! have no row. Where a projectile is made is the fighter's too ([`Start`]).

/// The object's joint and the fighter's it is held at, in lower case.
pub type Link = (&'static str, &'static str);

/// Whether two names are one object's: a script's `HOOKSHOT_HAND` is the folder `hookshothand`.
fn same_object(a: &str, b: &str) -> bool {
    let letters = |name: &'_ str| name.replace('_', "");
    a == b || letters(a) == letters(b)
}

pub fn link(fighter: &str, object: &str) -> Option<Link> {
    LINKS
        .iter()
        .find(|(owner, name, _)| *owner == fighter && same_object(name, object))
        .map(|(_, _, link)| *link)
}

/// Where a projectile is made: a joint of the fighter's and how far ahead and up of it, by the
/// parameters of the fighter's that the program reads as it makes one (each object's entry in
/// the fighter's list, from 0x64b730, names the code that does). `""` for one it doesn't read.
pub struct Start {
    pub joint: &'static str,
    pub block: &'static str,
    ahead: &'static str,
    up: &'static str,
    /// Its speed, when that is the fighter's too rather than the object's own.
    speed: &'static str,
}

/// [`Start`] with its parameters read: distances ahead and up of the joint, and the speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Started {
    pub joint: &'static str,
    pub ahead: f32,
    pub up: f32,
    pub speed: Option<f32>,
}

pub fn start(fighter: &str, object: &str) -> Option<&'static Start> {
    STARTS
        .iter()
        .find(|(owner, name, _)| *owner == fighter && same_object(name, object))
        .map(|(_, _, start)| start)
}

impl Start {
    /// `None` when the fighter's parameters don't have one it names.
    pub fn read(&self, block: &crate::articles::params::ParamBlock) -> Option<Started> {
        use crate::articles::params::ParamValue;
        let value = |name: &str| {
            let field = |(label, _): &(&String, _)| label.trim_end_matches('_') == name;
            match block.fields.iter().find(field)?.1 .0 {
                ParamValue::Float(value) => Some(value),
                ParamValue::Int(value) => Some(value as f32),
                ParamValue::Other(_) => None,
            }
        };
        let distance = |name: &str| {
            if name.is_empty() {
                Some(0.0)
            } else {
                value(name)
            }
        };
        Some(Started {
            joint: self.joint,
            ahead: distance(self.ahead)?,
            up: distance(self.up)?,
            speed: value(self.speed),
        })
    }
}

#[rustfmt::skip]
const STARTS: &[(&str, &str, Start)] = &[
    ("brave", "deathball", Start { joint: "top", block: "param_special_lw", ahead: "deathball1_offset_x", up: "deathball1_offset_y", speed: "" }),
    ("brave", "explosion", Start { joint: "top", block: "param_special_lw", ahead: "explosion1_offset_x", up: "explosion1_offset_y", speed: "" }),
    ("brave", "fireball", Start { joint: "top", block: "param_special_n", ahead: "offset_x_s", up: "offset_y_s", speed: "" }),
    ("brave", "flash", Start { joint: "top", block: "param_special_lw", ahead: "flash1_offset_x", up: "flash1_offset_y", speed: "" }),
    ("brave", "sleep", Start { joint: "top", block: "param_special_lw", ahead: "sleep_offset_x", up: "sleep_offset_y", speed: "" }),
    ("cloud", "wave", Start { joint: "top", block: "param_special_n", ahead: "shoot_x", up: "shoot_y", speed: "" }),
    ("dedede", "star", Start { joint: "top", block: "param_special_hi", ahead: "right_start_pos_offset_x", up: "right_start_pos_offset_y", speed: "" }),
    ("dedede", "starmissile", Start { joint: "top", block: "param_special_n", ahead: "spit_offset_x", up: "spit_offset_y", speed: "" }),
    ("edge", "flare1", Start { joint: "top", block: "param_special_s", ahead: "offset_x", up: "offset_y", speed: "" }),
    ("eflame", "firepillar", Start { joint: "top", block: "param_special_hi", ahead: "firepillar_offset_x", up: "firepillar_offset_y", speed: "" }),
    ("elight", "exprosiveshot", Start { joint: "top", block: "param_special_hi", ahead: "exprosiveshot_offset_x", up: "exprosiveshot_offset_y", speed: "" }),
    ("elight", "spreadbullet", Start { joint: "top", block: "param_special_hi", ahead: "spreadbullet_offset_x", up: "spreadbullet_offset_y", speed: "" }),
    ("falco", "blaster_bullet", Start { joint: "haver", block: "param_special_n", ahead: "blaster_shoot_x", up: "blaster_shoot_y", speed: "blaster_shoot_speed" }),
    ("fox", "blaster_bullet", Start { joint: "haver", block: "param_special_n", ahead: "blaster_shoot_x", up: "blaster_shoot_y", speed: "blaster_shoot_speed" }),
    ("inkling", "splash", Start { joint: "top", block: "param_special_hi", ahead: "splash_offset_x", up: "splash_offset_y", speed: "" }),
    ("jack", "fire", Start { joint: "top", block: "param_special_s", ahead: "fire_offset_x", up: "fire_offset_y", speed: "" }),
    ("ken", "hadoken", Start { joint: "top", block: "param_special_n", ahead: "shoot_x", up: "shoot_y", speed: "" }),
    ("lucario", "qigong", Start { joint: "top", block: "param_special_s", ahead: "offset_x", up: "offset_y", speed: "" }),
    ("lucas", "pk_fire", Start { joint: "haver", block: "param_special_s", ahead: "offset_x", up: "offset_y", speed: "" }),
    ("miiswordsman", "tornadoshot", Start { joint: "top", block: "param_special_n", ahead: "n1_offset_ground_x", up: "n1_offset_ground_y", speed: "" }),
    ("ness", "pk_fire", Start { joint: "havel", block: "param_special_s", ahead: "offset_x", up: "offset_y", speed: "" }),
    ("palutena", "reflectionboard", Start { joint: "top", block: "param_special_lw", ahead: "special_lw_reflectionboard_start_x", up: "special_lw_reflectionboard_start_y", speed: "" }),
    ("pfushigisou", "leafcutter", Start { joint: "top", block: "param_special_s", ahead: "shoot_offset_x", up: "shoot_offset_y", speed: "shoot_speed" }),
    ("pichu", "dengekidama", Start { joint: "top", block: "param_special_n", ahead: "special_n_offset_ground_x", up: "special_n_offset_ground_y", speed: "" }),
    ("pikachu", "dengekidama", Start { joint: "top", block: "param_special_n", ahead: "special_n_offset_ground_x", up: "special_n_offset_ground_y", speed: "" }),
    ("popo", "blizzard", Start { joint: "haver", block: "param_special_lw", ahead: "shot_offset_x", up: "shot_offset_y", speed: "" }),
    ("rockman", "crashbomb", Start { joint: "handl", block: "param_special_s", ahead: "offset_x", up: "", speed: "" }),
    ("rockman", "hardknuckle", Start { joint: "handl", block: "param_special_s", ahead: "offset_x", up: "", speed: "" }),
    ("rockman", "rockbuster", Start { joint: "handl", block: "param_private", ahead: "attack_1_offset_x", up: "", speed: "" }),
    ("ryu", "hadoken", Start { joint: "top", block: "param_special_n", ahead: "shoot_x", up: "shoot_y", speed: "" }),
    ("szerosuit", "paralyzer_bullet", Start { joint: "haver", block: "param_special_n", ahead: "offset_x", up: "offset_y", speed: "" }),
    ("trail", "fire", Start { joint: "top", block: "param_special_n", ahead: "fire_offset_x", up: "fire_offset_y", speed: "" }),
    ("wolf", "blaster_bullet", Start { joint: "haver", block: "param_special_n", ahead: "blaster_shoot_x", up: "blaster_shoot_y", speed: "blaster_shoot_speed" }),
];

#[rustfmt::skip]
const LINKS: &[(&str, &str, Link)] = &[
    ("buddy", "horn", ("have", "k_jaw")),
    ("buddy", "partner", ("top", "haver")),
    ("buddy", "piece", ("have", "haver")),
    ("buddy", "strings", ("have", "havel")),
    ("captain", "falconpunch", ("trans", "rot")),
    ("cloud", "fusionsword", ("haver", "trans")),
    ("daisy", "kassar", ("have", "havel")),
    ("dedede", "gordo", ("roty", "throw")),
    ("dedede", "jethammer", ("have", "haver")),
    ("dedede", "mask", ("attach", "head")),
    ("dedede", "newdededehammer", ("have", "haver")),
    ("diddy", "barreljet", ("top", "waist")),
    ("diddy", "barreljets", ("attach", "shaft")),
    ("diddy", "gun", ("attach", "havel")),
    ("duckhunt", "can", ("can", "throw")),
    ("eflame", "esword", ("haver", "haver")),
    ("elight", "esword", ("haver", "haver")),
    ("falco", "blaster", ("attach", "haver")),
    ("fox", "blaster", ("attach", "haver")),
    ("gamewatch", "bomb", ("top", "havel")),
    ("gamewatch", "oil", ("have", "tophave")),
    ("gamewatch", "parachute", ("have", "tophave")),
    ("ganon", "sword", ("have", "haver")),
    ("gaogaen", "championbelt", ("base", "haver")),
    ("gekkouga", "shuriken", ("top", "havel")),
    ("ike", "sword", ("haver", "sword")),
    ("inkling", "blaster", ("have", "havel")),
    ("inkling", "brush", ("have", "havel")),
    ("inkling", "copy_inkling_gun", ("handr", "haver")),
    ("inkling", "copy_inkling_tank", ("bust", "throw")),
    ("inkling", "megaphonelaser", ("have", "haver")),
    ("inkling", "roller", ("have", "haver")),
    ("inkling", "slosher", ("have", "haver")),
    ("inkling", "splashbomb", ("rot", "haver")),
    ("inkling", "squid", ("trans", "trans")),
    ("jack", "doyle", ("top", "doyle")),
    ("jack", "wing", ("attach", "bust")),
    ("jack", "wirerope", ("have", "havel")),
    ("kamui", "dragonhand", ("attach", "arml")),
    ("kamui", "spearhand", ("attach", "arml")),
    ("ken", "shinkuhadoken", ("top", "armr")),
    ("kirby", "finalcutter", ("have", "haver")),
    ("kirby", "hammer", ("have", "haver")),
    ("kirby", "miipartshead", ("top", "body")),
    ("kirby", "starmissile", ("top", "rot")),
    ("kirby", "ultrasword", ("top", "haver")),
    ("kirby", "ultraswordhat", ("top", "body")),
    ("koopag", "breath", ("top", "haver")),
    ("koopajr", "hammer", ("have", "haver")),
    ("koopajr", "kart", ("trans", "clownhip")),
    ("koopajr", "picopicohammer", ("have", "havel")),
    ("krool", "backpack", ("rot", "bust")),
    ("krool", "blunderbuss", ("have", "haver")),
    ("krool", "piratehat", ("rot", "crown1")),
    ("link", "ancientbowarrow", ("arrow", "haver")),
    ("link", "boomerang", ("have", "haver")),
    // On his back (`bowb`) until a move takes it out, which the stowed object shows.
    ("link", "bow", ("have", "havel")),
    ("link", "bowarrow", ("have", "haver")),
    ("littlemac", "championbelt", ("base", "waist")),
    ("littlemac", "littlemacg", ("rot", "rot")),
    ("lucario", "auraball", ("top", "throw")),
    ("lucas", "himohebi", ("have", "haver")),
    ("lucas", "himohebi2", ("have", "haver")),
    ("lucina", "mask", ("attach", "head")),
    ("luigi", "obakyumu", ("rot", "bust")),
    ("mario", "cappy", ("cappy", "hat")),
    ("mario", "hugeflame", ("have", "top")),
    ("mario", "mantle", ("have", "bust")),
    ("mario", "pump", ("have", "hip")),
    ("mariod", "drmantle", ("have", "haver")),
    ("mariod", "hugecapsule", ("have", "top")),
    ("mariod", "stethoscope", ("attach", "head")),
    ("master", "arrow1", ("have", "haver")),
    ("master", "arrow2", ("have", "haver")),
    ("master", "axe", ("haver", "haver")),
    ("master", "baton", ("haver", "haver")),
    ("master", "bow", ("have", "havel")),
    ("master", "spear", ("haver", "haver")),
    ("master", "sword", ("haver", "haver")),
    ("master", "sword2", ("haver", "haver")),
    ("metaknight", "fourwings", ("f_wingbase", "wingbase")),
    ("metaknight", "mantle", ("have", "mantlebase")),
    ("mewtwo", "bindball", ("top", "throw")),
    ("mewtwo", "shadowball", ("top", "throw")),
    ("miifighter", "ironball", ("top", "haver")),
    ("miigunner", "bottomshoot", ("top", "handr")),
    ("miigunner", "flamepillar", ("top", "handr")),
    ("miigunner", "fullthrottle", ("top", "armr")),
    ("miigunner", "grenadelauncher", ("top", "handr")),
    ("miigunner", "groundbomb", ("top", "handr")),
    ("miigunner", "gunnercharge", ("top", "armr")),
    ("miigunner", "laser", ("top", "armr")),
    ("miigunner", "miimissile", ("top", "handr")),
    ("miigunner", "supermissile", ("top", "handr")),
    ("miiswordsman", "chakram", ("top", "havel")),
    ("miiswordsman", "lightshuriken", ("top", "havel")),
    ("murabito", "balloon", ("have", "throw")),
    ("murabito", "bowlingball", ("top", "haver")),
    ("murabito", "butterflynet", ("have", "haver")),
    ("murabito", "clayrocket", ("top", "throw")),
    ("murabito", "firework", ("have", "throw")),
    ("murabito", "flowerpot", ("have", "haver")),
    ("murabito", "helmet", ("root", "head")),
    ("murabito", "seed", ("have", "haver")),
    ("murabito", "slingshot", ("have", "havel")),
    ("murabito", "umbrella", ("have", "haver")),
    ("murabito", "weeds", ("have", "throw")),
    ("nana", "rubber", ("top", "virtualrubberpopo")),
    ("ness", "yoyo", ("have", "haver")),
    ("ness", "yoyohead", ("attach", "throw")),
    ("palutena", "godwing", ("attach", "bust")),
    ("peach", "kassar", ("have", "havel")),
    ("pfushigisou", "vine", ("have", "grass")),
    ("pickel", "axe", ("have", "weaponr")),
    ("pickel", "fishingrod", ("have", "haver")),
    ("pickel", "pick", ("have", "weaponr")),
    ("pickel", "shovel", ("have", "weaponr")),
    ("pickel", "stuff", ("have", "weaponr")),
    ("pickel", "sword", ("have", "weaponr")),
    ("pickel", "wing", ("attach", "bust")),
    ("pikmin", "pikmin", ("hip", "haver")),
    ("pit", "bow", ("attach", "haver")),
    ("pit", "bowarrow", ("have", "havel")),
    ("pitb", "bow", ("attach", "haver")),
    ("pitb", "bowarrow", ("have", "havel")),
    ("popo", "rubber", ("top", "virtualrubberpopo")),
    ("reflet", "book", ("have", "havel")),
    ("reflet", "elwind", ("top", "handr")),
    ("reflet", "gigafire", ("top", "armr")),
    ("reflet", "thunder", ("top", "armr")),
    ("richter", "axe", ("have", "haver")),
    ("richter", "cross", ("have", "haver")),
    ("richter", "whip", ("have", "haver")),
    ("richter", "whiphand", ("have", "haver")),
    ("richter", "whipwire", ("have", "haver")),
    ("robot", "finalbeam", ("top", "head1")),
    ("robot", "gyro", ("have", "throw")),
    ("robot", "gyroholder", ("attach", "gyroattach")),
    ("robot", "hugebeam", ("top", "head1")),
    ("robot", "mainlaser", ("top", "head1")),
    ("robot", "narrowbeam", ("top", "head1")),
    ("robot", "widebeam", ("top", "head1")),
    ("rockman", "leafshield", ("top", "virtualleafshield")),
    ("rockman", "leftarm", ("haven", "arml")),
    ("rockman", "rightarm", ("haven", "armr")),
    ("rosetta", "ring", ("top", "throw")),
    ("ryu", "shinkuhadoken", ("top", "armr")),
    ("samus", "cshot", ("top", "armr")),
    ("samus", "gbeam", ("attach", "virtualgbeam")),
    ("samus", "gun", ("attach", "armr")),
    ("samus", "laser", ("top", "armr")),
    ("samus", "missile", ("top", "handr")),
    ("samus", "supermissile", ("top", "handr")),
    ("samusd", "cshot", ("top", "armr")),
    ("samusd", "gbeam", ("attach", "armr")),
    ("samusd", "gun", ("attach", "armr")),
    ("samusd", "laser", ("top", "armr")),
    ("samusd", "missile", ("top", "handr")),
    ("samusd", "supermissile", ("top", "handr")),
    ("sheik", "knife", ("top", "haver")),
    ("sheik", "needlehave", ("have", "havel")),
    ("shizue", "balloon", ("have", "swing")),
    ("shizue", "broom", ("have", "havel")),
    ("shizue", "bucket", ("trans", "haver")),
    ("shizue", "butterflynet", ("have", "haver")),
    ("shizue", "cracker", ("have", "havel")),
    ("shizue", "fishingline", ("have", "havel")),
    ("shizue", "fishingrod", ("have", "havel")),
    ("shizue", "picopicohammer", ("have", "havel")),
    ("shizue", "pompon", ("havel", "havel")),
    ("shizue", "pot", ("have", "haver")),
    ("shizue", "slingshot", ("have", "havel")),
    ("shizue", "swing", ("trans", "rot")),
    ("shizue", "trafficsign", ("trans", "throw")),
    ("shizue", "umbrella", ("have", "haver")),
    ("shizue", "weeds", ("have", "throw")),
    ("simon", "axe", ("have", "haver")),
    ("simon", "cross", ("have", "haver")),
    ("simon", "whip", ("have", "haver")),
    ("simon", "whiphand", ("have", "haver")),
    ("simon", "whipwire", ("have", "haver")),
    ("snake", "c4", ("have", "haver")),
    ("snake", "c4switch", ("switchhave", "haver")),
    ("snake", "cypher", ("haverb", "haver")),
    ("snake", "flaregrenades", ("top", "haver")),
    ("snake", "nikita", ("have", "haver")),
    ("snake", "rpg7", ("have2", "haver")),
    ("szerosuit", "paralyzer", ("have", "haver")),
    ("szerosuit", "whip", ("have", "haver")),
    ("szerosuit", "whip2", ("have", "whipattach")),
    ("tantan", "gongfinal", ("trans", "haver")),
    ("tantan", "punch1", ("have", "hand")),
    ("tantan", "punch2", ("have", "hand")),
    ("tantan", "punch3", ("have", "hand")),
    ("tantan", "spiral", ("have", "freeattach")),
    ("tantan", "spiral", ("have", "shoulderl1")),
    ("tantan", "spiral", ("have", "shoulderr1")),
    ("toonlink", "boomerang", ("have", "havel")),
    ("toonlink", "bow", ("have", "haver")),
    ("toonlink", "bowarrow", ("have", "havel")),
    ("toonlink", "hookshot", ("have", "haver")),
    ("toonlink", "hookshothand", ("have", "haver")),
    ("toonlink", "takt", ("have", "havel")),
    ("wario", "garlic", ("have", "havel")),
    ("wiifit", "hulahoop", ("have", "rot")),
    ("wiifit", "towel", ("have", "bust")),
    ("wolf", "blaster", ("attach", "haver")),
    ("yoshi", "tamago", ("have", "havel")),
    ("younglink", "boomerang", ("have", "havel")),
    ("younglink", "bow", ("have", "haver")),
    ("younglink", "bowarrow", ("have", "havel")),
    ("younglink", "hookshot", ("have", "haver")),
    ("younglink", "hookshothand", ("have", "haver")),
    ("younglink", "milk", ("have", "havel")),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_are_found_by_the_folder_and_the_script_name() {
        assert_eq!(link("younglink", "hookshothand"), Some(("have", "haver")));
        assert_eq!(link("younglink", "hookshot_hand"), Some(("have", "haver")));
        assert_eq!(link("fox", "blaster"), Some(("attach", "haver")));
        assert_eq!(link("link", "bow"), Some(("have", "havel")));
        assert_eq!(link("mario", "fireball"), None);
        assert_eq!(link("mario", "hookshothand"), None);
        assert_eq!(start("ness", "pkfire").unwrap().joint, "havel");
        assert!(start("ness", "fireball").is_none());
    }

    /// Every parameter named is one the game's own files have.
    #[test]
    fn the_starts_name_the_games_parameters() {
        let Some(root) = std::env::var_os("VISIONARY_DATA_ROOT") else {
            return;
        };
        let csv = crate::scratch_dirs::app_storage_root().join("ParamLabels.csv");
        let labels =
            crate::param_labels::parse_csv(&std::fs::read_to_string(csv).unwrap_or_default());
        for (fighter, object, start) in STARTS {
            let file = std::path::Path::new(&root).join(format!("fighter/{fighter}/param/vl.prc"));
            let block = start.block.strip_prefix("param_").unwrap();
            let block = crate::articles::params::param_block(&[file], &[], block, &labels);
            let started = start.read(&block);
            assert!(started.is_some(), "{fighter} {object}");
            assert_eq!(
                started.unwrap().speed.is_some(),
                !start.speed.is_empty(),
                "{fighter} {object}"
            );
        }
    }
}
