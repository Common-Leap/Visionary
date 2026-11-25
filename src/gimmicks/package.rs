//! Building the plugin an exported mod carries.
//!
//! The selected plugin source is copied into a folder of the export's own, the export's
//! generated behavior and ACMD go into that copy, and the copy is what gets built (or, for
//! Export source code, handed to the user). Exporting never changes the plugin source a user
//! links or edits, and the built plugin is the mod flavour with no editor connection.

use std::fmt;
use std::path::{Path, PathBuf};

use super::acmd_adapter::RenderedAcmdAdapter;
use super::adapter::{self, RenderedAdapter};

const PLUGIN_NRO_NAME: &str = "plugin.nro";

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

/// Copy the plugin template at `plugin_root` into `dest` (which must not exist yet) as a crate
/// called `name`, with this export's behavior and ACMD in the two generated slots. The template
/// is the mod flavour of Visionary's plugin cut down to the files it compiles (see
/// plugins/slight_replica/tools/build_plugin.py), so the copy is an ordinary smashline plugin
/// that names nothing outside itself. Build output is left behind.
pub fn write_source_tree(
    plugin_root: &Path,
    dest: &Path,
    name: &str,
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
    let copy = |e: std::io::Error| {
        error(
            "plugin source",
            format!("copying to {}: {e}", dest.display()),
        )
    };
    copy_tree(plugin_root, dest).map_err(copy)?;
    adapter::export_combined_to_plugin(dest, behavior, acmd)
        .map_err(|e| error(&e.subject, e.reason))?;
    name_plugin(dest, name).map_err(copy)
}

/// Give the copied crate, its library and the plugin the game loads the mod's own name.
fn name_plugin(crate_dir: &Path, name: &str) -> std::io::Result<()> {
    let manifest = crate_dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)?
        .replacen(
            "name = \"slight_replica\"",
            &format!("name = \"{name}\""),
            1,
        )
        .replacen(
            "name = \"_effect_viewer\"",
            &format!("name = \"{name}\""),
            1,
        );
    let text = text
        .lines()
        .filter(|line| !line.starts_with("authors") && !line.starts_with("description"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&manifest, text + "\n")?;
    let lib = crate_dir.join("src/lib.rs");
    let text = std::fs::read_to_string(&lib)?.replacen(
        "#[skyline::main(name = \"visionary_mod\")]",
        &format!("#[skyline::main(name = \"{name}\")]"),
        1,
    );
    std::fs::write(&lib, text)?;
    rename_hooks(crate_dir, name)
}

/// `#[skyline::hook]` exports each hook under its function name, and the game's loader links a
/// name every plugin exports to the first plugin that has it. Two mods built from this template
/// (or a mod and Visionary's live plugin) would then run each other's hooks, so every hook in
/// the copy is renamed after the mod.
fn rename_hooks(crate_dir: &Path, name: &str) -> std::io::Result<()> {
    let mut files = Vec::new();
    collect_sources(&crate_dir.join("src"), &mut files);
    let mut hooks = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file)?;
        for (at, _) in text.match_indices("#[skyline::hook(") {
            let rest = &text[at..];
            let Some(fn_at) = rest.find("fn ") else {
                continue;
            };
            let ident: String = rest[fn_at + 3..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !ident.is_empty() && !hooks.contains(&ident) {
                hooks.push(ident);
            }
        }
    }
    if hooks.is_empty() {
        return Ok(());
    }
    for file in &files {
        let text = std::fs::read_to_string(file)?;
        let renamed = rename_identifiers(&text, &hooks, name);
        if renamed != text {
            std::fs::write(file, renamed)?;
        }
    }
    Ok(())
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Replace whole-word uses of each identifier with `<identifier>_<suffix>`.
fn rename_identifiers(text: &str, idents: &[String], suffix: &str) -> String {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if word(c) && !text[..at].ends_with(word) {
            let end = text[at..]
                .find(|c: char| !word(c))
                .map_or(text.len(), |len| at + len);
            let token = &text[at..end];
            out.push_str(token);
            if idents.iter().any(|ident| ident == token) {
                out.push('_');
                out.push_str(suffix);
            }
            while chars.peek().is_some_and(|(next, _)| *next < end) {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
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
    name: &str,
    behavior: &RenderedAdapter,
    acmd: &RenderedAcmdAdapter,
) -> Result<PackagedPlugin, PackageError> {
    let work = crate::scratch_dirs::app_storage_root().join("export-plugin");
    package_combined_with(
        plugin_root,
        package_root,
        &work,
        name,
        behavior,
        acmd,
        |tree| build_plugin(tree, &work.join("target"), name),
    )
}

/// [`build_combined_into_package`] with the build folder and builder chosen by the caller.
/// The builder gets the prepared source copy and returns the plugin it built.
pub fn package_combined_with<F>(
    plugin_root: &Path,
    package_root: &Path,
    work: &Path,
    name: &str,
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
    write_source_tree(plugin_root, &tree, name, behavior, acmd)?;
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

/// `cargo skyline build` of the copied crate, with build output kept in `target` between exports.
fn build_plugin(tree: &Path, target: &Path, name: &str) -> Result<PathBuf, PackageError> {
    let temp = crate::scratch_dirs::app_storage_root().join("export-plugin-tmp");
    std::fs::create_dir_all(&temp)
        .map_err(|e| error("plugin build", format!("creating {}: {e}", temp.display())))?;
    let output = crate::rust_tools::command("cargo")
        .args(["skyline", "build", "--release"])
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
    Ok(target.join(format!("aarch64-skyline-switch/release/lib{name}.nro")))
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
            "my_mod_plugin",
            &artifact(),
            &super::super::acmd_adapter::render_empty(),
            |tree| {
                assert!(!tree.join("target").exists(), "build output is not copied");
                let manifest = std::fs::read_to_string(tree.join("Cargo.toml")).unwrap();
                assert!(manifest.contains("name = \"my_mod_plugin\""), "{manifest}");
                assert!(!manifest.contains("slight_replica"), "{manifest}");
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
    fn hooks_are_renamed_after_the_mod_so_two_plugins_never_share_one() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "#[skyline::hook(offset = 0x10)]\nunsafe fn hook_get(a: u64) -> u64 { original!()(a) }\nfn install() { skyline::install_hooks!(hook_get); }\nfn hook_getter() {}\n",
        )
        .unwrap();
        rename_hooks(dir.path(), "my_mod").unwrap();
        let text = std::fs::read_to_string(src.join("lib.rs")).unwrap();
        assert!(text.contains("unsafe fn hook_get_my_mod(a: u64)"), "{text}");
        assert!(text.contains("install_hooks!(hook_get_my_mod)"), "{text}");
        assert!(text.contains("fn hook_getter()"), "{text}");
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
            "my_mod_plugin",
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
