//! What the running game's effect carrier holds: every character's edited and borrowed effects,
//! each reaching only its own costumes.

use std::collections::{BTreeSet, HashMap};

use crate::app::{carrier_stored_name, effect_name_hash, rides_the_carrier};
use crate::eff_editor::SendOutcome;
use crate::eff_export::CarrierAuthored;
use crate::game_link::{EffectAliasWire, LiveGimmickAgent};
use crate::mod_project::{
    AuthoredEdit, EffMod, EmitterRoster, EntryEdit, TextureAddition, TextureImport, TransplantOp,
    EDIT_CLONE_PREFIX, EDIT_COPY_PREFIX,
};

/// An effect of a file, and its set.
pub type Effect = (String, usize);

/// One character's part of the carrier.
#[derive(Default)]
pub struct Share {
    pub owner: String,
    /// The fighter the game runs for it.
    pub fighter: String,
    /// What is copied in, each under the name the carrier keeps it by.
    pub ops: Vec<TransplantOp>,
    pub authored: Vec<CarrierAuthored>,
    pub aliases: Vec<EffectAliasWire>,
    pub textures: Vec<TextureImport>,
    pub new_textures: Vec<TextureAddition>,
    pub gone_textures: Vec<String>,
    /// Edits to an effect that has the name of a borrowed one.
    pub conflicts: Vec<String>,
}

/// What a character's share is made from.
pub struct ShareOf<'a> {
    pub owner: &'a str,
    pub fighter: &'a str,
    pub eff: &'a EffMod,
    /// The file its edits are made on, as a lowercase game path.
    pub source_rel: &'a str,
    /// The costumes its edits reach; empty for all.
    pub costumes: &'a [u8],
    /// The costumes one of its borrowed effects reaches.
    pub borrowed_costumes: &'a dyn Fn(&TransplantOp) -> Vec<u8>,
    /// The effects of its file that use a texture, and their sets.
    pub using_texture: &'a dyn Fn(&str) -> Vec<Effect>,
    /// Effects the running game loaded changed that the character no longer changes.
    pub restore: &'a BTreeSet<String>,
}

type Edits = (
    usize,
    Vec<AuthoredEdit>,
    Option<EmitterRoster>,
    Option<EntryEdit>,
);

/// What `eff` changes about each effect of its file, by the effect's lowercase name. Edits with no
/// effect recorded (projects saved before `entry_name` existed) are skipped rather than guessed:
/// a set's name is not an effect's, and a carrier naming one that does not exist can hang the
/// game's loader.
fn edited_effects(eff: &EffMod) -> Vec<(String, Edits)> {
    let mut by_entry: Vec<(String, Edits)> = Vec::new();
    let mut slot = |name: &str, set_idx: usize, put: &mut dyn FnMut(&mut Edits)| {
        let name = name.to_lowercase();
        if name.is_empty() {
            return;
        }
        let at = by_entry.iter().position(|(entry, _)| *entry == name);
        let at = at.unwrap_or_else(|| {
            by_entry.push((name, (set_idx, Vec::new(), None, None)));
            by_entry.len() - 1
        });
        put(&mut by_entry[at].1);
    };
    for edit in &eff.authored {
        slot(&edit.entry_name, edit.set_idx, &mut |edits| {
            edits.1.push(edit.clone())
        });
    }
    for roster in &eff.rosters {
        slot(&roster.entry_name, roster.set_idx, &mut |edits| {
            edits.2 = Some(roster.clone())
        });
    }
    for header in &eff.entry_edits {
        slot(&header.entry_name, 0, &mut |edits| {
            edits.3 = Some(header.clone())
        });
    }
    by_entry
}

/// The effects `eff` changes in the file the game loads for its fighter, by lowercase name.
pub fn changed_effects(
    eff: &EffMod,
    using_texture: &dyn Fn(&str) -> Vec<Effect>,
) -> BTreeSet<String> {
    let edited = edited_effects(eff).into_iter().map(|(name, _)| name);
    let replaced = eff.transplants.iter().filter_map(|op| {
        let name = op.replace_entry.as_ref()?;
        Some(name.to_lowercase())
    });
    let textures = eff.textures.iter().map(|import| &import.texture_name);
    let shown = textures.flat_map(|name| using_texture(name));
    edited
        .chain(replaced)
        .chain(shown.map(|(name, _)| name.to_lowercase()))
        .collect()
}

impl Share {
    pub fn new(from: &ShareOf) -> Self {
        let eff = from.eff;
        let own_prefix = format!("effect/fighter/{}/", from.fighter);
        // A costume's effects have the names of its fighter's; a fighter's are its own.
        let tag = match from.owner.split_once('#') {
            Some((_, costume)) => format!("{costume}_"),
            None => String::new(),
        };
        let mut share = Self {
            owner: from.owner.to_owned(),
            fighter: from.fighter.to_owned(),
            textures: eff.textures.clone(),
            new_textures: eff.textures_added.clone(),
            gone_textures: eff.textures_removed.clone(),
            ..Self::default()
        };
        let edited = edited_effects(eff);
        // Names in the carrier of what was borrowed, edited ones only.
        let mut borrowed: Vec<(&TransplantOp, String)> = Vec::new();
        for op in &eff.transplants {
            let lands = |name: &str| {
                op.new_entry_name.eq_ignore_ascii_case(name)
                    || op
                        .replace_entry
                        .as_deref()
                        .is_some_and(|replaced| replaced.eq_ignore_ascii_case(name))
            };
            let is_edited = edited.iter().any(|(name, _)| lands(name));
            let mut stored = op.src_set_name.to_lowercase();
            // A copy from its own file is the game's own effect until it is edited.
            if is_edited || rides_the_carrier(&op.src_file_rel) {
                let mut placed = op.clone();
                if placed.src_file_rel.is_empty() {
                    placed.src_file_rel = from.source_rel.to_owned();
                }
                let name = carrier_stored_name(&placed, Some(&own_prefix));
                stored = match name == stored {
                    true => name,
                    false => format!("{tag}{name}"),
                };
                share.put(&stored, &placed.src_file_rel, op);
            }
            let to = match is_edited {
                true => stored.clone(),
                false => op.src_set_name.to_lowercase(),
            };
            if is_edited {
                borrowed.push((op, stored));
            }
            let name = op.replace_entry.as_ref().unwrap_or(&op.new_entry_name);
            let (from_hash, to) = (effect_name_hash(name), effect_name_hash(&to));
            if from_hash != to {
                let wire = EffectAliasWire {
                    from: from_hash,
                    to,
                    slots: (from.borrowed_costumes)(op),
                };
                if !share.aliases.contains(&wire) {
                    share.aliases.push(wire);
                }
            }
        }
        let clone_of = |name: &str| format!("{EDIT_CLONE_PREFIX}{tag}{name}");
        let mut redirects: Vec<Effect> = Vec::new();
        for (name, (set_idx, edits, roster, entry_edit)) in edited {
            // An edited copy is put in from its donor under the copy's name, and its alias already
            // points there; the fighter's own file has no such effect to clone.
            let copy = borrowed.iter().find(|(op, _)| {
                op.new_entry_name.eq_ignore_ascii_case(&name)
                    || op
                        .replace_entry
                        .as_deref()
                        .is_some_and(|replaced| replaced.eq_ignore_ascii_case(&name))
            });
            let set_name = match copy {
                Some((_, stored)) => stored.clone(),
                None => {
                    redirects.push((name.clone(), set_idx));
                    clone_of(&name)
                }
            };
            share.authored.push(CarrierAuthored {
                set_name,
                edits,
                roster,
                entry_edit,
            });
        }
        // A replaced texture brings every effect that uses it: the carrier's pool only holds the
        // textures its own effects reference.
        for import in &share.textures {
            redirects.extend((from.using_texture)(&import.texture_name));
        }
        // The game's own again, where it loaded a changed one with the fighter.
        redirects.extend(from.restore.iter().map(|name| (name.clone(), 0)));
        for (name, set_idx) in redirects {
            let name = name.to_lowercase();
            let clone = clone_of(&name);
            let rides = |op: &TransplantOp| {
                op.new_entry_name == clone
                    || (op.src_file_rel.eq_ignore_ascii_case(from.source_rel)
                        && op.src_set_name.eq_ignore_ascii_case(&name))
            };
            if share.ops.iter().any(rides) {
                continue;
            }
            let op = TransplantOp {
                src_set_name: name.clone(),
                src_set_idx: set_idx,
                new_entry_name: clone.clone(),
                src_file_rel: String::new(),
                one_slot_slots: Vec::new(),
                replace_entry: None,
            };
            share.put(&clone, from.source_rel, &op);
            let wire = EffectAliasWire {
                from: effect_name_hash(&name),
                to: effect_name_hash(&clone),
                slots: from.costumes.to_vec(),
            };
            // One name leads to one effect on a costume. A borrowed copy keeps it.
            if share.aliases.iter().any(|alias| clash(alias, &wire)) {
                share.conflicts.push(name);
            } else {
                share.aliases.push(wire);
            }
        }
        share
    }

    fn put(&mut self, stored: &str, src_file_rel: &str, op: &TransplantOp) {
        if self.ops.iter().any(|op| op.new_entry_name == stored) {
            return;
        }
        self.ops.push(TransplantOp {
            new_entry_name: stored.to_owned(),
            src_file_rel: src_file_rel.to_lowercase(),
            src_set_name: op.src_set_name.to_lowercase(),
            src_set_idx: op.src_set_idx,
            one_slot_slots: Vec::new(),
            replace_entry: None,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
            && self.textures.is_empty()
            && self.new_textures.is_empty()
            && self.gone_textures.is_empty()
    }

    /// The textures it changes: the carrier has one pool, so each name is one picture in it.
    pub fn changed_textures(&self) -> impl Iterator<Item = &str> {
        let replaced = self.textures.iter().map(|t| t.texture_name.as_str());
        let added = self.new_textures.iter().map(|t| t.texture_name.as_str());
        let gone = self.gone_textures.iter().map(String::as_str);
        replaced.chain(added).chain(gone)
    }

    fn rename(&mut self, from: &str, to: &str) {
        for op in self.ops.iter_mut().filter(|op| op.new_entry_name == from) {
            op.new_entry_name = to.to_owned();
        }
        for entry in self.authored.iter_mut().filter(|e| e.set_name == from) {
            entry.set_name = to.to_owned();
        }
        let (from, to) = (effect_name_hash(from), effect_name_hash(to));
        for alias in self.aliases.iter_mut().filter(|alias| alias.to == from) {
            alias.to = to;
        }
    }
}

/// Whether two redirects send one effect of one costume to different places.
fn clash(a: &EffectAliasWire, b: &EffectAliasWire) -> bool {
    let shared = a.slots.is_empty()
        || b.slots.is_empty()
        || a.slots.iter().any(|slot| b.slots.contains(slot));
    a.from == b.from && a.to != b.to && shared
}

/// The shares that go into one carrier, in an order that does not change with the open character,
/// and the owners of those that can't go in with them: `first`'s always does. `uses(share, name)`
/// says whether a share's effects use a texture.
pub fn together(
    mut shares: Vec<Share>,
    first: Option<&str>,
    uses: &dyn Fn(&Share, &str) -> bool,
) -> (Vec<Share>, Vec<String>) {
    let mut taken: Vec<Share> = Vec::new();
    let mut left = Vec::new();
    shares.sort_by_key(|share| (Some(share.owner.as_str()) != first, share.owner.clone()));
    for share in shares {
        let fits = taken.iter().all(|other| {
            let picture = |a: &Share, b: &Share| {
                a.changed_textures()
                    .any(|name| b.changed_textures().any(|n| n == name) || uses(b, name))
            };
            !picture(other, &share)
                && !picture(&share, other)
                && !share
                    .aliases
                    .iter()
                    .any(|alias| other.aliases.iter().any(|theirs| clash(alias, theirs)))
        });
        match fits {
            true => taken.push(share),
            false => left.push(share.owner),
        }
    }
    taken.sort_by(|a, b| a.owner.cmp(&b.owner));
    // Copies of one donor are one copy, unless somebody edited theirs.
    let mut names: HashMap<String, (String, String, bool)> = HashMap::new();
    for share in &mut taken {
        for op in share.ops.clone() {
            let edited = share
                .authored
                .iter()
                .any(|entry| entry.set_name == op.new_entry_name);
            let held = (op.src_file_rel.clone(), op.src_set_name.clone(), edited);
            match names.get(&op.new_entry_name) {
                Some(other) if edited || *other != held => {
                    let tag = effect_name_hash(&share.owner) & 0xffff;
                    let own = format!("{EDIT_COPY_PREFIX}{tag:04x}_{}", op.new_entry_name);
                    share.rename(&op.new_entry_name, &own);
                }
                Some(_) => {}
                None => {
                    names.insert(op.new_entry_name.clone(), held);
                }
            }
        }
    }
    (taken, left)
}

/// The end of a send's line: the characters whose edits can't be in the match with those sent, by
/// name, and whether each one's load at the next match.
pub fn left_note(left: Vec<(bool, String)>) -> String {
    let note = |staged: bool, label: &str| {
        let names = left.iter().filter(|(at, _)| *at == staged);
        let names: Vec<&str> = names.map(|(_, name)| name.as_str()).collect();
        match names.is_empty() {
            true => String::new(),
            false => format!(" ({label}: {})", names.join(", ")),
        }
    };
    note(true, "next match") + &note(false, "not sent")
}

/// Whether nobody of the match that is on plays a fighter the carrier is for.
pub fn nobody_holds(agents: &[LiveGimmickAgent], fighters: &[String]) -> bool {
    let mut playing = agents.iter().filter_map(|agent| agent.fighter.as_ref());
    !agents.is_empty() && !playing.any(|fighter| fighters.contains(fighter))
}

/// What a send the game has taken means for its character. `agents` is what is in the match, when
/// the game says; `unknown` whether the fighter loaded effects changed in an earlier session.
pub fn reached(
    edited: bool,
    unknown: bool,
    agents: Option<&[LiveGimmickAgent]>,
    fighter: &str,
    costumes: &[u8],
) -> SendOutcome {
    if !edited {
        return match unknown {
            true => SendOutcome::NextLoad,
            false => SendOutcome::Restored,
        };
    }
    let wears = |costume: Option<i32>| {
        let slot = costume.and_then(|costume| u8::try_from(costume).ok());
        costumes.is_empty() || slot.is_none_or(|slot| costumes.contains(&slot))
    };
    let plays = |agent: &LiveGimmickAgent| {
        agent.fighter.as_deref() == Some(fighter) && wears(agent.costume)
    };
    match agents {
        Some(agents) if !agents.iter().any(plays) => SendOutcome::NextMatch,
        _ => SendOutcome::InGame,
    }
}

/// The plugin's half, run here: tests in the plugin crate never run.
#[cfg(test)]
#[path = "../plugins/slight_replica/src/slight/effect_viewer/carrier_gate.rs"]
mod plugin_carrier;
#[cfg(test)]
#[path = "../plugins/slight_replica/src/slight/effect_viewer/staged_files.rs"]
mod plugin_staged;

#[cfg(test)]
mod tests {
    use super::*;

    const MARIO: &str = "effect/fighter/mario/ef_mario.eff";

    fn edit(entry: &str) -> AuthoredEdit {
        AuthoredEdit {
            set_name: format!("P_{entry}"),
            entry_name: entry.into(),
            set_idx: 3,
            emitter_name: "em0".into(),
            emitter_idx: 0,
            fields: Default::default(),
        }
    }

    fn edits(source_rel: &str, entries: &[&str]) -> EffMod {
        EffMod {
            source_rel: source_rel.into(),
            authored: entries.iter().map(|entry| edit(entry)).collect(),
            ..Default::default()
        }
    }

    fn borrow(src: &str, entry: &str) -> TransplantOp {
        TransplantOp {
            new_entry_name: format!("{entry}_tp"),
            src_file_rel: src.into(),
            src_set_name: entry.into(),
            src_set_idx: 0,
            one_slot_slots: Vec::new(),
            replace_entry: None,
        }
    }

    fn texture(name: &str) -> TextureImport {
        TextureImport {
            texture_name: name.into(),
            png_path: "picture.png".into(),
            raw: false,
        }
    }

    fn share(owner: &str, eff: &EffMod, costumes: &[u8], restore: &[&str]) -> Share {
        Share::new(&ShareOf {
            owner,
            fighter: owner.split('#').next().unwrap(),
            eff,
            source_rel: &eff.source_rel,
            costumes,
            borrowed_costumes: &|_| costumes.to_vec(),
            using_texture: &|_| vec![(format!("{owner}_flash"), 0)],
            restore: &restore.iter().map(|name| name.to_string()).collect(),
        })
    }

    fn both(shares: Vec<Share>, first: Option<&str>) -> (Vec<Share>, Vec<String>) {
        together(shares, first, &|_, _| false)
    }

    /// Where an effect leads on a costume, as the plugin looks it up.
    fn leads(shares: &[Share], name: &str, costume: u8) -> Option<u64> {
        let aliases = shares.iter().flat_map(|share| &share.aliases);
        aliases
            .into_iter()
            .find(|alias| {
                alias.from == effect_name_hash(name)
                    && (alias.slots.is_empty() || alias.slots.contains(&costume))
            })
            .map(|alias| alias.to)
    }

    /// A made character's edit and its fighter's to the same effect are both in the carrier, each
    /// on its own costumes: nothing else is needed for it, whatever files the game has.
    #[test]
    fn a_made_characters_edit_and_its_fighters_are_live_together() {
        let mario = edits(MARIO, &["mario_atkhi3_arc"]);
        let vision = edits(
            "effect/fighter/mario/ef_mario_c07.eff",
            &["mario_atkhi3_arc"],
        );
        let base = [0, 1, 2, 3, 4, 5, 6];
        let (shares, left) = both(
            vec![
                share("mario", &mario, &base, &[]),
                share("mario#c07", &vision, &[7], &[]),
            ],
            Some("mario#c07"),
        );
        assert_eq!(left, Vec::<String>::new());
        let hash = effect_name_hash;
        assert_eq!(
            leads(&shares, "mario_atkhi3_arc", 0),
            Some(hash("vsnedit_mario_atkhi3_arc"))
        );
        assert_eq!(
            leads(&shares, "mario_atkhi3_arc", 7),
            Some(hash("vsnedit_c07_mario_atkhi3_arc"))
        );
        // An effect only Mario changed stays the game's own on hers.
        let mario = edits(MARIO, &["mario_atkhi3_arc", "mario_fb_bound"]);
        let (shares, _) = both(
            vec![
                share("mario", &mario, &base, &[]),
                share("mario#c07", &vision, &[7], &[]),
            ],
            None,
        );
        assert!(leads(&shares, "mario_fb_bound", 0).is_some());
        assert_eq!(leads(&shares, "mario_fb_bound", 7), None);
        let edited: Vec<&str> = shares[1]
            .authored
            .iter()
            .map(|e| e.set_name.as_str())
            .collect();
        assert_eq!(edited, ["vsnedit_c07_mario_atkhi3_arc"]);
        assert_eq!(shares[1].ops[0].src_set_idx, 3);
    }

    /// The carrier holds every character's edits, and holds the same whichever one is open: opening
    /// another, or sending with another open, takes nothing out of the match.
    #[test]
    fn the_carrier_does_not_depend_on_the_open_character() {
        let mario = edits(MARIO, &["mario_atkhi3_arc"]);
        let fox = edits("effect/fighter/fox/ef_fox.eff", &["fox_blaster_shot"]);
        let names = |first| {
            let shares = vec![
                share("mario", &mario, &[], &[]),
                share("fox", &fox, &[], &[]),
            ];
            let (shares, left) = both(shares, first);
            assert_eq!(left, Vec::<String>::new());
            let ops = shares.iter().flat_map(|share| share.ops.clone());
            ops.map(|op| (op.new_entry_name, op.src_file_rel))
                .collect::<Vec<_>>()
        };
        let open_mario = names(Some("mario"));
        assert_eq!(open_mario.len(), 2);
        assert_eq!(open_mario, names(Some("fox")));
        assert_eq!(open_mario, names(None));
    }

    /// What can't be in one carrier is named, and the open character's is the one that is in it.
    #[test]
    fn a_picture_two_characters_change_is_the_open_ones_and_the_other_is_named() {
        let pictures = |rel: &str| EffMod {
            source_rel: rel.into(),
            textures: vec![texture("ef_cmn_smoke")],
            ..Default::default()
        };
        let mario = pictures(MARIO);
        let fox = edits("effect/fighter/fox/ef_fox.eff", &["fox_blaster_shot"]);
        let shares = || {
            vec![
                share("mario", &mario, &[], &[]),
                share("fox", &fox, &[], &[]),
            ]
        };
        // Fox's effects don't use it: both.
        assert_eq!(both(shares(), Some("fox")).1, Vec::<String>::new());
        let uses = |share: &Share, name: &str| share.owner == "fox" && name == "ef_cmn_smoke";
        for (open, left) in [("fox", "mario"), ("mario", "fox")] {
            let (taken, named) = together(shares(), Some(open), &uses);
            assert_eq!(named, [left]);
            assert_eq!(taken.len(), 1);
            assert_eq!(taken[0].owner, open);
        }
    }

    #[test]
    fn a_character_left_out_is_named_with_when_it_loads() {
        let name = |staged, name: &str| (staged, name.to_owned());
        assert_eq!(left_note(Vec::new()), "");
        assert_eq!(
            left_note(vec![
                name(true, "Fox"),
                name(false, "Vision"),
                name(true, "Luigi")
            ]),
            " (next match: Fox, Luigi) (not sent: Vision)"
        );
    }

    /// Two characters that edited their own copies of one donor each keep theirs.
    #[test]
    fn edited_copies_of_one_donor_are_kept_apart() {
        let pacman = "effect/fighter/pacman/ef_pacman.eff";
        let with = |rel: &str, edited: bool| EffMod {
            transplants: vec![borrow(pacman, "pacman_akabei")],
            ..edits(
                rel,
                if edited {
                    &["pacman_akabei_tp"][..]
                } else {
                    &[]
                },
            )
        };
        let mario = with(MARIO, true);
        let vision = with("effect/fighter/mario/ef_mario_c07.eff", true);
        let (shares, _) = both(
            vec![
                share("mario", &mario, &[0], &[]),
                share("mario#c07", &vision, &[7], &[]),
            ],
            None,
        );
        let name = |share: &Share| share.ops[0].new_entry_name.clone();
        assert_eq!(name(&shares[0]), "pacman_akabei");
        assert!(name(&shares[1]).starts_with(EDIT_COPY_PREFIX));
        assert_eq!(shares[1].authored[0].set_name, name(&shares[1]));
        for (costume, share) in [(0, &shares[0]), (7, &shares[1])] {
            assert_eq!(
                leads(&shares, "pacman_akabei_tp", costume),
                Some(effect_name_hash(&name(share)))
            );
        }
        assert!(crate::mod_project::is_reserved_entry_name(&name(
            &shares[1]
        )));
        // Unedited, one copy does for both.
        let (mario, vision) = (with(MARIO, false), with(MARIO, false));
        let (shares, _) = both(
            vec![
                share("mario", &mario, &[0], &[]),
                share("mario#c07", &vision, &[7], &[]),
            ],
            None,
        );
        assert_eq!(name(&shares[0]), name(&shares[1]));
    }

    /// An effect copied within its own file plays the game's until it is edited; then the copy is
    /// put in from the original, since no file of the game has it.
    #[test]
    fn an_edited_copy_of_its_own_effect_is_made_from_the_original() {
        let mut eff = edits(MARIO, &[]);
        eff.transplants = vec![borrow("", "mario_dash")];
        let plain = share("mario", &eff, &[], &[]);
        assert!(plain.is_empty());
        assert_eq!(
            leads(&[plain], "mario_dash_tp", 0),
            Some(effect_name_hash("mario_dash"))
        );
        eff.authored = vec![edit("mario_dash_tp")];
        let edited = share("mario", &eff, &[], &[]);
        assert_eq!(edited.ops.len(), 1);
        assert_eq!(edited.ops[0].src_set_name, "mario_dash");
        assert_eq!(edited.ops[0].src_file_rel, MARIO);
        assert_eq!(edited.ops[0].new_entry_name, "mario_dash_tp");
        assert_eq!(edited.authored[0].set_name, "mario_dash_tp");
        // Spawned under the name the carrier has it by.
        assert_eq!(leads(&[edited], "mario_dash_tp", 0), None);
    }

    /// Where the game loaded a changed effect with its fighter, the reset one leads to the game's
    /// own, unedited, until the fighter loads again.
    #[test]
    fn a_reset_effect_the_game_loaded_changed_leads_to_its_own_again() {
        let mario = edits(MARIO, &["mario_atkhi3_arc"]);
        let staged = changed_effects(&mario, &|_| Vec::new());
        assert_eq!(staged, BTreeSet::from(["mario_atkhi3_arc".to_owned()]));
        let reset = edits(MARIO, &[]);
        assert!(share("mario", &reset, &[], &[]).is_empty());
        let restored = share("mario", &reset, &[], &["mario_atkhi3_arc"]);
        assert!(restored.authored.is_empty());
        assert_eq!(restored.ops[0].src_set_name, "mario_atkhi3_arc");
        assert_eq!(restored.ops[0].src_file_rel, MARIO);
        assert_eq!(
            leads(&[restored], "mario_atkhi3_arc", 0),
            Some(effect_name_hash("vsnedit_mario_atkhi3_arc"))
        );
        // A replaced effect and a replaced picture change the file too.
        let mut eff = edits(MARIO, &[]);
        eff.textures = vec![texture("ef_cmn_smoke")];
        eff.transplants = vec![TransplantOp {
            replace_entry: Some("Mario_Dash".into()),
            ..borrow("effect/fighter/fox/ef_fox.eff", "fox_dash")
        }];
        let staged = changed_effects(&eff, &|_| vec![("mario_flash".into(), 0)]);
        assert_eq!(
            staged.into_iter().collect::<Vec<_>>(),
            ["mario_dash", "mario_flash"]
        );
    }

    /// "In the game" is said only of a character that is in the match, and "its own effects again"
    /// only when they are.
    #[test]
    fn a_send_says_only_what_is_in_the_match() {
        let agent = |fighter: &str, costume: Option<i32>| LiveGimmickAgent {
            boid: 0,
            category: 0,
            kind: 0,
            entry_id: 0,
            owner_boid: None,
            fighter: Some(fighter.into()),
            costume,
        };
        let c00 = [agent("mario", Some(0)), agent("fox", Some(1))];
        let c07 = [agent("mario", Some(7))];
        use SendOutcome as S;
        for (edited, unknown, agents, fighter, costumes, says) in [
            (true, false, Some(&c00[..]), "mario", &[][..], S::InGame),
            (true, false, Some(&c00[..]), "luigi", &[], S::NextMatch),
            (true, false, Some(&c00[..]), "mario", &[7], S::NextMatch),
            (true, false, Some(&c07[..]), "mario", &[7], S::InGame),
            (true, false, Some(&[][..]), "mario", &[], S::NextMatch),
            // A game that does not say what is in the match is taken at its word.
            (true, false, None, "mario", &[7], S::InGame),
            (false, false, Some(&c00[..]), "mario", &[], S::Restored),
            (false, true, Some(&c00[..]), "mario", &[], S::NextLoad),
        ] {
            assert_eq!(
                reached(edited, unknown, agents, fighter, costumes),
                says,
                "{edited} {unknown} {fighter} {costumes:?}"
            );
        }
        // A match without any of the carrier's fighters does not take it; no match says nothing.
        let fighters = |names: &[&str]| {
            names
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        };
        assert!(nobody_holds(&c00, &fighters(&["pacman"])));
        assert!(!nobody_holds(&c00, &fighters(&["pacman", "fox"])));
        assert!(!nobody_holds(&[], &fighters(&["pacman"])));
    }

    /// A match with a fighter the carrier is for always has one that looks after it, whoever held
    /// it before and whatever was left of it; one without still lets an old one go.
    #[test]
    fn someone_always_looks_after_the_carrier() {
        use super::plugin_carrier::{looks_after, Turn};
        // Two fighters in the match. `host` is who it says holds the carrier: either of them, or
        // one that is gone.
        for targets in [[true, true], [true, false], [false, true], [false, false]] {
            for host in [Some(0), Some(1), None] {
                for state in 1..=5 {
                    let turn = |me: usize| Turn {
                        target: targets[me],
                        holds: host == Some(me),
                        held_elsewhere: host.is_some_and(|host| host != me),
                        holder_is_target: host.is_some_and(|host| targets[host]),
                    };
                    let who: Vec<usize> =
                        (0..2).filter(|me| looks_after(state, turn(*me))).collect();
                    let says = format!("state {state} targets {targets:?} host {host:?}: {who:?}");
                    match state {
                        // Made and held only by a fighter it is for.
                        1 => {
                            assert!(who.iter().all(|me| targets[*me]), "{says}");
                            assert_eq!(who.is_empty(), targets == [false, false], "{says}");
                        }
                        // Held: its holder sees it gone even when it holds another's.
                        2 => assert!(
                            !who.is_empty() || (host.is_none() && targets == [false, false]),
                            "{says}"
                        ),
                        // Let go by anyone.
                        _ => assert!(!who.is_empty(), "{says}"),
                    }
                    if let Some(host) = host {
                        assert!(
                            state == 1 || who == [host],
                            "the one holding it does, alone: {says}"
                        );
                    }
                }
            }
        }
    }

    /// The fighter that held a carrier may never play again, so what its match took with it is
    /// forgotten by whoever plays next: the effect sends that followed waited for it until the
    /// game was restarted.
    #[test]
    fn a_carrier_gone_with_its_match_is_not_waited_for() {
        use super::plugin_carrier::{outgoing, still_registered, Outgoing};
        assert_eq!(outgoing(0x40000003, false, 0), Outgoing::GoneWithItsMatch);
        assert_eq!(
            outgoing(0x40000003, false, 1463),
            Outgoing::GoneWithItsMatch
        );
        assert_eq!(outgoing(0x40000003, true, 0), Outgoing::Unidentified);
        assert_eq!(outgoing(0x40000003, true, 1463), Outgoing::Up);
        assert_eq!(outgoing(0, false, 1463), Outgoing::Removed);
        // A borrowed effect's own fighter is in the next match: its name never goes away.
        let names = [Some(1463), None, Some(774)];
        assert_eq!(still_registered(names.into_iter(), 1463), 1);
        assert_eq!(still_registered(names[1..].iter().copied(), 1463), 0);
        assert_eq!(still_registered(names.into_iter(), 0), 2);
    }

    /// The game holds what it read last, not what is staged now.
    #[test]
    fn the_game_holds_what_it_read() {
        use super::plugin_staged::StagedFiles;
        let mut files = StagedFiles::new();
        assert!(!files.replace(vec![(1, "mario.eff".into(), 10)]));
        assert_eq!(files.in_the_game(), []);
        assert_eq!(files.read(1).as_deref(), Some("mario.eff"));
        assert_eq!(files.in_the_game(), [(1, 10)]);
        // Staged again, or no longer staged: the match still has the first.
        assert!(files.replace(vec![(1, "mario.eff".into(), 11), (2, "fox.eff".into(), 12)]));
        assert_eq!(files.in_the_game(), [(1, 10)]);
        assert!(files.replace(Vec::new()));
        assert_eq!(files.in_the_game(), [(1, 10)]);
        // The fighter loads again: its own file.
        assert_eq!(files.read(1), None);
        assert_eq!(files.in_the_game(), []);
        // A file that was there and could not be read is not held either.
        files.replace(vec![(1, "mario.eff".into(), 13)]);
        assert!(files.read(1).is_some());
        files.not_read(1);
        assert_eq!(files.in_the_game(), []);
    }

    const LIVE_EFF: &str =
        include_str!("../plugins/slight_replica/src/slight/effect_viewer/live_eff.rs");

    /// ARCropolis panics, and takes the game with it, when it is asked for a path the game has no
    /// file at: the plugin registers none and reads none back, whatever an editor stages.
    #[test]
    fn the_plugin_refuses_a_path_the_game_has_no_file_at() {
        let reload = LIVE_EFF
            .split("fn reload_inner(")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("reload_inner");
        let refuse = reload
            .find("game_has_file(hash) == Some(false)")
            .expect("refused");
        assert!(reload[refuse..].contains("continue;"));
        for after in ["served.push(", "register_disk("] {
            assert!(refuse < reload.find(after).expect(after), "{after}");
        }
        assert!(!LIVE_EFF.contains("load_file("));
    }

    /// Nothing the plugin is sent about aliases is dropped: a list that was left out edits the
    /// editor counted in the game.
    #[test]
    fn an_alias_list_is_never_dropped() {
        let rules =
            include_str!("../plugins/slight_replica/src/slight/effect_viewer/spawn_rules.rs");
        let set = rules
            .split("pub fn set_aliases(")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("set_aliases");
        assert!(set.contains("*current = aliases;") && !set.contains("else {"));
        assert!(set.contains("loop {"));
    }
}
