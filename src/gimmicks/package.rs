//! Building the plugin an exported mod carries.
//!
//! The selected plugin source is copied into a folder of the export's own, the export's
//! generated behavior and ACMD go into that copy, and the copy is what gets built (or, for
//! Export source code, handed to the user). Exporting never changes the plugin source a user
//! links or edits, and the built plugin is the mod flavour with no editor connection.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::acmd_adapter::RenderedAcmdAdapter;
use super::adapter::{self, RenderedAdapter};

const PLUGIN_NRO_NAME: &str = "plugin.nro";
const BUILT_NRO_RELATIVE_PATH: &str = "aarch64-skyline-switch/release/lib_effect_viewer.nro";

/// The one artifact this packaging path creates in the selected mod folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagedPlugin {
    pub plugin_nro: PathBuf,
}

/// A packaging failure and the step that refused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageError {
    pub subject: String,
    pub reason: String,
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.subject, self.reason)
    }
}

fn error(subject: &str, reason: impl Into<String>) -> PackageError {
    PackageError {
        subject: subject.to_string(),
        reason: reason.into(),
    }
}

/// Copy the plugin source at `plugin_root` to `dest` (which must not exist yet), with this
/// export's behavior and ACMD in the two generated slots. Build output is left behind.
pub fn write_source_tree(
    plugin_root: &Path,
    dest: &Path,
    behavior: &RenderedAdapter,
    acmd: &RenderedAcmdAdapter,
) -> Result<(), PackageError> {
    adapter::verify_plugin_for_combined_artifacts(plugin_root, behavior, acmd)
        .map_err(|e| error(&e.subject, e.reason))?;
    if dest.exists() {
        return Err(error(
            "plugin source",
            format!("{} already exists", dest.display()),
        ));
    }
    copy_tree(plugin_root, dest).map_err(|e| {
        error(
            "plugin source",
            format!("copying to {}: {e}", dest.display()),
        )
    })?;
    adapter::export_combined_to_plugin(dest, behavior, acmd)
        .map_err(|e| error(&e.subject, e.reason))?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "target" || name == ".git" {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &to.join(&name))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), to.join(&name))?;
        }
    }
    Ok(())
}

/// Build the export's plugin into `package_root/plugin.nro`.
pub fn build_combined_into_package(
    plugin_root: &Path,
    package_root: &Path,
    behavior: &RenderedAdapter,
    acmd: &RenderedAcmdAdapter,
) -> Result<PackagedPlugin, PackageError> {
    let work = crate::scratch_dirs::app_storage_root().join("export-plugin");
    package_combined_with(plugin_root, package_root, &work, behavior, acmd, |tree| {
        build_plugin(tree, &work.join("target"))
    })
}

/// [`build_combined_into_package`] with the build folder and builder chosen by the caller.
/// The builder gets the prepared source copy and returns the plugin it built.
pub fn package_combined_with<F>(
    plugin_root: &Path,
    package_root: &Path,
    work: &Path,
    behavior: &RenderedAdapter,
    acmd: &RenderedAcmdAdapter,
    build: F,
) -> Result<PackagedPlugin, PackageError>
where
    F: FnOnce(&Path) -> Result<PathBuf, PackageError>,
{
    if !package_root.is_dir() {
        return Err(error(
            "package",
            format!("{} is not a folder", package_root.display()),
        ));
    }
    let plugin_nro = package_root.join(PLUGIN_NRO_NAME);
    if plugin_nro.exists() {
        return Err(error(
            "package",
            format!("{} already exists", plugin_nro.display()),
        ));
    }
    let tree = work.join("source");
    if tree.exists() {
        std::fs::remove_dir_all(&tree)
            .map_err(|e| error("plugin source", format!("clearing {}: {e}", tree.display())))?;
    }
    write_source_tree(plugin_root, &tree, behavior, acmd)?;
    let built = build(&tree)?;
    let size = std::fs::metadata(&built).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err(error(
            "plugin build",
            format!("no plugin at {}", built.display()),
        ));
    }
    std::fs::copy(&built, &plugin_nro)
        .map_err(|e| error("package", format!("copying the plugin: {e}")))?;
    Ok(PackagedPlugin { plugin_nro })
}

/// `cargo skyline build` of the mod flavour, with build output kept in `target` between exports.
fn build_plugin(tree: &Path, target: &Path) -> Result<PathBuf, PackageError> {
    let temp = crate::scratch_dirs::app_storage_root().join("export-plugin-tmp");
    std::fs::create_dir_all(&temp)
        .map_err(|e| error("plugin build", format!("creating {}: {e}", temp.display())))?;
    let output = Command::new("cargo")
        .args(["skyline", "build", "--release", "--features", "exported"])
        .current_dir(tree)
        .env("CARGO_TARGET_DIR", target)
        .env("TMPDIR", &temp)
        .output()
        .map_err(|e| {
            error(
                "plugin build",
                format!("could not start cargo skyline: {e}"),
            )
        })?;
    if !output.status.success() {
        let log = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = log.lines().rev().take(12).collect();
        return Err(error(
            "plugin build",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n"),
        ));
    }
    Ok(target.join(BUILT_NRO_RELATIVE_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::fixtures::plugin as compatible_plugin;

    fn artifact() -> RenderedAdapter {
        RenderedAdapter {
            source: "const VISIONARY_STATIC_BEHAVIOR_ADAPTER_SLOT: &str = \"visionary.gimmicks.generated-slot.v1\";\npub fn register_all() -> Result<(), String> { Ok(()) }\n".into(),
            fighters: Vec::new(),
        }
    }

    #[test]
    fn exporting_builds_a_copy_and_leaves_the_plugin_source_alone() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        let package = dir.path().join("package");
        let work = dir.path().join("work");
        compatible_plugin(&plugin);
        std::fs::create_dir_all(plugin.join("target/big")).unwrap();
        std::fs::create_dir(&package).unwrap();
        let slot = plugin.join("src/slight/gimmicks/generated.rs");
        let before = std::fs::read_to_string(&slot).unwrap();

        let packaged = package_combined_with(
            &plugin,
            &package,
            &work,
            &artifact(),
            &super::super::acmd_adapter::render_empty(),
            |tree| {
                assert_ne!(tree, plugin.as_path());
                assert!(!tree.join("target").exists(), "build output is not copied");
                let generated =
                    std::fs::read_to_string(tree.join("src/slight/gimmicks/generated.rs")).unwrap();
                assert!(generated.contains("register_all"));
                let nro = tree.join("built.nro");
                std::fs::write(&nro, b"plugin").unwrap();
                Ok(nro)
            },
        )
        .unwrap();

        assert_eq!(std::fs::read(packaged.plugin_nro).unwrap(), b"plugin");
        assert_eq!(std::fs::read_to_string(&slot).unwrap(), before);
    }

    #[test]
    fn an_existing_plugin_in_the_mod_folder_is_refused_before_building() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        let package = dir.path().join("package");
        compatible_plugin(&plugin);
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join(PLUGIN_NRO_NAME), b"old").unwrap();
        let result = package_combined_with(
            &plugin,
            &package,
            &dir.path().join("work"),
            &artifact(),
            &super::super::acmd_adapter::render_empty(),
            |_| panic!("must not build"),
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(package.join(PLUGIN_NRO_NAME)).unwrap(),
            b"old"
        );
    }
}
