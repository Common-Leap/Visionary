//! Pure helpers shared by the console-package and developer export paths.

use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::acmd::ModProject;
use crate::mod_project::{EffMod, LiveTweak, ModProjectFile, PROJECT_VERSION};

/// A filesystem-, Cargo-, and Rust-friendly project name.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut separator = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            if separator && !out.is_empty() {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if out.is_empty() {
        out.push_str("visionary_mod");
    }
    if out.as_bytes()[0].is_ascii_digit() {
        out.insert_str(0, "mod_");
    }
    out
}

pub fn plugin_name(project: &ModProjectFile) -> String {
    format!("{}_plugin", slugify(&project.name))
}

/// True when the base EFF must be written. Costume-scoped transplants are the only edit class
/// that belongs exclusively in cXX files; every texture operation is a base-file edit too.
pub fn base_eff_has_content(eff: &EffMod) -> bool {
    !eff.authored.is_empty()
        || !eff.textures.is_empty()
        || !eff.textures_added.is_empty()
        || !eff.textures_removed.is_empty()
        || !eff.rosters.is_empty()
        || !eff.entry_edits.is_empty()
        || eff
            .transplants
            .iter()
            .any(|operation| operation.one_slot_slots.is_empty())
}

/// A generated source project, and what verification had to say about it short of refusing it.
///
/// The warnings are returned rather than logged because this function is the only thing that
/// ever sees them and the user is the only one they are for. Until C6c they were built and
/// dropped on the floor here: verification produced a full loss report on every export and
/// `source_project` read one bit of it — whether anything was fatal. An export that quietly
/// deletes a line the editor could not model was reported to nobody.
pub struct GeneratedSource {
    pub project: ModProject,
    /// Ready-to-show lines, already capped. Empty when the export carried everything.
    pub warnings: Vec<String>,
}

/// A runtime choice prepared before a normal mod-folder export touches its
/// destination.
///
/// Without an explicit runtime selection, a project remains on Visionary's
/// legacy standalone ACMD path. With an explicit compatible selection,
/// ordinary ACMD and gimmick behavior are rendered into two fixed slots inside
/// that runtime and compile into one `plugin.nro`.
pub struct NormalExportPlan {
    /// The ordinary standalone ACMD source, when no combined runtime was
    /// selected. It is always `None` for a combined plan so callers cannot
    /// accidentally build a second plugin.
    pub generated_source: Option<GeneratedSource>,
    /// The caller-selected, contract-verified combined runtime. Its package
    /// method writes both generated slots transactionally and builds the one
    /// package plugin.
    pub gimmick_runtime: Option<SelectedGimmickRuntime>,
    /// Ordinary source verification warnings, whether that source will be
    /// built standalone or embedded in the selected runtime.
    pub warnings: Vec<String>,
    /// Whether ACMD/effect/sound/expression source was rendered for this plan.
    pub has_acmd_source: bool,
}

/// A prepared static gimmick artifact plus the one plugin root the caller
/// explicitly selected. The path is never discovered, persisted, or replaced
/// by this module.
#[derive(Debug, Clone)]
pub struct SelectedGimmickRuntime {
    plugin_root: PathBuf,
    behavior: crate::gimmicks::adapter::RenderedAdapter,
    acmd: crate::gimmicks::acmd_adapter::RenderedAcmdAdapter,
}

impl SelectedGimmickRuntime {
    /// The rendered, parse-checked behavior/meter adapter that will be put in
    /// the plugin's one managed source slot during packaging.
    pub fn artifact(&self) -> &crate::gimmicks::adapter::RenderedAdapter {
        &self.behavior
    }

    /// The parse-checked ACMD module embedded in this runtime. It is a no-op
    /// module when this export has no ordinary script edits, deliberately
    /// clearing any prior Visionary-managed ACMD registration slot.
    pub fn acmd_artifact(&self) -> &crate::gimmicks::acmd_adapter::RenderedAcmdAdapter {
        &self.acmd
    }

    /// Recompile this selected package plan from the current project and
    /// selected plugin contract without writing either source slot. Callers
    /// use this immediately before the first destination-side effect.
    pub fn validate_for_project(&self, project: &ModProjectFile) -> Result<()> {
        let fresh = plan_normal_export(project, Some(&self.plugin_root))?;
        let Some(fresh_runtime) = fresh.gimmick_runtime else {
            bail!("selected controller runtime is no longer part of the export plan");
        };
        if fresh_runtime.plugin_root != self.plugin_root
            || fresh_runtime.behavior != self.behavior
            || fresh_runtime.acmd != self.acmd
        {
            bail!("selected controller package plan is stale for the current project/runtime");
        }
        Ok(())
    }

    /// Build this explicit runtime into an existing mod folder whose
    /// `plugin.nro` slot is absent. The package helper owns all source
    /// replacement, no-clobber, and rollback guarantees.
    pub fn build_into_package(
        &self,
        package_root: &Path,
    ) -> Result<crate::gimmicks::package::PackagedPlugin, crate::gimmicks::package::PackageError>
    {
        crate::gimmicks::package::build_combined_into_package(
            &self.plugin_root,
            package_root,
            &self.behavior,
            &self.acmd,
        )
    }

    #[cfg(test)]
    fn package_with<F>(
        &self,
        package_root: &Path,
        build: F,
    ) -> Result<crate::gimmicks::package::PackagedPlugin, crate::gimmicks::package::PackageError>
    where
        F: FnOnce(&Path) -> Result<(), crate::gimmicks::package::PackageError>,
    {
        crate::gimmicks::package::package_combined_with(
            &self.plugin_root,
            package_root,
            &self.behavior,
            &self.acmd,
            build,
        )
    }
}

/// Preflight the one plugin package a normal export may create.
///
/// `selected_gimmick_plugin_root` is intentionally an argument rather than a
/// configuration lookup: callers must obtain and pass the user's explicit
/// selection. If active gimmicks have no selection, or if the selected runtime
/// is not the reviewed two-slot contract, this returns before any export
/// destination or plugin source is changed.
///
/// Coordinator/UI wiring should call this before staging a mod folder, then
/// use [`SelectedGimmickRuntime::build_into_package`] after the fresh folder
/// exists. No selection means legacy non-gimmick projects continue through the
/// standalone ACMD exporter unchanged. Passing a selected root opts normal
/// ACMD into the one-NRO combined runtime even when it has no gimmicks.
pub fn plan_normal_export(
    project: &ModProjectFile,
    selected_gimmick_plugin_root: Option<&Path>,
) -> Result<NormalExportPlan> {
    if selected_gimmick_plugin_root.is_none() {
        refuse_uninstalled_controller_charts(project)?;
    }
    let expanded = crate::move_nodes::project::expand(project).map_err(node_errors)?;
    let project = &expanded;
    let active_gimmicks = active_gimmicks(project);
    if selected_gimmick_plugin_root.is_none() {
        if !active_gimmicks.is_empty() {
            bail!(
                "active gimmicks for {} require an explicitly selected compatible plugin source; normal export was refused so it cannot silently omit them",
                active_gimmicks.join(", ")
            );
        }
        let generated_source = source_project_for_standalone(project)?;
        let warnings = generated_source
            .as_ref()
            .map(|source| source.warnings.clone())
            .unwrap_or_default();
        let has_acmd_source = generated_source.is_some();
        return Ok(NormalExportPlan {
            generated_source,
            gimmick_runtime: None,
            warnings,
            has_acmd_source,
        });
    }

    let plugin_root = selected_gimmick_plugin_root.expect("checked above");
    let ordinary_source = source_project_for_selected_runtime(project)?;
    let warnings = ordinary_source
        .as_ref()
        .map(|source| source.warnings.clone())
        .unwrap_or_default();
    let has_acmd_source = ordinary_source.is_some();

    let graphs = project
        .fighters
        .iter()
        .map(|(fighter, edits)| (fighter.clone(), edits.behavior.clone()))
        .collect();
    let controllers = project
        .fighters
        .iter()
        .filter_map(|(fighter, edits)| {
            (!edits.controller_charts.is_empty())
                .then_some((fighter.clone(), edits.controller_charts.clone()))
        })
        .collect();
    let meters = project
        .fighters
        .iter()
        .filter_map(|(fighter, edits)| {
            (!edits.meters.is_empty()).then_some((fighter.clone(), edits.meters.clone()))
        })
        .collect();
    let artifact =
        crate::gimmicks::adapter::render_with_controllers(&graphs, &controllers, &meters).map_err(
            |errors| {
                anyhow::anyhow!(
                    "gimmick runtime was not rendered:\n{}",
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            },
        )?;
    let acmd = crate::gimmicks::acmd_adapter::render(
        ordinary_source.as_ref().map(|source| &source.project),
    )
    .map_err(|error| anyhow::anyhow!("combined ACMD runtime was not rendered: {error}"))?;
    crate::gimmicks::adapter::verify_plugin_for_combined_artifacts(plugin_root, &artifact, &acmd)
        .map_err(|error| anyhow::anyhow!("selected gimmick plugin is not compatible: {error}"))?;

    Ok(NormalExportPlan {
        generated_source: None,
        gimmick_runtime: Some(SelectedGimmickRuntime {
            plugin_root: plugin_root.to_path_buf(),
            behavior: artifact,
            acmd,
        }),
        warnings,
        has_acmd_source,
    })
}

/// Convert every ACMD/effect-call record in a saved project into one buildable Skyline source
/// project. Effect-call deltas are not sufficient by themselves because the generated script
/// replaces the entire original effect script.
pub fn source_project(project: &ModProjectFile) -> Result<Option<GeneratedSource>> {
    refuse_uninstalled_controller_charts(project)?;
    let expanded = crate::move_nodes::project::expand(project).map_err(node_errors)?;
    let project = &expanded;
    let active_gimmicks = active_gimmicks(project);
    if !active_gimmicks.is_empty() {
        bail!(
            "the standard ACMD package cannot include active gimmicks for {} without an explicitly selected compatible plugin source. Use plan_normal_export with that selection; this export was refused so it cannot silently omit them",
            active_gimmicks.join(", ")
        );
    }
    render_ordinary_source(project)
}

/// Preserve the standalone source-export gate at its private plan boundary as
/// well as at the public API. The selected-runtime path has its own explicitly
/// named entry point below and is the only chart-capable caller.
fn source_project_for_standalone(project: &ModProjectFile) -> Result<Option<GeneratedSource>> {
    refuse_uninstalled_controller_charts(project)?;
    render_ordinary_source(project)
}

/// Render ordinary ACMD source after selected-runtime planning has chosen the
/// combined package path. Controller charts are carried separately by the
/// selected behavior artifact and never pass through this ACMD renderer.
fn source_project_for_selected_runtime(
    project: &ModProjectFile,
) -> Result<Option<GeneratedSource>> {
    render_ordinary_source(project)
}

/// Refuse source/runtime exports while saved named controller charts are not
/// represented in the selected runtime. Silently exporting only the legacy
/// main graph would otherwise discard authored chart behavior from the mod.
/// Project-file save and validation deliberately do not use this guard.
pub fn refuse_uninstalled_controller_charts(project: &ModProjectFile) -> Result<()> {
    let mut charts = project
        .fighters
        .iter()
        .flat_map(|(fighter, edits)| {
            edits
                .controller_charts
                .keys()
                .map(move |chart| (fighter.as_str(), chart.as_str()))
        })
        .collect::<Vec<_>>();
    charts.sort_unstable();
    if !charts.is_empty() {
        let details = charts
            .into_iter()
            .map(|(fighter, chart)| format!("{fighter}/{chart}"))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "this export format cannot carry additional controller charts: {details}. Use Export Mod Folder with an explicitly selected compatible SLight source to include them"
        );
    }
    Ok(())
}

fn node_errors(errors: Vec<crate::move_nodes::model::NodeError>) -> anyhow::Error {
    anyhow::anyhow!(
        "move nodes were not compiled:\n{}",
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// List all behavior/meter declarations that would require the static gimmick
/// runtime, in deterministic order for diagnostics and tests.
fn active_gimmicks(project: &ModProjectFile) -> Vec<String> {
    let mut active = project
        .fighters
        .iter()
        .filter_map(|(fighter, edits)| {
            let behavior = !edits.behavior.is_empty();
            let meters = edits.meters.iter().any(|binding| binding.enabled);
            (behavior || meters).then(|| {
                format!(
                    "{fighter} ({})",
                    match (behavior, meters) {
                        (true, true) => "behavior and meter",
                        (true, false) => "behavior",
                        (false, true) => "meter",
                        (false, false) => unreachable!(),
                    }
                )
            })
        })
        .collect::<Vec<_>>();
    active.sort();
    active
}

/// The established standalone ACMD source renderer after the caller has
/// already selected a non-conflicting package runtime. Keeping the gimmick gate
/// outside this function lets [`plan_normal_export`] validate a potential ACMD
/// collision using exactly the same source-emitting semantics as the normal
/// exporter.
fn render_ordinary_source(project: &ModProjectFile) -> Result<Option<GeneratedSource>> {
    let mut acmd_edits = Vec::new();
    let mut effect_edits = Vec::new();
    let mut effect_scripts = Vec::new();
    let mut sound_edits = Vec::new();
    let mut expression_edits = Vec::new();
    let mut tweaks: Vec<LiveTweak> = Vec::new();
    let mut dropped: HashMap<String, Vec<String>> = HashMap::new();
    let mut incomplete = Vec::new();
    let mut exported_effect_names = HashSet::new();
    let mut capture_warnings = Vec::new();
    let mut slot_gates = crate::acmd::SlotGates::new();

    let mut fighters: Vec<_> = project.fighters.iter().collect();
    fighters.sort_by(|a, b| a.0.cmp(b.0));
    for (fighter, edits) in fighters {
        let mut moves: Vec<_> = edits.acmd.iter().collect();
        moves.sort_by(|a, b| a.0.cmp(b.0));
        for (move_name, record) in moves {
            // A move scoped to costumes needs the fighter's own script as the other arm.
            // Without it the generated script would remove this move from every other costume
            // of the fighter, so a missing original is refused rather than exported wide.
            let scopes = record.effective_scopes();
            if !scopes.is_empty() {
                let original_body = edits
                    .move_sources
                    .get(move_name)
                    .map(|snapshot| snapshot.body.trim())
                    .filter(|body| !body.is_empty());
                let Some(body) = original_body else {
                    bail!(
                        "{fighter}'s {move_name} is set to costume(s) {}, but the fighter's own version of it was not saved with the project. Exporting it now would remove {move_name} from every other costume of {fighter}. Open the move in the editor once so its original is recorded, then export again.",
                        format_slots(&scopes)
                    )
                };
                let game_original = matching_category_original(body, fighter, move_name, "game")?;
                slot_gates.insert(
                    (fighter.clone(), move_name.clone()),
                    (scopes.clone(), game_original),
                );

                for (category, replaced) in [
                    (
                        "effect",
                        edits.effect_calls_full.contains_key(move_name)
                            || edits.effect_scripts.contains_key(move_name),
                    ),
                    ("sound", edits.sound_scripts.contains_key(move_name)),
                    (
                        "expression",
                        edits.expression_scripts.contains_key(move_name),
                    ),
                ] {
                    if !replaced {
                        continue;
                    }
                    let original = matching_category_original(body, fighter, move_name, category)?;
                    slot_gates.insert(
                        (fighter.clone(), format!("{category}:{move_name}")),
                        (scopes.clone(), original),
                    );
                }
            }
            acmd_edits.push((fighter.clone(), move_name.clone(), record.script.clone()));
        }

        let mut effect_moves: Vec<_> = edits.effect_calls_full.iter().collect();
        effect_moves.sort_by(|a, b| a.0.cmp(b.0));
        for (move_name, calls) in effect_moves {
            if let Some(snapshot) = edits.move_sources.get(move_name) {
                let original = crate::acmd::parse_effect_script(&snapshot.body);
                if let Some(reason) = original.preview_budget_error() {
                    bail!("{fighter}/{move_name}: legacy Effect source cannot be flattened safely: {reason}. Retain the source or reduce its repeat expansion before exporting");
                }
            }
            let calls = calls
                .iter()
                .cloned()
                .map(crate::data::EffectCall::normalized_timing)
                .collect::<Vec<_>>();
            exported_effect_names.extend(
                calls
                    .iter()
                    .map(|call| call.effect_name.to_ascii_lowercase()),
            );
            effect_edits.push((
                fighter.clone(),
                move_name.clone(),
                calls,
                edits
                    .effect_frame_residue
                    .get(move_name)
                    .cloned()
                    .unwrap_or_default(),
            ));
        }
        let mut structured_effects: Vec<_> = edits.effect_scripts.iter().collect();
        structured_effects.sort_by(|a, b| a.0.cmp(b.0));
        for (move_name, script) in structured_effects {
            if edits.effect_calls_full.contains_key(move_name)
                || edits
                    .effect_calls
                    .get(move_name)
                    .is_some_and(|calls| !calls.is_empty())
                || edits
                    .effect_frame_residue
                    .get(move_name)
                    .is_some_and(|lines| !lines.is_empty())
            {
                bail!("{fighter}/{move_name}: structured and legacy effect edits overlap; reimport the current move and Apply nodes to choose one source");
            }
            effect_scripts.push((fighter.clone(), move_name.clone(), script.clone()));
        }
        for (move_name, lost) in &edits.effect_dropped_lines {
            // Re-keyed to match what the report looks moves up by, and otherwise passed through
            // whole. A note for a move this build ships no function for needs no filtering here:
            // `verify_export` walks the exported moves and asks the map about each, so an orphan
            // note is never consulted. C6c wrote that filter first and removed it once a mutation
            // showed it could not change an outcome.
            dropped.insert(format!("{fighter}/{move_name}"), lost.clone());
        }
        // Sound scripts. An empty script is intentional when the user removed every call; it
        // must be emitted so the generated replacement silences the stock category. A missing
        // map entry, not an empty script, means the category was never edited.
        let mut sound_moves: Vec<_> = edits.sound_scripts.iter().collect();
        sound_moves.sort_by(|a, b| a.0.cmp(b.0));
        for (move_name, script) in sound_moves {
            sound_edits.push((fighter.clone(), move_name.clone(), script.clone()));
        }
        let mut expression_moves: Vec<_> = edits.expression_scripts.iter().collect();
        expression_moves.sort_by(|a, b| a.0.cmp(b.0));
        for (move_name, script) in expression_moves {
            expression_edits.push((fighter.clone(), move_name.clone(), script.clone()));
        }

        // A provenance note is useful only beside a function this export actually ships. A
        // saved project may retain a note for a move whose edit was later removed, and reporting
        // that orphan would make the warning impossible to act on.
        for (move_name, warning) in &edits.capture_branch_warnings {
            let exported = edits.acmd.contains_key(move_name)
                || edits.effect_calls_full.contains_key(move_name)
                || edits.effect_scripts.contains_key(move_name)
                || edits.sound_scripts.contains_key(move_name);
            let exported = exported || edits.expression_scripts.contains_key(move_name);
            if exported {
                capture_warnings.push(warning.clone());
            }
        }

        for (move_name, deltas) in &edits.effect_calls {
            if !deltas.is_empty() && !edits.effect_calls_full.contains_key(move_name) {
                incomplete.push(format!("{fighter}/{move_name}"));
            }
        }
        for tweak in &edits.live_tweaks {
            if !tweaks
                .iter()
                .any(|existing| existing.effect_name == tweak.effect_name)
            {
                tweaks.push(tweak.clone());
            }
        }
    }
    if !incomplete.is_empty() {
        incomplete.sort();
        bail!(
            "effect-call edits are missing their complete source list: {}. Open each move and save the project again",
            incomplete.join(", ")
        );
    }
    if !effect_scripts.is_empty() && !tweaks.is_empty() {
        bail!("structured effect tracks cannot be combined with global live color/speed overrides; put color and rate commands in the Effect nodes and remove the global overrides before exporting");
    }
    let mut uncovered_tweaks: Vec<_> = tweaks
        .iter()
        .filter(|tweak| !exported_effect_names.contains(&tweak.effect_name.to_ascii_lowercase()))
        .map(|tweak| tweak.effect_name.clone())
        .collect();
    if !uncovered_tweaks.is_empty() {
        uncovered_tweaks.sort();
        uncovered_tweaks.dedup();
        bail!(
            "live color/speed edits have no captured effect script to attach to: {}. Open or perform a move that uses each effect, then save again",
            uncovered_tweaks.join(", ")
        );
    }
    if acmd_edits.is_empty()
        && effect_edits.is_empty()
        && effect_scripts.is_empty()
        && sound_edits.is_empty()
        && expression_edits.is_empty()
    {
        return Ok(None);
    }
    let built = crate::acmd::build_mod_project_with_effect_structures(
        &acmd_edits,
        &effect_edits,
        &effect_scripts,
        &sound_edits,
        &expression_edits,
        &tweaks,
        &plugin_name(project),
        &slot_gates,
    );

    // Nothing reaches disk until the generated code has been read back and matched against the
    // edits it came from. A mod that will not compile, or that ships numbers other than the
    // ones on screen, is worse than an export that stops and says why.
    let mut report = crate::acmd_verify::verify_export_with_expression(
        &built,
        &acmd_edits,
        &effect_edits,
        &sound_edits,
        &expression_edits,
        &tweaks,
        &dropped,
    );
    crate::acmd_verify::verify_effect_structures(&built, &effect_scripts, &mut report);
    if report.has_blockers() {
        bail!(
            "the generated mod source did not pass verification:\n{}",
            report.blocker_summary()
        );
    }
    let mut warnings = report.warning_summary();
    warnings.extend(capture_warnings);
    warnings.sort();
    warnings.dedup();
    Ok(Some(GeneratedSource {
        project: built,
        warnings,
    }))
}

fn format_slots(scopes: &[u8]) -> String {
    scopes
        .iter()
        .map(|slot| format!("c{slot:02}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Find exactly one source function for this move and ACMD category. Snapshots may contain
/// several categories and move aliases, so extracting the first `effect_`/`sound_` function
/// would risk calling the wrong stock move from the fallback arm.
fn matching_category_original(
    body: &str,
    fighter: &str,
    move_name: &str,
    category: &str,
) -> Result<String> {
    let candidates = crate::acmd::acmd_script_candidates(category, move_name);
    let matches = body
        .lines()
        .filter_map(declared_source_function)
        .filter(|name| candidates.iter().any(|candidate| candidate == name))
        .collect::<Vec<_>>();
    let subject = format!("{fighter}/{move_name} {category}");
    if matches.len() != 1 {
        let tried = candidates.join(", ");
        let detail = if matches.is_empty() {
            format!("no matching original function was found (tried {tried})")
        } else {
            format!("the original function is ambiguous: {}", matches.join(", "))
        };
        bail!(
            "{subject}: costume-scoped replacement cannot preserve other costumes because {detail}. Reopen or reimport this move with its {category} source, then export again."
        );
    }
    let matching_name = matches[0];
    for (offset, line) in body.split_inclusive('\n').scan(0usize, |offset, line| {
        let start = *offset;
        *offset += line.len();
        Some((start, line))
    }) {
        if declared_source_function(line) != Some(matching_name) {
            continue;
        }
        let extracted = crate::acmd::extract_function(&body[offset..], &format!("{category}_"))
            .ok_or_else(|| {
                anyhow::anyhow!("{subject}: matching original function could not be extracted")
            })?;
        if declared_source_function(extracted.lines().next().unwrap_or_default())
            != Some(matching_name)
        {
            bail!("{subject}: extracted original function did not match {matching_name}");
        }
        return Ok(extracted);
    }
    bail!("{subject}: matching original function could not be located")
}

fn declared_source_function(line: &str) -> Option<&str> {
    let line = line.trim();
    if !line.starts_with("unsafe extern") && !line.starts_with("fn ") {
        return None;
    }
    let function = line.split_once("fn ")?.1;
    let name = function
        .chars()
        .take_while(|ch| *ch == '_' || ch.is_ascii_alphanumeric())
        .collect::<String>();
    (!name.is_empty() && function[name.len()..].trim_start().starts_with('('))
        .then_some(&function[..name.len()])
}

/// Materialize a generated Cargo project at exactly `root` (without adding another surprise
/// project-name directory).
pub fn write_source_project(project: &ModProject, root: &Path) -> Result<()> {
    for file in &project.files {
        let destination = root.join(&file.rel_path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&destination, &file.contents)
            .with_context(|| format!("writing {}", destination.display()))?;
    }
    Ok(())
}

fn portable_component(value: &str) -> String {
    let value = slugify(value);
    if value.len() > 80 {
        value[..80].to_string()
    } else {
        value
    }
}

fn copy_asset(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_file() {
        bail!("texture image does not exist: {}", source.display());
    }
    if let (Ok(source), Ok(destination)) = (source.canonicalize(), destination.canonicalize()) {
        if source == destination {
            return Ok(());
        }
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, destination).with_context(|| {
        format!(
            "copying texture image {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn valid_relative(path: &Path) -> bool {
    path.components().all(|part| {
        !matches!(
            part,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

/// Copy every external texture beside the project and rewrite its path to be relative. The
/// returned project is the one that should be serialized.
pub fn make_portable_project(
    project: &ModProjectFile,
    project_dir: &Path,
    asset_dir_name: &str,
) -> Result<ModProjectFile> {
    let mut portable = project.clone();
    let mut used_destinations = HashSet::new();
    let asset_relative = PathBuf::from(asset_dir_name).join("textures");
    if !valid_relative(&asset_relative) {
        bail!("asset directory must be relative");
    }

    let mut fighters: Vec<_> = portable.fighters.iter_mut().collect();
    fighters.sort_by(|a, b| a.0.cmp(b.0));
    for (fighter, edits) in fighters {
        let Some(eff) = &mut edits.eff else { continue };
        for (index, texture) in eff.textures.iter_mut().enumerate() {
            if texture.png_path.is_empty() {
                continue;
            }
            let source = PathBuf::from(&texture.png_path);
            let extension = source
                .extension()
                .and_then(|part| part.to_str())
                .unwrap_or("png");
            let relative = asset_relative
                .join(portable_component(fighter))
                .join(format!(
                    "replace_{index}_{}.{}",
                    portable_component(&texture.texture_name),
                    extension
                ));
            if !used_destinations.insert(relative.clone()) {
                bail!(
                    "duplicate project asset destination: {}",
                    relative.display()
                );
            }
            copy_asset(&source, &project_dir.join(&relative))?;
            texture.png_path = relative.to_string_lossy().replace('\\', "/");
        }
        for (index, texture) in eff.textures_added.iter_mut().enumerate() {
            if texture.png_path.is_empty() {
                continue;
            }
            let source = PathBuf::from(&texture.png_path);
            let extension = source
                .extension()
                .and_then(|part| part.to_str())
                .unwrap_or("png");
            let relative = asset_relative
                .join(portable_component(fighter))
                .join(format!(
                    "add_{index}_{}.{}",
                    portable_component(&texture.texture_name),
                    extension
                ));
            if !used_destinations.insert(relative.clone()) {
                bail!(
                    "duplicate project asset destination: {}",
                    relative.display()
                );
            }
            copy_asset(&source, &project_dir.join(&relative))?;
            texture.png_path = relative.to_string_lossy().replace('\\', "/");
        }
    }
    // Roster UI image overrides (portraits / stock icons) — same portable asset handling.
    let asset_ui = PathBuf::from(asset_dir_name).join("roster_ui");
    for (roster_key, kinds) in portable.roster.ui_images.iter_mut() {
        for (kind, ov) in kinds.iter_mut() {
            if ov.png_path.is_empty() {
                continue;
            }
            let source = PathBuf::from(&ov.png_path);
            let extension = source
                .extension()
                .and_then(|part| part.to_str())
                .unwrap_or("png");
            let stem = source
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("image");
            let relative = asset_ui
                .join(portable_component(&roster_key.to_string()))
                .join(format!(
                    "{}_{}.{}",
                    portable_component(kind),
                    portable_component(stem),
                    extension
                ));
            if !used_destinations.insert(relative.clone()) {
                bail!(
                    "duplicate project asset destination: {}",
                    relative.display()
                );
            }
            copy_asset(&source, &project_dir.join(&relative))?;
            ov.png_path = relative.to_string_lossy().replace('\\', "/");
        }
    }
    Ok(portable)
}

pub fn write_portable_project(
    project: &ModProjectFile,
    path: &Path,
    asset_dir_name: &str,
) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let portable = make_portable_project(project, parent, asset_dir_name)?;
    let json = serde_json::to_string_pretty(&portable)?;
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

/// Resolve portable texture paths against the loaded project's directory for all in-memory
/// rebuild and live-send code, which expects readable filesystem paths.
pub fn resolve_project_assets(project: &mut ModProjectFile, project_path: &Path) -> Result<()> {
    let parent = project_path.parent().unwrap_or_else(|| Path::new("."));
    for edits in project.fighters.values_mut() {
        let Some(eff) = &mut edits.eff else { continue };
        for path in eff
            .textures
            .iter_mut()
            .map(|texture| &mut texture.png_path)
            .chain(
                eff.textures_added
                    .iter_mut()
                    .map(|texture| &mut texture.png_path),
            )
        {
            if path.is_empty() {
                continue;
            }
            let candidate = PathBuf::from(&*path);
            if candidate.is_relative() {
                if !valid_relative(&candidate) {
                    bail!("project texture path escapes its project folder: {path}");
                }
                *path = parent.join(candidate).to_string_lossy().to_string();
            }
        }
    }
    for kinds in project.roster.ui_images.values_mut() {
        for ov in kinds.values_mut() {
            if ov.png_path.is_empty() {
                continue;
            }
            let candidate = PathBuf::from(&ov.png_path);
            if candidate.is_relative() {
                if !valid_relative(&candidate) {
                    bail!(
                        "project roster UI image path escapes its project folder: {}",
                        ov.png_path
                    );
                }
                ov.png_path = parent.join(candidate).to_string_lossy().to_string();
            }
        }
    }
    Ok(())
}

pub fn validate_project(project: &ModProjectFile) -> Result<()> {
    if project.version > PROJECT_VERSION {
        bail!(
            "project version {} is newer than this Visionary build supports (version {})",
            project.version,
            PROJECT_VERSION
        );
    }
    if project.name.trim().is_empty() {
        bail!("project name is empty");
    }
    crate::move_nodes::project::expand(project).map_err(node_errors)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{EditRecord, EffectCallEdit, EffectCallOp};
    use crate::gimmicks::fixtures::plugin as compatible_gimmick_plugin;
    use crate::mod_project::{ControllerChart, EffMod, FighterMod, TextureAddition, TextureImport};

    fn controller_chart_project() -> ModProjectFile {
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "controller_charts".into(),
            ..Default::default()
        };
        project.fighters.insert(
            "cloud".into(),
            FighterMod {
                controller_charts: std::collections::BTreeMap::from([
                    ("zeta".into(), ControllerChart::default()),
                    ("alpha".into(), ControllerChart::default()),
                ]),
                ..Default::default()
            },
        );
        project.fighters.insert(
            "mario".into(),
            FighterMod {
                controller_charts: std::collections::BTreeMap::from([(
                    "extra".into(),
                    ControllerChart::default(),
                )]),
                ..Default::default()
            },
        );
        project
    }

    fn selected_chart_project(chart_ids: &[&str], with_main: bool) -> ModProjectFile {
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "selected_controller_bundle".into(),
            ..Default::default()
        };
        let mut fighter = FighterMod::default();
        if with_main {
            fighter.behavior = crate::gimmicks::templates::Preset::Timer.graph();
        }
        for id in chart_ids {
            fighter.controller_charts.insert(
                (*id).to_string(),
                ControllerChart {
                    graph: crate::gimmicks::templates::Preset::Timer.graph(),
                    layout: crate::gimmicks::field_canvas::FieldLayout::default(),
                },
            );
        }
        project.fighters.insert("cloud".into(), fighter);
        project
    }

    fn rendered_controller_chart_ids(runtime: &SelectedGimmickRuntime) -> Vec<(String, String)> {
        let file = syn::parse_file(&runtime.artifact().source).unwrap();
        let encoded = file
            .items
            .iter()
            .find_map(|item| {
                let syn::Item::Const(item) = item else {
                    return None;
                };
                if item.ident != "VISIONARY_STATIC_CONTROLLER_BUNDLE" {
                    return None;
                }
                let syn::Expr::Lit(expression) = item.expr.as_ref() else {
                    return None;
                };
                let syn::Lit::Str(value) = &expression.lit else {
                    return None;
                };
                Some(value.value())
            })
            .expect("chart plan emits one encoded controller bundle");
        crate::gimmicks::controller_protocol::decode_request(&encoded)
            .unwrap()
            .charts
            .into_iter()
            .map(|chart| (chart.program.fighter, chart.chart_id))
            .collect()
    }

    fn active_meter_project() -> ModProjectFile {
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "gimmick_runtime".into(),
            ..Default::default()
        };
        project
            .fighters
            .entry("cloud".into())
            .or_default()
            .meters
            .push(crate::gimmicks::meter::MeterBinding {
                fighter: "cloud".into(),
                enabled: true,
                style: crate::gimmicks::meter::MeterStyle::Bar,
                value: crate::gimmicks::meter::MeterValue::Float {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                    min: 0.0,
                    max: 100.0,
                },
            });
        project
    }

    fn write_runtime_nro(root: &Path, bytes: &[u8]) {
        let output = root.join("target/aarch64-skyline-switch/release/lib_effect_viewer.nro");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(output, bytes).unwrap();
    }

    #[test]
    fn slug_is_safe_and_never_empty() {
        assert_eq!(slugify(" Kirby, but GOOD! "), "kirby_but_good");
        assert_eq!(slugify("123"), "mod_123");
        assert_eq!(slugify("--"), "visionary_mod");
    }

    #[test]
    fn every_base_eff_edit_class_requests_a_base_file() {
        let mut eff = EffMod {
            source_rel: "effect/fighter/kirby/ef_kirby.eff".into(),
            ..Default::default()
        };
        eff.textures_removed.push("old_texture".into());
        assert!(base_eff_has_content(&eff));
        eff.textures_removed.clear();
        eff.textures.push(TextureImport {
            texture_name: "replaced".into(),
            png_path: "image.png".into(),
            raw: false,
        });
        assert!(base_eff_has_content(&eff));
        eff.textures.clear();
        eff.textures_added.push(TextureAddition {
            texture_name: "new".into(),
            template_name: "old".into(),
            png_path: String::new(),
            raw: false,
        });
        assert!(base_eff_has_content(&eff));
    }

    #[test]
    fn portable_project_copies_and_resolves_texture_assets() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("outside.png");
        std::fs::write(&input, b"png fixture").unwrap();
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "portable".into(),
            ..Default::default()
        };
        project.fighters.insert(
            "kirby".into(),
            FighterMod {
                eff: Some(EffMod {
                    source_rel: "effect/fighter/kirby/ef_kirby.eff".into(),
                    textures: vec![TextureImport {
                        texture_name: "ef_test".into(),
                        png_path: input.to_string_lossy().to_string(),
                        raw: false,
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let project_path = temp.path().join("bundle/modproject.json");
        write_portable_project(&project, &project_path, "assets").unwrap();
        let mut loaded: ModProjectFile =
            serde_json::from_str(&std::fs::read_to_string(&project_path).unwrap()).unwrap();
        let stored = &loaded.fighters["kirby"].eff.as_ref().unwrap().textures[0].png_path;
        assert!(Path::new(stored).is_relative(), "{stored}");
        assert!(project_path.parent().unwrap().join(stored).is_file());
        resolve_project_assets(&mut loaded, &project_path).unwrap();
        let resolved = &loaded.fighters["kirby"].eff.as_ref().unwrap().textures[0].png_path;
        assert!(Path::new(resolved).is_absolute(), "{resolved}");
        assert!(Path::new(resolved).is_file());
    }

    #[test]
    fn incomplete_effect_edits_fail_instead_of_silently_disappearing() {
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "incomplete".into(),
            ..Default::default()
        };
        project.fighters.insert(
            "kirby".into(),
            FighterMod {
                effect_calls: std::collections::HashMap::from([(
                    "attack_dash".into(),
                    vec![EffectCallEdit {
                        index: 0,
                        op: EffectCallOp::Remove,
                        pristine: None,
                    }],
                )]),
                ..Default::default()
            },
        );
        let error = match source_project(&project) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("incomplete effect edits unexpectedly exported"),
        };
        assert!(error.contains("kirby/attack_dash"), "{error}");
    }

    #[test]
    fn legacy_effect_export_blocks_refused_preview_but_not_unrelated_source_categories() {
        let body = r#"unsafe extern "C" fn effect_attack11(agent: &mut L2CAgentBase) {
    for _ in 0..100 {
        for _ in 0..100 {
            for _ in 0..100 {
                frame(agent.lua_state_agent, 1.0);
            }
        }
    }
}
"#;
        let mut project = ModProjectFile::default();
        project.fighters.insert(
            "mario".into(),
            FighterMod {
                effect_calls_full: HashMap::from([("attack11".into(), Vec::new())]),
                move_sources: HashMap::from([(
                    "attack11".into(),
                    crate::mod_project::MoveSourceSnapshot {
                        body: body.into(),
                        captures: Vec::new(),
                    },
                )]),
                ..Default::default()
            },
        );
        let before = serde_json::to_value(&project).unwrap();
        let error = source_project(&project)
            .err()
            .expect("refused flattening cannot export an empty replacement");
        assert!(error
            .to_string()
            .contains("mario/attack11: legacy Effect source cannot be flattened safely"));
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .effect_calls_full
            .clear();
        assert!(
            source_project(&project).unwrap().is_none(),
            "an unedited category must not block unrelated export"
        );
    }

    /// C6c, end to end: the losses a move suffers on the way through the export survive being
    /// saved, reloaded, and exported again.
    ///
    /// The fixture is lifted from the corpus rather than written here, because a hand-made
    /// "lossy" script only proves that this test can construct one. `Run` is a real
    /// script that really loses a line, and the assertion below is that a user who saves that
    /// project, closes Visionary, reopens it and exports without ever opening the move again is
    /// still told so — which before C6c they were not, twice over: the project had nowhere to
    /// keep the list, and the export threw its whole report away bar the blockers.
    ///
    /// **Repointed from `SpecialHi2` by C2, which is the guard below doing its job.** Modelling
    /// the raw-command sword trail made `SpecialHi2` lossless, so it stopped exercising anything
    /// and said so instead of passing quietly. `Run` loses a `wait_loop_sync_mot`, which
    /// the `EffectStmt::Raw` arm drops on purpose and has no plan to ever carry — a deliberately
    /// unmodelled line is a steadier fixture than one that is merely unmodelled yet.
    #[test]
    fn a_reloaded_project_still_reports_the_lines_its_export_deletes() {
        let script_path = crate::scratch_dirs::app_storage_root()
            .join("script-cache")
            .join("kirby")
            .join("Run.txt");
        let Ok(body) = std::fs::read_to_string(&script_path) else {
            return;
        };
        let parsed = crate::acmd::parse_effect_script(&body);
        let calls = parsed.to_effect_calls();
        let lost = crate::acmd::unexportable_effect_lines(&parsed);
        // Guard the oracle with what it claims to test. If this move ever stops losing a line —
        // because the family got modelled, which is the good outcome — this test is measuring
        // nothing and should be repointed at a move that still does, not deleted.
        assert!(
            !calls.is_empty() && !lost.is_empty(),
            "Run no longer exercises a loss; repoint this test at a move that does"
        );

        let build = |dropped: HashMap<String, Vec<String>>| {
            let mut project = ModProjectFile {
                version: PROJECT_VERSION,
                name: "loss_report".into(),
                ..Default::default()
            };
            project.fighters.insert(
                "kirby".into(),
                FighterMod {
                    effect_calls_full: HashMap::from([("run".into(), calls.clone())]),
                    effect_dropped_lines: dropped,
                    ..Default::default()
                },
            );
            // Through JSON, because that is the trip being tested. Asserting on the struct in
            // memory would pass even if the new field never reached the file.
            let json = serde_json::to_string(&project).unwrap();
            let reloaded: ModProjectFile = serde_json::from_str(&json).unwrap();
            source_project(&reloaded)
                .unwrap()
                .expect("effect edits should export")
                .warnings
                .join("\n")
        };

        let said = build(HashMap::from([("run".into(), lost.clone())]));
        let first = lost[0].trim();
        assert!(
            said.contains(first),
            "the export never mentioned the line it deleted ({first}):\n{said}"
        );

        // The control, and the only thing that shows the stored list is what carried it: the
        // calls, the generated code and the verification are identical here.
        let silent = build(HashMap::new());
        assert!(
            !silent.contains(first),
            "the loss was reported without the saved list, so this test proves nothing about \
             it:\n{silent}"
        );

        // A note for a move this build ships no function for. Visionary never saves that pair —
        // the note only goes out beside the calls — but these files travel with mods and get
        // hand-edited, and a warning about a move that is not in the export is a warning the
        // user cannot act on. Enforced by the report only ever asking about moves it is
        // exporting, so this holds no matter what the gathering above passes along.
        let orphan = build(HashMap::from([("some_other_move".into(), lost.clone())]));
        assert!(
            !orphan.contains(first),
            "a loss note was reported for a move with no exported function:\n{orphan}"
        );
    }

    /// The saved residue has to survive the file, or E3 only works with the script open.
    ///
    /// `effect_frame_residue` is the one saved field that changes generated code. A dropped-line
    /// note is a remark; these lines are *written*, so a project that reloads without them
    /// exports exactly what the pre-E3 build did — the lines vanish, and the note that used to
    /// warn about them is gone too, because this build no longer produces one.
    ///
    /// Mutation that made this test necessary: `source_project` passing `Default::default()`
    /// instead of reading the field. Everything else in the suite stayed green, including the
    /// corpus ratchet, because every other test builds the residue and the calls from the same
    /// parse and never goes through a file.
    #[test]
    fn saved_frame_residue_reaches_the_generated_source() {
        // `dolly/FinalAirEnd`'s two frames, with its `FILL_SCREEN_MODEL_COLOR` left out. That
        // line is carried verbatim and the dump spells it `EffectScreenLayer:*GROUND`, which is
        // not valid Rust, so the real file is stopped by export verification long before it
        // reaches this — the designed behaviour, and a different test's subject. The
        // `CANCEL_FILL_SCREEN` lines are verbatim from that script.
        let body = r#"unsafe extern "C" fn effect_finalairend(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("dolly_buster_ground"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);
    }
    frame(agent.lua_state_agent, 40.0);
    if macros::is_excute(agent) {
        macros::CANCEL_FILL_SCREEN(agent, 1, 30);
        macros::CANCEL_FILL_SCREEN(agent, 0, 30);
    }
}
"#;
        let (calls, residue) = crate::acmd::parse_effect_script(body).to_effect_calls_and_residue();
        assert!(
            !calls.is_empty() && !residue.is_empty(),
            "the premise: a call list plus a frame that owns lines of its own"
        );

        let build = |residue: HashMap<String, std::collections::BTreeMap<u32, Vec<String>>>| {
            let mut project = ModProjectFile {
                version: PROJECT_VERSION,
                name: "residue_export".into(),
                ..Default::default()
            };
            project.fighters.insert(
                "dolly".into(),
                FighterMod {
                    effect_calls_full: HashMap::from([("finalairend".into(), calls.clone())]),
                    effect_frame_residue: residue,
                    ..Default::default()
                },
            );
            // Through JSON, because that is the trip being tested. Asserting on the struct in
            // memory would pass even if the new field never reached the file.
            let json = serde_json::to_string(&project).unwrap();
            let reloaded: ModProjectFile = serde_json::from_str(&json).unwrap();
            let generated = source_project(&reloaded)
                .unwrap()
                .expect("effect edits should export");
            generated
                .project
                .files
                .iter()
                .map(|f| f.contents.clone())
                .collect::<Vec<_>>()
                .join("\n")
        };

        let carried = build(HashMap::from([("finalairend".into(), residue.clone())]));
        assert!(
            carried.contains("CANCEL_FILL_SCREEN(agent, 1, 30)")
                && carried.contains("CANCEL_FILL_SCREEN(agent, 0, 30)"),
            "the saved residue never reached the generated script:\n{carried}"
        );

        // The control. Without it, "the lines are present" would pass for a build that got them
        // from somewhere else entirely — the calls, say — and prove nothing about the field.
        let dropped = build(HashMap::new());
        assert!(
            !dropped.contains("CANCEL_FILL_SCREEN"),
            "the lines appeared without the saved residue, so this test proves nothing about \
             it:\n{dropped}"
        );
    }

    #[test]
    fn active_gimmicks_are_refused_instead_of_silently_dropped() {
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "gimmick".into(),
            ..Default::default()
        };
        project
            .fighters
            .entry("cloud".into())
            .or_default()
            .meters
            .push(crate::gimmicks::meter::MeterBinding {
                fighter: "cloud".into(),
                enabled: true,
                style: crate::gimmicks::meter::MeterStyle::Bar,
                value: crate::gimmicks::meter::MeterValue::Float {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                    min: 0.0,
                    max: 100.0,
                },
            });

        let error = match source_project(&project) {
            Ok(_) => panic!("active gimmick package unexpectedly succeeded"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("cloud (meter)"), "{error}");
        assert!(error.contains("cannot silently omit"), "{error}");
    }

    #[test]
    fn controller_charts_refuse_source_normal_and_scoped_exports_deterministically() {
        let project = controller_chart_project();
        let expected = "cloud/alpha, cloud/zeta, mario/extra";

        let source_error = match source_project(&project) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("stored charts unexpectedly passed source export"),
        };
        assert!(source_error.contains(expected), "{source_error}");

        let scoped = crate::mod_project::scope_acmd_project(
            &project,
            crate::mod_project::AcmdProjectScope::Move {
                fighter: "cloud",
                move_name: "attack11",
            },
        );
        let scoped_error = match source_project(&scoped) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("stored charts unexpectedly passed scoped source export"),
        };
        assert!(scoped_error.contains("cloud/alpha"), "{scoped_error}");
        assert!(scoped_error.contains("cloud/zeta"), "{scoped_error}");
        assert!(!scoped_error.contains("mario/extra"), "{scoped_error}");

        let normal_error = match plan_normal_export(&project, None) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("stored charts unexpectedly passed normal export"),
        };
        assert!(normal_error.contains(expected), "{normal_error}");
    }

    #[test]
    fn controller_chart_error_precedes_selected_plugin_path_validation() {
        let project = controller_chart_project();
        let dir = tempfile::tempdir().unwrap();
        let invalid_plugin = dir.path().join("does-not-exist");

        let error = match plan_normal_export(&project, Some(&invalid_plugin)) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("stored charts unexpectedly passed selected export"),
        };

        assert!(
            error.contains("cloud") && error.contains("alpha"),
            "{error}"
        );
        assert!(error.contains("graph is empty"), "{error}");
        assert!(error.contains("controller"), "{error}");
        assert!(!error.contains("plugin"), "{error}");
        assert!(!invalid_plugin.exists());
    }

    #[test]
    fn portable_project_save_preserves_uninstalled_controller_chart_graphs_and_layouts() {
        let mut project = controller_chart_project();
        let graph = crate::gimmicks::templates::Preset::Timer.graph();
        let layout = crate::gimmicks::field_canvas::FieldLayout {
            positions: std::collections::BTreeMap::from([("state/idle".into(), [12.5, -4.0])]),
        };
        project
            .fighters
            .get_mut("cloud")
            .unwrap()
            .controller_charts
            .insert(
                "alpha".into(),
                ControllerChart {
                    graph: graph.clone(),
                    layout: layout.clone(),
                },
            );

        validate_project(&project).unwrap();
        let portable = make_portable_project(&project, Path::new("."), "assets").unwrap();
        assert_eq!(
            portable.fighters["cloud"].controller_charts["alpha"].graph,
            graph
        );
        assert_eq!(
            portable.fighters["cloud"].controller_charts["alpha"].layout,
            layout
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("modproject.json");
        write_portable_project(&project, &path, "assets").unwrap();
        let restored: ModProjectFile =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            restored.fighters["cloud"].controller_charts["alpha"],
            ControllerChart { graph, layout }
        );
    }

    #[test]
    fn normal_export_plan_keeps_no_gimmick_projects_on_the_existing_acmd_path() {
        let project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "ordinary".into(),
            ..Default::default()
        };

        let plan = plan_normal_export(&project, None).unwrap();

        assert!(plan.generated_source.is_none());
        assert!(plan.gimmick_runtime.is_none());
    }

    #[test]
    fn normal_export_plan_refuses_active_gimmicks_without_an_explicit_plugin_selection() {
        let error = match plan_normal_export(&active_meter_project(), None) {
            Ok(_) => panic!("active gimmicks unexpectedly planned without a selected runtime"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("cloud (meter)"), "{error}");
        assert!(
            error.contains("explicitly selected compatible plugin"),
            "{error}"
        );
        assert!(error.contains("silently omit"), "{error}");
    }

    #[test]
    fn normal_export_plan_embeds_acmd_and_gimmicks_in_the_selected_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("selected-plugin");
        let package = dir.path().join("fresh-package");
        compatible_gimmick_plugin(&plugin);
        std::fs::create_dir(&package).unwrap();
        let mut project = active_meter_project();
        project.fighters.get_mut("cloud").unwrap().acmd.insert(
            "attack_11".into(),
            EditRecord {
                fighter: "cloud".into(),
                fighter_display: "Cloud".into(),
                move_name: "attack_11".into(),
                script: crate::data::AcmdScript::default(),
                hitboxes_pristine: Vec::new(),
                hitboxes: Vec::new(),
                slot_scope: None,
                slot_scopes: Vec::new(),
            },
        );

        let plan = plan_normal_export(&project, Some(&plugin)).unwrap();

        assert!(plan.generated_source.is_none());
        assert!(plan.has_acmd_source);
        let runtime = plan.gimmick_runtime.unwrap();
        assert_eq!(runtime.acmd_artifact().fighters, ["cloud"]);
        assert!(runtime.acmd_artifact().source.contains("mod cloud"));
        let packaged = runtime
            .package_with(&package, |root| {
                write_runtime_nro(root, b"combined-runtime");
                Ok(())
            })
            .unwrap();
        assert_eq!(
            std::fs::read(packaged.plugin_nro).unwrap(),
            b"combined-runtime"
        );
        assert!(
            std::fs::read_to_string(plugin.join("src/slight/gimmicks/generated.rs"))
                .unwrap()
                .contains("METER_BINDINGS")
        );
        assert!(
            std::fs::read_to_string(plugin.join("src/slight/gimmicks/acmd_generated.rs"))
                .unwrap()
                .contains("mod cloud")
        );
    }

    #[test]
    fn selected_controller_plan_renders_main_and_every_extra_chart_and_revalidates() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("selected-plugin");
        compatible_gimmick_plugin(&plugin);
        let project = selected_chart_project(&["alpha", "zeta"], true);

        let plan = plan_normal_export(&project, Some(&plugin)).unwrap();
        let runtime = plan.gimmick_runtime.unwrap();
        assert!(plan.generated_source.is_none());
        assert_eq!(
            rendered_controller_chart_ids(&runtime),
            vec![
                ("cloud".into(), "alpha".into()),
                ("cloud".into(), "main".into()),
                ("cloud".into(), "zeta".into()),
            ]
        );
        let package = dir.path().join("fresh-package");
        std::fs::create_dir(&package).unwrap();
        let packaged = runtime
            .package_with(&package, |root| {
                write_runtime_nro(root, b"charted-package");
                Ok(())
            })
            .unwrap();
        assert_eq!(
            std::fs::read(packaged.plugin_nro).unwrap(),
            b"charted-package"
        );
        runtime.validate_for_project(&project).unwrap();

        let mut changed = project.clone();
        let fighter = changed.fighters.get_mut("cloud").unwrap();
        let alpha = fighter.controller_charts["alpha"].clone();
        fighter.controller_charts.insert("beta".into(), alpha);
        assert!(runtime.validate_for_project(&changed).is_err());
    }

    #[test]
    fn selected_chart_only_plan_has_no_implicit_main_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("selected-plugin");
        compatible_gimmick_plugin(&plugin);
        let project = selected_chart_project(&["alpha"], false);

        let plan = plan_normal_export(&project, Some(&plugin)).unwrap();
        let runtime = plan.gimmick_runtime.unwrap();
        assert_eq!(
            rendered_controller_chart_ids(&runtime),
            vec![("cloud".into(), "alpha".into())]
        );
        assert!(runtime.acmd_artifact().fighters.is_empty());
        runtime.validate_for_project(&project).unwrap();
    }

    #[test]
    fn selected_chart_plan_rejects_invalid_chart_without_writes() {
        let dir = tempfile::tempdir().unwrap();
        let invalid_plugin = dir.path().join("invalid-chart-plugin");
        compatible_gimmick_plugin(&invalid_plugin);
        let before =
            std::fs::read(invalid_plugin.join("src/slight/gimmicks/generated.rs")).unwrap();
        let invalid = controller_chart_project();
        assert!(plan_normal_export(&invalid, Some(&invalid_plugin)).is_err());
        assert_eq!(
            std::fs::read(invalid_plugin.join("src/slight/gimmicks/generated.rs")).unwrap(),
            before
        );
    }

    #[test]
    fn selected_runtime_builder_failure_restores_both_managed_sources() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("selected-plugin");
        let package = dir.path().join("fresh-package");
        compatible_gimmick_plugin(&plugin);
        std::fs::create_dir(&package).unwrap();
        let generated = plugin.join("src/slight/gimmicks/generated.rs");
        let acmd_generated = plugin.join("src/slight/gimmicks/acmd_generated.rs");
        let behavior_before = std::fs::read_to_string(&generated).unwrap();
        let acmd_before = std::fs::read_to_string(&acmd_generated).unwrap();
        let plan = plan_normal_export(&active_meter_project(), Some(&plugin)).unwrap();
        let runtime = plan.gimmick_runtime.unwrap();

        let error = runtime
            .package_with(&package, |_| {
                Err(crate::gimmicks::package::PackageError {
                    subject: "test builder".into(),
                    reason: "deliberate failure".into(),
                })
            })
            .unwrap_err();

        assert!(error.reason.contains("deliberate failure"), "{error}");
        assert_eq!(std::fs::read_to_string(generated).unwrap(), behavior_before);
        assert_eq!(
            std::fs::read_to_string(acmd_generated).unwrap(),
            acmd_before
        );
        assert!(!package.join("plugin.nro").exists());
    }

    #[test]
    fn disabled_meters_do_not_force_a_runtime_package() {
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "disabled_meter".into(),
            ..Default::default()
        };
        project
            .fighters
            .entry("cloud".into())
            .or_default()
            .meters
            .push(crate::gimmicks::meter::MeterBinding {
                fighter: "cloud".into(),
                enabled: false,
                style: crate::gimmicks::meter::MeterStyle::Bar,
                value: crate::gimmicks::meter::MeterValue::Float {
                    symbol: "not_read_while_disabled".into(),
                    min: 0.0,
                    max: 1.0,
                },
            });

        assert!(source_project(&project).unwrap().is_none());
    }

    #[test]
    fn a_live_capture_branch_warning_survives_a_project_round_trip() {
        let body = r#"
unsafe extern "C" fn effect_x(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FLAG) {
        if macros::is_excute(agent) {
            macros::EFFECT(agent, Hash40::new("sys_flash"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);
        }
    }
}
"#;
        let calls = crate::acmd::parse_effect_script(body).to_effect_calls();
        assert!(
            !calls.is_empty(),
            "the fixture must ship an effect function"
        );
        let note = "Live capture is partial for kirby/effect_x: one observed path came from a cached source with 1 conditional branch(es). Export will write only the observed calls as unconditional code; inspect the source before shipping".to_string();
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "capture_provenance".into(),
            ..Default::default()
        };
        project.fighters.insert(
            "kirby".into(),
            FighterMod {
                effect_calls_full: HashMap::from([(String::from("effect_x"), calls)]),
                capture_branch_warnings: HashMap::from([(String::from("effect_x"), note.clone())]),
                ..Default::default()
            },
        );

        let json = serde_json::to_string(&project).unwrap();
        let reloaded: ModProjectFile = serde_json::from_str(&json).unwrap();
        let generated = source_project(&reloaded)
            .unwrap()
            .expect("the captured effect should still export");
        assert!(
            generated.warnings.contains(&note),
            "{:?}",
            generated.warnings
        );

        let orphan = FighterMod {
            capture_branch_warnings: HashMap::from([(String::from("not_exported"), note)]),
            ..Default::default()
        };
        let mut only_note = ModProjectFile {
            version: PROJECT_VERSION,
            name: "note_only".into(),
            ..Default::default()
        };
        only_note.fighters.insert("kirby".into(), orphan);
        assert!(source_project(&only_note).unwrap().is_none());
    }

    #[test]
    fn native_lr_negative_compound_branch_exports_on_game_effect_and_expression() {
        use crate::move_nodes::model::{
            MoveCondition, MoveGraph, MoveNode, MoveTrack, NodeKind, Track,
        };

        let condition = MoveCondition::Not(Box::new(MoveCondition::Group(Box::new(
            MoveCondition::And {
                left: Box::new(MoveCondition::NativeLrNegative),
                right: Box::new(MoveCondition::Grounded),
            },
        ))));
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "native_lr_negative_source_contract".into(),
            ..Default::default()
        };
        let fighter = project.fighters.entry("mario".into()).or_default();
        let name = "lr_sign_contract";
        for track in [Track::Game, Track::Effect, Track::Expression] {
            let graph = MoveGraph {
                tracks: vec![MoveTrack {
                    track,
                    nodes: vec![MoveNode {
                        id: 1,
                        kind: NodeKind::Branch {
                            condition: condition.clone(),
                            then_nodes: vec![MoveNode {
                                id: 2,
                                kind: NodeKind::Wait { frames: 1.0 },
                            }],
                            else_nodes: vec![MoveNode {
                                id: 3,
                                kind: NodeKind::Wait { frames: 2.0 },
                            }],
                        },
                    }],
                    baseline: Vec::new(),
                    effect_execute_baseline: Vec::new(),
                }],
                ..Default::default()
            };
            let compiled = crate::move_nodes::timeline::compile("mario", name, &graph)
                .unwrap_or_else(|errors| panic!("{track:?} source contract failed: {errors:?}"));
            let script = compiled[0].1.clone();
            match track {
                Track::Game => {
                    fighter.acmd.insert(
                        name.into(),
                        EditRecord {
                            fighter: "mario".into(),
                            fighter_display: "Mario".into(),
                            move_name: name.into(),
                            script,
                            hitboxes_pristine: Vec::new(),
                            hitboxes: Vec::new(),
                            slot_scope: None,
                            slot_scopes: Vec::new(),
                        },
                    );
                }
                Track::Effect => {
                    fighter.effect_scripts.insert(name.into(), script);
                }
                Track::Expression => {
                    fighter.expression_scripts.insert(name.into(), script);
                }
                Track::Sound => unreachable!("the LR-sign condition is not admitted on Sound"),
            }
        }

        let json = serde_json::to_string(&project).unwrap();
        let reloaded: ModProjectFile = serde_json::from_str(&json).unwrap();
        let generated = source_project(&reloaded)
            .unwrap()
            .expect("the three source categories must export");
        let acmd = generated
            .project
            .files
            .iter()
            .find(|file| file.rel_path == "src/mario/acmd.rs")
            .map(|file| file.contents.as_str())
            .expect("generated source must contain all three ACMD categories");
        let header = crate::move_nodes::compound_conditions::header("mario", &condition).unwrap();
        for function_name in [
            "game_lrsigncontract",
            "effect_lrsigncontract",
            "expression_lrsigncontract",
        ] {
            assert!(
                acmd.contains(function_name),
                "missing generated {function_name}"
            );
        }
        assert!(
            acmd.contains(&header),
            "compound native condition was not emitted: {header}"
        );
        assert!(acmd.contains("*SO_VAR_FLOAT_LR") && acmd.contains("< 0.0"));
        assert!(generated.warnings.is_empty(), "{:?}", generated.warnings);
    }

    #[test]
    fn uncovered_live_tweak_fails_instead_of_exporting_no_code() {
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "tweak_only".into(),
            ..Default::default()
        };
        project.fighters.insert(
            "kirby".into(),
            FighterMod {
                live_tweaks: vec![LiveTweak {
                    effect_name: "kirby_dash".into(),
                    color: Some([2.0, 1.0, 1.0, 1.0]),
                    speed: None,
                }],
                ..Default::default()
            },
        );
        let error = match source_project(&project) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("uncovered live tweak unexpectedly exported"),
        };
        assert!(error.contains("kirby_dash"), "{error}");
    }

    #[test]
    fn empty_sound_and_expression_scripts_are_explicit_project_replacements() {
        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "clear_categories".into(),
            ..Default::default()
        };
        project.fighters.insert(
            "mario".into(),
            FighterMod {
                sound_scripts: HashMap::from([("attack_air_n".into(), Default::default())]),
                expression_scripts: HashMap::from([("attack_air_n".into(), Default::default())]),
                ..Default::default()
            },
        );

        assert!(
            !project.is_empty(),
            "an explicit empty replacement is still an edit"
        );
        let round_tripped: ModProjectFile =
            serde_json::from_str(&serde_json::to_string(&project).unwrap()).unwrap();
        let generated = source_project(&round_tripped)
            .unwrap()
            .expect("empty category replacements should generate source");
        let acmd = generated
            .project
            .files
            .iter()
            .find(|file| file.rel_path == "src/mario/acmd.rs")
            .map(|file| file.contents.as_str())
            .expect("generated fighter source");
        assert!(
            acmd.contains("unsafe extern \"C\" fn sound_attackairn"),
            "{acmd}"
        );
        assert!(
            acmd.contains("unsafe extern \"C\" fn expression_attackairn"),
            "{acmd}"
        );
        assert!(acmd.contains("agent.acmd(\"sound_attackairn\""), "{acmd}");
        assert!(
            acmd.contains("agent.acmd(\"expression_attackairn\""),
            "{acmd}"
        );
    }
}

/// Costume-scoped moves: the export half of the character an authored slot gets.
///
/// A Smashline `game_` script replaces the move for **every** costume of the fighter, so a
/// scoped move that shipped without the fighter's own script as its other arm would delete
/// that move from the donor and every other costume of it. These pin both directions.
#[cfg(test)]
mod slot_gate_tests {
    use super::*;
    use crate::data::EditRecord;
    use crate::mod_project::{FighterMod, MoveSourceSnapshot};

    const ORIGINAL: &str = "unsafe extern \"C\" fn game_attack11(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, 2.0);\n}\n";

    const ALL_CATEGORY_ORIGINALS: &str = "unsafe extern \"C\" fn game_attack11(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, 2.0);\n}\n\
unsafe extern \"C\" fn effect_attack11(agent: &mut L2CAgentBase) {\n    // original effect category\n    frame(agent.lua_state_agent, 2.0);\n}\n\
unsafe extern \"C\" fn sound_attack11(agent: &mut L2CAgentBase) {\n    // original sound category\n    frame(agent.lua_state_agent, 2.0);\n}\n\
unsafe extern \"C\" fn expression_attack11(agent: &mut L2CAgentBase) {\n    // original expression category\n    frame(agent.lua_state_agent, 2.0);\n}\n";

    fn project(slot: Option<u8>, original: Option<&str>) -> ModProjectFile {
        let record = EditRecord {
            fighter: "mario".into(),
            fighter_display: "Mario".into(),
            move_name: "attack_11".into(),
            script: crate::data::AcmdScript::default(),
            hitboxes_pristine: Vec::new(),
            hitboxes: Vec::new(),
            slot_scope: slot,
            slot_scopes: Vec::new(),
        };
        let mut fighter = FighterMod {
            acmd: HashMap::from([("attack_11".to_string(), record)]),
            ..Default::default()
        };
        if let Some(original) = original {
            fighter.move_sources.insert(
                "attack_11".to_string(),
                MoveSourceSnapshot {
                    body: original.to_string(),
                    captures: Vec::new(),
                },
            );
        }
        let mut project = ModProjectFile {
            name: "gate_test".into(),
            ..Default::default()
        };
        project.fighters.insert("mario".into(), fighter);
        project
    }

    fn generated(project: &ModProjectFile) -> String {
        let source = source_project(project)
            .expect("export should succeed")
            .expect("a project with one edit generates source");
        source
            .project
            .files
            .iter()
            .find(|file| file.rel_path.ends_with("acmd.rs"))
            .expect("a per-fighter acmd.rs")
            .contents
            .clone()
    }

    fn project_with_category_replacements(original: &str) -> ModProjectFile {
        let mut project = project(Some(8), Some(original));
        let fighter = project.fighters.get_mut("mario").unwrap();
        fighter
            .effect_scripts
            .insert("attack_11".into(), crate::data::AcmdScript::default());
        fighter
            .sound_scripts
            .insert("attack_11".into(), crate::data::AcmdScript::default());
        fighter
            .expression_scripts
            .insert("attack_11".into(), crate::data::AcmdScript::default());
        project
    }

    #[test]
    fn a_costume_scoped_move_is_exported_behind_a_costume_gate() {
        let source = generated(&project(Some(8), Some(ORIGINAL)));
        assert!(
            source.contains("FIGHTER_INSTANCE_WORK_ID_INT_COLOR"),
            "{source}"
        );
        assert!(source.contains("if color == 8 {"), "{source}");
        assert!(source.contains("_costume(agent);"), "{source}");
        assert!(source.contains("_original(agent);"), "{source}");
    }

    /// The paired positive for the refusal below: with everything present the same project
    /// exports, so the refusal is about the missing original and not about the fixture.
    #[test]
    fn an_unscoped_move_is_exported_plainly() {
        let source = generated(&project(None, None));
        assert!(
            !source.contains("FIGHTER_INSTANCE_WORK_ID_INT_COLOR"),
            "{source}"
        );
        assert!(!source.contains("_costume(agent);"), "{source}");
    }

    /// Exporting this would silently remove the move from every other costume of the donor.
    /// Refusing, with a message that says what to do, is the only safe answer.
    #[test]
    fn a_costume_scoped_move_with_no_original_is_refused_rather_than_exported_wide() {
        let message = match source_project(&project(Some(8), None)) {
            Ok(_) => panic!("a scoped move with no original must be refused"),
            Err(error) => error.to_string(),
        };
        assert!(message.contains("every other costume"), "{message}");
        assert!(message.contains("attack_11"), "{message}");
    }

    /// An empty stored body is the same problem as a missing one, and is what a project saved
    /// from a move that never loaded its source actually contains.
    #[test]
    fn an_empty_stored_original_counts_as_missing() {
        assert!(source_project(&project(Some(8), Some("   \n"))).is_err());
    }

    #[test]
    fn scoped_sound_expression_and_structured_effect_keep_their_own_original_categories() {
        let source = generated(&project_with_category_replacements(ALL_CATEGORY_ORIGINALS));
        for category in ["effect", "sound", "expression"] {
            let authored = format!("fn {category}_attack11_costume(");
            let original = format!("fn {category}_attack11_original(");
            let dispatcher = format!("{category}_attack11_costume(agent);");
            assert!(source.contains(&authored), "missing {authored}:\n{source}");
            assert!(source.contains(&original), "missing {original}:\n{source}");
            assert!(
                source.contains(&dispatcher),
                "missing {dispatcher}:\n{source}"
            );
        }
        assert!(source.contains("original effect category"), "{source}");
        assert!(source.contains("original sound category"), "{source}");
        assert!(source.contains("original expression category"), "{source}");
    }

    #[test]
    fn scoped_flat_effect_replacement_uses_its_effect_original() {
        let mut project = project(Some(8), Some(ALL_CATEGORY_ORIGINALS));
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .effect_calls_full
            .insert("attack_11".into(), Vec::new());
        let source = generated(&project);
        assert!(source.contains("fn effect_attack11_costume("), "{source}");
        assert!(source.contains("fn effect_attack11_original("), "{source}");
        assert!(source.contains("original effect category"), "{source}");
    }

    #[test]
    fn scoped_category_replacement_requires_a_matching_original_category_function() {
        let project = project_with_category_replacements(ORIGINAL);
        let error = match source_project(&project) {
            Ok(_) => panic!("category replacements without their originals must fail preflight"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("effect_attack11"), "{error}");
        assert!(error.contains("costume-scoped replacement"), "{error}");
        assert!(error.contains("other costumes"), "{error}");
    }

    #[test]
    fn ambiguous_move_alias_originals_are_refused() {
        let source = "unsafe extern \"C\" fn sound_attacks3s(agent: &mut L2CAgentBase) { }\n\
unsafe extern \"C\" fn sound_attacks3(agent: &mut L2CAgentBase) { }\n";
        let error = matching_category_original(source, "mario", "attack_s3_s", "sound")
            .unwrap_err()
            .to_string();
        assert!(error.contains("ambiguous"), "{error}");
        assert!(error.contains("sound_attacks3s"), "{error}");
        assert!(error.contains("sound_attacks3"), "{error}");
    }

    #[test]
    fn an_expression_script_survives_a_saved_project_export() {
        let source = r#"
unsafe extern "C" fn expression_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::RUMBLE_HIT(agent, Hash40::new("rbkind_attackm"), 0);
        macros::QUAKE(agent, *CAMERA_QUAKE_KIND_M);
        ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_attackm"), 3, false, *BATTLE_OBJECT_ID_INVALID as u32);
    }
}
"#;
        let script = crate::acmd::parse_expression_script(source);
        assert_eq!(script.to_expression_events().len(), 3);

        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "expression_export".into(),
            ..Default::default()
        };
        project.fighters.insert(
            "mario".into(),
            FighterMod {
                expression_scripts: std::collections::HashMap::from([(
                    "attack_air_n".into(),
                    script,
                )]),
                ..Default::default()
            },
        );

        let json = serde_json::to_string(&project).unwrap();
        let reloaded: ModProjectFile = serde_json::from_str(&json).unwrap();
        let generated = source_project(&reloaded)
            .unwrap()
            .expect("expression script should export");
        assert!(generated.warnings.is_empty(), "{:?}", generated.warnings);
        let acmd = generated
            .project
            .files
            .iter()
            .find(|file| file.rel_path == "src/mario/acmd.rs")
            .map(|file| file.contents.as_str())
            .expect("generated fighter ACMD");
        assert!(acmd.contains("unsafe extern \"C\" fn expression_attackairn"));
        assert!(acmd.contains("macros::RUMBLE_HIT(agent"));
        assert!(acmd.contains("macros::QUAKE(agent"));
        assert!(acmd.contains(
            "ControlModule::set_rumble(agent.module_accessor, Hash40::new(\"rbkind_attackm\"), 3, false, *BATTLE_OBJECT_ID_INVALID as u32);"
        ));
        assert!(acmd.contains(
            "agent.acmd(\"expression_attackairn\", expression_attackairn, smashline::Priority::Default);"
        ));
    }

    #[test]
    fn a_moves_own_spawn_macros_survive_the_whole_export_path() {
        const SOURCE: &str = r#"
unsafe extern "C" fn effect_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("mario_hit_l"), Hash40::new("mario_hit_r"), Hash40::new("haver"), 1.0, 2.0, 3.0, 0.0, 90.0, 45.0, 1.5, true, *EF_FLIP_YZ);
        macros::EFFECT_ALPHA(agent, Hash40::new("sys_smoke"), Hash40::new("top"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.6);
    }
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(agent, Hash40::new("mario_hit_l"), false, true);
    }
}
"#;
        let calls = crate::acmd::parse_effect_script(SOURCE).to_effect_calls();
        assert_eq!(calls.len(), 2);

        let mut project = ModProjectFile {
            version: PROJECT_VERSION,
            name: "fidelity".into(),
            ..Default::default()
        };
        project.fighters.insert(
            "mario".into(),
            FighterMod {
                effect_calls_full: std::collections::HashMap::from([(
                    "attack_air_n".into(),
                    calls.clone(),
                )]),
                ..Default::default()
            },
        );
        // Through a real save/load, so the new fields have to actually serialize.
        let json = serde_json::to_string(&project).unwrap();
        let project: ModProjectFile = serde_json::from_str(&json).unwrap();

        let generated = source_project(&project)
            .unwrap()
            .expect("source project")
            .project;
        let acmd = generated
            .files
            .iter()
            .find(|f| f.rel_path == "src/mario/acmd.rs")
            .map(|f| f.contents.as_str())
            .expect("generated fighter ACMD");

        assert!(
            acmd.contains(
                r#"macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("mario_hit_l"), Hash40::new("mario_hit_r"), Hash40::new("haver"), 1.0, 2.0, 3.0, 0.0, 90.0, 45.0, 1.5, true, *EF_FLIP_YZ);"#
            ),
            "the flipped follow spawn and its second graphic must survive:\n{acmd}"
        );
        assert!(
            acmd.contains(
                r#"macros::EFFECT_ALPHA(agent, Hash40::new("sys_smoke"), Hash40::new("top"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.6);"#
            ),
            "EFFECT_ALPHA's alpha argument must survive:\n{acmd}"
        );
        assert!(
            acmd.contains(
                r#"macros::EFFECT_OFF_KIND(agent, Hash40::new("mario_hit_l"), false, true);"#
            ),
            "the follow effect's end must still close it:\n{acmd}"
        );
    }
}
