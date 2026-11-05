//! Bounded controller asset files and atomic, no-clobber writes.

use std::io::{Read, Write};
use std::path::Path;

use super::controller_asset::{ControllerAsset, MAX_ASSET_BYTES};

/// Read one regular, non-link asset after checking its size before allocation.
pub fn load(path: &Path) -> Result<ControllerAsset, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect controller asset: {error}"))?;
    if !metadata.is_file() {
        return Err("Choose a regular controller asset file, not a link or special file.".into());
    }
    if metadata.len() > MAX_ASSET_BYTES as u64 {
        return Err(format!(
            "Controller asset exceeds {} MiB.",
            MAX_ASSET_BYTES / 1024 / 1024
        ));
    }
    let file = std::fs::File::open(path)
        .map_err(|error| format!("Could not open controller asset: {error}"))?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Choose a regular controller asset file.".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_ASSET_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read controller asset: {error}"))?;
    if bytes.len() > MAX_ASSET_BYTES {
        return Err(format!(
            "Controller asset exceeds {} MiB.",
            MAX_ASSET_BYTES / 1024 / 1024
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "Controller asset is not UTF-8 JSON.".to_owned())?;
    ControllerAsset::decode(text)
}

/// Write beside the destination and atomically claim the selected path.
/// Existing files are never silently replaced; the caller can ask for a new path.
pub fn save_new(path: &Path, asset: &ControllerAsset) -> Result<(), String> {
    let text = asset.encode()?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("Could not create controller asset file: {error}"))?;
    temporary
        .write_all(text.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("Could not write controller asset file: {error}"))?;
    temporary.persist_noclobber(path).map_err(|error| {
        format!(
            "Could not create controller asset without replacing an existing file: {}",
            error.error
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::{field_canvas::FieldLayout, templates::Preset};
    use crate::mod_project::ControllerChart;

    fn asset() -> ControllerAsset {
        ControllerAsset::capture(
            "cloud",
            "timer",
            "Timer",
            &ControllerChart {
                graph: Preset::Timer.graph(),
                layout: FieldLayout::default(),
            },
        )
        .unwrap()
    }

    #[test]
    fn asset_file_round_trip_is_atomic_and_preserves_neighbors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("timer.controller.json");
        let neighbor = directory.path().join("keep.txt");
        std::fs::write(&neighbor, "untouched").unwrap();
        save_new(&path, &asset()).unwrap();
        assert_eq!(load(&path).unwrap(), asset());
        assert_eq!(std::fs::read_to_string(neighbor).unwrap(), "untouched");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn existing_destination_is_never_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("existing.json");
        std::fs::write(&path, "keep me").unwrap();
        assert!(save_new(&path, &asset()).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "keep me");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn load_bounds_size_and_rejects_nonregular_or_non_utf8_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("asset.json");
        std::fs::write(&path, vec![b' '; MAX_ASSET_BYTES + 1]).unwrap();
        assert!(load(&path).unwrap_err().contains("MiB"));
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(load(&path).unwrap_err().contains("UTF-8"));
        assert!(load(directory.path()).is_err());
        assert!(load(&directory.path().join("missing.json")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn load_refuses_symbolic_links_without_following_them() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("asset.json");
        save_new(&path, &asset()).unwrap();
        let link = directory.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(load(&link).unwrap_err().contains("link or special file"));
        assert_eq!(load(&path).unwrap(), asset());
    }
}
