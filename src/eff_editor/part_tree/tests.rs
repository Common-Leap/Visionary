use super::*;
use crate::effects::EmitterSet;
use crate::ui::TestFrame;

/// A part `depth` deep, with every attribute present.
pub(in crate::eff_editor) fn part(name: &str, source_idx: usize, depth: u8) -> EmitterDef {
    EmitterDef {
        name: name.into(),
        attrs: crate::eff_attrs::read_all(&crate::eff_attrs::blank_emitter_data(
            crate::eff_attrs::SSBU_VFX_VERSION,
        )),
        subsections: Vec::new(),
        depth,
        source_idx,
        color0: Vec::new(),
        color1: Vec::new(),
        alpha0_keys: Vec::new(),
        texture_index: None,
    }
}

/// An editor with one effect open per set of `sets`, the first one open.
pub(in crate::eff_editor) fn editor_with(sets: &[(&str, &[(&str, u8)])]) -> EffEditor {
    let emitter_sets: Vec<EmitterSet> = sets
        .iter()
        .map(|(name, parts)| EmitterSet {
            name: format!("P_{name}"),
            emitters: parts
                .iter()
                .enumerate()
                .map(|(i, (name, depth))| part(name, i, *depth))
                .collect(),
        })
        .collect();
    EffEditor {
        pristine: emitter_sets
            .iter()
            .map(|set| set.emitters.iter().map(EmitterSnapshot::of).collect())
            .collect(),
        entries: sets
            .iter()
            .enumerate()
            .map(|(set_idx, (name, _))| EffEntry {
                name: (*name).to_owned(),
                hash: hash40::hash40(name).0,
                set_idx,
            })
            .collect(),
        ptcl: Some(PtclFile {
            emitter_sets,
            bntx_textures: Vec::new(),
        }),
        selected_entry: Some(0),
        loaded_path: Some(PathBuf::from("/fx/ef_test.eff")),
        ..Default::default()
    }
}

/// Take the parts as they are for the file's own.
fn as_the_file_has_it(editor: &mut EffEditor) {
    editor.pristine = editor
        .ptcl
        .as_ref()
        .unwrap()
        .emitter_sets
        .iter()
        .map(|set| set.emitters.iter().map(EmitterSnapshot::of).collect())
        .collect();
}

/// Two families and a part on its own: clock makes trace and spark, flare nothing, smoke puff.
fn families() -> EffEditor {
    editor_with(&[(
        "dash",
        &[
            ("clock1", 0),
            ("trace1", 1),
            ("spark1", 1),
            ("flare1", 0),
            ("smoke1", 0),
            ("puff1", 1),
        ],
    )])
}

/// The parts of set `set`, each child a step in: "clock1 -trace1 -spark1".
pub(in crate::eff_editor) fn list(editor: &EffEditor, set: usize) -> String {
    let emitters = &editor.ptcl.as_ref().unwrap().emitter_sets[set].emitters;
    let names: Vec<String> = emitters
        .iter()
        .map(|e| format!("{}{}", "-".repeat(e.depth.into()), e.name))
        .collect();
    names.join(" ")
}

/// What `does` leaves of the list, done to part `i` with part `open` open, and the part open
/// after. It must be one undo step under the name returned, or no step at all.
fn after(open: usize, i: usize, does: PartAction) -> (String, String, Option<String>) {
    let mut editor = families();
    editor.selected_emitter = open;
    let before = list(&editor, 0);
    let t0 = Instant::now();
    editor.history_begin_frame(false, t0);
    let rev = editor.edit_rev;
    editor.part_action(0, i, does);
    let changed = editor.edit_rev != rev;
    if changed {
        editor.history_note_edit(t0);
    }
    let (now, at) = (list(&editor, 0), editor.selected_emitter);
    let name = editor.ptcl.as_ref().unwrap().emitter_sets[0].emitters[at]
        .name
        .clone();
    let label = editor.undo_label();
    assert_eq!(changed, label.is_some());
    if changed {
        editor.undo();
        assert_eq!(list(&editor, 0), before, "one step");
        assert!(!editor.can_undo());
    }
    (now, name, label)
}

#[test]
fn the_tree_says_who_is_made_from_whom() {
    let tree = families().part_tree(0);
    assert_eq!(tree.len(), 6);
    assert_eq!(
        (0..6).map(|i| tree.parent(i)).collect::<Vec<_>>(),
        [None, Some(0), Some(0), None, None, Some(4)]
    );
    assert_eq!(tree.root(2), 0);
    assert_eq!(tree.children(0), [1, 2]);
    assert!(tree.children(3).is_empty());
    assert_eq!((tree.family(0), tree.family(2)), (0..3, 2..3));
    assert_eq!(tree.path(2), "Clock 1 › Spark 1");
    assert_eq!(tree.counted(0), "Clock 1 (2)");
    assert_eq!(tree.counted(3), "Flare 1");
    assert_eq!(tree.with_children(0), "Clock 1 and its 2 child parts");
    assert_eq!(tree.with_children(4), "Smoke 1 and its child part");
    // Moving stays among the parts made from the same one.
    assert_eq!(
        (tree.sibling(1, true), tree.sibling(1, false)),
        (Some(2), None)
    );
    assert_eq!(
        (tree.sibling(2, true), tree.sibling(0, true)),
        (None, Some(3))
    );
    assert_eq!(
        (tree.sibling(4, false), tree.sibling(4, true)),
        (Some(3), None)
    );
}

/// A parent moves with what is made from it; a child only among its parent's own.
#[test]
fn moving_keeps_every_part_under_the_one_it_is_made_from() {
    let (list, open, label) = after(1, 0, PartAction::DrawLater);
    assert_eq!(list, "flare1 clock1 -trace1 -spark1 smoke1 -puff1");
    assert_eq!(open, "trace1", "the open part stays open");
    assert_eq!(label.as_deref(), Some("Draw Clock 1 later in dash"));
    let (list, ..) = after(0, 4, PartAction::DrawEarlier);
    assert_eq!(list, "clock1 -trace1 -spark1 smoke1 -puff1 flare1");
    let (list, ..) = after(0, 3, PartAction::DrawEarlier);
    assert_eq!(list, "flare1 clock1 -trace1 -spark1 smoke1 -puff1");
    let (list, _, label) = after(0, 2, PartAction::DrawEarlier);
    assert_eq!(list, "clock1 -spark1 -trace1 flare1 smoke1 -puff1");
    assert_eq!(
        label.as_deref(),
        Some("Draw Clock 1 › Spark 1 earlier in dash")
    );
    // Past its own part's: nothing, and no step.
    for (i, does) in [
        (1, PartAction::DrawEarlier),
        (2, PartAction::DrawLater),
        (5, PartAction::DrawEarlier),
        (5, PartAction::DrawLater),
        (0, PartAction::DrawEarlier),
        (4, PartAction::DrawLater),
    ] {
        let (now, _, label) = after(0, i, does.clone());
        assert_eq!(
            now, "clock1 -trace1 -spark1 flare1 smoke1 -puff1",
            "{i} {does:?}"
        );
        assert_eq!(label, None);
    }
}

#[test]
fn removing_and_duplicating_take_the_whole_family() {
    let (list, open, label) = after(1, 0, PartAction::Remove);
    assert_eq!(list, "flare1 smoke1 -puff1");
    assert_eq!(open, "flare1");
    assert_eq!(
        label.as_deref(),
        Some("Remove Clock 1 and its 2 child parts from dash")
    );
    let (list, ..) = after(0, 1, PartAction::Remove);
    assert_eq!(list, "clock1 -spark1 flare1 smoke1 -puff1");
    let (list, open, label) = after(0, 4, PartAction::Duplicate);
    assert_eq!(
        list,
        "clock1 -trace1 -spark1 flare1 smoke1 -puff1 smoke1_copy -puff1_copy"
    );
    assert_eq!(open, "smoke1_copy");
    assert_eq!(
        label.as_deref(),
        Some("Duplicate Smoke 1 and its child part in dash")
    );
    let (list, ..) = after(0, 1, PartAction::Duplicate);
    assert_eq!(
        list,
        "clock1 -trace1 -trace1_copy -spark1 flare1 smoke1 -puff1"
    );
}

/// Taken out of its parent, a part goes after what that still makes: left in place, the parts
/// under it would have become its own.
#[test]
fn a_part_made_on_its_own_takes_none_of_its_parents_others() {
    let (list, _, label) = after(0, 1, PartAction::MakeOnItsOwn);
    assert_eq!(list, "clock1 -spark1 trace1 flare1 smoke1 -puff1");
    assert_eq!(
        label.as_deref(),
        Some("Make Clock 1 › Trace 1 on its own in dash")
    );
    let (list, _, label) = after(0, 3, PartAction::MakeFromAbove);
    assert_eq!(list, "clock1 -trace1 -spark1 -flare1 smoke1 -puff1");
    assert_eq!(label.as_deref(), Some("Make Flare 1 from Clock 1 in dash"));
    // A part that makes others cannot be made from one.
    let (list, ..) = after(0, 4, PartAction::MakeFromAbove);
    assert_eq!(list, "clock1 -trace1 -spark1 flare1 smoke1 -puff1");
}

/// The step an edit of a child part makes is named by both parts.
#[test]
fn steps_name_the_part_a_child_is_made_from() {
    let mut editor = families();
    let t0 = Instant::now();
    editor.history_begin_frame(false, t0);
    let em = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[1];
    set_attr(em, "particle_data.life", AttrValue::Int(30));
    editor.touch();
    editor.history_note_edit(t0);
    let label = editor.undo_label().unwrap();
    assert!(label.ends_with(" on Clock 1 › Trace 1 in dash"), "{label}");
    assert_eq!(
        editor.reset_part_label(0, 5),
        "Reset Smoke 1 › Puff 1 in dash"
    );
}

/// The hover's words are what the game does: it reads these bytes (Smash 0x8d70).
#[test]
fn a_childs_hover_says_what_the_game_does() {
    let mut em = part("trace1", 0, 1);
    assert_eq!(
        made_from_words(&em, "Clock 1"),
        "Made from Clock 1: its particles come out of each particle of Clock 1"
    );
    set_attr(
        &mut em,
        "child_inheritance.enable_emitter_particle",
        AttrValue::Int(1),
    );
    assert_eq!(
        made_from_words(&em, "Clock 1"),
        "Made from Clock 1: the game starts one for each of its particles"
    );
    set_attr(&mut em, "emission.timing", AttrValue::Int(30));
    set_attr(&mut em, "emission.is_one_time", AttrValue::Int(1));
    set_attr(&mut em, "emission.duration", AttrValue::Int(4));
    assert_eq!(
        made_from_words(&em, "Clock 1"),
        "Made from Clock 1: the game starts one for each of its particles, 30% into that \
         particle's life, for 4 frames"
    );
    use crate::eff_attrs::{blank_emitter_data, write, SSBU_VFX_VERSION};
    let base = blank_emitter_data(SSBU_VFX_VERSION);
    let before = base.write(SSBU_VFX_VERSION).unwrap();
    for (id, at) in [
        ("emission.is_one_time", 0x7f0),
        ("emission.is_emit_dist_enabled", 0x7f2),
        ("emission.timing", 0x7f8),
        ("emission.duration", 0x7fc),
    ] {
        let mut data = base.clone();
        assert!(write(&mut data, id, AttrValue::Int(1)), "{id}");
        let after = data.write(SSBU_VFX_VERSION).unwrap();
        let changed: Vec<usize> = (0..after.len())
            .filter(|&i| after[i] != before[i])
            .collect();
        assert_eq!(changed, [at], "{id}");
    }
}

/// A column for each part on its own with its children under it, wrapping whole.
#[test]
fn part_tabs_put_children_under_their_part() {
    let widths = [60.0, 50.0, 50.0, 60.0, 60.0, 50.0];
    let depths = [0, 1, 1, 0, 0, 1];
    let places = tab_places(&widths, &depths, 300.0, 4.0);
    assert_eq!((places.rows, places.lines), (1, 3));
    assert_eq!(
        places.at,
        [
            (0.0, 0),
            (INDENT, 1),
            (INDENT, 2),
            (70.0, 0),
            (134.0, 0),
            (134.0 + INDENT, 1)
        ]
    );
    // Too narrow for three columns: the third starts a row under the tallest.
    let places = tab_places(&widths, &depths, 140.0, 4.0);
    assert_eq!((places.rows, places.lines), (2, 5));
    assert_eq!(places.at[4..], [(0.0, 3), (INDENT, 4)]);
    // Parts on their own sit as they did: a row, wrapping.
    let places = tab_places(&[60.0; 4], &[0; 4], 130.0, 4.0);
    assert_eq!((places.rows, places.lines), (2, 2));
    assert_eq!(tab_lines_allowed(700.0, 21.0), 6);
    assert_eq!(tab_lines_allowed(100.0, 21.0), 2);
}

/// Everything the editor column draws in a panel `width` wide.
fn drawn(editor: &mut EffEditor, width: f32) -> Vec<egui::Shape> {
    let context = egui::Context::default();
    let mut shapes = Vec::new();
    for _ in 0..3 {
        let output = context.run_test_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 900.0),
                )),
                ..Default::default()
            },
            |ui| editor.draw_emitter_editor(ui),
        );
        shapes = output.shapes.into_iter().map(|s| s.shape).collect();
    }
    shapes
}

pub(in crate::eff_editor) fn texts(shapes: &[egui::Shape], out: &mut Vec<(String, egui::Rect)>) {
    for shape in shapes {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.job.text.clone(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(inner) => texts(inner, out),
            _ => {}
        }
    }
}

pub(in crate::eff_editor) fn text_rect(shapes: &[egui::Shape], wanted: &str) -> egui::Rect {
    let mut all = Vec::new();
    texts(shapes, &mut all);
    all.iter()
        .find(|(text, _)| text == wanted)
        .unwrap_or_else(|| {
            panic!(
                "no {wanted:?} in {:?}",
                all.iter().map(|t| &t.0).collect::<Vec<_>>()
            )
        })
        .1
}

/// How many lines of the tree are drawn that end at the left of `name`.
fn branches_to(shapes: &[egui::Shape], name: egui::Rect) -> usize {
    shapes
        .iter()
        .filter(|shape| {
            matches!(shape, egui::Shape::LineSegment { points, .. }
                if points[0].y == points[1].y
                    && (points[0].y - name.center().y).abs() < 3.0
                    && points.iter().all(|p| p.x < name.left() && p.x > name.left() - 30.0))
        })
        .count()
}

/// Both lists of the parts draw a child under its parent, a step in and joined to it, in a
/// narrow panel too.
#[test]
fn both_part_lists_draw_a_child_under_its_parent() {
    for width in [300.0, 460.0] {
        let mut editor = families();
        let shapes = drawn(&mut editor, width);
        let parent = text_rect(&shapes, "Clock 1 (2)");
        for (child, line) in [("Trace 1", 1.0), ("Spark 1", 2.0)] {
            let child = text_rect(&shapes, child);
            assert!(child.left() > parent.left() + 8.0);
            let down = child.center().y - parent.center().y;
            assert!((down - line * 21.0).abs() < 6.0, "{down}");
            assert_eq!(branches_to(&shapes, child), 1);
        }
        assert_eq!(branches_to(&shapes, parent), 0);
        let smoke = text_rect(&shapes, "Smoke 1 (1)");
        let puff = text_rect(&shapes, "Puff 1");
        assert!(puff.top() > smoke.bottom() - 4.0 && puff.left() > smoke.left() + 8.0);
        assert!(puff.right() <= width, "inside a {width} px panel");
        // The rest of the part row is there too, apart, and the row after it under them all.
        let (add, paste) = (text_rect(&shapes, "Add part"), text_rect(&shapes, "Paste"));
        assert!(add.right() <= width && paste.right() <= width);
        assert!((add.top() - paste.top()).abs() > 8.0 || paste.left() > add.right() + 8.0);
        let under = text_rect(&shapes, "Show only this part");
        assert!(under.top() > text_rect(&shapes, "Spark 1").bottom());
        assert!(under.top() > paste.bottom());

        editor.emitter_tab = EmitterTab::Spawning;
        let shapes = drawn(&mut editor, width);
        let parent = text_rect(&shapes, "Clock 1 (2)");
        let child = text_rect(&shapes, "Spark 1");
        assert!(child.left() > parent.left() + 8.0 && child.top() > parent.bottom());
        assert_eq!(branches_to(&shapes, child), 1);
        assert_eq!(branches_to(&shapes, parent), 0);
    }
}

/// An effect with too many parts for tabs picks them from a list, which names the open child by
/// both parts.
#[test]
fn the_part_picker_names_the_open_child_by_both_parts() {
    let mut parts = vec![("clock1", 0u8)];
    let names: Vec<String> = (0..16).map(|i| format!("trace{i}")).collect();
    parts.extend(names.iter().map(|name| (name.as_str(), 1)));
    let mut editor = editor_with(&[("dash", &parts)]);
    editor.selected_emitter = 3;
    let shapes = drawn(&mut editor, 460.0);
    text_rect(&shapes, "Clock 1 › Trace 2");
}

/// A child starts by its parent particle's life, which is the value the game reads for it: its
/// start frame means nothing.
#[test]
fn a_childs_start_is_a_share_of_its_parents_particles_life() {
    let mut editor = families();
    let em = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters;
    set_attr(&mut em[0], "emission.start", AttrValue::Int(6));
    set_attr(&mut em[1], "emission.start", AttrValue::Int(41));
    set_attr(&mut em[1], "emission.timing", AttrValue::Int(35));
    as_the_file_has_it(&mut editor);
    editor.emitter_tab = EmitterTab::Spawning;
    let shapes = drawn(&mut editor, 700.0);
    let (parent, child) = (
        text_rect(&shapes, "Clock 1 (2)"),
        text_rect(&shapes, "Trace 1"),
    );
    let mut all = Vec::new();
    texts(&shapes, &mut all);
    let on_row = |name: egui::Rect, text: &str| {
        all.iter()
            .any(|(t, rect)| t == text && (rect.center().y - name.center().y).abs() < 8.0)
    };
    assert!(on_row(parent, "7") && !on_row(child, "42"));
    assert!(on_row(child, "35") && on_row(child, "%") && !on_row(parent, "%"));
}

/// What a part takes from the one it is made from is open on a child and named there; on a part
/// on its own it is folded away, saying what is on.
#[test]
fn what_a_child_takes_shows_on_a_child_only() {
    let mut editor = families();
    for set in editor.ptcl.as_mut().unwrap().emitter_sets.iter_mut() {
        for em in set.emitters.iter_mut() {
            set_attr(em, "child_inheritance.scale", AttrValue::Int(1));
            set_attr(em, "child_inheritance.rotate", AttrValue::Int(1));
            // Drawn first: nothing taken.
            set_attr(em, "child_inheritance.pre_draw", AttrValue::Int(1));
        }
    }
    as_the_file_has_it(&mut editor);
    let count = crate::eff_attrs::table()
        .iter()
        .filter(|a| a.group == crate::eff_attrs::G_INHERIT)
        .count();
    let mut all = |selected: usize| {
        editor.selected_emitter = selected;
        // Tall enough for everything the settings scroll holds.
        let context = egui::Context::default();
        let mut out = Vec::new();
        for _ in 0..3 {
            let output = context.run_test_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(460.0, 4000.0),
                    )),
                    ..Default::default()
                },
                |ui| editor.draw_emitter_editor(ui),
            );
            out.clear();
            let shapes: Vec<_> = output.shapes.into_iter().map(|s| s.shape).collect();
            texts(&shapes, &mut out);
        }
        out.into_iter().map(|(text, _)| text).collect::<Vec<_>>()
    };
    let child = all(1);
    let has = |list: &[String], text: &str| list.iter().any(|t| t == text);
    assert!(
        has(&child, "Taken from Clock 1: Scale, Rotation"),
        "{child:?}"
    );
    assert!(has(&child, "Emitter per particle"), "its rows are open");
    assert!(has(&child, &format!("Taken from Clock 1  ({count})")));
    let alone = all(3);
    assert!(!alone.iter().any(|t| t.starts_with("Taken from")));
    assert!(!has(&alone, "Emitter per particle"));
    assert!(
        has(&alone, &format!("Only for a child part  ({count}, 3 on)")),
        "{alone:?}"
    );
}

#[test]
fn show_only_this_part_says_what_stays() {
    let tree = families().part_tree(0);
    assert!(tree.solo_hover(1).contains("Clock 1 still runs, not drawn"));
    assert!(tree
        .solo_hover(0)
        .ends_with("The parts made from this one stay: Trace 1, Spark 1."));
    assert!(!tree.solo_hover(3).contains("made from"));
}

/// The menu says what Remove takes with it and why a move or a copy cannot be done.
#[test]
fn the_menu_names_what_goes_with_a_part() {
    let editor = families();
    let tree = editor.part_tree(0);
    let labels = |part: usize| {
        let context = egui::Context::default();
        let mut renaming = None;
        let output = context.run_test_ui(Default::default(), |ui| {
            PartMenu {
                set_idx: 0,
                part,
                tree: &tree,
                file_name: "x",
                edited: false,
                clip: None,
                clip_blocked: None,
            }
            .show(ui, &mut renaming);
        });
        let shapes: Vec<_> = output.shapes.into_iter().map(|s| s.shape).collect();
        let mut all = Vec::new();
        texts(&shapes, &mut all);
        all.into_iter().map(|(text, _)| text).collect::<Vec<_>>()
    };
    let parent = labels(0);
    for label in [
        "Copy",
        "Paste",
        "Paste as child",
        "Duplicate",
        "Remove with its 2 child parts",
    ] {
        assert!(parent.iter().any(|t| t == label), "{label} in {parent:?}");
    }
    let child = labels(5);
    assert!(child.iter().any(|t| t == "Remove"));
    assert!(child.iter().any(|t| t == "Make on its own"));
    assert!(labels(3).iter().any(|t| t == "Make from Clock 1"));
}
