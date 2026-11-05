//! The `--gimmick-package` command-line surface, used by in-game testing.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::mod_project::ModProjectFile;

const PACKAGE_USAGE: &str = "usage: visionary --gimmick-package <project.json> <explicit-slight-replica-plugin-root> <existing-new-mod-folder>";

/// Render a saved project's behavior, enabled meters and complete move scripts, build the explicitly
/// selected compatible plugin, and create its `plugin.nro` in one already-new
/// ARCropolis mod folder. This command never discovers a plugin, deploys to a
/// game or emulator, creates a mod folder, or replaces an existing package
/// artifact.
pub fn run_package(mut args: impl Iterator<Item = OsString>) -> anyhow::Result<()> {
    let (Some(project_path), Some(plugin_root), Some(package_root), None) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        anyhow::bail!(PACKAGE_USAGE);
    };
    let project_path = PathBuf::from(project_path);
    // The build runs cargo from another directory, so a relative root would miss its output.
    let plugin_root = std::path::absolute(PathBuf::from(plugin_root))?;
    let package_root = PathBuf::from(package_root);
    let plan = plan_project_runtime(&project_path, &plugin_root)?;
    let runtime = plan
        .gimmick_runtime
        .as_ref()
        .expect("explicit plugin selection produces a combined runtime");
    if runtime.artifact().fighters.is_empty() && runtime.acmd_artifact().fighters.is_empty() {
        anyhow::bail!("combined package has no behavior, meters, or ACMD source to build");
    }
    let packaged = runtime.build_into_package(&package_root).map_err(|error| {
        anyhow::anyhow!("combined behavior and ACMD package was not created: {error}")
    })?;
    println!(
        "built combined runtime for {} behavior fighter(s) and {} ACMD fighter(s) into {}{}",
        runtime.artifact().fighters.len(),
        runtime.acmd_artifact().fighters.len(),
        packaged.plugin_nro.display(),
        if packaged.adapter_changed {
            ""
        } else {
            " (adapter source was already current)"
        }
    );
    Ok(())
}

fn plan_project_runtime(
    project_path: &Path,
    plugin_root: &Path,
) -> anyhow::Result<crate::mod_export::NormalExportPlan> {
    let project = load_project(project_path)?;
    crate::mod_export::plan_normal_export(&project, Some(plugin_root))
}

fn load_project(project_path: &Path) -> anyhow::Result<ModProjectFile> {
    let text = std::fs::read_to_string(project_path)
        .map_err(|error| anyhow::anyhow!("could not read {}: {error}", project_path.display()))?;
    let mut project: ModProjectFile = serde_json::from_str(&text)
        .map_err(|error| anyhow::anyhow!("could not parse {}: {error}", project_path.display()))?;
    project.migrate()?;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::ExcuteStmt;
    use crate::gimmicks::behavior::Action;
    use crate::move_nodes::model::{MoveGraph, MoveNode, MoveTrack, NodeKind, Track};

    // Copy only the small files inspected by export preflight, never a plugin's
    // dependencies, build output, or user-selected source tree.
    const CONTRACT_FILES: &[&str] = &[
        "Cargo.toml",
        "Cargo.lock",
        "src/slight/gimmicks/mod.rs",
        "src/slight/systems/main_module.rs",
        "src/slight/gimmicks/behavior.rs",
        "src/slight/gimmicks/dispatcher.rs",
        "src/slight/gimmicks/live_behavior.rs",
        "src/slight/gimmicks/state.rs",
        "src/slight/gimmicks/meter_snapshot.rs",
        "src/slight/gimmicks/registry.rs",
        "src/slight/gimmicks/operations.rs",
        "src/slight/gimmicks/generated.rs",
        "src/slight/gimmicks/acmd_generated.rs",
    ];

    fn plugin_fixture(dir: &Path) -> PathBuf {
        let root = dir.join("selected_plugin");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/slight_replica");
        for relative in CONTRACT_FILES {
            let destination = root.join(relative);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::copy(source.join(relative), destination).unwrap();
        }
        root
    }

    fn contract_contents(root: &Path) -> Vec<Vec<u8>> {
        CONTRACT_FILES
            .iter()
            .map(|relative| std::fs::read(root.join(relative)).unwrap())
            .collect()
    }

    fn timed_event_project() -> ModProjectFile {
        let mut project = ModProjectFile::default();
        project
            .fighters
            .entry("mario".into())
            .or_default()
            .move_graphs
            .insert(
                "attack11".into(),
                MoveGraph {
                    tracks: vec![MoveTrack {
                        track: Track::Game,
                        effect_execute_baseline: Vec::new(),
                        nodes: vec![
                            MoveNode {
                                id: 1,
                                kind: NodeKind::Frame { frame: 2.0 },
                            },
                            MoveNode {
                                id: 2,
                                kind: NodeKind::Execute {
                                    commands: vec![ExcuteStmt::GimmickEvent {
                                        name: "jab_fireball".into(),
                                    }],
                                },
                            },
                        ],
                        baseline: Vec::new(),
                    }],
                    ..MoveGraph::default()
                },
            );
        project.fighters.get_mut("mario").unwrap().behavior =
            crate::move_nodes::test_support::controller_event(
                "jab_fireball",
                vec![Action::GenerateArticle {
                    symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
                }],
            );
        project
    }

    fn save_project(dir: &Path, project: &ModProjectFile) -> PathBuf {
        let path = dir.join("project.json");
        std::fs::write(&path, serde_json::to_vec_pretty(project).unwrap()).unwrap();
        path
    }

    fn export_project(project: &Path, plugin: &Path) -> anyhow::Result<()> {
        let plugin = std::path::absolute(plugin)?;
        let plan = plan_project_runtime(project, &plugin)?;
        let runtime = plan
            .gimmick_runtime
            .as_ref()
            .expect("explicit plugin selection produces a combined runtime");
        crate::gimmicks::adapter::export_combined_to_plugin(
            &plugin,
            runtime.artifact(),
            runtime.acmd_artifact(),
        )
        .map_err(|error| anyhow::anyhow!("{error}"))?;
        Ok(())
    }

    #[test]
    fn export_command_installs_both_timed_event_slots_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = plugin_fixture(dir.path());
        let before = contract_contents(&root);
        let project_path = save_project(dir.path(), &timed_event_project());
        let saved_project = std::fs::read(&project_path).unwrap();

        export_project(&project_path, &root).unwrap();
        let after = contract_contents(&root);
        for (index, relative) in CONTRACT_FILES.iter().enumerate() {
            if !relative.ends_with("/generated.rs") && !relative.ends_with("/acmd_generated.rs") {
                assert_eq!(
                    after[index], before[index],
                    "unmanaged file changed: {relative}"
                );
            }
        }
        let behavior =
            std::fs::read_to_string(root.join("src/slight/gimmicks/generated.rs")).unwrap();
        let acmd =
            std::fs::read_to_string(root.join("src/slight/gimmicks/acmd_generated.rs")).unwrap();
        assert!(behavior.contains("jab_fireball"));
        assert!(behavior.contains("FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL"));
        assert!(
            acmd.contains("dispatch_named_from_module(agent.module_accessor, \"jab_fireball\")")
        );
        assert!(acmd.contains("game_attack11"));
        syn::parse_file(&behavior).unwrap();
        syn::parse_file(&acmd).unwrap();
        let slot_times: Vec<_> = CONTRACT_FILES
            .iter()
            .map(|relative| {
                std::fs::metadata(root.join(relative))
                    .unwrap()
                    .modified()
                    .unwrap()
            })
            .collect();

        export_project(&project_path, &root).unwrap();
        assert_eq!(contract_contents(&root), after);
        for (relative, modified) in CONTRACT_FILES.iter().zip(slot_times) {
            assert_eq!(
                std::fs::metadata(root.join(relative))
                    .unwrap()
                    .modified()
                    .unwrap(),
                modified
            );
        }
        assert_eq!(std::fs::read(project_path).unwrap(), saved_project);
    }

    #[test]
    fn export_command_supports_source_only_and_clears_stale_acmd_for_behavior_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = plugin_fixture(dir.path());
        let mut project = timed_event_project();
        project.fighters.get_mut("mario").unwrap().behavior = Default::default();
        let graph = project
            .fighters
            .get_mut("mario")
            .unwrap()
            .move_graphs
            .get_mut("attack11")
            .unwrap();
        graph.tracks[0].nodes[1].kind = NodeKind::Execute {
            commands: vec![ExcuteStmt::ClearAll],
        };
        let project_path = save_project(dir.path(), &project);
        let plan = plan_project_runtime(&project_path, &root).unwrap();
        let runtime = plan.gimmick_runtime.unwrap();
        assert!(runtime.artifact().fighters.is_empty());
        assert_eq!(runtime.acmd_artifact().fighters, ["mario"]);
        export_project(&project_path, &root).unwrap();
        assert!(
            std::fs::read_to_string(root.join("src/slight/gimmicks/acmd_generated.rs"))
                .unwrap()
                .contains("game_attack11")
        );

        let fighter = project.fighters.get_mut("mario").unwrap();
        fighter.move_graphs.clear();
        fighter.behavior = super::super::templates::Preset::KoTally.graph();
        save_project(dir.path(), &project);
        export_project(&project_path, &root).unwrap();
        let behavior =
            std::fs::read_to_string(root.join("src/slight/gimmicks/generated.rs")).unwrap();
        let acmd =
            std::fs::read_to_string(root.join("src/slight/gimmicks/acmd_generated.rs")).unwrap();
        assert!(behavior.contains("stock_losses"));
        assert!(!acmd.contains("game_attack11"));
        assert!(acmd.contains(super::super::acmd_adapter::GENERATED_ACMD_SLOT_MARKER));
        syn::parse_file(&acmd).unwrap();
    }

    #[test]
    fn invalid_timed_receiver_or_plugin_selection_leaves_all_contract_files_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let root = plugin_fixture(dir.path());
        let before = contract_contents(&root);
        let mut project = timed_event_project();
        project.fighters.get_mut("mario").unwrap().behavior = Default::default();
        let project_path = save_project(dir.path(), &project);
        assert!(export_project(&project_path, &root).is_err());
        assert_eq!(contract_contents(&root), before);

        save_project(dir.path(), &timed_event_project());
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"unrelated\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let incompatible = contract_contents(&root);
        assert!(export_project(&project_path, &root).is_err());
        assert_eq!(contract_contents(&root), incompatible);
    }
}
