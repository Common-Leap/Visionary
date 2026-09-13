//! Install the reusable native graph before ARCropolis starts loading game resources.
use anyhow::Context;
use ssbh_data::prelude::AnimData;
use std::path::{Path, PathBuf};

#[path = "../plugins/slight_replica/src/slight/effect_viewer/asset_pool.rs"]
pub mod pool;

/// Only writes this application's support package. The marker is committed last, so an
/// interrupted installation is repaired on the next launch without rewriting a complete pool.
pub fn install(sd: &Path) -> anyhow::Result<PathBuf> {
    let root = sd.join("ultimate/mods/visionary_live_assets");
    let marker = sd.join("effect_viewer/live_assets/pool.version");
    if root.join("config.json").is_file()
        && std::fs::read_to_string(&marker).ok().as_deref() == Some(pool::VERSION)
    {
        return Ok(root);
    }
    let model = root.join(pool::MODEL_ROOT);
    let motion = root.join(pool::MOTION_ROOT);
    std::fs::create_dir_all(&model)?;
    std::fs::create_dir_all(&motion)?;
    let animation = AnimData {
        major_version: 2,
        minor_version: 1,
        final_frame_index: 1.0,
        groups: Vec::new(),
    };
    let mut model_paths = Vec::new();
    let mut motion_paths = Vec::new();
    for index in 0..pool::TEXTURE_CAPACITY {
        let name = pool::texture_name(index);
        let texture = nutexb::NutexbFile::from_surface(
            nutexb::Surface {
                width: 1,
                height: 1,
                depth: 1,
                image_data: [255u8; 4],
                mipmap_count: 1,
                layer_count: 1,
                image_format: nutexb::NutexbFormat::R8G8B8A8Unorm,
            },
            name.clone(),
        )?;
        texture.write_to_file(model.join(format!("{name}.nutexb")))?;
        model_paths.push(format!("{}/{name}.nutexb", pool::MODEL_ROOT));
    }
    for index in 0..pool::MOTION_CAPACITY {
        let name = pool::motion_name(index);
        debug_assert!(pool::is_pool_file(&name, true));
        animation.write_to_file(motion.join(&name))?;
        motion_paths.push(format!("{}/{name}", pool::MOTION_ROOT));
    }
    for name in ["swingblend.prc", "ik.prc"] {
        prc::save(motion.join(name), &prc::ParamStruct::default())?;
        motion_paths.push(format!("{}/{name}", pool::MOTION_ROOT));
    }
    // ARCropolis discovers the files, but new-dir-files also puts them in the constructor's
    // recursive load/release graph. Callback registration alone does not do this.
    let config = serde_json::json!({"new-dir-files": {
        pool::MODEL_ROOT: model_paths,
        pool::MOTION_ROOT: motion_paths,
    }});
    std::fs::write(
        root.join("config.json"),
        serde_json::to_vec_pretty(&config)?,
    )?;
    std::fs::write(root.join("info.toml"), "display_name = \"Visionary Live Assets\"\nversion = \"1\"\ndescription = \"Reusable resources for live character previews.\"\n")?;
    std::fs::create_dir_all(marker.parent().unwrap())?;
    std::fs::write(&marker, pool::VERSION).context("finishing carrier support installation")?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn support_package_has_parseable_independent_resources_and_directory_membership() {
        let dir = tempfile::tempdir().unwrap();
        let root = install(dir.path()).unwrap();
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("config.json")).unwrap()).unwrap();
        let entries = &config["new-dir-files"];
        assert_eq!(
            entries[pool::MODEL_ROOT].as_array().unwrap().len(),
            pool::TEXTURE_CAPACITY
        );
        assert_eq!(
            entries[pool::MOTION_ROOT].as_array().unwrap().len(),
            pool::MOTION_CAPACITY + 2
        );
        for (directory, last) in [
            (pool::MODEL_ROOT, pool::TEXTURE_CAPACITY - 1),
            (pool::MOTION_ROOT, pool::MOTION_CAPACITY - 1),
        ] {
            for index in [0, last] {
                let path = root.join(entries[directory][index].as_str().unwrap());
                if directory == pool::MODEL_ROOT {
                    let texture = nutexb::NutexbFile::read_from_file(path).unwrap();
                    assert_eq!(texture.deswizzled_data().unwrap(), [255u8; 4]);
                } else {
                    assert_eq!(AnimData::from_file(path).unwrap().final_frame_index, 1.0);
                }
            }
        }
        let before = std::fs::metadata(marker_path(dir.path()))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(install(dir.path()).unwrap(), root);
        assert_eq!(
            std::fs::metadata(marker_path(dir.path()))
                .unwrap()
                .modified()
                .unwrap(),
            before
        );
    }
    fn marker_path(root: &Path) -> PathBuf {
        root.join("effect_viewer/live_assets/pool.version")
    }
}
