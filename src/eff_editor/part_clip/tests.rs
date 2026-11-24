use super::super::part_tree::tests::{editor_with, list};
use super::super::part_tree::PartAction;
use super::*;
use crate::ui::TestFrame;

const SIZE: &str = "particle_scale.scale_x";

fn size(editor: &EffEditor, set: usize, name: &str) -> f32 {
    let emitters = &editor.ptcl.as_ref().unwrap().emitter_sets[set].emitters;
    let em = emitters.iter().find(|e| e.name == name);
    attr_f32(em.unwrap_or_else(|| panic!("no {name}")), SIZE)
}

fn resize(editor: &mut EffEditor, set: usize, name: &str, to: f32) {
    let emitters = &mut editor.ptcl.as_mut().unwrap().emitter_sets[set].emitters;
    let em = emitters.iter_mut().find(|e| e.name == name).unwrap();
    set_attr(em, SIZE, AttrValue::Float(to));
    editor.touch();
}

/// "dash": a clock that makes a trace, and a flare. "jump": a ring.
fn two() -> EffEditor {
    editor_with(&[
        ("dash", &[("clock1", 0), ("trace1", 1), ("flare1", 0)]),
        ("jump", &[("ring1", 0)]),
    ])
}

fn index(editor: &EffEditor, set: usize, name: &str) -> usize {
    editor.ptcl.as_ref().unwrap().emitter_sets[set]
        .emitters
        .iter()
        .position(|e| e.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

fn copy(editor: &mut EffEditor, set: usize, name: &str) {
    let i = index(editor, set, name);
    editor.copy_part(set, i);
    assert!(editor.clip.is_some());
}

/// Paste with part `open` of set `set` open, as one undo step, and say how it went.
fn paste(editor: &mut EffEditor, set: usize, open: &str, as_child: bool) -> Result<String, String> {
    editor.select_entry(
        editor
            .entries
            .iter()
            .position(|e| e.set_idx == set)
            .unwrap(),
    );
    editor.selected_emitter = index(editor, set, open);
    let t0 = Instant::now();
    editor.history_begin_frame(false, t0);
    let (before, rev) = (list(editor, set), editor.edit_rev);
    editor.paste_part(set, as_child);
    if let Some(why) = editor.paste_error.clone() {
        assert_eq!(list(editor, set), before);
        assert_eq!(editor.edit_rev, rev);
        return Err(why);
    }
    assert_ne!(editor.edit_rev, rev, "the preview builds again");
    assert!(editor.is_unsent());
    editor.history_note_edit(t0);
    let (after, at) = (list(editor, set), editor.selected_emitter);
    let label = editor.undo_label().unwrap();
    editor.undo();
    assert_eq!(list(editor, set), before, "one step");
    editor.redo();
    assert_eq!(list(editor, set), after);
    editor.selected_emitter = at;
    Ok(label)
}

/// The saved project's edits of the open file, through its file.
type Saved = (
    Vec<crate::mod_project::EmitterRoster>,
    Vec<crate::mod_project::AuthoredEdit>,
);

fn saved(editor: &EffEditor) -> Saved {
    let json = serde_json::to_string(&(editor.collect_rosters(), editor.collect_authored_edits()));
    serde_json::from_str(&json.unwrap()).unwrap()
}

fn text(saved: &Saved) -> String {
    serde_json::to_string_pretty(saved).unwrap()
}

/// Copied with what is made from it and its edits, into the same effect: after the open part's
/// family, under names of its own, selected, as copies of the same file parts.
#[test]
fn a_family_pastes_into_its_own_effect_with_its_edits() {
    let mut editor = two();
    resize(&mut editor, 0, "clock1", 2.0);
    resize(&mut editor, 0, "trace1", 3.0);
    copy(&mut editor, 0, "clock1");
    // What is copied is what it was then.
    resize(&mut editor, 0, "clock1", 9.0);
    let label = paste(&mut editor, 0, "flare1", false).unwrap();
    assert_eq!(label, "Paste Clock 1 and its child part Trace 1 to dash");
    assert_eq!(list(&editor, 0), "clock1 -trace1 flare1 clock1_2 -trace1_2");
    assert_eq!(editor.selected_emitter, 3);
    assert_eq!(size(&editor, 0, "clock1_2"), 2.0);
    assert_eq!(size(&editor, 0, "trace1_2"), 3.0);
    assert_eq!(size(&editor, 0, "clock1"), 9.0);
    let slots = &editor.collect_rosters()[0].slots;
    assert!(slots.iter().all(|slot| !slot.is_foreign()));
    assert_eq!((slots[3].source_idx, slots[4].source_idx), (0, 1));

    let first = saved(&editor);
    let mut reloaded = two();
    reloaded.apply_structure_edits(&first.0, &[]);
    reloaded.apply_authored_edits(&first.1);
    assert_eq!(list(&reloaded, 0), list(&editor, 0));
    assert_eq!(size(&reloaded, 0, "trace1_2"), 3.0);
    assert_eq!(text(&saved(&reloaded)), text(&first));
}

/// Into another effect of the file: named for where it is from, and unmoved by what becomes of
/// the effect it came from.
#[test]
fn a_family_pastes_into_another_effect() {
    let mut editor = two();
    resize(&mut editor, 0, "trace1", 3.0);
    copy(&mut editor, 0, "clock1");
    editor.part_action(0, 0, PartAction::Remove);
    assert_eq!(list(&editor, 0), "flare1");
    paste(&mut editor, 1, "ring1", false).unwrap();
    assert_eq!(list(&editor, 1), "ring1 clock1 -trace1");
    assert_eq!(size(&editor, 1, "trace1"), 3.0);
    let rosters = editor.collect_rosters();
    let slot = &rosters[1].slots[2];
    assert_eq!(
        (
            slot.source_set.as_str(),
            slot.source_name.as_str(),
            slot.depth
        ),
        ("P_dash", "trace1", 1)
    );
    // Copied again from there, after the copy it came from is changed: still that file's part.
    resize(&mut editor, 1, "clock1", 5.0);
    copy(&mut editor, 1, "clock1");
    resize(&mut editor, 1, "clock1", 6.0);
    paste(&mut editor, 0, "flare1", false).unwrap();
    assert_eq!(list(&editor, 0), "flare1 clock1 -trace1");
    assert_eq!(size(&editor, 0, "clock1"), 5.0);
    assert_eq!(size(&editor, 0, "trace1"), 3.0);

    let first = saved(&editor);
    let mut reloaded = two();
    reloaded.apply_structure_edits(&first.0, &[]);
    reloaded.apply_authored_edits(&first.1);
    assert_eq!(list(&reloaded, 1), "ring1 clock1 -trace1");
    assert_eq!(text(&saved(&reloaded)), text(&first));
}

/// Where a pasted part goes, and what it is made from there.
#[test]
fn a_pasted_part_goes_after_the_open_one_or_under_it() {
    // A copied child stays one beside another child, and is on its own elsewhere.
    let mut editor = two();
    copy(&mut editor, 0, "trace1");
    paste(&mut editor, 0, "trace1", false).unwrap();
    assert_eq!(list(&editor, 0), "clock1 -trace1 -trace1_2 flare1");
    paste(&mut editor, 0, "clock1", false).unwrap();
    assert_eq!(list(&editor, 0), "clock1 -trace1 -trace1_2 trace1_3 flare1");
    // As asked, it is made from the open part.
    paste(&mut editor, 0, "flare1", true).unwrap();
    assert_eq!(
        list(&editor, 0),
        "clock1 -trace1 -trace1_2 trace1_3 flare1 -trace1_4"
    );
    assert_eq!(editor.collect_rosters()[0].slots[5].depth, 1);
    // Into an effect with no parts left.
    let mut empty = two();
    copy(&mut empty, 0, "clock1");
    empty.part_action(0, 2, PartAction::Remove);
    empty.part_action(0, 0, PartAction::Remove);
    assert_eq!(list(&empty, 0), "");
    empty.part_action(0, 0, PartAction::PasteAsChild);
    assert!(empty.paste_error.is_some());
    empty.part_action(0, 0, PartAction::Paste);
    assert_eq!(list(&empty, 0), "clock1 -trace1");
    // A copied part on its own never lands among children.
    copy(&mut editor, 0, "trace1_3");
    paste(&mut editor, 0, "trace1", false).unwrap();
    assert_eq!(
        list(&editor, 0),
        "clock1 -trace1 -trace1_2 trace1_3_2 trace1_3 flare1 -trace1_4"
    );
}

/// An edit still open is its own step: undoing the paste leaves it.
#[test]
fn an_edit_just_before_a_paste_is_its_own_step() {
    let mut editor = two();
    copy(&mut editor, 0, "flare1");
    let t0 = Instant::now();
    editor.history_begin_frame(false, t0);
    resize(&mut editor, 0, "clock1", 2.0);
    editor.history_note_edit(t0);
    editor.history_begin_frame(false, t0);
    editor.paste_part(0, false);
    editor.history_note_edit(t0);
    assert_eq!(list(&editor, 0), "clock1 -trace1 flare1_2 flare1");
    editor.undo();
    assert_eq!(list(&editor, 0), "clock1 -trace1 flare1");
    assert_eq!(size(&editor, 0, "clock1"), 2.0);
    assert!(editor.can_undo(), "the edit before it");
}

/// One level of child parts, sixteen of them, and the reason said on the greyed button.
#[test]
fn pasting_stops_at_what_the_game_plays() {
    let mut editor = two();
    copy(&mut editor, 0, "clock1");
    let why = paste(&mut editor, 0, "flare1", true).unwrap_err();
    assert_eq!(
        why,
        "Clock 1 has child parts of its own, and the game plays one level of them"
    );
    let tree = editor.part_tree(0);
    let state = paste_state(editor.clip.as_deref(), None, &tree, 2, true);
    assert!(state.unwrap_err().ends_with(&why));
    // Said until another effect is open.
    assert!(editor.paste_error.is_some());
    editor.select_entry(1);
    assert!(editor.paste_error.is_none());
    editor.select_entry(0);
    copy(&mut editor, 0, "flare1");
    let why = paste(&mut editor, 0, "trace1", true).unwrap_err();
    assert_eq!(
        why,
        "Trace 1 is itself made from Clock 1, and the game plays one level of child parts"
    );
    for _ in 1..crate::eff_validate::MAX_CHILDREN {
        paste(&mut editor, 0, "clock1", true).unwrap();
    }
    let why = paste(&mut editor, 0, "clock1", true).unwrap_err();
    assert_eq!(why, "Clock 1 has 16 child parts, the most the game plays");
    // On its own there is always room.
    paste(&mut editor, 0, "clock1", false).unwrap();
}

#[test]
fn pastes_hover_says_what_is_copied_and_where_it_goes() {
    let mut editor = two();
    let tree = editor.part_tree(0);
    assert!(paste_state(None, None, &tree, 0, false)
        .unwrap_err()
        .starts_with("No part is copied."));
    copy(&mut editor, 0, "clock1");
    let clip = editor.clip.clone();
    assert_eq!(
        paste_state(clip.as_deref(), None, &tree, 0, false).unwrap(),
        "Clock 1 and its child part Trace 1 from dash in ef_test.eff\nGoes after Clock 1 › \
         Trace 1 (Ctrl+V)"
    );
    copy(&mut editor, 0, "trace1");
    let clip = editor.clip.clone();
    assert_eq!(
        paste_state(clip.as_deref(), None, &tree, 2, true).unwrap(),
        "Trace 1 from dash in ef_test.eff\nGoes under Flare 1, made from it"
    );
    assert_eq!(
        paste_state(clip.as_deref(), Some("why"), &tree, 2, false).unwrap_err(),
        "Trace 1 from dash in ef_test.eff\nwhy"
    );
}

/// One frame of the page's keys, with `events`, and the text it put on the clipboard.
fn keys(
    editor: &mut EffEditor,
    context: &egui::Context,
    events: Vec<egui::Event>,
    typing: bool,
) -> Option<String> {
    let mut words = "some text".to_owned();
    let output = context.run_test_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ui| {
            let field = ui.text_edit_singleline(&mut words);
            if typing {
                field.request_focus();
            }
            editor.part_keys(ui.ctx());
            editor.publish_clip(ui.ctx());
        },
    );
    output
        .platform_output
        .commands
        .iter()
        .find_map(|c| match c {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
}

/// Ctrl+C puts the open part on the clipboard and Ctrl+V pastes it, in a text box's place only
/// when none has focus; the clipboard's text is enough after a restart.
#[test]
fn the_keys_copy_and_paste_and_outlive_a_restart() {
    let context = egui::Context::default();
    let mut editor = two();
    resize(&mut editor, 0, "trace1", 3.0);
    for _ in 0..2 {
        keys(&mut editor, &context, Vec::new(), true);
    }
    keys(&mut editor, &context, vec![egui::Event::Copy], true);
    assert!(editor.clip.is_none(), "the text box's own keys");

    let context = egui::Context::default();
    let copied = keys(&mut editor, &context, vec![egui::Event::Copy], false)
        .expect("on the system clipboard");
    assert_eq!(keys(&mut editor, &context, Vec::new(), false), None, "once");
    // Other text on the clipboard: the part copied here.
    let other = vec![egui::Event::Paste("flare".into())];
    keys(&mut editor, &context, other, false);
    assert_eq!(list(&editor, 0), "clock1 -trace1 clock1_2 -trace1_2 flare1");

    let mut restarted = two();
    let paste = vec![egui::Event::Paste(copied.clone())];
    keys(&mut restarted, &context, paste, false);
    assert_eq!(
        list(&restarted, 0),
        "clock1 -trace1 clock1_2 -trace1_2 flare1"
    );
    assert_eq!(size(&restarted, 0, "trace1_2"), 3.0);

    // Text that is no part, or one that reads files it should not, is left alone.
    for bad in [
        "{\"visionary_part\":1}".to_owned(),
        copied.replace("effect/fighter/test/ef_test.eff", "../../secret.eff"),
        copied.replace("\"visionary_part\":1", "\"visionary_part\":99"),
        copied.replace("\"name\":\"trace1\"", "\"name\":\"tr\u{e4}ce\""),
    ] {
        assert!(PartClip::read(&bad).is_none(), "{bad}");
    }
    assert!(PartClip::read(&copied).is_some());
}

/// The Paste button on the part row runs the same paste.
#[test]
fn a_part_pasted_from_the_part_row_is_the_copied_one() {
    let mut editor = two();
    copy(&mut editor, 0, "flare1");
    let context = egui::Context::default();
    let mut frame = |events: Vec<egui::Event>| {
        context.run_test_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(460.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ui| editor.draw_emitter_editor(ui),
        )
    };
    frame(Vec::new());
    let output = frame(Vec::new());
    let at = output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == "Paste" => {
                Some(t.pos + t.galley.size() / 2.0)
            }
            _ => None,
        })
        .expect("Paste on the part row");
    for pressed in [true, false] {
        frame(vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }
    assert_eq!(list(&editor, 0), "clock1 -trace1 flare1_2 flare1");
}

// ── The game's own files ────────────────────────────────────────────────────────

const BAYONETTA: &str = "effect/fighter/bayonetta/ef_bayonetta.eff";
const MARIO: &str = "effect/fighter/mario/ef_mario.eff";
const LUIGI: &str = "effect/fighter/luigi/ef_luigi.eff";

fn game_root() -> Option<PathBuf> {
    let root = std::env::var_os("VISIONARY_EFF_ROOT").map(PathBuf::from);
    if root.is_none() {
        eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
    }
    root
}

/// Open effect `entry` of the game's `rel`.
fn open(editor: &mut EffEditor, root: &Path, rel: &str, entry: &str) -> usize {
    editor.export_root = root.to_path_buf();
    editor.load_eff(&root.join(rel));
    editor.selected_entry = editor.entries.iter().position(|e| e.name == entry);
    editor.entries[editor.selected_entry.expect(entry)].set_idx
}

/// The file export writes, and that file read again.
struct Shipped {
    bytes: Vec<u8>,
    read: crate::effects::LoadedEffect,
}

impl Shipped {
    /// The part `name` of set `set`: its depth, the texture it samples first and its size.
    fn part(&self, set: &str, name: &str) -> Option<(u8, String, f32)> {
        let sets = &self.read.ptcl.emitter_sets;
        let set = sets.iter().find(|s| s.name == set)?;
        let em = set.emitters.iter().find(|e| e.name == name)?;
        let texture = &self.read.ptcl.bntx_textures[em.texture_index? as usize];
        Some((em.depth, texture.tex_name.clone(), attr_f32(em, SIZE)))
    }
}

/// Export the open file's edits, as the project saves them.
fn exported(editor: &EffEditor, root: &Path, rel: &str) -> Shipped {
    let (rosters, authored) = saved(editor);
    let eff = crate::mod_project::EffMod {
        source_rel: rel.into(),
        rosters,
        authored,
        ..Default::default()
    };
    let src = std::fs::read(root.join(rel)).unwrap();
    let bytes = crate::eff_export::rebuild_eff_bytes(&src, &eff, Some(root)).expect("export");
    assert!(crate::eff_validate::problems(&bytes).is_empty());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.eff");
    std::fs::write(&path, &bytes).unwrap();
    let read = load_effect(&path).expect("loads");
    Shipped { bytes, read }
}

/// Bayonetta's Witch Time clock hand and the trail made from it, edited, pasted into Mario's
/// fireball: shown with their textures, saved, opened again and exported as a file that keeps
/// every game-file rule, the trail still made from the hand. The same from the clipboard's text
/// alone, into an editor that never opened Bayonetta's file.
#[test]
fn a_family_pastes_into_another_characters_file() {
    let Some(root) = game_root() else { return };
    let mut editor = EffEditor::default();
    let clock = open(&mut editor, &root, BAYONETTA, "bayonetta_witchtime_clock");
    assert_eq!(list(&editor, clock).split(' ').next(), Some("clock1"));
    let texture = |editor: &EffEditor, set: usize, name: &str| {
        let ptcl = editor.ptcl.as_ref().unwrap();
        let em = &ptcl.emitter_sets[set].emitters[index(editor, set, name)];
        ptcl.bntx_textures[em.texture_index.unwrap() as usize]
            .tex_name
            .clone()
    };
    let wanted = (
        texture(&editor, clock, "clock1"),
        texture(&editor, clock, "trace1"),
    );
    resize(&mut editor, clock, "clock1", 2.5);
    resize(&mut editor, clock, "trace1", 0.75);
    copy(&mut editor, clock, "clock1");
    let on_clipboard = editor.clip_publish.clone().unwrap();
    // Changed after: the copy is what it was.
    resize(&mut editor, clock, "trace1", 8.0);

    let set = open(&mut editor, &root, MARIO, "mario_fb_shoot");
    let set_name = editor.ptcl.as_ref().unwrap().emitter_sets[set].name.clone();
    assert_eq!(editor.clip_blocked(), None);
    let first = editor.ptcl.as_ref().unwrap().emitter_sets[set].emitters[0]
        .name
        .clone();
    paste(&mut editor, set, &first, false).unwrap();
    assert!(list(&editor, set).contains(" clock1 -trace1 "));
    assert_eq!(texture(&editor, set, "clock1"), wanted.0);
    assert_eq!(texture(&editor, set, "trace1"), wanted.1);
    assert_eq!(size(&editor, set, "clock1"), 2.5);
    let first = saved(&editor);
    let slot = first.0[0]
        .slots
        .iter()
        .find(|s| s.name == "trace1")
        .unwrap();
    assert_eq!((slot.source_file.as_str(), slot.depth), (BAYONETTA, 1));

    let shipped = exported(&editor, &root, MARIO);
    assert_eq!(
        shipped.part(&set_name, "clock1"),
        Some((0, wanted.0.clone(), 2.5))
    );
    assert_eq!(
        shipped.part(&set_name, "trace1"),
        Some((1, wanted.1.clone(), 0.75))
    );
    // Each draws with the shader it has in its own file.
    let program = |bytes: &[u8], set: &str, part: &str| {
        let file = effect_library::NamcoEffectFile::load(bytes).unwrap();
        crate::eff_export::tests::part_program(&file, set, part)
    };
    let bayonetta = std::fs::read(root.join(BAYONETTA)).unwrap();
    let home = {
        let file = effect_library::NamcoEffectFile::load(&bayonetta).unwrap();
        file.ptcl_file.unwrap().emitter_list.emitter_sets[clock]
            .name
            .clone()
    };
    for part in ["clock1", "trace1"] {
        assert_eq!(
            program(&shipped.bytes, &set_name, part),
            program(&bayonetta, &home, part),
            "{part}"
        );
    }

    let mut reloaded = EffEditor::default();
    open(&mut reloaded, &root, MARIO, "mario_fb_shoot");
    reloaded.apply_structure_edits(&first.0, &[]);
    reloaded.apply_authored_edits(&first.1);
    assert_eq!(list(&reloaded, set), list(&editor, set));
    assert_eq!(texture(&reloaded, set, "trace1"), wanted.1);
    assert_eq!(text(&saved(&reloaded)), text(&first));

    let mut other = EffEditor::default();
    open(&mut other, &root, LUIGI, "luigi_fb_shoot");
    other.selected_emitter = 0;
    let context = egui::Context::default();
    keys(
        &mut other,
        &context,
        vec![egui::Event::Paste(on_clipboard)],
        false,
    );
    assert_eq!(other.paste_error, None);
    let set = other.entries[other.selected_entry.unwrap()].set_idx;
    assert!(list(&other, set).contains("clock1 -trace1"));
    let set_name = other.ptcl.as_ref().unwrap().emitter_sets[set].name.clone();
    let shipped = exported(&other, &root, LUIGI);
    assert_eq!(shipped.part(&set_name, "trace1"), Some((1, wanted.1, 0.75)));
}

/// A part whose texture was changed to another of its file's takes that one along: into the
/// export and the live carrier, and again when it is copied on to a third file.
#[test]
fn a_changed_texture_goes_with_the_part() {
    let Some(root) = game_root() else { return };
    let mut editor = EffEditor::default();
    let set = open(&mut editor, &root, MARIO, "mario_fb_shoot");
    let luigi = load_effect(&root.join(LUIGI)).unwrap();
    let pool = editor.ptcl.as_ref().unwrap().bntx_textures.clone();
    // One of Mario's that Luigi's file does not have.
    let at = pool
        .iter()
        .position(|t| {
            let mut theirs = luigi.ptcl.bntx_textures.iter();
            t.convertible && !theirs.any(|o| o.tex_name == t.tex_name || o.id == t.id)
        })
        .expect("a texture only Mario has");
    let wanted = pool[at].tex_name.clone();
    editor.selected_emitter = 0;
    editor.point_slot_at(0, at);
    let name = editor.ptcl.as_ref().unwrap().emitter_sets[set].emitters[0]
        .name
        .clone();
    copy(&mut editor, set, &name);

    // In its own file the edit says so.
    paste(&mut editor, set, &name, false).unwrap();
    let mine = saved(&editor);
    assert!(mine.0[0].slots.iter().all(|s| s.textures.is_empty()));
    let swaps = mine.1.iter().filter(|e| e.fields.texture_name.is_some());
    assert_eq!(swaps.count(), 2);

    let into = open(&mut editor, &root, LUIGI, "luigi_fb_shoot");
    let into_name = editor.ptcl.as_ref().unwrap().emitter_sets[into]
        .name
        .clone();
    let open_part = editor.ptcl.as_ref().unwrap().emitter_sets[into].emitters[0]
        .name
        .clone();
    paste(&mut editor, into, &open_part, false).unwrap();
    let pasted =
        editor.ptcl.as_ref().unwrap().emitter_sets[into].emitters[editor.selected_emitter].clone();
    let shown =
        &editor.ptcl.as_ref().unwrap().bntx_textures[pasted.texture_index.unwrap() as usize];
    assert_eq!(shown.tex_name, wanted);
    // The part's own there, so its picker offers it.
    let own = editor.pristine[into][pasted.source_idx].clone();
    let at = pasted.texture_index.unwrap() as usize;
    assert!(!editor.texture_belongs_to_added_part(at, Some(&own)));
    let first = saved(&editor);
    let slot = first.0[0].slots.iter().find(|s| s.name == pasted.name);
    assert_eq!(slot.unwrap().textures[0].name, wanted);
    assert!(first.1.iter().all(|e| e.fields.texture_name.is_none()));
    let shipped = exported(&editor, &root, LUIGI);
    assert_eq!(shipped.part(&into_name, &pasted.name).unwrap().1, wanted);

    let mut reloaded = EffEditor::default();
    open(&mut reloaded, &root, LUIGI, "luigi_fb_shoot");
    reloaded.apply_structure_edits(&first.0, &[]);
    reloaded.apply_authored_edits(&first.1);
    assert_eq!(text(&saved(&reloaded)), text(&first));
    let again = &reloaded.ptcl.as_ref().unwrap().emitter_sets[into].emitters
        [index(&reloaded, into, &pasted.name)];
    let shown =
        &reloaded.ptcl.as_ref().unwrap().bntx_textures[again.texture_index.unwrap() as usize];
    assert_eq!(shown.tex_name, wanted);

    // On to a third file.
    copy(&mut editor, into, &pasted.name);
    let third = open(&mut editor, &root, BAYONETTA, "bayonetta_witchtime_clock");
    let third_name = editor.ptcl.as_ref().unwrap().emitter_sets[third]
        .name
        .clone();
    paste(&mut editor, third, "clock1", false).unwrap();
    let name = editor.ptcl.as_ref().unwrap().emitter_sets[third].emitters[editor.selected_emitter]
        .name
        .clone();
    let shipped = exported(&editor, &root, BAYONETTA);
    assert_eq!(shipped.part(&third_name, &name).unwrap().1, wanted);

    // The live carrier holds it too, past the prune of what nothing samples.
    let entry = "luigi_fb_shoot";
    let clone = format!("{}{entry}", crate::mod_project::EDIT_CLONE_PREFIX);
    let op = crate::mod_project::TransplantOp {
        new_entry_name: clone.clone(),
        src_file_rel: LUIGI.into(),
        src_set_name: entry.into(),
        src_set_idx: into,
        one_slot_slots: Vec::new(),
        replace_entry: None,
    };
    const CARRIER: &str = "effect/assist/bomberman/ef_bomberman.eff";
    let mut warnings = Vec::new();
    let carrier = crate::eff_export::rebuild_runtime_carrier_eff_bytes_with_edits(
        &std::fs::read(root.join(CARRIER)).unwrap(),
        CARRIER,
        &[op],
        &root,
        &[crate::eff_export::CarrierAuthored {
            set_name: clone.clone(),
            edits: first.1.clone(),
            roster: Some(first.0[0].clone()),
            entry_edit: None,
        }],
        &[],
        &[],
        &[],
        &mut warnings,
    )
    .expect("carrier");
    assert!(warnings.is_empty(), "{warnings:?}");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("c.eff"), carrier).unwrap();
    let carrier = load_effect(&dir.path().join("c.eff")).unwrap();
    let live = carrier
        .ptcl
        .emitter_sets
        .iter()
        .find(|s| s.name == clone)
        .and_then(|s| s.emitters.iter().find(|e| e.name == pasted.name))
        .expect("the pasted part");
    assert_eq!(
        carrier.ptcl.bntx_textures[live.texture_index.unwrap() as usize].tex_name,
        wanted
    );

    // A saved part whose texture is gone is kept, shown as no other, and not exported without it.
    let mut gone = first.clone();
    let slot = gone.0[0].slots.iter_mut().find(|s| !s.textures.is_empty());
    slot.unwrap().textures[0].name = "no_such_texture".into();
    let mut editor = EffEditor::default();
    open(&mut editor, &root, LUIGI, "luigi_fb_shoot");
    editor.apply_structure_edits(&gone.0, &[]);
    assert!(!list(&editor, into).contains(&format!(" {} ", pasted.name)));
    assert_eq!(editor.collect_rosters(), gone.0);
    let eff = crate::mod_project::EffMod {
        source_rel: LUIGI.into(),
        rosters: gone.0,
        ..Default::default()
    };
    let src = std::fs::read(root.join(LUIGI)).unwrap();
    let error = crate::eff_export::rebuild_eff_bytes(&src, &eff, Some(&root)).unwrap_err();
    assert!(error.to_string().contains("no_such_texture"), "{error}");
}

/// What names a thing of one file only keeps the part in it, and the greyed Paste says what.
#[test]
fn a_part_that_cannot_leave_its_file_says_why() {
    let Some(root) = game_root() else { return };
    let mut editor = EffEditor::default();
    let set = open(&mut editor, &root, MARIO, "mario_fb_shoot");
    let name = editor.ptcl.as_ref().unwrap().emitter_sets[set].emitters[0]
        .name
        .clone();
    let em = &mut editor.ptcl.as_mut().unwrap().emitter_sets[set].emitters[0];
    set_attr(em, "shader_references.shader_index", AttrValue::Int(3));
    copy(&mut editor, set, &name);
    paste(&mut editor, set, &name, false).expect("in its own file");
    open(&mut editor, &root, LUIGI, "luigi_fb_shoot");
    let why = "Its shader was changed to one of ef_mario.eff";
    assert_eq!(editor.clip_blocked().as_deref(), Some(why));
    editor.paste_part(0, false);
    assert_eq!(editor.paste_error.as_deref(), Some(why));

    // A texture the author added is in no file yet.
    let set = open(&mut editor, &root, MARIO, "mario_fb_shoot");
    editor.selected_emitter = 0;
    let pool = editor.texture_pool.clone().unwrap();
    let names = editor.texture_names();
    let first = editor.ptcl.as_ref().unwrap().bntx_textures[0].clone();
    let added = crate::texture_import::unique_texture_name(&names, &first.tex_name);
    let pool = crate::texture_import::duplicate_texture(&pool, &names, 0, &added).unwrap();
    let info = TextureInfo {
        id: None,
        tex_name: added.clone(),
        ..first
    };
    editor.adopt_appended_texture(pool, info);
    copy(&mut editor, set, &name);
    open(&mut editor, &root, LUIGI, "luigi_fb_shoot");
    assert_eq!(
        editor.clip_blocked(),
        Some(format!("Its texture {added} was added to ef_mario.eff"))
    );

    // The file it is from, gone.
    open(&mut editor, &root, MARIO, "mario_fb_shoot");
    copy(&mut editor, set, &name);
    editor.export_root = PathBuf::from("/nowhere");
    editor.loaded_path = Some(PathBuf::from("/fx/ef_luigi.eff"));
    assert_eq!(
        editor.clip_blocked().as_deref(),
        Some("ef_mario.eff was not found")
    );
}
