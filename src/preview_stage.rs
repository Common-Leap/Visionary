//! The stage the 3D preview stands the fighter on.
//!
//! A stage is `stage/<name>/normal` in the data folder: its models are the `model/*_set`
//! folders (some placed by a one-frame animation in `motion/<set>/`), its lighting is a light
//! animation in `render/light`, and its colour grading a LUT in `lut`. With no stage picked the
//! preview keeps its plain backdrop and floor, lit like the training stage.

use std::path::{Path, PathBuf};

/// A stage's files, as the preview loads them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StageFiles {
    /// Each model folder, with the animation that places it if it has one.
    pub models: Vec<(PathBuf, Option<PathBuf>)>,
    pub light: Option<PathBuf>,
    pub lut: Option<PathBuf>,
}

/// The stages in the data folder that have models, by folder name, sorted by their plain name.
pub fn stages(data_root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(data_root.join("stage"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("normal/model").is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    names.sort_by_key(|name| label(name));
    names
}

/// A stage's name as players know it.
pub fn label(name: &str) -> String {
    let known = match name {
        "training" => "Training",
        "battlefield" => "Battlefield",
        "battlefield_l" => "Big Battlefield",
        "battlefields" => "Small Battlefield",
        "end" => "Final Destination",
        "village2" => "Smashville",
        "pokemon_stadium2" => "Pokémon Stadium 2",
        "fox_lylatcruise" => "Lylat Cruise",
        "dk_waterfall" => "Kongo Falls",
        "zelda_skyward" => "Skyloft",
        "kirby_greens" => "Green Greens",
        "mario_castle64" => "Peach's Castle (64)",
        "yoshi_story" => "Yoshi's Story",
        "poke_kalos" => "Kalos Pokémon League",
        "wufu_island" => "Wuhu Island",
        "dracula_castle" => "Dracula's Castle",
        "fe_shrine" => "Coliseum",
        "mario_paper" => "Paper Mario",
        "jack_mementoes" => "Mementos",
        "brave_altar" => "Yggdrasil's Altar",
        "dolly_stadium" => "King of Fighters Stadium",
        "tantan_spring" => "Spring Stadium",
        "pickel_world" => "Minecraft World",
        "ff_cave" => "Northern Cave",
        "trail_castle" => "Hollow Bastion",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_owned();
    }
    let mut words: Vec<String> = name
        .split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        })
        .collect();
    if words.is_empty() {
        words.push(name.to_owned());
    }
    words.join(" ")
}

/// The files of `stage` in the data folder.
pub fn files(data_root: &Path, stage: &str) -> StageFiles {
    let root = data_root.join("stage").join(stage).join("normal");
    let mut models: Vec<(PathBuf, Option<PathBuf>)> = std::fs::read_dir(root.join("model"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|dir| dir.join("model.numdlb").exists())
        .filter(|dir| {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            shown_in_normal_play(stage, name)
        })
        .map(|dir| {
            let set = dir.file_name().unwrap_or_default().to_owned();
            let anim = std::fs::read_dir(root.join("motion").join(&set))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .find(|path| path.extension().is_some_and(|e| e == "nuanmb"));
            (dir, anim)
        })
        .collect();
    models.sort();
    // A stage whose background goes through phases (Final Destination: light, dark, space)
    // carries a set for each; the first phase is the one a match starts in.
    let phase = |dir: &PathBuf| {
        let name = dir.file_name()?.to_str()?;
        let rest = name.strip_prefix("landscape_loop_")?;
        rest.split('_').next().map(str::to_owned)
    };
    let phases: Vec<String> = models.iter().filter_map(|(dir, _)| phase(dir)).collect();
    if let Some(first) = ["light"]
        .iter()
        .map(|p| p.to_string())
        .find(|p| phases.contains(p))
        .or_else(|| phases.first().cloned())
    {
        models.retain(|(dir, _)| phase(dir).is_none_or(|p| p == first));
    }
    StageFiles {
        models,
        light: light_file(&root.join("render/light")),
        lut: Some(root.join("lut/color_grading_lut.nutexb")).filter(|p| p.exists()),
    }
}

/// Whether a model set is part of the stage as it looks in a normal match. Sets for the
/// change between background phases (`landscape_change_*`), for stage transitions, and Final
/// Destination's platform sets (the stage has no platforms in its normal form) are not.
fn shown_in_normal_play(stage: &str, set: &str) -> bool {
    if set.starts_with("transition") || set.contains("_change_") {
        return false;
    }
    !(stage == "end" && set.starts_with("floating"))
}

/// The stage's normal light set: `light00_set`, `light00` or `light_light` when there is one
/// (Final Destination has several for its changing backgrounds), else the first.
fn light_file(dir: &Path) -> Option<PathBuf> {
    let mut lights: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "nuanmb"))
        .collect();
    lights.sort();
    for preferred in ["light00_set.nuanmb", "light00.nuanmb", "light_light.nuanmb"] {
        if let Some(path) = lights.iter().find(|p| p.ends_with(preferred)) {
            return Some(path.clone());
        }
    }
    lights.into_iter().next()
}

/// The stage's space turned to the preview's: a stage runs left to right along the game's X,
/// and the preview has the fighter facing +Z (model = Ry(-90) * game). Meshes with no parent
/// bone have their vertices turned; a skeleton gets a new root carrying the turn, so bones,
/// their animations and the meshes hung on them follow. Skinned vertices are turned too, which
/// keeps them in step with the turned bind pose.
pub fn turn_to_preview(folder: &mut ssbh_wgpu::ModelFolder) {
    let turn = glam::Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    let turn_vector = |data: &mut ssbh_data::mesh_data::VectorData| {
        use ssbh_data::mesh_data::VectorData;
        let rotate = |x: f32, z: f32| (-z, x);
        match data {
            VectorData::Vector3(values) => {
                for v in values.iter_mut() {
                    (v[0], v[2]) = rotate(v[0], v[2]);
                }
            }
            VectorData::Vector4(values) => {
                for v in values.iter_mut() {
                    (v[0], v[2]) = rotate(v[0], v[2]);
                }
            }
            VectorData::Vector2(_) => {}
        }
    };
    for (_, mesh) in folder.meshes.iter_mut() {
        let Some(mesh) = mesh else { continue };
        for object in mesh.objects.iter_mut() {
            if !object.parent_bone_name.is_empty() {
                continue;
            }
            for attribute in object
                .positions
                .iter_mut()
                .chain(object.normals.iter_mut())
                .chain(object.binormals.iter_mut())
                .chain(object.tangents.iter_mut())
            {
                turn_vector(&mut attribute.data);
            }
        }
    }
    for (_, skel) in folder.skels.iter_mut() {
        let Some(skel) = skel else { continue };
        if skel.bones.is_empty() {
            continue;
        }
        for bone in skel.bones.iter_mut() {
            bone.parent_index = Some(bone.parent_index.map_or(0, |parent| parent + 1));
        }
        skel.bones.insert(
            0,
            ssbh_data::skel_data::BoneData {
                name: "VisionaryStageTurn".to_owned(),
                transform: turn.to_cols_array_2d(),
                parent_index: None,
                billboard_type: ssbh_data::skel_data::BillboardType::Disabled,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_normal_form_of_a_stage_is_shown() {
        assert!(shown_in_normal_play("end", "ring_set"));
        assert!(!shown_in_normal_play("end", "floating_plate_set"));
        assert!(shown_in_normal_play("battlefield", "floating_plate_set"));
        assert!(!shown_in_normal_play(
            "end",
            "landscape_change_darkspace_main_set"
        ));
        assert!(!shown_in_normal_play("battlefield", "transition_temporary"));
    }

    #[test]
    fn stages_have_the_names_players_know() {
        assert_eq!(label("end"), "Final Destination");
        assert_eq!(label("battlefield"), "Battlefield");
        assert_eq!(label("some_new_stage"), "Some New Stage");
    }

    /// Needs VISIONARY_DATA_ROOT with the training stage extracted; skipped otherwise.
    #[test]
    fn the_training_stage_has_its_models_light_and_grading() {
        let Some(root) = std::env::var_os("VISIONARY_DATA_ROOT") else {
            return;
        };
        let root = Path::new(&root);
        if !root.join("stage/training/normal/model").is_dir() {
            return;
        }
        let stage = files(root, "training");
        assert!(stage
            .models
            .iter()
            .any(|(dir, _)| dir.ends_with("training_base_set")));
        assert!(stage
            .light
            .as_ref()
            .is_some_and(|p| p.ends_with("light00_set.nuanmb")));
        assert!(stage.lut.is_some());
        assert!(stages(root).contains(&"training".to_owned()));
    }
}
