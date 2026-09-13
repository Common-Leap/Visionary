//! Bounded, immutable SD snapshots for carrier reloads.

use anyhow::{bail, Context};
use std::path::{Path, PathBuf};

const CACHE_LIMIT: u64 = 1024 * 1024 * 1024;
const MAX_SNAPSHOT: u64 = 256 * 1024 * 1024;

fn tree_bytes(root: &Path) -> anyhow::Result<u64> {
    if !root.exists() {
        return Ok(0);
    }
    let metadata = std::fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() {
        bail!(
            "preview cache must not contain symbolic links: {}",
            root.display()
        );
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(root)? {
        bytes = bytes
            .checked_add(tree_bytes(&entry?.path())?)
            .context("preview cache size overflow")?;
    }
    Ok(bytes)
}

/// Reserve a previously unused directory. Never overwrite a snapshot that a game worker may
/// still be reading, even after an editor restart or a rejected reload.
pub fn reserve(root: &Path, generation: u64) -> anyhow::Result<PathBuf> {
    if generation == 0 {
        bail!("preview generation must be nonzero");
    }
    if tree_bytes(root)? > CACHE_LIMIT - MAX_SNAPSHOT {
        bail!(
            "preview cache is full; stop the game before removing old folders from {}",
            root.display()
        );
    }
    std::fs::create_dir_all(root)?;
    let path = root.join(generation.to_string());
    std::fs::create_dir(&path).context("could not reserve a new preview snapshot")?;
    Ok(path)
}

/// Delete only numeric snapshot directories older than an acknowledged retirement watermark.
/// The caller must have confirmation that no callback or rollback snapshot retains those files.
pub fn retire_before(root: &Path, generation: u64) -> anyhow::Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        bail!("preview cache must not be a symbolic link");
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(old) = name.parse::<u64>() else {
            continue;
        };
        if old == 0 || old >= generation || old.to_string() != name {
            continue;
        }
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

/// Remove a reserved generation that was never sent to the plugin.
pub fn discard_unpublished(root: &Path, generation: u64) -> anyhow::Result<()> {
    if generation == 0 || std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        bail!("invalid unpublished preview snapshot");
    }
    let path = root.join(generation.to_string());
    if std::fs::symlink_metadata(&path)?.file_type().is_dir() {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_reservations_and_precise_retirement() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("files");
        reserve(&root, 7).unwrap();
        reserve(&root, 8).unwrap();
        reserve(&root, 9).unwrap();
        assert!(reserve(&root, 8).is_err());
        assert!(reserve(&root, 0).is_err());
        reserve(&root, 10).unwrap();
        discard_unpublished(&root, 10).unwrap();
        assert!(!root.join("10").exists());
        std::fs::create_dir(root.join("07")).unwrap();
        std::fs::create_dir(root.join("notes")).unwrap();
        retire_before(&root, 8).unwrap();
        assert!(!root.join("7").exists());
        for kept in ["8", "9", "07", "notes"] {
            assert!(root.join(kept).exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn cache_links_are_not_followed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("files");
        std::fs::create_dir(&root).unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("1")).unwrap();
        assert!(reserve(&root, 2).is_err());
        retire_before(&root, 2).unwrap();
        assert!(outside.exists());
        assert!(root.join("1").symlink_metadata().is_ok());
    }
}
