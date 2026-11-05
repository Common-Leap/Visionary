//! Explicit, fail-closed packaging of generated gimmick and ACMD adapters.
//!
//! This module deliberately has no plugin discovery or deployment behavior.
//! Its caller supplies both an already-compatible `slight_replica` source root
//! and an existing, newly-created ARCropolis mod directory. The only source
//! source files it may replace are the two generated modules that
//! [`super::adapter`] has already verified as Visionary-managed; the only
//! package file it may create is a previously absent `plugin.nro`.

use std::any::Any;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use fs2::FileExt;

use super::acmd_adapter::{self, RenderedAcmdAdapter};
use super::adapter::{self, RenderedAdapter};

const PLUGIN_NRO_NAME: &str = "plugin.nro";
const BUILT_NRO_RELATIVE_PATH: &str = "target/aarch64-skyline-switch/release/lib_effect_viewer.nro";
const BUILD_LOCK_NAME: &str = ".visionary-combined-package.lock";

/// The one artifact this packaging path creates in the selected mod folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagedPlugin {
    pub plugin_nro: PathBuf,
    /// Whether the managed adapter source changed before this build.
    pub adapter_changed: bool,
}

/// A packaging failure that identifies the safe boundary which refused it.
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

/// An advisory lock shared by every combined package attempt for one selected
/// plugin. It lives under the ignored Cargo target directory, so it neither
/// changes reviewed source nor survives as a repository file. Holding it from
/// adapter replacement through NRO publication prevents two Visionary exports
/// from compiling different generated slots into the same fixed output.
#[derive(Debug)]
struct PluginBuildLock {
    _file: File,
}

impl PluginBuildLock {
    fn acquire(plugin_root: &Path) -> Result<Self, PackageError> {
        let target = plugin_root.join("target");
        ensure_regular_directory(&target, "plugin build target directory")?;
        let path = target.join(BUILD_LOCK_NAME);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(PackageError {
                    subject: "gimmick plugin build".to_string(),
                    reason: format!(
                        "build lock {} must be a non-symlink regular file",
                        path.display()
                    ),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(PackageError {
                    subject: "gimmick plugin build".to_string(),
                    reason: format!("could not inspect build lock {}: {error}", path.display()),
                });
            }
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            // The file is only an advisory lock identity; never truncate a
            // concurrently opened lock file while acquiring it.
            .truncate(false)
            .open(&path)
            .map_err(|error| PackageError {
                subject: "gimmick plugin build".to_string(),
                reason: format!("could not open build lock {}: {error}", path.display()),
            })?;
        require_regular_file(&path, "plugin build lock")?;
        FileExt::try_lock_exclusive(&file).map_err(|error| PackageError {
            subject: "gimmick plugin build".to_string(),
            reason: format!(
                "another Visionary combined export is already building {}: {error}",
                plugin_root.display()
            ),
        })?;
        Ok(Self { _file: file })
    }
}

/// Build a selected compatible plugin and install its fresh NRO into one empty
/// package slot.
///
/// `plugin_root` must be the source tree the caller explicitly selected. It
/// is contract-checked by [`adapter::export_to_plugin`], never searched for.
/// `package_root` must already be a normal directory, and its `plugin.nro`
/// slot must be absent. That makes this fit a freshly staged export without
/// replacing a compiled artifact owned by another export or user.
///
/// The managed adapter source is restored if any later build, artifact, or
/// package-copy step fails.
pub fn build_into_package(
    plugin_root: &Path,
    package_root: &Path,
    artifact: &RenderedAdapter,
) -> Result<PackagedPlugin, PackageError> {
    build_combined_into_package(
        plugin_root,
        package_root,
        artifact,
        &acmd_adapter::render_empty(),
    )
}

/// Build an explicitly selected compatible runtime with both managed source
/// adapters in one fresh package slot. This is the normal-export integration
/// path: conventional ACMD and gimmick behavior compile into the same NRO.
pub fn build_combined_into_package(
    plugin_root: &Path,
    package_root: &Path,
    behavior: &RenderedAdapter,
    acmd: &RenderedAcmdAdapter,
) -> Result<PackagedPlugin, PackageError> {
    package_combined_with(plugin_root, package_root, behavior, acmd, |root| {
        build_plugin(root)
    })
}

/// Testable combined-package half of [`build_combined_into_package`]. The
/// injected builder is called only after both managed source slots and the
/// fresh `plugin.nro` destination pass their fail-closed checks.
pub fn package_combined_with<F>(
    plugin_root: &Path,
    package_root: &Path,
    behavior: &RenderedAdapter,
    acmd: &RenderedAcmdAdapter,
    build: F,
) -> Result<PackagedPlugin, PackageError>
where
    F: FnOnce(&Path) -> Result<(), PackageError>,
{
    require_regular_directory(package_root, "package directory")?;
    let package_nro = package_root.join(PLUGIN_NRO_NAME);
    require_absent_regular_slot(&package_nro, "package plugin")?;

    // Check the complete contract before creating the target lock, then lock
    // the one fixed Cargo target directory before either generated source slot
    // changes. Rechecking after acquisition closes the normal concurrent
    // Visionary-export window between preflight and mutation.
    adapter::verify_plugin_for_combined_artifacts(plugin_root, behavior, acmd)
        .map_err(adapter_error)?;
    let _build_lock = PluginBuildLock::acquire(plugin_root)?;
    let managed = adapter::verify_plugin_for_combined_artifacts(plugin_root, behavior, acmd)
        .map_err(adapter_error)?;
    let previous_behavior = read_managed_source(&managed.behavior, "gimmick behavior adapter")?;
    let previous_acmd = read_managed_source(&managed.acmd, "gimmick ACMD adapter")?;
    let exported =
        adapter::export_combined_to_plugin(plugin_root, behavior, acmd).map_err(adapter_error)?;

    // Every failure after changing either source slot takes the same
    // restoration path. That includes a command which reports success but
    // leaves no usable NRO, and a package copy failure after a successful
    // build.
    let package_result =
        build_and_copy(plugin_root, &package_nro, build).map(|plugin_nro| PackagedPlugin {
            plugin_nro,
            adapter_changed: exported.changed(),
        });
    package_result.map_err(|error| {
        restore_after_package_failure(
            &managed,
            behavior,
            &previous_behavior,
            acmd,
            &previous_acmd,
            error,
        )
    })
}

/// Quarantine a previous known output, run exactly one builder, and publish
/// only a fresh non-empty NRO. The build's target tree is intentionally not a
/// general writable boundary: every pre-existing path component is required
/// to be a normal directory, so a selected plugin cannot redirect this path
/// through a `target` symlink.
fn build_and_copy<F>(
    plugin_root: &Path,
    package_nro: &Path,
    build: F,
) -> Result<PathBuf, PackageError>
where
    F: FnOnce(&Path) -> Result<(), PackageError>,
{
    let built_nro = checked_build_output(plugin_root, false)?;
    let backup = BuildOutputBackup::quarantine(&built_nro)?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        build(plugin_root)?;
        let fresh_nro = checked_build_output(plugin_root, true)?;
        require_nonempty_regular_file(&fresh_nro, "built plugin")?;
        copy_new_regular_file(&fresh_nro, package_nro)?;
        Ok(package_nro.to_path_buf())
    }));
    match result {
        Ok(Ok(packaged)) => {
            backup.discard();
            Ok(packaged)
        }
        Ok(Err(build_error)) => Err(backup.restore(build_error)),
        Err(payload) => Err(backup.restore(panic_error(payload))),
    }
}

fn panic_error(payload: Box<dyn Any + Send>) -> PackageError {
    let message = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string());
    PackageError {
        subject: "gimmick plugin build".to_string(),
        reason: format!("builder panicked: {message}"),
    }
}

/// Build the reviewed plugin with the one Skyline command supported by its
/// Cargo setup. No install or emulator command is run here.
fn build_plugin(plugin_root: &Path) -> Result<(), PackageError> {
    let temp_dir = build_temp_dir()?;
    std::fs::create_dir_all(&temp_dir).map_err(|error| PackageError {
        subject: "gimmick plugin build".to_string(),
        reason: format!(
            "could not create build scratch directory {}: {error}",
            temp_dir.display()
        ),
    })?;
    let output = Command::new("cargo")
        .args(["skyline", "build", "--release"])
        .current_dir(plugin_root)
        // Fix the output path this packaging contract verifies, instead of
        // inheriting a caller's arbitrary CARGO_TARGET_DIR.
        .env("CARGO_TARGET_DIR", plugin_root.join("target"))
        .env("TMPDIR", &temp_dir)
        .output()
        .map_err(|error| PackageError {
            subject: "gimmick plugin build".to_string(),
            reason: format!("could not start cargo skyline build: {error}"),
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(PackageError {
        subject: "gimmick plugin build".to_string(),
        reason: format!(
            "cargo skyline build --release failed with {}; stderr: {}",
            output.status,
            concise_output(&output.stderr)
        ),
    })
}

fn build_temp_dir() -> Result<PathBuf, PackageError> {
    dirs::cache_dir()
        .map(|root| root.join("builds").join("visionary"))
        .ok_or_else(|| PackageError {
            subject: "gimmick plugin build".to_string(),
            reason: "could not determine the user's cache directory for build scratch space"
                .to_string(),
        })
}

fn concise_output(output: &[u8]) -> String {
    const LIMIT: usize = 2_000;
    let text = String::from_utf8_lossy(output);
    let text = text.trim();
    if text.chars().count() <= LIMIT {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(LIMIT).collect::<String>())
    }
}

/// Return the fixed build artifact path after rejecting any existing symlink
/// in the output chain. Missing components are allowed before a build, but
/// must all be present and normal after it succeeds.
fn checked_build_output(
    plugin_root: &Path,
    require_complete: bool,
) -> Result<PathBuf, PackageError> {
    let mut current = plugin_root.to_path_buf();
    let mut components = Path::new(BUILT_NRO_RELATIVE_PATH).components().peekable();
    while let Some(component) = components.next() {
        let std::path::Component::Normal(part) = component else {
            return Err(PackageError {
                subject: "built plugin".to_string(),
                reason: "internal build artifact path is not a normal relative path".to_string(),
            });
        };
        current.push(part);
        let is_final = components.peek().is_none();
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(PackageError {
                    subject: "built plugin".to_string(),
                    reason: format!("{} must not be a symlink", current.display()),
                });
            }
            Ok(metadata) if is_final => {
                if !metadata.is_file() {
                    return Err(PackageError {
                        subject: "built plugin".to_string(),
                        reason: format!("{} must be a regular file", current.display()),
                    });
                }
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(PackageError {
                    subject: "built plugin".to_string(),
                    reason: format!("{} must be a non-symlink directory", current.display()),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !require_complete => {
                return Ok(plugin_root.join(BUILT_NRO_RELATIVE_PATH));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(PackageError {
                    subject: "built plugin".to_string(),
                    reason: format!("{} was not created by the build", current.display()),
                });
            }
            Err(error) => {
                return Err(PackageError {
                    subject: "built plugin".to_string(),
                    reason: format!("could not inspect {}: {error}", current.display()),
                });
            }
        }
    }
    Ok(current)
}

/// A same-directory quarantine for the previous NRO. It prevents Cargo's
/// exit status alone from being treated as proof that the desired artifact was
/// built, while making a failed package attempt leave this known output as it
/// was before the attempt.
struct BuildOutputBackup {
    output: PathBuf,
    previous: Option<tempfile::TempPath>,
}

impl BuildOutputBackup {
    fn quarantine(output: &Path) -> Result<Self, PackageError> {
        let previous = match std::fs::symlink_metadata(output) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(PackageError {
                        subject: "built plugin".to_string(),
                        reason: format!("{} must be a non-symlink regular file", output.display()),
                    });
                }
                let parent = output.parent().ok_or_else(|| PackageError {
                    subject: "built plugin".to_string(),
                    reason: "build artifact has no parent directory".to_string(),
                })?;
                let temporary = tempfile::NamedTempFile::new_in(parent)
                    .map_err(|error| PackageError {
                        subject: "built plugin".to_string(),
                        reason: format!("could not quarantine {}: {error}", output.display()),
                    })?
                    .into_temp_path();
                // `rename` may not replace an existing destination on
                // Windows. Keep the unique pathname but remove the empty
                // reservation before moving the prior NRO into it.
                std::fs::remove_file(&temporary).map_err(|error| PackageError {
                    subject: "built plugin".to_string(),
                    reason: format!(
                        "could not prepare quarantine for {}: {error}",
                        output.display()
                    ),
                })?;
                std::fs::rename(output, &temporary).map_err(|error| PackageError {
                    subject: "built plugin".to_string(),
                    reason: format!("could not quarantine {}: {error}", output.display()),
                })?;
                Some(temporary)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(PackageError {
                    subject: "built plugin".to_string(),
                    reason: format!("could not inspect {}: {error}", output.display()),
                });
            }
        };
        Ok(Self {
            output: output.to_path_buf(),
            previous,
        })
    }

    fn discard(self) {
        // Dropping the TempPath removes only our private, same-directory
        // quarantine. The new output remains at its fixed path.
        drop(self);
    }

    fn restore(self, build_error: PackageError) -> PackageError {
        let restore = (|| {
            match std::fs::symlink_metadata(&self.output) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                    return Err(PackageError {
                        subject: "built plugin".to_string(),
                        reason: format!(
                            "{} changed concurrently and was not removed",
                            self.output.display()
                        ),
                    });
                }
                Ok(_) => std::fs::remove_file(&self.output).map_err(|error| PackageError {
                    subject: "built plugin".to_string(),
                    reason: format!(
                        "could not remove failed output {}: {error}",
                        self.output.display()
                    ),
                })?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(PackageError {
                        subject: "built plugin".to_string(),
                        reason: format!(
                            "could not inspect failed output {}: {error}",
                            self.output.display()
                        ),
                    });
                }
            }
            if let Some(previous) = self.previous {
                std::fs::rename(&previous, &self.output).map_err(|error| PackageError {
                    subject: "built plugin".to_string(),
                    reason: format!(
                        "could not restore previous output {}: {error}",
                        self.output.display()
                    ),
                })?;
            }
            Ok(())
        })();
        match restore {
            Ok(()) => build_error,
            Err(restore_error) => PackageError {
                subject: build_error.subject,
                reason: format!(
                    "{}; additionally {}",
                    build_error.reason, restore_error.reason
                ),
            },
        }
    }
}

fn read_managed_source(path: &Path, subject: &str) -> Result<String, PackageError> {
    std::fs::read_to_string(path).map_err(|error| PackageError {
        subject: subject.to_string(),
        reason: format!("could not read managed slot {}: {error}", path.display()),
    })
}

fn restore_after_package_failure(
    managed: &adapter::PluginDestinations,
    behavior: &RenderedAdapter,
    previous_behavior: &str,
    acmd: &RenderedAcmdAdapter,
    previous_acmd: &str,
    build_error: PackageError,
) -> PackageError {
    let slots = [
        (
            &managed.acmd,
            acmd.source.as_str(),
            previous_acmd,
            "managed ACMD adapter",
        ),
        (
            &managed.behavior,
            behavior.source.as_str(),
            previous_behavior,
            "managed behavior adapter",
        ),
    ];
    let mut problems = Vec::new();
    for (path, expected, previous, label) in slots {
        let current = std::fs::read_to_string(path);
        match current {
            Ok(current) if current == expected => {
                if let Err(error) = replace_regular_file(path, previous) {
                    problems.push(format!("could not restore {label}: {}", error.reason));
                }
            }
            Ok(_) => problems.push(format!("{label} changed concurrently and was not restored")),
            Err(error) => problems.push(format!(
                "could not inspect {label} {} for restoration: {error}",
                path.display()
            )),
        }
    }
    if problems.is_empty() {
        build_error
    } else {
        PackageError {
            subject: build_error.subject,
            reason: format!(
                "{}; additionally {}",
                build_error.reason,
                problems.join("; ")
            ),
        }
    }
}

fn adapter_error(error: adapter::AdapterError) -> PackageError {
    PackageError {
        subject: error.subject,
        reason: error.reason,
    }
}

fn require_regular_directory(path: &Path, label: &str) -> Result<(), PackageError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| PackageError {
        subject: label.to_string(),
        reason: format!("{} is unavailable: {error}", path.display()),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PackageError {
            subject: label.to_string(),
            reason: format!("{} must be a non-symlink directory", path.display()),
        });
    }
    Ok(())
}

fn ensure_regular_directory(path: &Path, label: &str) -> Result<(), PackageError> {
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(PackageError {
                subject: label.to_string(),
                reason: format!("could not create {}: {error}", path.display()),
            });
        }
    }
    require_regular_directory(path, label)
}

fn require_regular_file(path: &Path, label: &str) -> Result<(), PackageError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| PackageError {
        subject: label.to_string(),
        reason: format!("{} is unavailable: {error}", path.display()),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PackageError {
            subject: label.to_string(),
            reason: format!("{} must be a non-symlink regular file", path.display()),
        });
    }
    Ok(())
}

fn require_nonempty_regular_file(path: &Path, label: &str) -> Result<(), PackageError> {
    require_regular_file(path, label)?;
    let length = std::fs::metadata(path)
        .map_err(|error| PackageError {
            subject: label.to_string(),
            reason: format!("could not inspect {}: {error}", path.display()),
        })?
        .len();
    if length == 0 {
        return Err(PackageError {
            subject: label.to_string(),
            reason: format!("{} is empty", path.display()),
        });
    }
    Ok(())
}

fn require_absent_regular_slot(path: &Path, label: &str) -> Result<(), PackageError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(PackageError {
            subject: label.to_string(),
            reason: format!(
                "{} already exists; refusing to overwrite it",
                path.display()
            ),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PackageError {
            subject: label.to_string(),
            reason: format!("could not inspect {}: {error}", path.display()),
        }),
    }
}

fn copy_new_regular_file(source: &Path, destination: &Path) -> Result<(), PackageError> {
    require_absent_regular_slot(destination, "package plugin")?;
    let parent = destination.parent().ok_or_else(|| PackageError {
        subject: "package plugin".to_string(),
        reason: "destination has no parent directory".to_string(),
    })?;
    require_regular_directory(parent, "package directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| PackageError {
        subject: "package plugin".to_string(),
        reason: format!("could not stage {}: {error}", destination.display()),
    })?;
    let mut source_file = std::fs::File::open(source).map_err(|error| PackageError {
        subject: "built plugin".to_string(),
        reason: format!("could not read {}: {error}", source.display()),
    })?;
    std::io::copy(&mut source_file, temporary.as_file_mut())
        .and_then(|_| temporary.as_file_mut().sync_all())
        .map_err(|error| PackageError {
            subject: "package plugin".to_string(),
            reason: format!("could not stage {}: {error}", destination.display()),
        })?;
    temporary
        .persist_noclobber(destination)
        .map(|_| ())
        .map_err(|error| PackageError {
            subject: "package plugin".to_string(),
            reason: format!(
                "could not create {}: {}",
                destination.display(),
                error.error
            ),
        })
}

/// Restore a previously verified regular managed slot without parsing it again.
/// The bytes were read from that exact slot immediately before the package
/// attempt; parsing here would prevent restoring a pre-existing build failure.
fn replace_regular_file(path: &Path, contents: &str) -> Result<(), PackageError> {
    require_regular_file(path, "managed adapter")?;
    let parent = path.parent().ok_or_else(|| PackageError {
        subject: "managed adapter".to_string(),
        reason: "managed slot has no parent directory".to_string(),
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| PackageError {
        subject: "managed adapter".to_string(),
        reason: format!("could not stage {}: {error}", path.display()),
    })?;
    temporary
        .write_all(contents.as_bytes())
        .and_then(|()| temporary.as_file_mut().sync_all())
        .map_err(|error| PackageError {
            subject: "managed adapter".to_string(),
            reason: format!("could not stage {}: {error}", path.display()),
        })?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| PackageError {
            subject: "managed adapter".to_string(),
            reason: format!("could not restore {}: {}", path.display(), error.error),
        })
}

#[cfg(test)]
/// The testable half of [`build_into_package`]. The injected builder is called
/// only after all destination checks and managed-slot validation pass.
pub fn package_with<F>(
    plugin_root: &Path,
    package_root: &Path,
    artifact: &RenderedAdapter,
    build: F,
) -> Result<PackagedPlugin, PackageError>
where
    F: FnOnce(&Path) -> Result<(), PackageError>,
{
    package_combined_with(
        plugin_root,
        package_root,
        artifact,
        &acmd_adapter::render_empty(),
        build,
    )
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

    fn write_built_nro(root: &Path, bytes: &[u8]) {
        let output = root.join(BUILT_NRO_RELATIVE_PATH);
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(output, bytes).unwrap();
    }

    #[test]
    fn packages_only_a_new_plugin_slot_after_the_selected_builder_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        let package = dir.path().join("package");
        compatible_plugin(&plugin);
        std::fs::create_dir(&package).unwrap();
        let unrelated = package.join("info.toml");
        std::fs::write(&unrelated, "keep").unwrap();

        let packaged = package_with(&plugin, &package, &artifact(), |root| {
            write_built_nro(root, b"fresh-nro");
            Ok(())
        })
        .unwrap();

        assert!(packaged.adapter_changed);
        assert_eq!(std::fs::read(packaged.plugin_nro).unwrap(), b"fresh-nro");
        assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "keep");
    }

    #[test]
    fn selected_plugin_builds_are_serialized_before_adapter_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        compatible_plugin(&plugin);

        let held = PluginBuildLock::acquire(&plugin).unwrap();
        let error = PluginBuildLock::acquire(&plugin).unwrap_err();
        assert!(error.reason.contains("another Visionary combined export"));
        drop(held);
        PluginBuildLock::acquire(&plugin).unwrap();
    }

    #[test]
    fn an_existing_package_plugin_is_refused_before_adapter_or_builder_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        let package = dir.path().join("package");
        compatible_plugin(&plugin);
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join(PLUGIN_NRO_NAME), b"user artifact").unwrap();
        let generated = plugin.join("src/slight/gimmicks/generated.rs");
        let before = std::fs::read_to_string(&generated).unwrap();
        let mut called = false;

        let error = package_with(&plugin, &package, &artifact(), |_| {
            called = true;
            Ok(())
        })
        .unwrap_err();

        assert!(error.reason.contains("refusing to overwrite"));
        assert!(!called);
        assert_eq!(std::fs::read_to_string(generated).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn a_target_symlink_is_refused_and_never_reaches_the_builder() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        let package = dir.path().join("package");
        compatible_plugin(&plugin);
        std::fs::create_dir(&package).unwrap();
        let redirected_target = dir.path().join("outside-target");
        std::fs::create_dir(&redirected_target).unwrap();
        symlink(&redirected_target, plugin.join("target")).unwrap();
        let generated = plugin.join("src/slight/gimmicks/generated.rs");
        let before = std::fs::read_to_string(&generated).unwrap();
        let mut called = false;

        let error = package_with(&plugin, &package, &artifact(), |_| {
            called = true;
            Ok(())
        })
        .unwrap_err();

        assert!(error.reason.contains("non-symlink directory"), "{error}");
        assert!(!called);
        assert_eq!(std::fs::read_to_string(generated).unwrap(), before);
        assert!(!package.join(PLUGIN_NRO_NAME).exists());
    }
    #[test]
    fn failed_builds_restore_managed_sources_and_previous_output() {
        for (case, previous, expected) in [
            ("error", false, "deliberate failure"),
            ("panic", true, "builder panicked"),
            ("missing", false, "built plugin"),
            ("stale", true, "was not created by the build"),
            ("empty", true, "is empty"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let plugin = dir.path().join("plugin");
            let package = dir.path().join("package");
            compatible_plugin(&plugin);
            std::fs::create_dir(&package).unwrap();
            if previous {
                write_built_nro(&plugin, b"previous-nro");
            }
            let sources = ["generated.rs", "acmd_generated.rs"]
                .map(|name| plugin.join("src/slight/gimmicks").join(name));
            let before = sources.each_ref().map(|path| std::fs::read(path).unwrap());
            let error = package_with(&plugin, &package, &artifact(), |root| {
                match case {
                    "error" => {
                        return Err(PackageError {
                            subject: "test builder".into(),
                            reason: "deliberate failure".into(),
                        })
                    }
                    "panic" => {
                        write_built_nro(root, b"partial-nro");
                        panic!("deliberate builder panic");
                    }
                    "empty" => write_built_nro(root, b""),
                    _ => {}
                }
                Ok(())
            })
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{case}: {error}");
            assert_eq!(sources.map(|path| std::fs::read(path).unwrap()), before);
            assert_eq!(
                std::fs::read(plugin.join(BUILT_NRO_RELATIVE_PATH)).ok(),
                previous.then(|| b"previous-nro".to_vec())
            );
            assert!(!package.join(PLUGIN_NRO_NAME).exists());
        }
    }
}
