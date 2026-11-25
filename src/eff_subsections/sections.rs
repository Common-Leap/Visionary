//! Which sections a part can be given and which it can lose, and what a new one holds.

use super::{section_title, write_f32, write_u32};
use crate::effects::{EmitterDef, EmitterSubsectionDef};

/// The part animations in the order the game's files keep them. Other sections come before them.
pub(super) const ANIMATIONS: [&str; 14] = [
    "EASL", "EAES", "EAER", "EAET", "EATR", "EAOV", "EADV", "EAPL", "EAC0", "EAC1", "EAA0", "EAA1",
    "EAGV", "EASS",
];

/// The forces on a part's particles. The game applies them in the order the part has them.
pub(super) const FORCES: [&str; 8] = [
    "FRND", "FRN1", "FPAD", "FMAG", "FCOV", "FSPN", "FCOL", "FCLN",
];

/// How a part's particles are moved: by the game as it runs, or by a shader made for the part.
fn runs_in_game(em: &EmitterDef) -> bool {
    number(em, "emitter_info.calc_type") == 0.0
}

fn number(em: &EmitterDef, id: &str) -> f32 {
    crate::eff_attrs::index_of(id)
        .and_then(|at| em.attrs.get(at).copied().flatten())
        .map_or(0.0, |v| v.as_f32())
}

/// Why the part can't be given a section, when it can't.
pub(super) fn add_block(em: &EmitterDef, magic: &str) -> Option<&'static str> {
    if em.subsections.iter().any(|s| s.magic == magic) {
        return Some("The part has this already.");
    }
    (FORCES.contains(&magic) && !runs_in_game(em))
        .then_some("This part's particles are moved by a shader, which was made without this.")
}

/// Why a section can't be taken off the part, when it can't.
pub(super) fn remove_block(em: &EmitterDef, magic: &str) -> Option<&'static str> {
    if ANIMATIONS.contains(&magic) {
        return None;
    }
    if FORCES.contains(&magic) {
        return (!runs_in_game(em))
            .then_some("This part's particles are moved by a shader, which was made with this.");
    }
    Some("The game draws this part by it.")
}

/// Put a new section on the part, where the game's files keep one.
pub(super) fn add(em: &mut EmitterDef, magic: &str) {
    let section = EmitterSubsectionDef {
        magic: magic.to_owned(),
        data: new_data(em, magic),
        source: None,
    };
    let order = |magic: &str| ANIMATIONS.iter().position(|m| *m == magic);
    let at = match order(magic) {
        // After the animations that come before it.
        Some(mine) => em
            .subsections
            .iter()
            .position(|s| order(&s.magic).is_some_and(|other| other > mine)),
        None => em
            .subsections
            .iter()
            .position(|s| s.magic.starts_with("EA")),
    };
    let at = at.unwrap_or(em.subsections.len());
    em.subsections.insert(at, section);
}

/// What a new section holds. An animation starts as two keys of the value the part has, so
/// nothing changes until one is edited; a force as the game's files most often have it.
fn new_data(em: &EmitterDef, magic: &str) -> Vec<u8> {
    if ANIMATIONS.contains(&magic) {
        let value = animated_value(em, magic);
        let mut data = vec![0; 12 + 2 * 16];
        data[0] = 1;
        write_u32(&mut data, 4, 2);
        for (key, frame) in [0.0, 60.0].into_iter().enumerate() {
            for (k, value) in value.into_iter().chain([frame]).enumerate() {
                write_f32(&mut data, 12 + 16 * key + 4 * k, value);
            }
        }
        return data;
    }
    let (size, words): (usize, &[(usize, u32)]) = match magic {
        "FRND" => (
            208,
            &[
                (0x00, 0x0001_0000),
                (0x04, 0x3f8c_cccd),
                (0x08, 0x3f8c_cccd),
                (0x0c, 0x3f8c_cccd),
                (0x10, 1000),
                (0x1c, 0x4080_0000),
                (0x20, 0x4040_0000),
                (0x24, 0x4000_0000),
                (0x28, 0x3fc0_0000),
                (0x2c, 0x3f19_999a),
                (0x30, 0x3ed7_0a3d),
                (0x34, 0x3e6b_851f),
                (0x38, 0x3e19_999a),
                (0x48, 1),
                (0x4c, 100),
            ],
        ),
        "FRN1" => (
            164,
            &[
                (0x00, 0x3dcc_cccd),
                (0x04, 0x3dcc_cccd),
                (0x08, 0x3dcc_cccd),
                (0x0c, 4),
                (0x1c, 1),
                (0x20, 100),
            ],
        ),
        "FPAD" => (164, &[(0x08, 0x3dcc_cccd), (0x1c, 1), (0x20, 100)]),
        "FMAG" => (
            168,
            &[
                (0x00, 0x0101_0100),
                (0x04, 0x3a83_126f),
                (0x20, 1),
                (0x24, 100),
            ],
        ),
        "FCOV" => (168, &[(0x10, 0x3db8_51ec), (0x20, 1), (0x24, 100)]),
        "FSPN" => (
            308,
            &[
                (0x04, 1),
                (0x08, 0x3dcc_cccd),
                (0x18, 1),
                (0x1c, 100),
                (0xac, 1),
                (0xb0, 100),
            ],
        ),
        "FCOL" => (
            20,
            &[
                (0x00, 0x0000_0100),
                (0x08, 0x3f00_0000),
                (0x0c, 0xffff_ffff),
                (0x10, 0x3dcc_cccd),
            ],
        ),
        "FCLN" => (
            36,
            &[
                (0x00, 0x0000_0100),
                (0x10, 0x3dcc_cccd),
                (0x14, 0x3dcc_cccd),
                (0x18, 0x3dcc_cccd),
                (0x1c, 0x3f80_0000),
                (0x20, 0x3f80_0000),
            ],
        ),
        _ => (0, &[]),
    };
    let mut data = vec![0; size];
    for (at, word) in words {
        write_u32(&mut data, *at, *word);
    }
    data
}

/// What an animation replaces, as the part has it.
fn animated_value(em: &EmitterDef, magic: &str) -> [f32; 3] {
    let xyz = |prefix: &str, names: [&str; 3]| names.map(|n| number(em, &format!("{prefix}{n}")));
    let one = |id: &str| [number(em, id), 0.0, 0.0];
    const AXES: [&str; 3] = ["x", "y", "z"];
    const RGB: [&str; 3] = ["r", "g", "b"];
    match magic {
        "EAES" => xyz("emitter_info.scale_", AXES),
        // Degrees here, radians on the part.
        "EAER" => xyz("emitter_info.rotate_", AXES).map(f32::to_degrees),
        "EAET" => xyz("emitter_info.trans_", AXES),
        "EAC0" => xyz("emitter_info.color0_", RGB),
        "EAC1" => xyz("emitter_info.color1_", RGB),
        "EATR" => one("emission.rate"),
        "EAPL" => one("particle_data.life"),
        "EAA0" => one("emitter_info.color0_a"),
        "EAA1" => one("emitter_info.color1_a"),
        "EAOV" => one("particle_velocity.all_direction"),
        "EADV" => one("particle_velocity.designated_dir_scale"),
        "EASL" => xyz("particle_scale.scale_", AXES),
        "EASS" => xyz("shape_info.volume_form_scale_", AXES),
        "EAGV" => one("emission.gravity_scale"),
        _ => [0.0; 3],
    }
}

/// The sections a part can be given, by the group they are offered in.
pub(super) fn offered() -> [(&'static str, Vec<(&'static str, String)>); 2] {
    let titled = |magics: &[&'static str]| magics.iter().map(|m| (*m, section_title(m))).collect();
    [
        ("Animations", titled(&ANIMATIONS)),
        ("Forces", titled(&FORCES)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(magics: &[&str], calc: i64) -> EmitterDef {
        let blank = crate::eff_attrs::blank_emitter_data(crate::eff_attrs::SSBU_VFX_VERSION);
        let mut attrs = crate::eff_attrs::read_all(&blank);
        let mut set = |id: &str, value: crate::eff_attrs::AttrValue| {
            attrs[crate::eff_attrs::index_of(id).expect(id)] = Some(value);
        };
        set(
            "emitter_info.calc_type",
            crate::eff_attrs::AttrValue::Int(calc),
        );
        set("emission.rate", crate::eff_attrs::AttrValue::Float(2.5));
        set(
            "emitter_info.rotate_y",
            crate::eff_attrs::AttrValue::Float(std::f32::consts::FRAC_PI_2),
        );
        EmitterDef {
            name: "part".into(),
            attrs,
            subsections: magics
                .iter()
                .enumerate()
                .map(|(i, magic)| EmitterSubsectionDef {
                    magic: magic.to_string(),
                    data: vec![0; 4],
                    source: Some(i),
                })
                .collect(),
            depth: 0,
            source_idx: 0,
            color0: Vec::new(),
            color1: Vec::new(),
            alpha0_keys: Vec::new(),
            texture_index: None,
            shaders: Default::default(),
        }
    }

    fn magics(em: &EmitterDef) -> Vec<&str> {
        em.subsections.iter().map(|s| s.magic.as_str()).collect()
    }

    #[test]
    fn a_new_section_goes_where_the_games_files_keep_it() {
        let mut em = part(&["CSDP", "FSPN", "EATR", "EAA0"], 0);
        add(&mut em, "EAC0");
        add(&mut em, "FCLN");
        add(&mut em, "EASS");
        add(&mut em, "EAES");
        assert_eq!(
            magics(&em),
            ["CSDP", "FSPN", "FCLN", "EAES", "EATR", "EAC0", "EAA0", "EASS"]
        );
        assert!(add_block(&em, "FSPN").is_some(), "one of each");
        assert!(add_block(&em, "FMAG").is_none());
        assert!(add_block(&part(&[], 2), "FMAG").is_some());
        assert!(add_block(&part(&[], 2), "EAES").is_none());
        for (magic, kept) in [
            ("CSDP", true),
            ("EP03", true),
            ("CADP", true),
            ("EAA0", false),
        ] {
            assert_eq!(remove_block(&em, magic).is_some(), kept, "{magic}");
        }
    }

    /// A new animation holds what the part has, so it changes nothing until it is edited.
    #[test]
    fn a_new_animation_starts_as_the_part_is() {
        let mut em = part(&[], 0);
        add(&mut em, "EATR");
        add(&mut em, "EAER");
        let read = |s: &EmitterSubsectionDef, at: usize| super::super::read_f32(&s.data, at);
        let rate = em.subsections.iter().find(|s| s.magic == "EATR").unwrap();
        assert_eq!((rate.data.len(), rate.data[0]), (44, 1));
        for (at, value) in [(12, 2.5), (24, 0.0), (28, 2.5), (40, 60.0)] {
            assert_eq!(read(rate, at), Some(value));
        }
        let turn = em.subsections.iter().find(|s| s.magic == "EAER").unwrap();
        assert!((read(turn, 16).unwrap() - 90.0).abs() < 1e-3);
        for (_, list) in offered() {
            for (magic, title) in list {
                assert_ne!(title, "Extra data", "{magic}");
                assert_ne!(title, "Other animation", "{magic}");
                assert!(!new_data(&em, magic).is_empty(), "{magic}");
            }
        }
    }

    /// Every new section is the size the game's files have.
    #[test]
    fn new_sections_are_the_size_the_games_are() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let em = part(&[], 0);
        let mut seen = std::collections::BTreeSet::new();
        for fighter in [
            "bayonetta",
            "edge",
            "ryu",
            "kirby",
            "brave",
            "ganon",
            "link",
        ] {
            let path = root.join(format!("effect/fighter/{fighter}/ef_{fighter}.eff"));
            let effect = crate::effects::load_effect(&path).unwrap();
            for part in effect.ptcl.emitter_sets.iter().flat_map(|s| &s.emitters) {
                let mut last = 0;
                for section in &part.subsections {
                    let magic = section.magic.as_str();
                    if let Some(at) = ANIMATIONS.iter().position(|m| *m == magic) {
                        assert!(at >= last, "{fighter} {} {magic}", part.name);
                        last = at;
                    }
                    if FORCES.contains(&magic) {
                        assert_eq!(section.data.len(), new_data(&em, magic).len(), "{magic}");
                        seen.insert(magic.to_owned());
                    }
                }
            }
        }
        assert!(seen.len() >= 5, "{seen:?}");
    }

    /// Sections added to a part and taken off it are so in the written file, and the sections
    /// that stay keep their own edits.
    #[test]
    fn added_and_removed_sections_are_written() {
        use crate::mod_project::{
            AuthoredEdit, EffMod, EmitterFieldEdits, SectionSlot, SubsectionEdit,
        };
        const RYU: &str = "effect/fighter/ryu/ef_ryu.eff";
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let path = root.join(RYU);
        let sets = crate::effects::load_effect(&path)
            .unwrap()
            .ptcl
            .emitter_sets;
        let (set, part) = (0..sets.len())
            .flat_map(|s| (0..sets[s].emitters.len()).map(move |e| (s, e)))
            .find(|&(s, e)| {
                let part = &sets[s].emitters[e];
                part.subsections.len() > 1
                    && part.subsections.iter().all(|s| s.magic.starts_with("EA"))
                    && add_block(part, "FCLN").is_none()
            })
            .unwrap();
        let mut em = sets[set].emitters[part].clone();
        let gone = em.subsections.remove(0);
        add(&mut em, "FCLN");
        let kept = em.subsections.last().unwrap().clone();
        let write = |sections: Option<Vec<SectionSlot>>, subsections: Vec<SubsectionEdit>| {
            let eff = EffMod {
                source_rel: RYU.into(),
                authored: vec![AuthoredEdit {
                    set_name: sets[set].name.clone(),
                    entry_name: String::new(),
                    set_idx: set,
                    emitter_name: em.name.clone(),
                    emitter_idx: part,
                    fields: EmitterFieldEdits {
                        sections,
                        subsections,
                        ..Default::default()
                    },
                }],
                ..Default::default()
            };
            let bytes = std::fs::read(&path).unwrap();
            let out = crate::eff_export::rebuild_eff_bytes(&bytes, &eff, None).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("ef_ryu.eff");
            std::fs::write(&file, out).unwrap();
            let after = crate::effects::load_effect(&file).unwrap();
            after.ptcl.emitter_sets[set].emitters[part].clone()
        };
        let slots = em
            .subsections
            .iter()
            .map(|s| SectionSlot {
                magic: s.magic.clone(),
                source: s.source,
                data: if s.source.is_some() {
                    Vec::new()
                } else {
                    s.data.clone()
                },
            })
            .collect();
        let edit = SubsectionEdit {
            index: kept.source.unwrap(),
            magic: kept.magic.clone(),
            bytes: [(12, 0x55)].into_iter().collect(),
        };
        let after = write(Some(slots), vec![edit]);
        assert_eq!(magics(&after), magics(&em));
        assert!(!magics(&after).contains(&gone.magic.as_str()));
        assert_eq!(after.subsections[0].data, new_data(&em, "FCLN"));
        assert_eq!(after.subsections.last().unwrap().data[12], 0x55);
        // All of them taken off.
        assert!(write(Some(Vec::new()), Vec::new()).subsections.is_empty());
    }
}
