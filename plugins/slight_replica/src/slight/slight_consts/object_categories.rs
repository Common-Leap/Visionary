//! Object categories from `slight_consts::object_categories`.

use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectCategory {
    Fighter,
    Weapon,
    Enemy,
    Gimmick,
    Invalid,
}

const NAMES: &[(&str, ObjectCategory)] = &[
    ("FIGHTER", ObjectCategory::Fighter),
    ("WEAPON", ObjectCategory::Weapon),
    ("ENEMY", ObjectCategory::Enemy),
    ("GIMMICK", ObjectCategory::Gimmick),
    ("INVALID", ObjectCategory::Invalid),
];

impl FromStr for ObjectCategory {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key = s.trim().to_ascii_uppercase();
        for (name, cat) in NAMES {
            if *name == key {
                return Ok(*cat);
            }
        }
        Err(())
    }
}

impl ObjectCategory {}
