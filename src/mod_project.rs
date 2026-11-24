// Unified mod project: every edit the toolkit makes — hitboxes/ACMD scripts, effect-call
// (spawn) edits, authored .eff value edits, effect transplants — in one serializable file.
// This file travels WITH exported mods so a mod can be re-opened for further editing.
//
// Two distinct concepts live near each other here and must not be conflated:
//   * TRANSPLANT — copying an emitter set out of a donor .eff into a fighter's .eff.
//     That's the operation; see `TransplantOp`.
//   * ONE-SLOT — scoping an edit so it only applies to specific costume/skin slots
//     (c00, c07, c12, c50 …; NOT limited to 0–7). That's a modifier, expressed as
//     `TransplantOp::one_slot_slots`, and it can ride on top of a transplant.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::data::{EditRecord, EffectCallEdit};

pub const PROJECT_FILE_NAME: &str = "modproject.json";
/// Every project is saved as this. Two lines of Visionary wrote version 8: the one this file
/// belongs to, whose made characters have stores of their own (`characters`), and a fork, whose
/// projects keep a fighter list and a moveset link (`project_fighters`, `moveset_source`) and
/// whose made characters share their donor's edits. The fork's 9 has all three. See
/// [`ModProjectFile::migrate`].
pub const PROJECT_VERSION: u32 = 10;

/// Oldest project version supported by the load-time field defaults and migrations.
pub const OLDEST_SUPPORTED_VERSION: u32 = 1;

/// The source and runtime provenance needed to reopen one move without fetching it again.
///
/// `body` is the merged source body shown to the editor before any saved project edits are
/// applied. Captures are optional because ordinary source loads do not need them; when present
/// they preserve the typed tails required for safe live injection of added or retimed calls.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MoveSourceSnapshot {
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captures: Vec<crate::game_link::CaptureLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModProjectFile {
    pub version: u32,
    pub name: String,
    /// fighter name (e.g. "mario") → all edits for that fighter
    #[serde(default)]
    pub fighters: HashMap<String, FighterMod>,
    /// Characters made in Visionary, by roster key ("mario#c08") → that character's own move
    /// edits. A costume's own effect file no made character owns keeps its effect edits here too,
    /// under the costume's key ("pacman#c40"). A made character is its own character: these never apply to its donor's costumes,
    /// and the donor's edits in `fighters` never apply to its costumes. `fighters` holds engine
    /// fighters only. Left out of the file when empty, so a project with no made characters is
    /// written exactly as before.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub characters: BTreeMap<crate::roster::RosterKey, FighterMod>,
    /// Character select screen edits: ordering, names, hidden entries, and entries this
    /// project created. Added in v3.
    #[serde(default, skip_serializing_if = "RosterMod::is_empty")]
    pub roster: RosterMod,
    /// The fighters this project works on: the left fighter list shows exactly these. Projects
    /// from before it was kept ([`Self::keeps_scope`]) load with it empty and get it seeded from
    /// what they already edit.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub project_fighters: BTreeSet<ProjectFighter>,
    /// The moveset folder this project is linked to, as an absolute path in memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moveset_source: Option<String>,
}

/// One row of the project's fighter list: a vanilla fighter, or a character this project
/// added on a fighter's costume slot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProjectFighter {
    pub name: String,
    /// The slot character's display name (`FighterEntry::custom_name`). `None` is the
    /// fighter's own row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
}

impl ProjectFighter {
    /// The fighter's own row.
    pub fn fighter(name: &str) -> Self {
        Self {
            name: name.to_string(),
            character: None,
        }
    }

    /// A character this project added on `name`'s costume slot.
    pub fn character(name: &str, character: &str) -> Self {
        Self {
            name: name.to_string(),
            character: Some(character.to_string()),
        }
    }
}

/// Which ACMD portion of a saved project should be sent through the source exporter.
///
/// EFF edits deliberately do not belong to either scope: the source exporter produces a
/// Smashline project, while EFF edits ship through the dedicated mod-folder/developer export
/// paths. Keeping that boundary in one pure helper prevents the Edit Log's per-row export from
/// accidentally changing the meaning of a whole-project export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcmdProjectScope<'a> {
    /// Keep every fighter and every ACMD category represented by the project.
    All,
    /// Keep just one fighter/move pair, including every ACMD category recorded for it.
    Move {
        fighter: &'a str,
        move_name: &'a str,
    },
}

/// Return an ACMD-only view of `project` for a whole-project or one-move export.
///
/// This is intentionally a pure data transformation. It retains the complete edited values,
/// not just the sparse edit log: game scripts, effect deltas and full call lists, frame residue,
/// loss notes, sound/expression scripts, and capture provenance warnings all stay paired by move.
/// EFF records are removed because a Smashline source export cannot carry them. Runtime live
/// tweaks are kept only when their effect kind is present in a retained, enabled effect call,
/// except that structured effect tracks preserve the global tweak context so export can refuse
/// the unsupported combination instead of silently dropping the override.
pub fn scope_acmd_project(project: &ModProjectFile, scope: AcmdProjectScope<'_>) -> ModProjectFile {
    let mut scoped = ModProjectFile {
        version: project.version,
        name: project.name.clone(),
        fighters: HashMap::new(),
        characters: BTreeMap::new(),
        // Roster edits are dropped here for the same reason EFF records are: a Smashline
        // source export cannot carry a character select screen. They ship through the mod
        // folder path instead.
        roster: RosterMod::default(),
        // Neither is ACMD, so neither belongs in a Smashline source export.
        project_fighters: BTreeSet::new(),
        moveset_source: None,
    };

    // A whole-project export may have a tweak stored under one fighter while the corresponding
    // spawn is stored under another (older project files and hand-edited projects can do this).
    // Treat the retained ACMD project as one generated source and use the union of its effect
    // kinds, matching `source_project`'s global tweak collection.
    let all_retained_effects = match scope {
        AcmdProjectScope::All => {
            let mut retained = project
                .all_move_edits()
                .flat_map(|fighter| fighter.effect_calls_full.values())
                .flat_map(|calls| calls.iter())
                .filter(|call| !call.disabled)
                .map(|call| call.effect_name.to_ascii_lowercase())
                .filter(|name| !name.is_empty())
                .collect::<HashSet<_>>();
            if project
                .all_move_edits()
                .any(|fighter| has_structured_effects(fighter, None))
            {
                retained.extend(all_live_tweak_names(project));
            }
            retained
        }
        AcmdProjectScope::Move { .. } => HashSet::new(),
    };

    match scope {
        AcmdProjectScope::All => {
            for (fighter_name, fighter) in &project.fighters {
                scoped.fighters.insert(
                    fighter_name.clone(),
                    scoped_fighter(fighter, None, &all_retained_effects),
                );
            }
            for (key, character) in &project.characters {
                scoped.characters.insert(
                    key.clone(),
                    scoped_fighter(character, None, &all_retained_effects),
                );
            }
            // A made character's arm needs its authored entry for its costumes.
            scoped.roster.authored = project.roster.authored.clone();
        }
        AcmdProjectScope::Move {
            fighter: selected_fighter,
            move_name: selected_move,
        } => {
            // `fighter` is a character key: a base fighter's name or a made character's key.
            let character = crate::character::CharacterKey::parse(selected_fighter);
            let source = if character.is_made() {
                project.characters.get(character.roster_key())
            } else {
                project.fighters.get(selected_fighter)
            };
            if let Some(fighter) = source {
                let structured_effects = has_structured_effects(fighter, Some(selected_move));
                let retained_effects = fighter
                    .effect_calls_full
                    .get(selected_move)
                    .into_iter()
                    .flat_map(|calls| calls.iter())
                    .filter(|call| !call.disabled)
                    .map(|call| call.effect_name.to_ascii_lowercase())
                    .filter(|name| !name.is_empty())
                    .collect::<HashSet<_>>();
                let mut selected = scoped_fighter(fighter, Some(selected_move), &retained_effects);
                if structured_effects {
                    selected.live_tweaks = project
                        .all_move_edits()
                        .flat_map(|fighter| fighter.live_tweaks.iter().cloned())
                        .collect();
                }
                if character.is_made() {
                    scoped
                        .characters
                        .insert(character.roster_key().clone(), selected);
                } else {
                    scoped
                        .fighters
                        .insert(selected_fighter.to_string(), selected);
                }
            }
            // The move ships as one function for every costume of the fighter, so it carries
            // every character's version of it and the costumes each one owns: a donor's edit
            // must not reach a made character's costumes, and the reverse. A fighter with no
            // made characters is exported exactly as before.
            let engine = character.engine_fighter();
            let made: Vec<&AuthoredEntry> =
                crate::character::made_on(engine, &project.roster).collect();
            if !made.is_empty() {
                let only_the_move = |edits: &FighterMod| {
                    let moves = scoped_fighter(edits, Some(selected_move), &HashSet::new());
                    FighterMod {
                        display: moves.display,
                        acmd: moves.acmd,
                        effect_calls: moves.effect_calls,
                        effect_calls_full: moves.effect_calls_full,
                        effect_scripts: moves.effect_scripts,
                        effect_dropped_lines: moves.effect_dropped_lines,
                        effect_frame_residue: moves.effect_frame_residue,
                        sound_scripts: moves.sound_scripts,
                        expression_scripts: moves.expression_scripts,
                        move_sources: moves.move_sources,
                        move_graphs: moves.move_graphs,
                        capture_branch_warnings: moves.capture_branch_warnings,
                        ..FighterMod::default()
                    }
                };
                if let Some((name, edits)) = project
                    .fighters
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(engine))
                {
                    scoped
                        .fighters
                        .entry(name.clone())
                        .or_insert_with(|| only_the_move(edits));
                }
                for entry in &made {
                    if let Some(edits) = project.characters.get(&entry.key) {
                        scoped
                            .characters
                            .entry(entry.key.clone())
                            .or_insert_with(|| only_the_move(edits));
                    }
                }
                scoped.roster.authored = made.into_iter().cloned().collect();
            }
        }
    }

    scoped
}

fn has_structured_effects(fighter: &FighterMod, selected_move: Option<&str>) -> bool {
    fighter
        .effect_scripts
        .keys()
        .any(|name| selected_move.is_none_or(|selected| name == selected))
        || fighter.move_graphs.iter().any(|(name, graph)| {
            selected_move.is_none_or(|selected| name == selected)
                && graph
                    .tracks
                    .iter()
                    .any(|track| track.track == crate::move_nodes::model::Track::Effect)
        })
}

fn all_live_tweak_names(project: &ModProjectFile) -> HashSet<String> {
    project
        .all_move_edits()
        .flat_map(|fighter| fighter.live_tweaks.iter())
        .map(|tweak| tweak.effect_name.to_ascii_lowercase())
        .collect()
}

/// Copy one fighter's ACMD fields, retaining either every move or one named move.
fn scoped_fighter(
    fighter: &FighterMod,
    selected_move: Option<&str>,
    retained_effects: &HashSet<String>,
) -> FighterMod {
    let mut scoped = fighter.clone();
    scoped.eff = None;
    scoped.acmd = scope_move_map(&fighter.acmd, selected_move);
    scoped.effect_calls = scope_move_map(&fighter.effect_calls, selected_move);
    scoped.effect_calls_full = scope_move_map(&fighter.effect_calls_full, selected_move);
    scoped.effect_scripts = scope_move_map(&fighter.effect_scripts, selected_move);
    scoped.effect_dropped_lines = scope_move_map(&fighter.effect_dropped_lines, selected_move);
    scoped.effect_frame_residue = scope_move_map(&fighter.effect_frame_residue, selected_move);
    scoped.sound_scripts = scope_move_map(&fighter.sound_scripts, selected_move);
    scoped.expression_scripts = scope_move_map(&fighter.expression_scripts, selected_move);
    scoped.move_sources = scope_move_map(&fighter.move_sources, selected_move);
    scoped.move_graphs = fighter
        .move_graphs
        .iter()
        .filter(|(name, _)| selected_move.is_none_or(|selected| name.as_str() == selected))
        .map(|(name, graph)| (name.clone(), graph.clone()))
        .collect();
    scoped.capture_branch_warnings =
        scope_move_map(&fighter.capture_branch_warnings, selected_move);
    scoped.live_tweaks = fighter
        .live_tweaks
        .iter()
        .filter(|tweak| retained_effects.contains(&tweak.effect_name.to_ascii_lowercase()))
        .cloned()
        .collect();
    scoped
}

fn scope_move_map<V: Clone>(
    source: &HashMap<String, V>,
    selected_move: Option<&str>,
) -> HashMap<String, V> {
    source
        .iter()
        .filter(|(move_name, _)| {
            selected_move.is_none_or(|selected| move_name.as_str() == selected)
        })
        .map(|(move_name, value)| (move_name.clone(), value.clone()))
        .collect()
}

fn move_entry<V>(from: &mut HashMap<String, V>, to: &mut HashMap<String, V>, move_name: &str) {
    if let Some(value) = from.remove(move_name) {
        to.insert(move_name.to_owned(), value);
    }
}

impl Default for ModProjectFile {
    fn default() -> Self {
        Self {
            version: PROJECT_VERSION,
            name: "unnamed_mod".into(),
            fighters: HashMap::new(),
            characters: BTreeMap::new(),
            roster: RosterMod::default(),
            project_fighters: BTreeSet::new(),
            moveset_source: None,
        }
    }
}

impl ModProjectFile {
    /// Deliberately ignores `project_fighters` and `moveset_source`: a project that only names
    /// fighters has nothing to export. Likewise does not consult `effect_dropped_lines` (see
    /// [`FighterMod::has_edits`]).
    pub fn is_empty(&self) -> bool {
        self.all_move_edits().all(|f| !f.has_edits()) && self.roster.is_empty()
    }

    /// Whether this file is from a Visionary that kept a project's fighter list and moveset link
    /// in it, so one that is missing is one the user left empty. Version 8 was written by both
    /// lines (see [`PROJECT_VERSION`]), and only the fork's ever has either.
    pub fn keeps_scope(&self) -> bool {
        match self.version {
            1..=3 | 5..=7 => false,
            8 => !self.project_fighters.is_empty() || self.moveset_source.is_some(),
            _ => true,
        }
    }

    /// Add the fighters this project already edits to `project_fighters`: every fighter with
    /// edits, and every character it authored. Additive and idempotent. [`Self::migrate`] runs
    /// it for files saved before the list existed ([`Self::keeps_scope`]): otherwise the list is
    /// what the user left it as.
    pub fn seed_project_fighters(&mut self) {
        for (name, fighter) in &self.fighters {
            if fighter.has_edits() {
                self.project_fighters.insert(ProjectFighter::fighter(name));
            }
        }
        for entry in &self.roster.authored {
            self.project_fighters
                .insert(ProjectFighter::character(&entry.donor, &entry.display_name));
        }
    }

    /// The moves any character of a fighter edits, by engine fighter (lowercase) and move: the
    /// fighter's versions of a move ship together, and each reads the game's own from whichever
    /// of them has it.
    pub fn edited_moves(&self) -> HashSet<(String, String)> {
        let characters = self.characters.iter().map(|(key, edits)| {
            let key = crate::character::CharacterKey::made(key.clone());
            (key.engine_fighter().to_lowercase(), edits)
        });
        self.fighters
            .iter()
            .map(|(name, edits)| (name.to_lowercase(), edits))
            .chain(characters)
            .flat_map(|(engine, edits)| {
                edits
                    .edited_moves()
                    .map(move |name| (engine.clone(), name.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Every fighter's and every made character's edits. Anything that asks a question of all
    /// of a project's edits reads through this, so a made character's edits are never left out
    /// of a check written when only fighters held edits.
    pub fn all_move_edits(&self) -> impl Iterator<Item = &FighterMod> {
        self.fighters.values().chain(self.characters.values())
    }

    /// Move edits saved before made characters had their own store: a `game_` record in the
    /// donor's edits, scoped to costumes. One whose costumes are exactly a made character's
    /// moves into that character's store, unscoped, with the same move's sound, expression and
    /// effect edits, which the old export gated with it. One that only partly matches stays
    /// where it is, still exported behind its own gate, and is reported. Returns the reports.
    pub fn migrate_scoped_records(&mut self) -> Vec<String> {
        let mut findings = Vec::new();
        let authored = self.roster.authored.clone();
        let mut donors: Vec<String> = self.fighters.keys().cloned().collect();
        donors.sort();
        for donor in donors {
            let Some(fighter) = self.fighters.get_mut(&donor) else {
                continue;
            };
            let mut scoped: Vec<(String, Vec<u8>)> = fighter
                .acmd
                .iter()
                .map(|(move_name, record)| (move_name.clone(), record.effective_scopes()))
                .filter(|(_, scopes)| !scopes.is_empty())
                .collect();
            scoped.sort();
            let mut moved: Vec<(crate::roster::RosterKey, String)> = Vec::new();
            for (move_name, scopes) in scoped {
                let owner = authored.iter().find(|entry| {
                    entry.donor.eq_ignore_ascii_case(&donor) && entry.all_slots() == scopes
                });
                match owner {
                    Some(entry) => moved.push((entry.key.clone(), move_name)),
                    None => findings.push(format!(
                        "{donor}/{move_name} is set to costumes {} that match none of this \
                         project's characters; it stays limited to those costumes",
                        crate::character::slot_range(&scopes).unwrap_or_default()
                    )),
                }
            }
            for (key, move_name) in moved {
                let fighter = self.fighters.get_mut(&donor).expect("checked above");
                let mut taken = FighterMod {
                    display: fighter.display.clone(),
                    ..FighterMod::default()
                };
                if let Some(mut record) = fighter.acmd.remove(&move_name) {
                    record.slot_scope = None;
                    record.slot_scopes.clear();
                    taken.acmd.insert(move_name.clone(), record);
                }
                move_entry(
                    &mut fighter.effect_calls,
                    &mut taken.effect_calls,
                    &move_name,
                );
                move_entry(
                    &mut fighter.effect_calls_full,
                    &mut taken.effect_calls_full,
                    &move_name,
                );
                move_entry(
                    &mut fighter.effect_scripts,
                    &mut taken.effect_scripts,
                    &move_name,
                );
                move_entry(
                    &mut fighter.effect_dropped_lines,
                    &mut taken.effect_dropped_lines,
                    &move_name,
                );
                move_entry(
                    &mut fighter.effect_frame_residue,
                    &mut taken.effect_frame_residue,
                    &move_name,
                );
                move_entry(
                    &mut fighter.sound_scripts,
                    &mut taken.sound_scripts,
                    &move_name,
                );
                move_entry(
                    &mut fighter.expression_scripts,
                    &mut taken.expression_scripts,
                    &move_name,
                );
                move_entry(
                    &mut fighter.capture_branch_warnings,
                    &mut taken.capture_branch_warnings,
                    &move_name,
                );
                // A node graph saved for the move expanded into that same scoped record, so it
                // was the made character's too.
                if let Some(graph) = fighter.move_graphs.remove(&move_name) {
                    taken.move_graphs.insert(move_name.clone(), graph);
                }
                // The game's own version stays with the donor too: both arms of a gate read it.
                if let Some(source) = fighter.move_sources.get(&move_name) {
                    taken.move_sources.insert(move_name.clone(), source.clone());
                }
                let target = self.characters.entry(key).or_default();
                if target.display.is_empty() {
                    target.display = taken.display.clone();
                }
                target.acmd.extend(taken.acmd);
                target.effect_calls.extend(taken.effect_calls);
                target.effect_calls_full.extend(taken.effect_calls_full);
                target.effect_scripts.extend(taken.effect_scripts);
                target
                    .effect_dropped_lines
                    .extend(taken.effect_dropped_lines);
                target
                    .effect_frame_residue
                    .extend(taken.effect_frame_residue);
                target.sound_scripts.extend(taken.sound_scripts);
                target.expression_scripts.extend(taken.expression_scripts);
                target
                    .capture_branch_warnings
                    .extend(taken.capture_branch_warnings);
                target.move_sources.extend(taken.move_sources);
                target.move_graphs.extend(taken.move_graphs);
            }
        }
        findings
    }

    /// Bring a just-loaded project up to the current version.
    ///
    /// Legacy move behavior is folded into the main controller during deserialization.
    /// Stamp the current version and refuse newer files before they can be saved.
    pub fn migrate(&mut self) -> anyhow::Result<Vec<String>> {
        if self.version > PROJECT_VERSION {
            anyhow::bail!(
                "this project was saved by a newer Visionary (project version {}, this build \
                 understands {PROJECT_VERSION}). Update Visionary rather than opening it here \
                 — saving would discard whatever it contains that this build cannot read.",
                self.version
            );
        }
        if self.version < OLDEST_SUPPORTED_VERSION {
            anyhow::bail!("unsupported project version {}", self.version);
        }
        // Decided before the version is stamped.
        let keeps_scope = self.keeps_scope();
        // A removal from the list survives a reopen; a file from before the list gets it from
        // what it already edits.
        if !keeps_scope {
            self.seed_project_fighters();
        }
        // Older builds recorded an eff opened from outside the eff root by its absolute path.
        // Rewrite it to the game path it stands for, and keep where the file is; one with no game
        // path stays as it is and export refuses it.
        for fighter in self
            .fighters
            .values_mut()
            .chain(self.characters.values_mut())
        {
            if let Some(eff) = fighter.eff.as_mut() {
                if !eff.source_rel.is_empty() && !is_mod_relative(&eff.source_rel) {
                    let path = std::path::PathBuf::from(&eff.source_rel);
                    let rel = eff_game_rel(&path, None);
                    if rel != eff.source_rel {
                        eff.source_path = Some(path);
                        eff.set_source_rel(rel);
                    }
                }
            }
        }
        // A project saved before made characters had their own stores is one whose version
        // predates them: anything before 8, and the fork's 8. Builds between the change and the
        // version bump wrote 7 with a `characters` map, so a file that already has one is not
        // shared, whatever its version says: copying the donor's edits into it again would hand
        // the character donor edits made after it was created. A file with no made characters
        // needs nothing, so a base-only project is unchanged.
        let shared_with_donor = (self.version < 8 || (self.version == 8 && keeps_scope))
            && self.characters.is_empty()
            && !self.roster.authored.is_empty();
        self.version = PROJECT_VERSION;
        let mut findings = self.migrate_scoped_records();
        if shared_with_donor {
            findings.extend(self.copy_shared_donor_edits());
        }
        Ok(findings)
    }

    /// Give each made character its own copy of every donor edit it played before it had a
    /// store of its own, the way creating it copies them now: unscoped move edits with their
    /// node graphs, sound, expression and effect calls; the donor's gimmick and meters when
    /// they ran on every costume; the donor's effect file edits onto the character's own file.
    /// A move whose game record is limited to some costumes belongs to that limit and is left
    /// alone. Never replaces anything the character already has. Returns one line per
    /// character that got a copy.
    fn copy_shared_donor_edits(&mut self) -> Vec<String> {
        fn copy<V: Clone>(
            from: &HashMap<String, V>,
            to: &mut HashMap<String, V>,
            limited: &HashSet<String>,
        ) -> usize {
            let mut copied = 0;
            for (move_name, value) in from.iter().filter(|(name, _)| !limited.contains(*name)) {
                if !to.contains_key(move_name) {
                    to.insert(move_name.clone(), value.clone());
                    copied += 1;
                }
            }
            copied
        }
        let mut findings = Vec::new();
        for entry in self.roster.authored.clone() {
            let Some(donor) = self
                .fighters
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&entry.donor))
                .map(|(_, edits)| edits.clone())
            else {
                continue;
            };
            let key = crate::character::CharacterKey::made(entry.key.clone());
            let slots = entry.all_slots();
            let limited: HashSet<String> = donor
                .acmd
                .iter()
                .filter(|(_, record)| !record.effective_scopes().is_empty())
                .map(|(move_name, _)| move_name.clone())
                .collect();
            let existed = self.characters.contains_key(&entry.key);
            let mut own = self.characters.remove(&entry.key).unwrap_or_default();
            let mut copied = 0;
            for (move_name, record) in donor.acmd.iter() {
                if !limited.contains(move_name) && !own.acmd.contains_key(move_name) {
                    let mut record = record.clone();
                    record.slot_scope = None;
                    record.slot_scopes.clear();
                    own.acmd.insert(move_name.clone(), record);
                    copied += 1;
                }
            }
            copied += copy(&donor.effect_calls, &mut own.effect_calls, &limited);
            copied += copy(&donor.effect_scripts, &mut own.effect_scripts, &limited);
            copied += copy(&donor.sound_scripts, &mut own.sound_scripts, &limited);
            copied += copy(
                &donor.expression_scripts,
                &mut own.expression_scripts,
                &limited,
            );
            for (move_name, graph) in &donor.move_graphs {
                if !limited.contains(move_name) && !own.move_graphs.contains_key(move_name) {
                    own.move_graphs.insert(move_name.clone(), graph.clone());
                    copied += 1;
                }
            }
            // Companions of the copies above, not edits of their own.
            copy(
                &donor.effect_calls_full,
                &mut own.effect_calls_full,
                &limited,
            );
            copy(
                &donor.effect_dropped_lines,
                &mut own.effect_dropped_lines,
                &limited,
            );
            copy(
                &donor.effect_frame_residue,
                &mut own.effect_frame_residue,
                &limited,
            );
            copy(
                &donor.capture_branch_warnings,
                &mut own.capture_branch_warnings,
                &limited,
            );
            copy(&donor.move_sources, &mut own.move_sources, &HashSet::new());
            // The donor's gimmick, when it ran on all of the character's costumes. One limited
            // to exactly them was the character's and is claimed on load instead.
            let costumes = &donor.behavior.costumes;
            let ran_here = costumes.is_empty()
                || (costumes != &slots && slots.iter().all(|slot| costumes.contains(slot)));
            let has_gimmick = !donor.behavior.is_unset() || !donor.controller_charts.is_empty();
            if has_gimmick
                && own.behavior.is_unset()
                && own.controller_charts.is_empty()
                && ran_here
            {
                own.behavior = donor.behavior.clone();
                own.behavior.costumes = slots.clone();
                own.behavior_layout = donor.behavior_layout.clone();
                own.controller_charts = donor.controller_charts.clone();
                copied += 1;
            } else if has_gimmick
                && !ran_here
                && costumes != &slots
                && slots.iter().any(|slot| costumes.contains(slot))
            {
                findings.push(format!(
                    "{}'s gimmick ran on some of {}'s costumes; it stays with {}",
                    entry.donor, entry.display_name, entry.donor
                ));
            }
            if own.meters.is_empty() && ran_here && !donor.meters.is_empty() {
                own.meters = donor.meters.clone();
                copied += 1;
            }
            if let Some(mut eff) = donor.eff.clone().filter(|eff| !eff.is_empty()) {
                if own.eff.is_none() {
                    eff.set_source_rel(key.effect_rel(&self.roster));
                    eff.source_path = None;
                    eff.transplants.retain(|op| op.one_slot_slots.is_empty());
                    own.eff = Some(eff);
                    copied += 1;
                }
            }
            if copied > 0 && own.display.is_empty() {
                own.display = donor.display.clone();
            }
            if copied > 0 {
                findings.push(format!(
                    "{} keeps its own copy of {copied} {} edit{} it used to share",
                    entry.display_name,
                    entry.donor,
                    if copied == 1 { "" } else { "s" }
                ));
            }
            if existed || copied > 0 {
                self.characters.insert(entry.key.clone(), own);
            }
        }
        findings
    }
}

/// Character select screen edits.
///
/// Every map here is **sparse** and keyed by [`crate::roster::RosterKey`], a stable string.
/// Sparse because two mods editing different parts of the roster must merge, and a whole-file
/// copy of `ui_chara_db` cannot; stable-keyed because the index an entry sits at is rebuilt on
/// every rescan and a positional key would silently retarget after installing a mod.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RosterMod {
    /// Desired character select position, for entries the user moved.
    ///
    /// `i8` because that is the width of the `disp_order` field it is written into; a wider
    /// type here would let the editor hold a position the file cannot store.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub order: std::collections::BTreeMap<crate::roster::RosterKey, i8>,
    /// Display-name overrides (shorthand: one string writes chr0/chr1/chr2 together).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub names: std::collections::BTreeMap<crate::roster::RosterKey, String>,
    /// Per-label name overrides: when present, these replace the corresponding chrN
    /// label individually. Lets the user edit all three `nam_chr{0,1,2}` strings
    /// independently instead of being forced to keep them in sync.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub name_variants: std::collections::BTreeMap<crate::roster::RosterKey, NameVariants>,
    /// Per-costume display names for a fighter's individual slots (c00..c255),
    /// even when those costumes are not separate roster cells. This is what lets
    /// a vanilla fighter's alt costumes each carry their own name while the CSS
    /// entry remains one.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub per_costume_names:
        std::collections::BTreeMap<String, std::collections::BTreeMap<u8, String>>,
    /// Entries this project created from nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authored: Vec<AuthoredEntry>,
    /// Entries hidden from the character select screen.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub hidden: std::collections::BTreeSet<crate::roster::RosterKey>,
    /// UI image overrides: RosterKey -> (image kind -> replacement).
    ///
    /// `kind` is the portrait set name (`chara_0`, `chara_1`, `chara_2`, `stock_90`, …)
    /// and the value records the PNG the user picked plus gamma toggles. Stored
    /// per entry so each character's images are editable independently.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub ui_images: std::collections::BTreeMap<
        crate::roster::RosterKey,
        std::collections::BTreeMap<String, UiImageOverride>,
    >,
    /// Per-entry overrides for `ui_chara_db` fields beyond `disp_order`/`can_select`
    /// (which live in `order`/`hidden`). Lets the user edit everything about a
    /// roster row — `color_num`, etc. — without touching the file copy.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub chara_overrides: std::collections::BTreeMap<crate::roster::RosterKey, CharaOverrides>,
}

impl RosterMod {
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
            && self.names.is_empty()
            && self.name_variants.is_empty()
            && self.per_costume_names.is_empty()
            && self.authored.is_empty()
            && self.hidden.is_empty()
            && self.ui_images.is_empty()
            && self.chara_overrides.is_empty()
    }
}

/// Per-label name overrides for one roster entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NameVariants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chr0: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chr1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chr2: Option<String>,
}

impl NameVariants {
    pub fn is_empty(&self) -> bool {
        self.chr0.is_none() && self.chr1.is_none() && self.chr2.is_none()
    }
}

/// One UI portrait / stock / result image replacement.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiImageOverride {
    /// Filesystem path to the PNG the user picked.
    pub png_path: String,
    /// Apply gamma 2.2 correction when rendering the preview (brighten).
    #[serde(default)]
    pub gamma_render: bool,
    /// Apply inverse gamma when encoding to BNTX (darken/linearise).
    #[serde(default)]
    pub gamma_upload: bool,
}

/// Sparse overrides for fields of a `ui_chara_db` row.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharaOverrides {
    /// How many costume slots the CSS offers for this character.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_num: Option<u8>,
    /// Save slot, when the user wants to move it independently of display order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_no: Option<i8>,
}

/// A character this project added to the roster.
///
/// Backed by a costume slot on `donor`: a slot clone rather
/// than a new engine fighter ID. The scaffold deliberately records *where the user's files go*
/// rather than copying the donor's, so "did my model get picked up" is answerable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthoredEntry {
    /// Stable key, `"<donor>#c<NN>"` of the first slot.
    pub key: crate::roster::RosterKey,
    pub donor: String,
    /// First (primary) slot. Kept for every project written before multi-slot
    /// characters existed; [`AuthoredEntry::all_slots`] is what new code reads.
    pub slot: u8,
    /// Additional slots beyond [`AuthoredEntry::slot`]. Empty for every
    /// single-slot character, so old projects load unchanged. A new character
    /// with 8 skins holds all 8 here as primary + 7 additional.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<u8>,
    pub display_name: String,
    /// The `name_id` of the `ui_chara_db` row this entry adds. Distinct from the donor's, and
    /// what every select screen file for this character is named after.
    #[serde(default)]
    pub name_id: String,
    /// Whether the slot-gated ACMD agent has been scaffolded yet.
    #[serde(default)]
    pub moveset_scaffolded: bool,
    /// Where this character's files live on disk (the scaffold root the
    /// wizard created and imported as a mod). `None` for characters from
    /// before this was recorded — everything still works, there is just no
    /// folder to reveal. Optional so old projects load unchanged.
    #[serde(default)]
    pub files_root: Option<std::path::PathBuf>,
}

impl AuthoredEntry {
    /// Every costume slot this character owns, ascending and de-duplicated:
    /// the primary [`AuthoredEntry::slot`] plus [`AuthoredEntry::slots`].
    pub fn all_slots(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + self.slots.len());
        out.push(self.slot);
        out.extend(self.slots.iter().copied());
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Sparse parameter edits for one fighter.
///
/// Keyed by the param file's **game-relative** path and then by a dotted path scoped to this
/// fighter's part of that file. The traits proper live in one shared file that is not under any
/// fighter directory — `fighter/common/param/fighter_param.prc` holds a `fighter_param_table`
/// with one row per fighter, keyed by `fighter_kind` — so a path here is relative to this
/// fighter's row (`"weight"`), not to the file root.
///
/// Sparse for the same reason the ACMD edit log is, and more urgently: `fighter_param.prc` is a
/// single file shared by every fighter in the game, so a whole-file copy from one mod would
/// overwrite every other mod's fighter. It would also pin values from whatever base file
/// happened to be installed when the edit was made.
///
/// Personal `vl.prc` files reuse this same map with the game-relative key
/// `fighter/<name>/param/vl.prc` and inner keys in the nested spelling
/// (`param_special_lw[0].limit_gauge_add`); the typed accessors live in
/// `crate::gimmicks::params`. No schema change was needed for them, so projects
/// saved before personal edits existed load unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ParamMod {
    #[serde(default, flatten)]
    pub files: std::collections::BTreeMap<String, std::collections::BTreeMap<String, ParamValue>>,
}

impl ParamMod {
    pub fn is_empty(&self) -> bool {
        self.files.values().all(|fields| fields.is_empty())
    }

    pub fn field_count(&self) -> usize {
        self.files.values().map(|fields| fields.len()).sum()
    }
}

/// One edited parameter value, carrying the prc type it must be written back as.
///
/// The type is stored rather than inferred on write: `prc` files are typed, and writing a
/// float into a field the game reads as an int produces a file that loads and behaves wrongly
/// rather than one that fails.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ParamValue {
    Bool(bool),
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    Float(f32),
    Hash(u64),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ControllerChart {
    /// A chart's independent state machine. The legacy `behavior` field remains the reserved
    /// `main` chart and is not duplicated here.
    #[serde(
        default,
        skip_serializing_if = "crate::gimmicks::behavior::BehaviorGraph::is_empty"
    )]
    pub graph: crate::gimmicks::behavior::BehaviorGraph,
    /// Editor-only positions scoped to this chart.
    #[serde(
        default,
        skip_serializing_if = "crate::gimmicks::field_canvas::FieldLayout::is_empty"
    )]
    pub layout: crate::gimmicks::field_canvas::FieldLayout,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FighterMod {
    /// Complete effect statement trees authored by move nodes, without flattening branches.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub effect_scripts: HashMap<String, crate::data::AcmdScript>,
    /// Structured move scripts and their editor positions.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub move_graphs: std::collections::BTreeMap<String, crate::move_nodes::model::MoveGraph>,
    #[serde(default)]
    pub display: String,
    /// move name → hitbox/script edit (the hitbox editor's existing edit-log record)
    #[serde(default)]
    pub acmd: HashMap<String, EditRecord>,
    /// move name → effect-call (spawn) edits
    #[serde(default)]
    pub effect_calls: HashMap<String, Vec<EffectCallEdit>>,
    /// move name → full edited spawn list (what the exported effect script emits)
    #[serde(default)]
    pub effect_calls_full: HashMap<String, Vec<crate::data::EffectCall>>,
    /// move name → the lines the effect export throws away, as measured when the move was last
    /// read from a script.
    ///
    /// Stored rather than derived because it cannot be derived later: the export regenerates the
    /// function from `effect_calls_full`, and a line that became no call is in neither the calls
    /// nor the output. This is the only field here that changes no generated code — it exists so
    /// that exporting a reloaded project reports the same losses as the source pane does with the
    /// script open. Moves that lost nothing are left out, and `#[serde(default)]` keeps projects
    /// saved before C6c loadable; both simply report nothing, which is what they did before.
    #[serde(default)]
    pub effect_dropped_lines: HashMap<String, Vec<String>>,
    /// move name → effect lines keyed by the frame they belong to, for frames whose block held
    /// no spawn to attach them to. See [`crate::data::EffectScript::to_effect_calls_and_residue`].
    ///
    /// Stored for the same reason as `effect_dropped_lines` and derived at the same moment, but
    /// unlike that field this one **does** change generated code: the emitter writes these lines
    /// out at their frame. A project saved before E3 has none, so `#[serde(default)]` loads it
    /// and it exports exactly as it did then — the lines are dropped, and the note in
    /// `effect_dropped_lines` written by that older build still says so.
    #[serde(default)]
    pub effect_frame_residue: HashMap<String, std::collections::BTreeMap<u32, Vec<String>>>,
    /// move name → the whole edited `sound_` script (what the exported sound script emits)
    ///
    /// The whole script, not the changed calls, for the reason `effect_calls_full` is whole:
    /// an installed `sound_` function replaces the fighter's own, so a partial one would
    /// silence every call it left out. `#[serde(default)]` keeps projects saved before D1d
    /// loadable — they simply have no sounds, which is what was true when they were written.
    #[serde(default)]
    pub sound_scripts: HashMap<String, crate::data::AcmdScript>,
    /// move name → whole edited `expression_` script. Like `sound_scripts`, installing one
    /// replaces that category for the move, so the complete statement tree is stored.
    #[serde(default)]
    pub expression_scripts: HashMap<String, crate::data::AcmdScript>,
    /// move name → warning recorded when the move was loaded from one live-capture path while
    /// the cached source contained runtime branches. This is provenance, not an edit: it keeps
    /// an export from presenting an observed arm as if it were the complete source.
    #[serde(default)]
    pub capture_branch_warnings: HashMap<String, String>,
    /// move name → the merged source body and optional typed live-capture payload used to load it
    #[serde(default)]
    pub move_sources: HashMap<String, MoveSourceSnapshot>,
    /// authored .eff edits for this fighter's effect file
    #[serde(default)]
    pub eff: Option<EffMod>,
    /// User-set runtime color/speed multipliers, exported as LAST_EFFECT_SET_* lines in
    /// the generated effect scripts and re-applied live on project load.
    #[serde(default)]
    pub live_tweaks: Vec<LiveTweak>,
    /// Sparse edits to this fighter's `.prc` parameter files — weight, gravity, speeds, and
    /// the rest of the fighter-wide values. Added in v3.
    #[serde(default, skip_serializing_if = "ParamMod::is_empty")]
    pub params: ParamMod,
    /// The fighter's gimmick state machine (slice F2.1): typed variables,
    /// guarded transitions, timers, and resets. Empty means no behavior
    /// authored; `#[serde(default)]` keeps projects saved before F2.1
    /// loadable, and they behave exactly as before — no behavior runs.
    #[serde(
        default,
        skip_serializing_if = "crate::gimmicks::behavior::BehaviorGraph::is_unset"
    )]
    pub behavior: crate::gimmicks::behavior::BehaviorGraph,
    /// Editor-only character field positions. Coordinates never affect generated behavior.
    #[serde(
        default,
        skip_serializing_if = "crate::gimmicks::field_canvas::FieldLayout::is_empty"
    )]
    pub behavior_layout: crate::gimmicks::field_canvas::FieldLayout,
    /// Validated HUD bindings (slice F3.1). Rendering is intentionally a later
    /// slice; storing these now keeps the schema typed and backward compatible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meters: Vec<crate::gimmicks::meter::MeterBinding>,
    /// Additional named controller charts. The legacy `behavior` and `behavior_layout` fields
    /// remain the reserved `main` chart; IDs are validated by the controller project layer.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub controller_charts: BTreeMap<String, ControllerChart>,
}

impl FighterMod {
    /// Every move with an edit of any category.
    fn edited_moves(&self) -> impl Iterator<Item = &String> {
        self.acmd
            .keys()
            .chain(
                self.effect_calls
                    .iter()
                    .filter(|(_, edits)| !edits.is_empty())
                    .map(|(name, _)| name),
            )
            .chain(self.effect_calls_full.keys())
            .chain(self.effect_scripts.keys())
            .chain(self.sound_scripts.keys())
            .chain(self.expression_scripts.keys())
            .chain(self.move_graphs.keys())
    }

    /// Whether this fighter holds anything an export would build.
    ///
    /// Deliberately does not consult `effect_dropped_lines`. It is a note about what an export
    /// would lose, not an edit, so a fighter holding only that has nothing to build and must
    /// still count as empty — otherwise "No edits to export yet" turns into an export that
    /// produces no files.
    pub fn has_edits(&self) -> bool {
        !(self.acmd.is_empty()
            && self.effect_calls.values().all(|v| v.is_empty())
            && self.effect_calls_full.values().all(|v| v.is_empty())
            // An empty category script is an explicit replacement: it means the user
            // removed every call and wants the generated plugin to silence the stock
            // category. The map's presence, not the statement count, carries that intent.
            && self.sound_scripts.is_empty()
            && self.expression_scripts.is_empty()
            && self.effect_scripts.is_empty()
            && self.eff.as_ref().map(|e| e.is_empty()).unwrap_or(true)
            && self.live_tweaks.is_empty()
            && self.params.is_empty()
            && self.behavior.is_unset()
            && self.meters.is_empty()
            && self.move_graphs.is_empty()
            && self.controller_charts.is_empty())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EffMod {
    /// Source .eff path relative to the dump export root,
    /// e.g. "effect/fighter/mario/ef_mario.eff".
    pub source_rel: String,
    /// The file on disk the edits were made on, when the game path alone leads to another: an
    /// eff opened from a download or a mod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<std::path::PathBuf>,
    #[serde(default)]
    pub authored: Vec<AuthoredEdit>,
    /// Emitter sets copied in from a donor eff. Serialized as `one_slot` before the
    /// transplant/one-slot split; the alias keeps pre-split projects loadable.
    #[serde(default, alias = "one_slot")]
    pub transplants: Vec<TransplantOp>,
    /// Pool textures the user replaced with their own image.
    #[serde(default)]
    pub textures: Vec<TextureImport>,
    /// Pool textures the user ADDED, beyond the ones the eff shipped with.
    #[serde(default)]
    pub textures_added: Vec<TextureAddition>,
    /// Names of pool textures the user removed.
    #[serde(default)]
    pub textures_removed: Vec<String>,
    /// Emitter sets whose emitter LIST the user changed — emitters removed, duplicated or
    /// re-ordered. Recorded per set, and only for sets that differ from the source.
    #[serde(default)]
    pub rosters: Vec<EmitterRoster>,
    /// Effect entries whose spawn structure the user changed: which parts play at which frame,
    /// and which external model comes with them.
    #[serde(default)]
    pub entry_edits: Vec<EntryEdit>,
}

impl EffMod {
    /// Name the edited file `rel`. What was copied from the file itself still is: a part added
    /// from another of its effects, an effect borrowed within it.
    pub fn set_source_rel(&mut self, rel: String) {
        let slashed = |path: &str| path.replace('\\', "/").to_lowercase();
        let own = slashed(&std::mem::replace(&mut self.source_rel, rel));
        let from = self
            .rosters
            .iter_mut()
            .flat_map(|roster| &mut roster.slots)
            .flat_map(|slot| {
                let textures = slot.textures.iter_mut().map(|texture| &mut texture.file);
                std::iter::once(&mut slot.source_file).chain(textures)
            })
            .chain(self.transplants.iter_mut().map(|op| &mut op.src_file_rel));
        for file in from {
            if !own.is_empty() && slashed(file) == own {
                file.clone_from(&self.source_rel);
            }
        }
    }

    /// How many effects differ from the file: a part's values or texture, the part list, or the
    /// timing. Counted by effect name, so an effect with all three is one.
    pub fn changed_effects(&self) -> usize {
        let name = |entry: &str, set: &str| {
            if entry.is_empty() {
                format!("set:{set}")
            } else {
                entry.to_lowercase()
            }
        };
        let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();
        names.extend(
            self.authored
                .iter()
                .map(|e| name(&e.entry_name, &e.set_name)),
        );
        names.extend(
            self.rosters
                .iter()
                .map(|r| name(&r.entry_name, &r.set_name)),
        );
        names.extend(self.entry_edits.iter().map(|e| e.entry_name.to_lowercase()));
        names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.authored.is_empty()
            && self.transplants.is_empty()
            && self.textures.is_empty()
            && self.textures_added.is_empty()
            && self.textures_removed.is_empty()
            && self.rosters.is_empty()
            && self.entry_edits.is_empty()
    }
}

/// The emitter list one effect should end up with.
///
/// Declarative rather than a log of add/remove operations: each slot names an emitter of the
/// SOURCE set to clone, so applying the roster twice gives the same result as applying it once,
/// and a source eff that gained an emitter between sessions cannot shift the whole list by one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmitterRoster {
    pub set_name: String,
    /// The entry (kind) name that plays this set, for the UI and for carrier retargeting.
    #[serde(default)]
    pub entry_name: String,
    pub set_idx: usize,
    pub slots: Vec<EmitterSlot>,
}

/// One emitter in the result, and where its data comes from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EmitterSlot {
    /// Flat parent-first index of the SOURCE emitter this slot clones.
    pub source_idx: usize,
    /// Name of that source emitter, preferred over the index when both are available.
    #[serde(default)]
    pub source_name: String,
    /// Name this emitter carries in the result. A duplicate is renamed so that authored edits,
    /// which address emitters by name, can still tell the two apart.
    #[serde(default)]
    pub name: String,
    /// Nesting depth in the result: 0 is a root emitter, 1 a child of the slot above it.
    #[serde(default)]
    pub depth: u8,
    /// The emitter set the source emitter is in, when it is not this roster's own: a part added
    /// from another effect. Empty means this set, and `source_idx` indexes it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_set: String,
    /// The file that set is in, relative to the game folder: this roster's own for a part of
    /// another of its effects, `ef_common.eff` for a shared part. Empty means this roster's file.
    /// The part's textures, models and shaders come with it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_file: String,
    /// Textures it samples instead of its source's, for a part copied into another file after
    /// its texture was changed: the file it goes to need not have them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub textures: Vec<SlotTexture>,
}

/// A texture a copied part brings from another file than its source's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotTexture {
    /// Which of the part's texture slots samples it, 0 to 2.
    pub slot: usize,
    pub name: String,
    /// The file it is in, relative to the game folder.
    pub file: String,
}

impl EmitterSlot {
    /// Whether this slot copies a part from another effect or file rather than from its own set.
    pub fn is_foreign(&self) -> bool {
        !self.source_set.is_empty() || !self.source_file.is_empty()
    }
}

/// Changes to what one effect entry SPAWNS — the eff header's side of an effect, as opposed to
/// the particle data the emitters carry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryEdit {
    /// The entry (kind) name, e.g. `kirby_dash`. Entries are addressed by name only: the entry
    /// table's order is not stable across a transplant, which appends to it.
    pub entry_name: String,
    /// Replacement primary emitter set. `None` leaves it alone; `Some("")` removes it.
    /// Multi-part entries commonly have no primary set because their content lives in variants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitter_set: Option<String>,
    /// Replacement part list, each with its own start frame. `None` leaves the entry's parts
    /// alone; `Some(empty)` makes it a plain single-part effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<VariantEdit>>,
    /// Replacement external model. `None` leaves it alone; `Some` with an empty name removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelEdit>,
}

/// One part of a multi-part effect: an emitter set, when it starts, and what it hangs off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantEdit {
    /// Frames after the effect begins before this part comes in.
    pub start_frame: u16,
    /// Emitter set this part plays, by name. Empty means "no set", which the format stores as 0.
    pub set_name: String,
    /// Bone the part attaches to. Empty keeps the effect's own attachment.
    #[serde(default)]
    pub bone: String,
}

/// The model an effect spawns alongside its particles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelEdit {
    /// Model name as the eff's model table spells it. Empty removes the model.
    pub name: String,
    /// The model's spawn condition byte.
    #[serde(default)]
    pub flag: u8,
}

/// One pool texture replaced by an image of the user's.
///
/// Keyed by texture NAME, not pool index: the carrier and the exported eff both rebuild their
/// pools (pruning drops everything unreferenced), so an index recorded against the editor's
/// view names a different texture by the time it is applied. Names survive that.
///
/// The PNG is referenced by path rather than embedded, so the project file stays small and a
/// rebuild picks up whatever the image file says now — edit it in your paint program, send
/// again, and the new pixels ship without re-importing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextureImport {
    pub texture_name: String,
    pub png_path: String,
    /// The PNG holds the texture's STORED channels rather than the editable form.
    ///
    /// Recorded per import because the two are not interchangeable: reading an editable image as
    /// raw (or the reverse) puts the shape in the wrong channel, which the game draws as a solid
    /// square. Defaults to false, so a project written before this field existed is read as
    /// editable — the only form there was.
    #[serde(default)]
    pub raw: bool,
}

/// A pool texture the user ADDED, on top of the ones the eff shipped with.
///
/// This is how one effect gets a texture of its own. A pool texture is shared by every emitter
/// that samples it, and the `ef_cmn_*` names are shared by dozens of effects inside a single eff,
/// so editing one to change a single effect changes all of them. Adding a private copy and
/// repointing just that emitter is the only way to alter one effect and leave the rest alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextureAddition {
    /// The new pool texture's name — unique within the eff.
    pub texture_name: String,
    /// The existing pool texture this one is shaped like: it supplies the format, the channel
    /// swizzle and the required dimensions, and the pixels too when `png_path` is empty.
    pub template_name: String,
    /// Pixels from this PNG. Empty means "an exact copy of `template_name`", which is a rename
    /// rather than a re-encode and so loses nothing.
    #[serde(default)]
    pub png_path: String,
    /// The PNG holds stored channels rather than the editable form. See [`TextureImport::raw`].
    #[serde(default)]
    pub raw: bool,
}

/// One emitter's edited authored fields. Names are stored alongside indices; appliers
/// prefer the name match and fall back to the index with a warning (dump-revision drift).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthoredEdit {
    /// The EMITTER SET name (e.g. `P_KirbyDash`) — what `apply_authored` matches against
    /// `ptcl.emitter_list.emitter_sets[].name`.
    pub set_name: String,
    /// The ENTRY / KIND name (e.g. `kirby_dash`) — a DIFFERENT namespace: this is what the
    /// game spawns, what `TransplantOp::src_set_name` is resolved against (`entry_names`),
    /// and what an effect alias hashes. The two are related by
    /// `entries[i].emitter_set_id - 1 == set_idx`, not by position.
    ///
    /// Empty on projects saved before this field existed; callers that need a kind name must
    /// skip such edits rather than fall back to `set_name`, which would name a kind that does
    /// not exist and ship a carrier the game's loader can hang on.
    #[serde(default)]
    pub entry_name: String,
    pub set_idx: usize,
    pub emitter_name: String,
    pub emitter_idx: usize,
    pub fields: EmitterFieldEdits,
}

/// Absolute new values (pristine values are re-derivable from the source eff).
///
/// `attrs` is the general case and covers every field of the emitter; it is keyed by the stable
/// ids in [`crate::eff_attrs`]. The named fields below it predate that table and are kept for
/// projects saved before it existed — the editor re-records them as `attrs` the first time it
/// loads such a project, and both paths are applied (named first, so `attrs` wins a tie).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EmitterFieldEdits {
    /// Attribute id → new value, e.g. `"emission.rate": 12.5`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub attrs: std::collections::BTreeMap<String, crate::eff_attrs::AttrValue>,
    /// Sparse byte edits to emitter child sections. The section index disambiguates repeated
    /// magic values; the magic prevents a changed source file from applying bytes to the wrong
    /// section. Storing only changed offsets keeps projects small and avoids embedding source
    /// subsection blobs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subsections: Vec<SubsectionEdit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emission_rate: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scale: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitter_scale: Option<[f32; 3]>,
    /// Color key rows: [r, g, b, time]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color0: Option<Vec<[f32; 4]>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color1: Option<Vec<[f32; 4]>>,
    /// Alpha key rows: [value, time]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha0: Option<Vec<[f32; 2]>>,
    /// Pool texture NAME this emitter's `sampler0` should read instead of its original one.
    ///
    /// A name rather than the picker's index: the index is against the editor's view of the
    /// merged eff, and every path that ships these bytes rebuilds its own pool — the carrier
    /// prunes everything unreferenced, so indices there mean something else entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SubsectionEdit {
    pub index: usize,
    pub magic: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub bytes: std::collections::BTreeMap<usize, u8>,
}

impl EmitterFieldEdits {
    pub fn is_empty(&self) -> bool {
        self.attrs.is_empty()
            && self.subsections.is_empty()
            && self.emission_rate.is_none()
            && self.lifetime.is_none()
            && self.scale.is_none()
            && self.color_scale.is_none()
            && self.emitter_scale.is_none()
            && self.color0.is_none()
            && self.color1.is_none()
            && self.alpha0.is_none()
            && self.texture_name.is_none()
    }

    /// Number of edited fields (for the edit-tree badges).
    pub fn count(&self) -> usize {
        self.attrs.len()
            + self
                .subsections
                .iter()
                .map(|section| section.bytes.len())
                .sum::<usize>()
            + [
                self.emission_rate.is_some(),
                self.lifetime.is_some(),
                self.scale.is_some(),
                self.color_scale.is_some(),
                self.emitter_scale.is_some(),
                self.color0.is_some(),
                self.color1.is_some(),
                self.alpha0.is_some(),
                self.texture_name.is_some(),
            ]
            .iter()
            .filter(|b| **b)
            .count()
    }
}

/// A user-set runtime multiplier on one effect kind (from the live-override color×/speed
/// controls). Exports as LAST_EFFECT_SET_COLOR / LAST_EFFECT_SET_RATE after each spawn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveTweak {
    pub effect_name: String,
    /// [r, g, b, a] multiplier (alpha currently display-only in the export).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

/// Suffix used when the editor SUGGESTS a name for a transplanted entry.
///
/// This is a default for the name box only — the user may rename a transplant to anything,
/// so nothing downstream may depend on it. The plugin is told each transplant's real kind
/// mapping explicitly (`EffectAliasWire { from: hash(new_entry_name), to: donor }`), and
/// that is what makes an arbitrary name resolve in game. The plugin's suffix matching is
/// only an additive fallback, and it accepts the historical `_os` spelling too.
pub const TRANSPLANT_SUFFIX: &str = "_tp";

/// Prefix for the RUNTIME-ONLY clone of an edited fighter effect.
///
/// Authored edits cannot be applied to the fighter's own eff in a live match (its reparse
/// rebuilds from the resident buffer and never re-reads the file), so the edited entry is
/// cloned into the live carrier and the original kind is aliased onto the clone. That clone
/// needs a name, and it must live in a namespace the user can never reach:
///
///  * NOT the transplant suffix — transplants take ANY user-chosen name, so a user could
///    type the exact name the editor generated and the two would fight over one alias.
///  * Reserved: [`is_reserved_entry_name`] rejects it in the transplant name box, so the
///    collision is impossible by construction rather than merely unlikely.
///
/// This name never reaches an exported mod. On export the edits are applied to the fighter's
/// OWN eff under its ORIGINAL entry name (`rebuild_eff_bytes` → `apply_authored`); the clone
/// exists purely so the running game can be updated without a reload it does not support.
pub const EDIT_CLONE_PREFIX: &str = "vsnedit_";

/// True if `name` is in a namespace Visionary reserves for its own generated entries.
///
/// Used to keep user-chosen transplant names out of the editor's internal namespace.
pub fn is_reserved_entry_name(name: &str) -> bool {
    name.trim().to_lowercase().starts_with(EDIT_CLONE_PREFIX)
}

/// A TRANSPLANT: copy an emitter set from a donor eff into this fighter's eff, either
/// under a new entry name or replacing an existing entry in place.
///
/// `one_slot_slots` is the (optional) ONE-SLOT scoping layered on top of the transplant —
/// the transplant is the operation, the slot list narrows which costumes see it.
///
/// Was named `OneSlotOp` before the transplant/one-slot split. The struct name is invisible
/// to serde, but the renamed `one_slot_slots` field carries a `serde(alias)` for its old
/// on-disk name (`slots`) so projects saved before the split still load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransplantOp {
    pub new_entry_name: String,
    /// Donor .eff relative to the export root ("" = same file as the target).
    pub src_file_rel: String,
    pub src_set_name: String,
    pub src_set_idx: usize,
    /// ONE-SLOT scoping: costume slots this transplant applies to (0 = c00 …). Slot numbers
    /// are real costume indices and are NOT limited to 0–7 — added-costume mods use much
    /// larger ones. Empty = every costume: the transplant lands in the base
    /// ef_<fighter>.eff; otherwise it lands in ef_<fighter>_cXX.eff files, one per slot.
    /// Serialized as `slots` before the transplant/one-slot split.
    #[serde(default, alias = "slots", skip_serializing_if = "Vec::is_empty")]
    pub one_slot_slots: Vec<u8>,
    /// When set, the donor REPLACES this existing entry's emitter set(s) in place — every
    /// use switches, with no ACMD redirect needed — instead of being appended as a new
    /// entry. This is how a one-slot-scoped transplant is normally authored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_entry: Option<String>,
}

/// The game path (`effect/fighter/mario/ef_mario.eff`) an eff on disk stands for — what
/// [`EffMod::source_rel`] records and what export writes under the mod folder.
///
/// Relative to `root` when the file sits under it at a game path. Otherwise the path from its
/// last `effect` folder on, so an eff opened from a mod, a download or a project `romfs/` is
/// recorded the same way as the dump's copy. A bare `ef_<fighter>.eff` maps to that fighter's
/// path. Anything else keeps the full path, which export then refuses rather than writes:
/// an absolute `source_rel` once made `mod_dir.join` overwrite the user's own file.
pub fn eff_game_rel(path: &std::path::Path, root: Option<&std::path::Path>) -> String {
    use std::path::Component;
    let slashed = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/");
    // Windows paths split on Linux too: a project travels between the two.
    let full = slashed(path);
    let path = std::path::Path::new(&full);
    let under_root = root
        .and_then(|root| path.strip_prefix(slashed(root)).ok())
        .filter(|rel| !rel.as_os_str().is_empty())
        .map(slashed);
    if let Some(rel) = &under_root {
        if rel.to_ascii_lowercase().starts_with("effect/") {
            return rel.clone();
        }
    }
    let parts: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if let Some(i) = parts
        .iter()
        .rposition(|p| p.eq_ignore_ascii_case("effect"))
        .filter(|i| i + 1 < parts.len())
    {
        return parts[i..].join("/");
    }
    if let Some(rel) = under_root {
        return rel;
    }
    let file = path
        .file_name()
        .map(|f| f.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if let Some(fighter) = file
        .strip_prefix("ef_")
        .and_then(|f| f.strip_suffix(".eff"))
        .filter(|f| !f.is_empty())
    {
        // ef_mario_c03.eff is a one-slot file of mario's, not a fighter named mario_c03.
        let fighter = match fighter.rsplit_once("_c") {
            Some((name, slot))
                if !name.is_empty()
                    && slot.len() == 2
                    && slot.bytes().all(|b| b.is_ascii_digit()) =>
            {
                name
            }
            _ => fighter,
        };
        return format!("effect/fighter/{fighter}/{file}");
    }
    full
}

/// True when `source_rel` is a relative game path that stays inside whatever folder it is
/// joined to: no drive, no root, no `..`. Export writes nothing for any other value.
pub fn is_mod_relative(source_rel: &str) -> bool {
    !source_rel.is_empty()
        && !source_rel.contains(':')
        && !source_rel.starts_with(['/', '\\'])
        && !source_rel.split(['/', '\\']).any(|part| part == "..")
}

/// "effect/fighter/mario/ef_mario.eff" → "mario"; falls back to the file stem.
pub fn fighter_from_source_rel(source_rel: &str) -> String {
    let parts: Vec<&str> = source_rel.split('/').collect();
    if let Some(pos) = parts.iter().position(|p| *p == "fighter") {
        if let Some(name) = parts.get(pos + 1) {
            return (*name).to_string();
        }
    }
    std::path::Path::new(source_rel)
        .file_stem()
        .map(|s| s.to_string_lossy().trim_start_matches("ef_").to_string())
        .unwrap_or_else(|| source_rel.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Edited chip counts effects, whatever changed in them, once each: a part list or a
    /// timing change alone counts, and an effect with every kind of change is one.
    #[test]
    fn changed_effects_count_each_effect_once() {
        let edit = |entry: &str| AuthoredEdit {
            set_name: format!("P_{entry}"),
            entry_name: entry.into(),
            set_idx: 0,
            emitter_name: "em0".into(),
            emitter_idx: 0,
            fields: Default::default(),
        };
        let roster = |entry: &str| EmitterRoster {
            set_name: format!("P_{entry}"),
            entry_name: entry.into(),
            set_idx: 0,
            slots: Vec::new(),
        };
        let timing = |entry: &str| EntryEdit {
            entry_name: entry.into(),
            emitter_set: None,
            variants: None,
            model: None,
        };
        let eff = EffMod {
            authored: vec![edit("a"), edit("a"), edit("b")],
            rosters: vec![roster("b"), roster("c")],
            entry_edits: vec![timing("a"), timing("d")],
            ..Default::default()
        };
        assert_eq!(eff.changed_effects(), 4);
        assert_eq!(EffMod::default().changed_effects(), 0);
    }
    use crate::data::{EffectCall, EffectCallOp};

    /// A `modproject.json` exactly as builds before the transplant/one-slot rename wrote it:
    /// `EffMod.one_slot` holding `OneSlotOp`s whose costume scoping lived in `slots`.
    /// Both were renamed (`transplants` / `one_slot_slots`); the `serde(alias)`es are the
    /// only thing keeping the user's existing saved projects loadable, so this pins them.
    const LEGACY_PROJECT_JSON: &str = r#"{
      "version": 1,
      "name": "legacy_mod",
      "fighters": {
        "kirby": {
          "display": "Kirby",
          "acmd": {},
          "effect_calls": {},
          "effect_calls_full": {},
          "eff": {
            "source_rel": "effect/fighter/kirby/ef_kirby.eff",
            "authored": [],
            "one_slot": [
              {
                "new_entry_name": "alucard_backdash_os",
                "src_file_rel": "effect/assist/alucard/ef_alucard.eff",
                "src_set_name": "alucard_backdash",
                "src_set_idx": 3
              },
              {
                "new_entry_name": "kirby_swing_for_kirby_appeal",
                "src_file_rel": "",
                "src_set_name": "kirby_swing",
                "src_set_idx": 7,
                "slots": [0, 7, 12, 50],
                "replace_entry": "kirby_appeal"
              }
            ]
          },
          "live_tweaks": []
        }
      }
    }"#;

    #[test]
    fn legacy_project_file_still_deserializes() {
        let project: ModProjectFile =
            serde_json::from_str(LEGACY_PROJECT_JSON).expect("pre-rename project must still load");
        let eff = project.fighters["kirby"]
            .eff
            .as_ref()
            .expect("eff mod present");

        // `one_slot` → `transplants`: both ops survive, in order.
        assert_eq!(eff.transplants.len(), 2, "legacy `one_slot` array dropped");
        assert!(!eff.is_empty());
        assert_eq!(eff.transplants[0].new_entry_name, "alucard_backdash_os");
        assert_eq!(
            eff.transplants[0].src_file_rel,
            "effect/assist/alucard/ef_alucard.eff"
        );
        assert_eq!(eff.transplants[0].src_set_idx, 3);
        // Absent `slots` means "every costume" — no one-slot scoping.
        assert!(eff.transplants[0].one_slot_slots.is_empty());
        assert!(eff.transplants[0].replace_entry.is_none());

        // `slots` → `one_slot_slots`: the one-slot scoping on the second (replace) op,
        // including slots well past the vanilla c00–c07.
        assert_eq!(eff.transplants[1].one_slot_slots, vec![0, 7, 12, 50]);
        assert_eq!(
            eff.transplants[1].replace_entry.as_deref(),
            Some("kirby_appeal")
        );
    }

    #[test]
    fn legacy_project_round_trips_through_the_new_field_names() {
        let loaded: ModProjectFile = serde_json::from_str(LEGACY_PROJECT_JSON).unwrap();
        let written = serde_json::to_string(&loaded).unwrap();

        // New saves use the new on-disk names…
        assert!(written.contains("\"transplants\""), "{written}");
        assert!(written.contains("\"one_slot_slots\""), "{written}");
        assert!(!written.contains("\"one_slot\":"), "{written}");

        // …and reloading them yields the same project.
        let reloaded: ModProjectFile = serde_json::from_str(&written).unwrap();
        let a = reloaded.fighters["kirby"].eff.as_ref().unwrap();
        let b = loaded.fighters["kirby"].eff.as_ref().unwrap();
        assert_eq!(a.transplants.len(), b.transplants.len());
        for (x, y) in a.transplants.iter().zip(&b.transplants) {
            assert_eq!(x.new_entry_name, y.new_entry_name);
            assert_eq!(x.src_file_rel, y.src_file_rel);
            assert_eq!(x.src_set_name, y.src_set_name);
            assert_eq!(x.src_set_idx, y.src_set_idx);
            assert_eq!(x.one_slot_slots, y.one_slot_slots);
            assert_eq!(x.replace_entry, y.replace_entry);
        }
    }

    #[test]
    fn move_source_snapshot_round_trips_the_merged_body_and_typed_capture_tail() {
        let snapshot = MoveSourceSnapshot {
            body: "unsafe extern \"C\" fn game_fixture(...) {}".into(),
            captures: vec![crate::game_link::CaptureLine {
                kind: 7,
                motion: 0x1234,
                frame: 3.5,
                func: "ATTACK".into(),
                args: vec![
                    crate::game_link::LuaArgWire::Hash(0xfeed),
                    crate::game_link::LuaArgWire::Num(1.25),
                    crate::game_link::LuaArgWire::Int(-4),
                    crate::game_link::LuaArgWire::Bool(true),
                    crate::game_link::LuaArgWire::Nil,
                ],
                run: 9,
                common: false,
            }],
        };
        let mut project = ModProjectFile::default();
        project.fighters.insert(
            "mario".into(),
            FighterMod {
                move_sources: HashMap::from([("fixture".into(), snapshot.clone())]),
                ..Default::default()
            },
        );

        let json = serde_json::to_string(&project).unwrap();
        assert!(json.contains("game_fixture"));
        assert!(json.contains("\"captures\""));
        let loaded: ModProjectFile = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.fighters["mario"].move_sources["fixture"], snapshot);
        assert!(
            loaded.is_empty(),
            "source provenance alone is not a mod edit"
        );
    }

    #[test]
    fn legacy_version_one_projects_default_move_sources_empty() {
        let project: ModProjectFile = serde_json::from_str(LEGACY_PROJECT_JSON).unwrap();
        assert_eq!(project.version, 1);
        assert!(project.fighters["kirby"].move_sources.is_empty());
    }

    /// The one-slot side must not assume the vanilla 8 costumes anywhere in the format.
    #[test]
    fn one_slot_scoping_survives_slot_numbers_past_the_vanilla_eight() {
        let op = TransplantOp {
            new_entry_name: "donor_os".into(),
            src_file_rel: String::new(),
            src_set_name: "donor".into(),
            src_set_idx: 0,
            one_slot_slots: vec![8, 15, 16, 99, 255],
            replace_entry: Some("target".into()),
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: TransplantOp = serde_json::from_str(&json).unwrap();
        assert_eq!(back.one_slot_slots, vec![8, 15, 16, 99, 255]);
    }

    #[test]
    fn kinetic_acmd_points_survive_project_round_trip() {
        use crate::data::{
            AcmdScript, AcmdStmt, ClrSpeedCall, ExcuteStmt, WorkFlagAction, WorkFlagCall,
            WorkModuleIncIntCall, WorkModuleSetCall, WorkModuleSetKind, WorkTransitionTermAction,
            WorkTransitionTermCall,
        };

        let script = AcmdScript {
            stmts: vec![
                AcmdStmt::Frame(4.0),
                AcmdStmt::Excute(vec![
                    ExcuteStmt::ClrSpeed(ClrSpeedCall {
                        kinetic_kind: "*FIGHTER_KINETIC_ENERGY_ID_GRAVITY".into(),
                    }),
                    ExcuteStmt::SetAir,
                    ExcuteStmt::WorkFlag(WorkFlagCall {
                        action: WorkFlagAction::On,
                        flag: "*FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD".into(),
                    }),
                    ExcuteStmt::WorkTransitionTerm(WorkTransitionTermCall {
                        receiver: Default::default(),
                        action: WorkTransitionTermAction::Enable,
                        transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN".into(),
                    }),
                    ExcuteStmt::WorkModuleIncInt(WorkModuleIncIntCall {
                        slot: "*FIGHTER_STATUS_WORK_INT_NEXT_STEP".into(),
                    }),
                    ExcuteStmt::WorkModuleSet(WorkModuleSetCall {
                        kind: WorkModuleSetKind::Int,
                        receiver: "agent.module_accessor".into(),
                        value: "4".into(),
                        slot: "*FIGHTER_STATUS_WORK_INT_NEXT_STEP".into(),
                    }),
                    ExcuteStmt::WorkModuleSet(WorkModuleSetCall {
                        kind: WorkModuleSetKind::Int64,
                        receiver: "boma".into(),
                        value: "hash40(\"fall_damage\") as i64".into(),
                        slot: "FIGHTER_STATUS_FINAL_WORK_INT_REQUEST_LOOP_DAMAGE_MOTION".into(),
                    }),
                ]),
            ],
        };
        let mut project = ModProjectFile::default();
        project.fighters.insert(
            "mario".into(),
            FighterMod {
                acmd: HashMap::from([(
                    "attack_air_n".into(),
                    EditRecord {
                        fighter: "mario".into(),
                        fighter_display: "Mario".into(),
                        move_name: "attack_air_n".into(),
                        script: script.clone(),
                        hitboxes_pristine: Vec::new(),
                        hitboxes: Vec::new(),
                        slot_scope: None,
                        slot_scopes: Vec::new(),
                    },
                )]),
                ..Default::default()
            },
        );

        let written = serde_json::to_string(&project).unwrap();
        let reloaded: ModProjectFile = serde_json::from_str(&written).unwrap();
        let loaded = &reloaded.fighters["mario"].acmd["attack_air_n"].script;
        assert_eq!(loaded.to_clr_speed_events(), script.to_clr_speed_events());
        assert_eq!(loaded.to_set_air_events(), script.to_set_air_events());
        assert_eq!(loaded.to_work_flag_events(), script.to_work_flag_events());
        assert_eq!(
            loaded.to_work_transition_term_events(),
            script.to_work_transition_term_events()
        );
        assert_eq!(
            loaded.to_work_module_inc_int_events(),
            script.to_work_module_inc_int_events()
        );
        assert_eq!(
            loaded.to_work_module_set_events(),
            script.to_work_module_set_events()
        );
    }

    fn fixture_effect_call(name: &str) -> EffectCall {
        let source = format!(
            r#"
unsafe extern "C" fn effect_fixture(agent: &mut L2CAgentBase) {{
    if macros::is_excute(agent) {{
        macros::EFFECT(agent, Hash40::new("{name}"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);
    }}
}}
"#
        );
        crate::acmd::parse_effect_script(&source)
            .to_effect_calls()
            .into_iter()
            .next()
            .expect("fixture effect call")
    }

    fn fixture_sound_script() -> crate::data::AcmdScript {
        crate::acmd::parse_sound_script(
            r#"
unsafe extern "C" fn sound_fixture(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::PLAY_SE(agent, Hash40::new("se_common_swing"));
    }
}
"#,
        )
    }

    fn fixture_expression_script() -> crate::data::AcmdScript {
        crate::acmd::parse_expression_script(
            r#"
unsafe extern "C" fn expression_fixture(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::QUAKE(agent, *CAMERA_QUAKE_KIND_L);
    }
}
"#,
        )
    }

    #[test]
    fn acmd_scope_keeps_effect_only_sound_only_expression_only_and_mixed_moves() {
        let effect_only = fixture_effect_call("sys_flash");
        let mixed_effect = fixture_effect_call("sys_smash_flash");
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "category_scope".into(),
            ..Default::default()
        };
        project.fighters.insert(
            "mario".into(),
            FighterMod {
                effect_calls_full: HashMap::from([
                    ("effect_only".into(), vec![effect_only]),
                    ("mixed".into(), vec![mixed_effect]),
                ]),
                sound_scripts: HashMap::from([
                    ("sound_only".into(), fixture_sound_script()),
                    ("mixed".into(), fixture_sound_script()),
                ]),
                expression_scripts: HashMap::from([
                    ("expression_only".into(), fixture_expression_script()),
                    ("mixed".into(), fixture_expression_script()),
                ]),
                acmd: HashMap::from([(
                    "mixed".into(),
                    EditRecord {
                        fighter: "mario".into(),
                        fighter_display: "Mario".into(),
                        move_name: "mixed".into(),
                        script: crate::data::AcmdScript::default(),
                        hitboxes_pristine: Vec::new(),
                        hitboxes: Vec::new(),
                        slot_scope: None,
                        slot_scopes: Vec::new(),
                    },
                )]),
                eff: Some(EffMod {
                    source_rel: "effect/fighter/mario/ef_mario.eff".into(),
                    ..Default::default()
                }),
                live_tweaks: vec![LiveTweak {
                    effect_name: "SYS_SMASH_FLASH".into(),
                    color: Some([1.2, 1.0, 1.0, 1.0]),
                    speed: None,
                }],
                ..Default::default()
            },
        );

        let scoped = scope_acmd_project(&project, AcmdProjectScope::All);
        let fighter = &scoped.fighters["mario"];
        assert!(fighter.eff.is_none(), "ACMD scope must omit EFF edits");
        assert!(fighter.effect_calls_full.contains_key("effect_only"));
        assert!(fighter.sound_scripts.contains_key("sound_only"));
        assert!(fighter.expression_scripts.contains_key("expression_only"));
        assert!(fighter.acmd.contains_key("mixed"));
        assert_eq!(fighter.live_tweaks.len(), 1);
        assert_eq!(fighter.live_tweaks[0].effect_name, "SYS_SMASH_FLASH");
    }

    #[test]
    fn one_move_acmd_scope_isolates_every_category_and_filters_live_tweaks() {
        let mut project = ModProjectFile::default();
        let fighter = project.fighters.entry("mario".into()).or_default();
        fighter.eff = Some(EffMod {
            source_rel: "effect/fighter/mario/ef_mario.eff".into(),
            ..Default::default()
        });
        for name in ["first", "second"] {
            let call = fixture_effect_call(&format!("sys_{name}"));
            fighter.acmd.insert(
                name.into(),
                EditRecord {
                    fighter: "mario".into(),
                    fighter_display: "Mario".into(),
                    move_name: name.into(),
                    script: crate::data::AcmdScript::default(),
                    hitboxes_pristine: vec![],
                    hitboxes: vec![],
                    slot_scope: None,
                    slot_scopes: vec![],
                },
            );
            fighter.effect_calls.insert(
                name.into(),
                vec![EffectCallEdit {
                    index: 0,
                    op: EffectCallOp::Modify(call.clone()),
                    pristine: Some(call.clone()),
                }],
            );
            fighter.effect_calls_full.insert(name.into(), vec![call]);
            fighter
                .effect_dropped_lines
                .insert(name.into(), vec![format!("{name} loss")]);
            fighter.effect_frame_residue.insert(
                name.into(),
                BTreeMap::from([(4, vec![format!("{name} residue")])]),
            );
            fighter
                .sound_scripts
                .insert(name.into(), fixture_sound_script());
            fighter
                .expression_scripts
                .insert(name.into(), fixture_expression_script());
            fighter
                .capture_branch_warnings
                .insert(name.into(), format!("{name} warning"));
            fighter.move_sources.insert(
                name.into(),
                MoveSourceSnapshot {
                    body: format!("{name} source"),
                    ..Default::default()
                },
            );
            fighter.live_tweaks.push(LiveTweak {
                effect_name: format!("sys_{name}"),
                color: Some([1.1, 1.0, 1.0, 1.0]),
                speed: None,
            });
        }
        project.fighters.insert(
            "luigi".into(),
            FighterMod {
                eff: Some(EffMod {
                    source_rel: "effect/fighter/luigi/ef_luigi.eff".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let scoped = scope_acmd_project(
            &project,
            AcmdProjectScope::Move {
                fighter: "mario",
                move_name: "first",
            },
        );
        assert_eq!(scoped.fighters.len(), 1);
        let fighter = &scoped.fighters["mario"];
        assert!(fighter.eff.is_none());
        let encoded = serde_json::to_value(fighter).unwrap();
        for field in [
            "acmd",
            "effect_calls",
            "effect_calls_full",
            "effect_dropped_lines",
            "effect_frame_residue",
            "sound_scripts",
            "expression_scripts",
            "capture_branch_warnings",
            "move_sources",
        ] {
            assert_eq!(
                encoded[field]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                ["first"],
                "{field}"
            );
        }
        assert_eq!(fighter.move_sources["first"].body, "first source");
        assert_eq!(fighter.live_tweaks.len(), 1);
        assert_eq!(fighter.live_tweaks[0].effect_name, "sys_first");
        assert_eq!(project.fighters["mario"].acmd.len(), 2);
    }
}

/// Project format v3 — the roster and parameter sections.
///
/// Version handling is the one part of the project format whose failures are silent: a file
/// that loads with a section quietly missing looks healthy and re-saves with the user's work
/// gone. These pin both directions of that.
#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::gimmicks::{
        behavior, codegen, field_canvas::FieldLayout, params, paths::NestedPath, templates::Preset,
    };
    use serde_json::json;

    fn round_trip(project: &ModProjectFile) -> ModProjectFile {
        serde_json::from_value(serde_json::to_value(project).unwrap()).unwrap()
    }

    #[test]
    fn older_projects_default_new_sections_and_migrate_without_inventing_edits() {
        for version in 1..PROJECT_VERSION {
            let mut project: ModProjectFile = serde_json::from_value(json!({
                "version": version, "name": "old", "fighters": {"mario": {"display": "Mario"}}
            }))
            .unwrap();
            let fighter = &project.fighters["mario"];
            assert!(project.is_empty());
            assert!(project.roster.is_empty());
            assert!(fighter.params.is_empty());
            assert!(fighter.behavior.is_empty());
            assert!(fighter.behavior.emission_plan.is_empty());
            assert!(fighter.controller_charts.is_empty());
            project.migrate().unwrap();
            assert_eq!(project.version, PROJECT_VERSION);
            assert!(project.is_empty());
        }
        let mut future = ModProjectFile {
            version: PROJECT_VERSION + 1,
            ..Default::default()
        };
        assert!(future
            .migrate()
            .unwrap_err()
            .to_string()
            .contains(&future.version.to_string()));
        assert_eq!(future.version, PROJECT_VERSION + 1);
    }

    #[test]
    fn each_edit_section_alone_survives_serialization_and_makes_the_project_nonempty() {
        let chart = ControllerChart {
            graph: Preset::Timer.graph(),
            layout: FieldLayout::default(),
        };
        for field in [
            json!({"params": {"param/fighter_param.prc": {"weight": {"Float": 104.0}}}}),
            json!({"meters": [{"fighter": "cloud", "enabled": true, "style": "bar",
                "value": {"kind": "float", "symbol": "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE", "min": 0.0, "max": 100.0}}]}),
            json!({"behavior": Preset::Charge.graph()}),
            json!({"controller_charts": {"limit": chart}}),
        ] {
            let project: ModProjectFile = serde_json::from_value(
                json!({"version": PROJECT_VERSION, "name": "test", "fighters": {"cloud": field}}),
            )
            .unwrap();
            assert!(!project.is_empty());
            let restored = round_trip(&project);
            assert!(!restored.is_empty());
            assert_eq!(
                serde_json::to_value(restored).unwrap(),
                serde_json::to_value(project).unwrap()
            );
        }
        let mut roster_only = ModProjectFile::default();
        roster_only
            .roster
            .hidden
            .insert(crate::roster::RosterKey::fighter("ridley"));
        assert!(!round_trip(&roster_only).is_empty());
    }

    #[test]
    fn personal_edits_keep_owner_and_trait_keys_and_clear_at_the_base_value() {
        let mut project = ModProjectFile::default();
        for (owner, path, value) in [
            (
                "cloud",
                "param_special_lw[0].limit_gauge_add",
                ParamValue::Float(0.667),
            ),
            (
                "cloud",
                "param_special_lw[0].limit_max",
                ParamValue::I32(120),
            ),
            (
                "link",
                "param_special_n[0].bomb_damage",
                ParamValue::Float(7.0),
            ),
        ] {
            params::record_personal_edit(
                &mut project.fighters.entry(owner.into()).or_default().params,
                owner,
                &NestedPath::parse(path).unwrap(),
                value,
                None,
            )
            .unwrap();
        }
        project
            .fighters
            .entry("mario".into())
            .or_default()
            .params
            .files
            .insert(
                "param/fighter_param.prc".into(),
                BTreeMap::from([("weight".into(), ParamValue::Float(104.0))]),
            );
        let restored = round_trip(&project);
        for (owner, fighter) in &project.fighters {
            assert_eq!(restored.fighters[owner].params, fighter.params);
        }
        assert_eq!(
            params::personal_edits(&restored.fighters["cloud"].params, "cloud").len(),
            2
        );
        assert_eq!(
            params::personal_edits(&restored.fighters["link"].params, "link").len(),
            1
        );
        assert!(params::personal_edits(&restored.fighters["mario"].params, "mario").is_empty());

        let mut cleared = ModProjectFile::default();
        let edits = &mut cleared.fighters.entry("cloud".into()).or_default().params;
        let path = NestedPath::parse("param_special_lw[0].limit_gauge_add").unwrap();
        let value = ParamValue::Float(0.667);
        params::record_personal_edit(edits, "cloud", &path, value, None).unwrap();
        params::record_personal_edit(edits, "cloud", &path, value, Some(value)).unwrap();
        assert!(round_trip(&cleared).is_empty());
        let wire = serde_json::to_value(cleared).unwrap();
        assert!(wire["fighters"]["cloud"].get("params").is_none());

        let old: ModProjectFile =
            serde_json::from_value(json!({"version": 3, "name": "old", "fighters": {
                "mario": {"params": {"param/fighter_param.prc": {"weight": {"Float": 104.0}}}}
            }}))
            .unwrap();
        assert!(params::personal_edits(&old.fighters["mario"].params, "mario").is_empty());
        assert_eq!(
            old.fighters["mario"].params.files["param/fighter_param.prc"]["weight"],
            ParamValue::Float(104.0)
        );
    }

    #[test]
    fn saved_behavior_retains_actions_and_resource_sources_and_stays_exportable() {
        for source in [
            None,
            Some(codegen::ResourceSource::CloudLimitGroundedNeutral),
            Some(codegen::ResourceSource::CloudLimitIdleExpiry),
        ] {
            let mut graph = Preset::Charge.graph();
            graph.transitions[0]
                .actions
                .push(behavior::Action::RequestWeaponStatus {
                    target: crate::gimmicks::live_model::WeaponTarget {
                        battle_object_id: 0x2000_0001,
                    },
                    symbol: "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR".into(),
                });
            if let Some(source) = source {
                graph.transitions.truncate(1);
                graph.emission_plan = codegen::EmissionPlan {
                    bindings: vec![codegen::HookBinding {
                        hook: source.hook(),
                        article: None,
                        transitions: vec![0],
                    }],
                    resource_triggers: vec![codegen::ResourceTrigger {
                        hook: source.hook(),
                        source,
                    }],
                    ..Default::default()
                };
            }
            let mut project = ModProjectFile::default();
            project.fighters.entry("cloud".into()).or_default().behavior = graph.clone();
            let restored = round_trip(&project);
            assert_eq!(restored.fighters["cloud"].behavior, graph);
            assert!(behavior::validate_for_fighter("cloud", &graph).is_empty());
            crate::gimmicks::adapter::render(&BTreeMap::from([(
                "cloud".into(),
                restored.fighters["cloud"].behavior.clone(),
            )]))
            .unwrap();
        }
    }

    #[test]
    fn controller_graphs_and_layouts_keep_independent_identity_through_save_and_scope() {
        let chart = |name: &str, position| ControllerChart {
            graph: behavior::BehaviorGraph {
                initial: name.into(),
                states: vec![behavior::StateDef {
                    status: None,
                    name: name.into(),
                }],
                ..Default::default()
            },
            layout: FieldLayout {
                positions: BTreeMap::from([(format!("state/{name}"), position)]),
            },
        };
        let mut project = ModProjectFile::default();
        let fighter = project.fighters.entry("cloud".into()).or_default();
        let main = chart("main_idle", [1.0, 2.0]);
        fighter.behavior = main.graph;
        fighter.behavior_layout = main.layout;
        fighter.controller_charts = BTreeMap::from([
            ("limit".into(), chart("limit_idle", [30.0, 40.0])),
            ("charge".into(), chart("charge_idle", [300.0, 400.0])),
        ]);
        let restored = round_trip(&project);
        let original = &project.fighters["cloud"];
        assert_eq!(restored.fighters["cloud"].behavior, original.behavior);
        assert_eq!(
            restored.fighters["cloud"].behavior_layout,
            original.behavior_layout
        );
        assert_eq!(
            restored.fighters["cloud"].controller_charts,
            original.controller_charts
        );
        let scoped = scope_acmd_project(
            &restored,
            AcmdProjectScope::Move {
                fighter: "cloud",
                move_name: "attack11",
            },
        );
        assert_eq!(
            scoped.fighters["cloud"].controller_charts,
            original.controller_charts
        );
    }
    #[test]
    fn roster_and_param_edits_round_trip_and_keep_the_project_non_empty() {
        let key = crate::roster::RosterKey::slot("mario", 8);
        let mut project = ModProjectFile::default();
        project.roster.order.insert(key.clone(), 12);
        project.roster.names.insert(key.clone(), "Vision".into());
        project.roster.authored.push(AuthoredEntry {
            key: key.clone(),
            donor: "mario".into(),
            slot: 8,
            slots: Vec::new(),
            display_name: "Vision".into(),
            name_id: "vision".into(),
            moveset_scaffolded: false,
            files_root: None,
        });
        project
            .fighters
            .entry("mario".into())
            .or_default()
            .params
            .files
            .entry("param/fighter_param.prc".into())
            .or_default()
            .insert("weight".into(), ParamValue::Float(104.0));

        assert!(!project.is_empty());
        let json = serde_json::to_string(&project).unwrap();
        let reloaded: ModProjectFile = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded.roster, project.roster);
        assert_eq!(
            reloaded.fighters["mario"].params.files["param/fighter_param.prc"]["weight"],
            ParamValue::Float(104.0)
        );
    }

    #[test]
    fn edit_clone_prefix_matches_the_plugin_copy() {
        let plugin_src =
            include_str!("../plugins/slight_replica/src/slight/effect_viewer/acmd_hooks.rs");
        let declared = plugin_src
            .lines()
            .find_map(|l| l.trim().strip_prefix("const EDIT_CLONE_PREFIX: &str = "))
            .map(|l| l.trim_end_matches(';').trim_matches('"'))
            .expect("the plugin no longer declares EDIT_CLONE_PREFIX");
        assert_eq!(
            declared, EDIT_CLONE_PREFIX,
            "editor and plugin disagree on the reserved edit-clone prefix"
        );
    }
}

#[cfg(test)]
mod older_project_tests {
    use super::*;

    /// A project as a build of that version saved it: the same one, Mario with a gimmick and
    /// Vision made on his c07. `v7` is as the app saved it; the others are that file opened and
    /// saved by the build named (8baaee5, c44379d, b5dc441).
    fn saved(json: &str) -> ModProjectFile {
        serde_json::from_str(json).unwrap()
    }
    const V7: &str = include_str!("../tests/data/projects/v7.json");
    const V8: &str = include_str!("../tests/data/projects/v8.json");
    const V8_NEW_CHARACTER: &str = include_str!("../tests/data/projects/v8-new-character.json");
    const V8_FORK: &str = include_str!("../tests/data/projects/v8-fork.json");
    const V9_FORK_NEW_CHARACTER: &str =
        include_str!("../tests/data/projects/v9-fork-new-character.json");

    fn rows() -> BTreeSet<ProjectFighter> {
        BTreeSet::from([
            ProjectFighter::fighter("mario"),
            ProjectFighter::character("mario", "Vision"),
        ])
    }

    /// Which projects give a made character the donor's edits it used to share, and which get a
    /// fighter list from what they edit. Version 8 is two formats.
    #[test]
    fn each_saved_version_is_brought_to_the_current_one() {
        let vision = crate::roster::RosterKey::slot("mario", 7);
        // The list the fork's user left, with Mario taken out of it.
        let mut kept: serde_json::Value = serde_json::from_str(V8_FORK).unwrap();
        kept["project_fighters"].as_array_mut().unwrap().remove(0);
        let mut linked: serde_json::Value = serde_json::from_str(V8_FORK).unwrap();
        linked.as_object_mut().unwrap().remove("project_fighters");
        linked["moveset_source"] = "moveset".into();
        let mut v9: serde_json::Value = serde_json::from_str(V9_FORK_NEW_CHARACTER).unwrap();
        v9["project_fighters"].as_array_mut().unwrap().remove(0);
        let only_vision = BTreeSet::from([ProjectFighter::character("mario", "Vision")]);
        for (name, json, copied, list) in [
            ("v7", V7.to_string(), false, rows()),
            ("v8", V8.to_string(), false, rows()),
            (
                "v8, a new character",
                V8_NEW_CHARACTER.to_string(),
                false,
                rows(),
            ),
            ("the fork's v8", V8_FORK.to_string(), true, rows()),
            (
                "the fork's v8, a row removed",
                kept.to_string(),
                true,
                only_vision.clone(),
            ),
            (
                "the fork's v8, linked only",
                linked.to_string(),
                true,
                BTreeSet::new(),
            ),
            ("the fork's v9", v9.to_string(), false, only_vision),
        ] {
            let before = saved(&json);
            let mut project = saved(&json);
            let findings = project.migrate().unwrap();
            assert_eq!(project.version, 10, "{name}");
            crate::mod_export::validate_project(&project).unwrap();
            assert_eq!(project.project_fighters, list, "{name}");
            let own = project.characters.get(&vision);
            if copied {
                let mut expected = before.fighters["mario"].behavior.clone();
                expected.costumes = vec![7];
                assert_eq!(own.unwrap().behavior, expected, "{name}");
                assert_eq!(findings.len(), 1, "{name}: {findings:?}");
            } else {
                assert_eq!(
                    serde_json::to_value(own).unwrap(),
                    serde_json::to_value(before.characters.get(&vision)).unwrap(),
                    "{name}"
                );
                assert!(findings.is_empty(), "{name}: {findings:?}");
            }
            // Saved, it is a current project: opening it again changes nothing.
            let current = serde_json::to_value(&project).unwrap();
            let mut reopened: ModProjectFile = serde_json::from_value(current.clone()).unwrap();
            assert!(reopened.migrate().unwrap().is_empty(), "{name}");
            assert_eq!(serde_json::to_value(&reopened).unwrap(), current, "{name}");
        }
    }

    /// Whether the file was saved by a build that kept a fighter list and a moveset link in it,
    /// so a project without a link is one the user left without.
    #[test]
    fn only_some_versions_keep_a_link_of_their_own() {
        for (json, keeps) in [
            (V7, false),
            (V8, false),
            (V8_NEW_CHARACTER, false),
            (V8_FORK, true),
            (V9_FORK_NEW_CHARACTER, true),
        ] {
            assert_eq!(saved(json).keeps_scope(), keeps);
        }
        assert!(ModProjectFile::default().keeps_scope());
        for version in [1, 2, 3, 5, 6, 7] {
            assert!(!ModProjectFile {
                version,
                ..Default::default()
            }
            .keeps_scope());
        }
    }

    /// An effect file opened from a download by 8baaee5, one of its parts copied from another of
    /// its effects. The game path takes the place of the file's own, and the part is still the
    /// file's own; where the file is stays known.
    #[test]
    fn an_effect_file_opened_from_outside_keeps_its_file_and_its_own_parts() {
        const OPENED: &str = include_str!("../tests/data/projects/v8-effect-opened-outside.json");
        const MARIO: &str = "effect/fighter/mario/ef_mario.eff";
        const LINK: &str = "/mods/other/effect/fighter/link/ef_link.eff";
        let mut project = saved(OPENED);
        let eff = project
            .fighters
            .get_mut("mario")
            .unwrap()
            .eff
            .as_mut()
            .unwrap();
        let path = eff.source_rel.clone();
        assert_eq!(eff.rosters[0].slots[1].source_file, path);
        // A texture of the file's own given to a part copied from another, and one of neither.
        eff.rosters[0].slots[1].textures = [path.clone(), LINK.to_string()]
            .map(|file| SlotTexture {
                slot: 0,
                name: "ef_x".into(),
                file,
            })
            .to_vec();
        // Borrowed within the file, spelled as Windows does, and from another file.
        for file in [path.replace('/', "\\"), LINK.to_string()] {
            eff.transplants.push(TransplantOp {
                new_entry_name: "borrowed".into(),
                src_file_rel: file,
                src_set_name: "mario_fb_bound".into(),
                src_set_idx: 0,
                one_slot_slots: Vec::new(),
                replace_entry: None,
            });
        }
        project.migrate().unwrap();
        let eff = project.fighters["mario"].eff.as_ref().unwrap();
        assert_eq!(eff.source_rel, MARIO);
        assert_eq!(
            eff.source_path.as_deref(),
            Some(std::path::Path::new(&path))
        );
        assert_eq!(eff.rosters[0].slots[1].source_file, MARIO);
        assert!(eff.rosters[0].slots[0].source_file.is_empty());
        let files = eff.rosters[0].slots[1].textures.iter().map(|t| &t.file);
        assert_eq!(files.collect::<Vec<_>>(), [MARIO, LINK]);
        assert_eq!(eff.transplants[0].src_file_rel, MARIO);
        assert_eq!(eff.transplants[1].src_file_rel, LINK);
        // Saved and opened again, the file is still known.
        let saved_again = serde_json::to_string(&project).unwrap();
        let mut reopened = saved(&saved_again);
        reopened.migrate().unwrap();
        let eff = reopened.fighters["mario"].eff.as_ref().unwrap();
        assert_eq!(
            eff.source_path.as_deref(),
            Some(std::path::Path::new(&path))
        );
    }

    /// A move of the donor's that only its made character edits keeps the game's own version of
    /// it, wherever it is saved; so does one edited only as nodes.
    #[test]
    fn a_moves_baseline_is_kept_for_every_character_that_edits_it() {
        let mut project = saved(V8);
        let vision = project.characters.values_mut().next().unwrap();
        vision
            .sound_scripts
            .insert("attack_11".into(), crate::data::AcmdScript::default());
        vision
            .move_graphs
            .insert("attack_s3".into(), Default::default());
        let mario = project.fighters.get_mut("mario").unwrap();
        mario
            .effect_scripts
            .insert("attack_hi3".into(), crate::data::AcmdScript::default());
        mario.effect_calls.insert("attack_lw3".into(), Vec::new());
        let edited = project.edited_moves();
        let mut moves: Vec<_> = edited
            .iter()
            .map(|(fighter, name)| (fighter.as_str(), name.as_str()))
            .collect();
        moves.sort();
        assert_eq!(
            moves,
            [
                ("mario", "attack_11"),
                ("mario", "attack_hi3"),
                ("mario", "attack_s3")
            ]
        );
    }

    #[test]
    fn v3_project_loads_with_empty_scope() {
        let mut project: ModProjectFile =
            serde_json::from_str(r#"{"version":3,"name":"x"}"#).expect("v3 must still load");
        project.migrate().expect("v3 is supported");
        assert_eq!(project.version, PROJECT_VERSION);
        assert!(project.project_fighters.is_empty());
        assert!(project.moveset_source.is_none());
    }

    #[test]
    fn project_fighters_round_trip() {
        let mut project = ModProjectFile::default();
        project
            .project_fighters
            .insert(ProjectFighter::fighter("mario"));
        project
            .project_fighters
            .insert(ProjectFighter::character("pacman", "Shy Guy"));

        let json = serde_json::to_string(&ProjectFighter::fighter("mario")).unwrap();
        assert!(
            !json.contains("character"),
            "a fighter's own row has no character key: {json}"
        );

        let reloaded: ModProjectFile =
            serde_json::from_str(&serde_json::to_string(&project).unwrap()).unwrap();
        assert_eq!(reloaded.project_fighters, project.project_fighters);
    }

    #[test]
    fn seed_adds_edited_and_authored() {
        let mut project = ModProjectFile::default();
        project
            .fighters
            .entry("fox".into())
            .or_default()
            .sound_scripts
            .insert("attack_s3_s".into(), crate::data::AcmdScript::default());
        project
            .fighters
            .insert("falco".into(), FighterMod::default());
        project.roster.authored.push(AuthoredEntry {
            key: crate::roster::RosterKey::slot("pacman", 8),
            donor: "pacman".into(),
            slot: 8,
            slots: Vec::new(),
            display_name: "Shy Guy".into(),
            name_id: "shy_guy".into(),
            moveset_scaffolded: false,
            files_root: None,
        });

        project.seed_project_fighters();
        let expected: std::collections::BTreeSet<ProjectFighter> = [
            ProjectFighter::fighter("fox"),
            ProjectFighter::character("pacman", "Shy Guy"),
        ]
        .into_iter()
        .collect();
        assert_eq!(project.project_fighters, expected);

        project.seed_project_fighters();
        assert_eq!(project.project_fighters, expected);
    }

    fn with_edited_fox(version: u32) -> ModProjectFile {
        let mut project = ModProjectFile {
            version,
            ..Default::default()
        };
        project
            .fighters
            .entry("fox".into())
            .or_default()
            .sound_scripts
            .insert("attack_s3_s".into(), crate::data::AcmdScript::default());
        project
    }

    #[test]
    fn v3_project_is_seeded_on_migrate() {
        let mut project = with_edited_fox(3);
        project.migrate().unwrap();
        assert!(project
            .project_fighters
            .contains(&ProjectFighter::fighter("fox")));
    }

    #[test]
    fn common_leap_v7_project_is_seeded_on_migrate() {
        let mut project = with_edited_fox(7);
        project.migrate().unwrap();
        assert!(project
            .project_fighters
            .contains(&ProjectFighter::fighter("fox")));
    }

    #[test]
    fn v4_project_is_not_reseeded() {
        // A v4 file's list is the user's own: fox was removed while it still had edits.
        let mut project = with_edited_fox(4);
        project.migrate().unwrap();
        assert!(project.project_fighters.is_empty());
    }

    #[test]
    fn scope_only_project_is_empty() {
        let mut project = ModProjectFile::default();
        project
            .project_fighters
            .insert(ProjectFighter::fighter("mario"));
        project.moveset_source = Some("C:/mods/my_moveset".into());
        assert!(project.is_empty());
    }
}

#[cfg(test)]
mod eff_game_rel_tests {
    use super::*;
    use std::path::Path;

    const MARIO: &str = "effect/fighter/mario/ef_mario.eff";

    #[test]
    fn an_eff_under_the_root_keeps_its_game_path() {
        let root = Path::new("C:/dump");
        assert_eq!(eff_game_rel(&root.join(MARIO), Some(root)), MARIO);
        assert_eq!(
            eff_game_rel(
                Path::new(r"C:\dump\custom\a.eff"),
                Some(Path::new(r"C:\dump"))
            ),
            "custom/a.eff"
        );
    }

    /// The bug: File → Open Effect File on a mod's eff recorded `C:/mods/…/ef_mario.eff`.
    #[test]
    fn an_eff_outside_the_root_is_recorded_from_its_effect_folder() {
        let root = Path::new("C:/dump");
        for outside in [
            "C:/Users/me/Downloads/coolmod/effect/fighter/mario/ef_mario.eff",
            r"D:\mods\coolmod\romfs\effect\fighter\mario\ef_mario.eff",
            "C:/dump/../elsewhere/Effect/fighter/mario/ef_mario.eff",
        ] {
            let rel = eff_game_rel(Path::new(outside), Some(root));
            assert!(rel.eq_ignore_ascii_case(MARIO), "{outside} → {rel}");
            assert!(is_mod_relative(&rel), "{outside} → {rel}");
        }
    }

    /// A root that wraps the game folders (`<mod>/romfs`) still yields the game path.
    #[test]
    fn a_wrapper_under_the_root_is_dropped() {
        let root = Path::new("C:/mod");
        let path = root.join("romfs").join(MARIO);
        assert_eq!(eff_game_rel(&path, Some(root)), MARIO);
    }

    #[test]
    fn a_bare_fighter_eff_maps_to_that_fighters_path() {
        assert_eq!(
            eff_game_rel(Path::new("C:/Users/me/Downloads/ef_Mario.eff"), None),
            MARIO
        );
        assert_eq!(
            eff_game_rel(Path::new(r"C:\Users\me\Downloads\ef_mario.eff"), None),
            MARIO
        );
        assert_eq!(
            eff_game_rel(Path::new("C:/Users/me/Downloads/ef_mario_c03.eff"), None),
            "effect/fighter/mario/ef_mario_c03.eff"
        );
    }

    /// No game path to derive: kept as it is, and `is_mod_relative` says so — that is what
    /// makes export refuse it instead of writing over it.
    #[test]
    fn an_eff_with_no_game_path_stays_absolute_and_unexportable() {
        let path = Path::new("C:/Users/me/Downloads/custom.eff");
        let rel = eff_game_rel(path, Some(Path::new("C:/dump")));
        assert_eq!(rel, "C:/Users/me/Downloads/custom.eff");
        assert!(!is_mod_relative(&rel));
    }

    #[test]
    fn only_game_paths_are_mod_relative() {
        assert!(is_mod_relative(MARIO));
        for bad in [
            "",
            "C:/Users/me/ef_mario.eff",
            r"C:\Users\me\ef_mario.eff",
            "/home/me/ef_mario.eff",
            r"\\server\share\ef_mario.eff",
            "effect/../../outside.eff",
            r"..\outside.eff",
        ] {
            assert!(!is_mod_relative(bad), "{bad:?} must not be exportable");
        }
    }

    /// A project saved by an older build carries the absolute path; loading it rewrites the
    /// path to the game path, and leaves one with no game path for export to refuse.
    #[test]
    fn migrate_rewrites_absolute_eff_sources() {
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            ..Default::default()
        };
        for (fighter, source) in [
            (
                "mario",
                "C:/Users/me/mods/x/effect/fighter/mario/ef_mario.eff",
            ),
            ("custom", "C:/Users/me/custom.eff"),
            ("link", "effect/fighter/link/ef_link.eff"),
        ] {
            project.fighters.entry(fighter.into()).or_default().eff = Some(EffMod {
                source_rel: source.into(),
                ..Default::default()
            });
        }
        project.migrate().unwrap();
        let source = |f: &str| project.fighters[f].eff.as_ref().unwrap().source_rel.clone();
        assert_eq!(source("mario"), MARIO);
        assert_eq!(source("custom"), "C:/Users/me/custom.eff");
        assert_eq!(source("link"), "effect/fighter/link/ef_link.eff");
    }
}

#[cfg(test)]
mod made_character_tests {
    use super::*;

    fn with_vision_edit() -> ModProjectFile {
        let mut project = ModProjectFile::default();
        project.roster.authored.push(AuthoredEntry {
            key: crate::roster::RosterKey::slot("mario", 8),
            donor: "mario".into(),
            slot: 8,
            slots: Vec::new(),
            display_name: "Vision".into(),
            name_id: "vision".into(),
            moveset_scaffolded: false,
            files_root: None,
        });
        project
            .characters
            .entry(crate::roster::RosterKey::slot("mario", 8))
            .or_default()
            .sound_scripts
            .insert("attack_11".into(), crate::data::AcmdScript::default());
        project
    }

    /// A project whose only edit is a made character's is not empty: it exports.
    #[test]
    fn a_made_characters_edit_makes_the_project_non_empty() {
        let mut project = with_vision_edit();
        let authored = std::mem::take(&mut project.roster.authored);
        assert!(!project.is_empty());
        project.characters.clear();
        assert!(project.is_empty());
        project.roster.authored = authored;
        assert!(
            !project.is_empty(),
            "the roster entry alone is still an edit"
        );
    }

    /// One move of a made character exports with that character and its costumes; a whole
    /// project keeps every character.
    #[test]
    fn acmd_scopes_keep_made_characters() {
        let project = with_vision_edit();
        let one = scope_acmd_project(
            &project,
            AcmdProjectScope::Move {
                fighter: "mario#c08",
                move_name: "attack_11",
            },
        );
        assert!(one.fighters.is_empty());
        assert!(one.characters[&crate::roster::RosterKey::slot("mario", 8)]
            .sound_scripts
            .contains_key("attack_11"));
        assert_eq!(one.roster.authored.len(), 1);
        let all = scope_acmd_project(&project, AcmdProjectScope::All);
        assert_eq!(all.characters.len(), 1);
        assert_eq!(all.roster.authored.len(), 1);
    }
}
