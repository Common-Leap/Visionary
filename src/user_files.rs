//! Writing the files a user works in: their source, params, project and settings.
//!
//! Every write goes beside the file first and is renamed over it, so a crash or a full disk
//! leaves the old file or the new one, never half of either.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

/// A file's new contents, planned against what was read from it.
#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    pub file: PathBuf,
    pub read: Vec<u8>,
    pub write: Vec<u8>,
}

impl Rewrite {
    pub fn text(file: &Path, read: &str, write: String) -> Self {
        Self {
            file: file.to_path_buf(),
            read: read.as_bytes().to_vec(),
            write: write.into_bytes(),
        }
    }
}

/// The game dump's root, which no edit of the user's own files may reach into.
static GAME_DUMP: RwLock<Option<PathBuf>> = RwLock::new(None);

pub fn set_game_dump(root: Option<&Path>) {
    if let Ok(mut dump) = GAME_DUMP.write() {
        *dump = root.map(real_path);
    }
}

/// The folders data.arc has at its root. A project kept beside them, inside the folder picked as
/// the dump, is still the user's.
const ARC_FOLDERS: [&str; 20] = [
    "append",
    "assist",
    "boss",
    "camera",
    "campaign",
    "common",
    "effect",
    "enemy",
    "fighter",
    "finalsmash",
    "item",
    "miihat",
    "param",
    "pokemon",
    "render",
    "snapshot",
    "sound",
    "spirits",
    "stage",
    "ui",
];

/// Whether `file` is one of the game dump's own files.
fn in_game_dump(file: &Path, dump: &Path) -> bool {
    file.strip_prefix(dump).is_ok_and(|rel| {
        rel.components().next().is_some_and(|first| {
            ARC_FOLDERS
                .iter()
                .any(|folder| first.as_os_str().eq_ignore_ascii_case(folder))
        })
    })
}

/// `path` with its links followed. Windows' `\\?\` form is left out: other programs, cargo among
/// them, do not take it. A path that does not exist is returned as it is.
pub fn real_path(path: &Path) -> PathBuf {
    let Ok(real) = std::fs::canonicalize(path) else {
        return path.to_path_buf();
    };
    if cfg!(windows) {
        let text = real.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    real
}

/// `contents` written beside `file`, ready to be renamed over it: through a link to the file it
/// names, and with that file's permissions.
struct Staged {
    target: PathBuf,
    temporary: tempfile::NamedTempFile,
}

fn stage(file: &Path, contents: &[u8]) -> Result<Staged> {
    let target = real_path(file);
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let existing = std::fs::metadata(&target).ok().map(|m| m.permissions());
    if existing.as_ref().is_some_and(|p| p.readonly()) {
        bail!("{} is read-only", file.display());
    }
    let mut builder = tempfile::Builder::new();
    builder.prefix(".visionary-");
    // A temporary file is private to its owner by default; a new file is not.
    #[cfg(unix)]
    builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o666));
    let written = (|| {
        let mut temporary = builder.tempfile_in(parent)?;
        temporary.write_all(contents)?;
        if let Some(permissions) = existing {
            temporary.as_file().set_permissions(permissions)?;
        }
        Ok::<_, std::io::Error>(temporary)
    })();
    let temporary = written.with_context(|| format!("writing {}", file.display()))?;
    Ok(Staged { target, temporary })
}

impl Staged {
    fn commit(self) -> Result<()> {
        self.temporary
            .persist(&self.target)
            .map_err(|error| error.error)
            .with_context(|| format!("writing {}", self.target.display()))?;
        Ok(())
    }
}

/// Write `contents` to `file`, replacing it whole or leaving it as it was.
pub fn write(file: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    stage(file, contents.as_ref())?.commit()
}

/// Whether `file` is one of the game dump's own files, which nothing here may write.
pub fn is_game_file(file: &Path) -> bool {
    let dump = GAME_DUMP.read().ok().and_then(|dump| dump.clone());
    dump.is_some_and(|dump| in_game_dump(&real_path_of_new(file), &dump))
}

/// [`real_path`] for a file that may not be there yet: its folder's links are followed.
fn real_path_of_new(file: &Path) -> PathBuf {
    if file.exists() {
        return real_path(file);
    }
    let mut missing = Vec::new();
    let mut folder = file;
    while !folder.exists() {
        let (Some(parent), Some(name)) = (folder.parent(), folder.file_name()) else {
            return file.to_path_buf();
        };
        missing.push(name.to_os_string());
        folder = parent;
    }
    missing
        .iter()
        .rev()
        .fold(real_path(folder), |path, name| path.join(name))
}

/// Write `contents` to `file` whole, making its folder: a new file, or over one the user picked.
/// Never one of the game dump's.
pub fn write_outside_dump(file: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    if is_game_file(file) {
        bail!(
            "{} is in the game dump, which Visionary never changes",
            file.display()
        );
    }
    if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    write(file, contents)
}

/// Write every planned file, or none of them. Each must still hold what was read from it, and
/// none may be one of the game dump's.
pub fn write_all(rewrites: &[Rewrite]) -> Result<()> {
    let dump = GAME_DUMP.read().ok().and_then(|dump| dump.clone());
    write_all_outside(rewrites, dump.as_deref())
}

fn write_all_outside(rewrites: &[Rewrite], dump: Option<&Path>) -> Result<()> {
    let mut staged = Vec::with_capacity(rewrites.len());
    for rewrite in rewrites {
        let file = &rewrite.file;
        if dump.is_some_and(|dump| in_game_dump(&real_path(file), dump)) {
            bail!(
                "{} is in the game dump, which Visionary never changes",
                file.display()
            );
        }
        let now = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        if now != rewrite.read {
            bail!(
                "{} changed on disk since it was read — rescan and try again",
                file.display()
            );
        }
        staged.push(stage(file, &rewrite.write)?);
    }
    commit_all(staged, rewrites)
}

/// Rename every staged file into place. An edit across files is one edit: when one fails, those
/// already replaced are put back.
fn commit_all(staged: Vec<Staged>, rewrites: &[Rewrite]) -> Result<()> {
    for (done, one) in staged.into_iter().enumerate() {
        if let Err(error) = one.commit() {
            for rewrite in &rewrites[..done] {
                if write(&rewrite.file, &rewrite.read).is_err() {
                    return Err(error.context(format!(
                        "{} was already changed and could not be put back",
                        rewrite.file.display()
                    )));
                }
            }
            return Err(error);
        }
    }
    Ok(())
}

/// What each settings file held when it was last read or written here.
static SETTINGS: Mutex<Option<HashMap<PathBuf, Option<Vec<u8>>>>> = Mutex::new(None);

/// Read a settings file, remembering what it held for [`save_settings`].
pub fn read_settings(file: &Path) -> Option<Vec<u8>> {
    let contents = std::fs::read(file).ok();
    remember(file, contents.clone());
    contents
}

fn remember(file: &Path, contents: Option<Vec<u8>>) {
    if let Ok(mut settings) = SETTINGS.lock() {
        settings
            .get_or_insert_with(HashMap::new)
            .insert(file.to_path_buf(), contents);
    }
}

/// Write a settings file, unless something else changed it since it was read: another Visionary
/// window, or the user by hand.
pub fn save_settings(file: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let known = SETTINGS
        .lock()
        .ok()
        .and_then(|settings| settings.as_ref()?.get(file).cloned());
    if let Some(known) = known {
        if std::fs::read(file).ok() != known {
            bail!(
                "{} changed on disk since it was read, so it was left as it is",
                file.display()
            );
        }
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    write(file, contents.as_ref())?;
    remember(file, Some(contents.as_ref().to_vec()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(file: &Path, read: &str, write: &str) -> Rewrite {
        Rewrite::text(file, read, write.to_string())
    }

    /// A file that is not there yet is still found to be the dump's, through a linked folder too.
    #[test]
    fn a_new_file_in_the_dump_is_the_dumps() {
        let dir = tempfile::tempdir().unwrap();
        let dump = real_path(dir.path()).join("dump");
        std::fs::create_dir_all(dump.join("fighter/mario")).unwrap();
        let new = dump.join("fighter/mario/motion/body/c00/motion_list.bin");
        assert!(in_game_dump(&real_path_of_new(&new), &dump));
        // A project kept inside the folder picked as the dump is the user's.
        let project = dump.join("my_project/romfs/fighter/mario/motion_list.bin");
        assert!(!in_game_dump(&real_path_of_new(&project), &dump));
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(dump.join("fighter"), &link).unwrap();
            let through = link.join("mario/motion/body/c00/motion_list.bin");
            assert!(in_game_dump(&real_path_of_new(&through), &dump));
        }
    }

    /// The second file cannot be written, so the first is not left changed.
    #[test]
    fn a_plan_is_written_whole_or_not_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a.rs"), tmp.path().join("b.rs"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        write_all(&[rewrite(&a, "a", "a2"), rewrite(&b, "b", "b2")]).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a2");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b2");

        // `b` changed since it was read.
        let error = write_all(&[rewrite(&a, "a2", "a3"), rewrite(&b, "b", "b3")]).unwrap_err();
        assert!(error.to_string().contains("changed on disk"), "{error}");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a2");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b2");

        // `b` is read-only.
        let mut permissions = std::fs::metadata(&b).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&b, permissions).unwrap();
        let error = write_all(&[rewrite(&a, "a2", "a3"), rewrite(&b, "b2", "b3")]).unwrap_err();
        assert!(error.to_string().contains("read-only"), "{error}");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a2");
        // Nothing is left beside them.
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 2);
    }

    /// A rename that fails after another file was already replaced puts that one back.
    #[test]
    fn a_failed_rename_puts_the_replaced_files_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a.rs"), tmp.path().join("b.rs"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let plan = [rewrite(&a, "a", "a2"), rewrite(&b, "b", "b2")];
        let staged = || -> Vec<Staged> {
            plan.iter()
                .map(|r| stage(&r.file, &r.write).unwrap())
                .collect()
        };
        commit_all(staged(), &plan).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a2");

        std::fs::write(&a, "a").unwrap();
        let mut staged = staged();
        // A folder that is not empty where the second file goes: renaming over it fails.
        staged[1].target = tmp.path().to_path_buf();
        assert!(commit_all(staged, &plan).is_err());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn a_rewrite_keeps_the_files_permissions_and_links() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let (file, link) = (tmp.path().join("real.rs"), tmp.path().join("link.rs"));
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&file, PermissionsExt::from_mode(0o664)).unwrap();
        std::os::unix::fs::symlink(&file, &link).unwrap();
        write(&link, "new").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o664);
    }

    #[test]
    fn the_game_dumps_own_files_are_never_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let dump = real_path(tmp.path());
        let param = dump.join("fighter/mario/param/vl.prcxml");
        let mine = dump.join("my-moveset/src/lib.rs");
        for file in [&param, &mine] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "old").unwrap();
        }
        let error = write_all_outside(&[rewrite(&param, "old", "new")], Some(&dump)).unwrap_err();
        assert!(error.to_string().contains("game dump"), "{error}");
        assert_eq!(std::fs::read_to_string(&param).unwrap(), "old");
        write_all_outside(&[rewrite(&mine, "old", "new")], Some(&dump)).unwrap();
        assert_eq!(std::fs::read_to_string(&mine).unwrap(), "new");
        // Without a dump, a file at the same path is the user's.
        write_all_outside(&[rewrite(&param, "old", "new")], None).unwrap();
    }

    #[test]
    fn a_settings_file_changed_by_something_else_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("made/preferences.json");
        assert_eq!(read_settings(&file), None);
        save_settings(&file, "one").unwrap();
        save_settings(&file, "two").unwrap();
        std::fs::write(&file, "by hand").unwrap();
        assert!(save_settings(&file, "three").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "by hand");
        // Read again, it is known again.
        assert_eq!(read_settings(&file).as_deref(), Some(&b"by hand"[..]));
        save_settings(&file, "three").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "three");
    }
}
