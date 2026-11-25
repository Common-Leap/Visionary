use super::*;
use crate::eff_attrs::{blank_emitter_data, read_all, SSBU_VFX_VERSION};

/// A part with a constant red colour, an opacity of two keys and a size of one.
fn part() -> EmitterDef {
    let mut em = EmitterDef {
        name: "fire".into(),
        attrs: read_all(&blank_emitter_data(SSBU_VFX_VERSION)),
        subsections: Vec::new(),
        depth: 0,
        source_idx: 0,
        color0: Vec::new(),
        color1: Vec::new(),
        alpha0_keys: Vec::new(),
        texture_index: None,
        shaders: Default::default(),
    };
    let float = AttrValue::Float;
    set_attr(&mut em, "particle_color.color0_r", float(1.0));
    set_attr(
        &mut em,
        "particle_color.alpha0_type",
        AttrValue::Int(OVER_LIFE),
    );
    set_attr(&mut em, "emitter_static.num_alpha0_keys", AttrValue::Int(2));
    set_key(&mut em, &names(Table::Alpha0), 0, ([1.0, 0.0, 0.0], 0.0));
    set_key(&mut em, &names(Table::Alpha0), 1, ([0.0; 3], 1.0));
    set_attr(&mut em, "emitter_static.num_scale_keys", AttrValue::Int(1));
    set_key(&mut em, &names(Table::Scale), 0, ([2.0; 3], 0.0));
    attrs_to_effective_keys(&mut em);
    em
}

fn opacities(em: &EmitterDef) -> Vec<(f32, f32)> {
    em.alpha0_keys.iter().map(|k| (k.r, k.frame)).collect()
}

/// Change a row as its control does.
fn set_row(em: &mut EmitterDef, pr: &EmitterSnapshot, id: &str, value: i64) {
    let was = attr(em, id).unwrap();
    set_attr(em, id, AttrValue::Int(value));
    follow(em, pr, id, was);
    attrs_to_effective_keys(em);
}

#[test]
fn every_table_names_attributes_there_are() {
    for table in Table::ALL {
        let names = names(table);
        let mut ids = vec![names.count.to_owned()];
        if let Some((kind, constant)) = names.constant {
            ids.push(kind.into());
            ids.extend(constant.iter().map(|id| id.to_string()));
        }
        for key in 0..SLOTS {
            for number in names.numbers.iter().chain(&["time"]) {
                let id = slot_id(names.keys, key, number);
                assert_eq!(row_table(&id), Some(table), "{id}");
                ids.push(id);
            }
        }
        for id in ids {
            assert!(crate::eff_attrs::index_of(&id).is_some(), "{id}");
        }
        assert_eq!(table_of(names.count), Some(table));
    }
    assert_eq!(row_table("emitter_static.num_param_keys"), None);
    assert_eq!(row_table("particle_color.color0_r"), None);
}

/// The tester's report: the key count was changed and the keys shown stayed the file's.
#[test]
fn a_key_count_changes_the_keys_shown() {
    let mut em = part();
    let pr = EmitterSnapshot::of(&em);
    let id = "emitter_static.num_alpha0_keys";
    assert_eq!(opacities(&em), [(1.0, 0.0), (0.0, 1.0)]);
    set_row(&mut em, &pr, id, 3);
    assert_eq!(opacities(&em), [(1.0, 0.0), (0.5, 0.5), (0.0, 1.0)]);
    set_row(&mut em, &pr, id, 4);
    assert_eq!(
        opacities(&em),
        [(1.0, 0.0), (0.5, 0.5), (0.25, 0.75), (0.0, 1.0)]
    );
    // Back down by dragging, the fade is the file's again and nothing counts as edited.
    set_row(&mut em, &pr, id, 2);
    assert_eq!(opacities(&em), [(1.0, 0.0), (0.0, 1.0)]);
    set_row(&mut em, &pr, id, 1);
    assert_eq!(opacities(&em), [(1.0, 0.0)]);
    // Never none: a part that runs through keys has one.
    set_row(&mut em, &pr, id, 0);
    assert_eq!(opacities(&em), [(1.0, 0.0)]);
    set_row(&mut em, &pr, id, 8);
    assert_eq!(em.alpha0_keys.len(), 8);
    assert!(!add(&mut em, Table::Alpha0), "eight at most");
    let times: Vec<f32> = em.alpha0_keys.iter().map(|k| k.frame).collect();
    assert!(times.windows(2).all(|w| w[0] < w[1]), "{times:?}");
    reset(&mut em, &pr, id);
    attrs_to_effective_keys(&mut em);
    assert_eq!(em.attrs, pr.attrs);
    assert_eq!(opacities(&em), [(1.0, 0.0), (0.0, 1.0)]);
}

#[test]
fn a_constant_becomes_keys_of_its_own_colour() {
    let mut em = part();
    let pr = EmitterSnapshot::of(&em);
    assert_eq!(count(&em, Table::Colour0), 0);
    assert!(add(&mut em, Table::Colour0));
    attrs_to_effective_keys(&mut em);
    let keys: Vec<_> = em.color0.iter().map(|k| (k.r, k.g, k.frame)).collect();
    assert_eq!(keys, [(1.0, 0.0, 0.0), (1.0, 0.0, 1.0)]);
    assert_eq!(
        attr(&em, "particle_color.color0_type"),
        Some(AttrValue::Int(OVER_LIFE))
    );
    // Set constant again, it is the constant it was.
    set_row(&mut em, &pr, "particle_color.color0_type", CONSTANT);
    assert_eq!(em.color0.len(), 1);
    assert_eq!((em.color0[0].r, em.color0[0].g), (1.0, 0.0));
    // Choosing the type makes its first key too.
    set_row(&mut em, &pr, "particle_color.color0_type", OVER_LIFE);
    assert_eq!((count(&em, Table::Colour0), em.color0[0].r), (1, 1.0));
    // A type the file has keys for brings them back.
    set_row(&mut em, &pr, "particle_color.alpha0_type", CONSTANT);
    assert_eq!(em.alpha0_keys.len(), 1);
    set_row(&mut em, &pr, "particle_color.alpha0_type", OVER_LIFE);
    assert_eq!(opacities(&em), [(1.0, 0.0), (0.0, 1.0)]);
}

#[test]
fn removing_a_key_moves_the_later_ones_up() {
    let mut em = part();
    for _ in 0..2 {
        add(&mut em, Table::Scale);
    }
    let scale = names(Table::Scale);
    set_key(&mut em, &scale, 1, ([3.0; 3], 0.4));
    assert!(remove(&mut em, Table::Scale, 0));
    assert_eq!(key_at(&em, &scale, 0), ([3.0; 3], 0.4));
    assert_eq!(key_at(&em, &scale, 1), ([2.0; 3], 1.0));
    assert!(remove(&mut em, Table::Scale, 1));
    assert!(!remove(&mut em, Table::Scale, 0), "the last one stays");
}

/// Keys added in the editor survive the project: what is saved puts them back on the part.
#[test]
fn added_keys_survive_the_project() {
    let mut em = part();
    let pr = EmitterSnapshot::of(&em);
    add(&mut em, Table::Colour0);
    add(&mut em, Table::Alpha0);
    attrs_to_effective_keys(&mut em);
    em.color0[1].g = 0.75;
    em.alpha0_keys[1].r = 0.9;
    effective_keys_to_attrs(&mut em);
    let saved = field_edits(&em, &pr, &[]);
    let mut again = pr.to_def(0);
    apply_field_edits(&mut again, &saved, &[]);
    assert_eq!(again.color0, em.color0);
    assert_eq!(opacities(&again), opacities(&em));
    assert_eq!(again.attrs, em.attrs);
}

/// Sections added to a part and taken off it survive the project, with their edits.
#[test]
fn added_and_removed_sections_survive_the_project() {
    use crate::effects::EmitterSubsectionDef;
    let section = |magic: &str, data: Vec<u8>, source| EmitterSubsectionDef {
        magic: magic.into(),
        data,
        source,
    };
    let mut em = part();
    em.subsections = vec![
        section("EATR", vec![1; 44], Some(0)),
        section("EAA0", vec![2; 44], Some(1)),
        section("EASS", vec![3; 44], Some(2)),
    ];
    let pr = EmitterSnapshot::of(&em);
    assert!(field_edits(&em, &pr, &[]).is_empty());
    let saved = |em: &EmitterDef| {
        let edits = field_edits(em, &pr, &[]);
        let text = serde_json::to_string(&edits).unwrap();
        let edits = serde_json::from_str(&text).unwrap();
        let mut again = pr.to_def(0);
        apply_field_edits(&mut again, &edits, &[]);
        again.subsections
    };
    // One taken off, one added, one changed and one made longer.
    em.subsections.remove(0);
    em.subsections[0].data[20] = 9;
    em.subsections[1].data.extend([4; 16]);
    em.subsections.insert(0, section("FCLN", vec![5; 36], None));
    let now = saved(&em);
    let shown = |list: &[EmitterSubsectionDef]| -> Vec<(String, Vec<u8>)> {
        list.iter()
            .map(|s| (s.magic.clone(), s.data.clone()))
            .collect()
    };
    assert_eq!(shown(&now), shown(&em.subsections));
    assert_eq!(now[1].source, Some(1), "still the file's own");
    em.subsections.clear();
    assert!(saved(&em).is_empty());
    // Put back as it was, nothing is edited.
    em.subsections = pr.subsections.clone();
    assert!(field_edits(&em, &pr, &[]).is_empty());
}

/// The keys shown from a part's attributes are the ones it is loaded with, for every part.
#[test]
fn shown_keys_are_the_loaded_ones() {
    let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from) else {
        eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
        return;
    };
    for fighter in ["mario", "edge", "inkling"] {
        let path = root.join(format!("effect/fighter/{fighter}/ef_{fighter}.eff"));
        let loaded = load_effect(&path).unwrap();
        for em in loaded.ptcl.emitter_sets.iter().flat_map(|s| &s.emitters) {
            let mut again = em.clone();
            attrs_to_effective_keys(&mut again);
            assert!(field_edits(&again, &EmitterSnapshot::of(em), &[]).is_empty());
            assert_eq!(again.color0, em.color0, "{fighter} {}", em.name);
            assert_eq!(again.color1, em.color1, "{fighter} {}", em.name);
            assert_eq!(again.alpha0_keys, em.alpha0_keys, "{fighter} {}", em.name);
            for table in Table::ALL {
                let read = used(em, table);
                let keys = count(em, table);
                assert!(
                    read.picks || read.keys < 2 || read.keys as usize == keys,
                    "{fighter} {} {table:?}: {read:?} {keys}",
                    em.name
                );
            }
        }
    }
}
