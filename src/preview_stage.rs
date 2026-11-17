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

/// Where on the stage a move is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    /// On the ground at the stage's centre.
    Ground,
    /// In the air above it, at about the height of a short hop.
    Air,
    /// Hanging from the stage's left ledge, facing the stage.
    Ledge,
}

/// How high above the ground the preview does an aerial: about a short hop.
pub const AIR_HEIGHT: f32 = 15.0;

/// Where a move (by motion name) is done.
pub fn spot(motion: &str) -> Spot {
    if motion.starts_with("cliff") {
        Spot::Ledge
    } else if crate::preview_floor::move_is_grounded(motion) {
        Spot::Ground
    } else {
        Spot::Air
    }
}

/// The stage's ledges, as game-space (x, y), from its level data (`param/<stage>_00.lvd`).
/// Each ledge is a `CLIFF` entry of a collision: the entry, then its position.
pub fn ledges(data_root: &Path, stage: &str) -> Vec<[f32; 2]> {
    let param = data_root.join("stage").join(stage).join("normal/param");
    let lvd = std::fs::read_dir(&param)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "lvd"))
        .min();
    lvd.and_then(|path| std::fs::read(path).ok())
        .map(|bytes| cliff_positions(&bytes))
        .unwrap_or_default()
}

/// The positions of the `CLIFF` entries in an LVD file. A cliff starts with its tag (type 3,
/// version 4), then the common entry (223 bytes), then its x and y, big endian.
fn cliff_positions(lvd: &[u8]) -> Vec<[f32; 2]> {
    const TAG: &[u8] = b"\x03\x04\x01\x01\x77\x35\xBB\x75\x00\x00\x00\x02";
    const ENTRY: usize = 223;
    let float = |at: usize| -> Option<f32> {
        Some(f32::from_be_bytes(lvd.get(at..at + 4)?.try_into().ok()?))
    };
    lvd.windows(TAG.len())
        .enumerate()
        .filter(|(_, window)| *window == TAG)
        .filter_map(|(at, _)| {
            let pos = at + TAG.len() + ENTRY + 1;
            Some([float(pos)?, float(pos + 4)?])
        })
        .filter(|[x, y]| x.is_finite() && y.is_finite())
        .collect()
}

/// How far the stage moves, in game space, so the fighter (always at the origin) is where the
/// move is done: on the ground, in the air above it, or on the left ledge.
pub fn stage_shift(spot: Spot, ledges: &[[f32; 2]]) -> glam::Vec3 {
    let left = ledges.iter().copied().min_by(|a, b| a[0].total_cmp(&b[0]));
    let ground = left.map_or(0.0, |[_, y]| y);
    match (spot, left) {
        (Spot::Ledge, Some([x, y])) => glam::Vec3::new(-x, -y, 0.0),
        (Spot::Air, _) => glam::Vec3::new(0.0, -ground - AIR_HEIGHT, 0.0),
        _ => glam::Vec3::new(0.0, -ground, 0.0),
    }
}

/// The preview-space transform of a stage's root for a shift in game space.
pub fn root_transform(shift: glam::Vec3) -> glam::Mat4 {
    glam::Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2) * glam::Mat4::from_translation(shift)
}

/// The stage's root bone, which carries the turn into preview space and the shift for a move.
pub const ROOT_BONE: &str = "VisionaryStageTurn";

/// An animation that holds the stage's root at `transform`.
pub fn root_anim(transform: glam::Mat4) -> ssbh_data::anim_data::AnimData {
    use ssbh_data::anim_data::*;
    let (scale, rotation, translation) = transform.to_scale_rotation_translation();
    AnimData {
        major_version: 2,
        minor_version: 0,
        final_frame_index: 0.0,
        groups: vec![GroupData {
            group_type: GroupType::Transform,
            nodes: vec![NodeData {
                name: ROOT_BONE.to_owned(),
                tracks: vec![TrackData {
                    name: "Transform".to_owned(),
                    compensate_scale: false,
                    transform_flags: TransformFlags::default(),
                    values: TrackValues::Transform(vec![Transform {
                        scale,
                        rotation,
                        translation,
                    }]),
                }],
            }],
        }],
    }
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
/// and the preview has the fighter facing +Z (model = Ry(-90) * game). The skeleton gets a new
/// root bone carrying the turn (and later the stage's shift for a move, by [`root_anim`]), and
/// every mesh hangs from it: meshes with no bone are bound to it, and skinned vertices are
/// turned to stay in step with the turned bind pose. A stage with no skeleton gets one.
pub fn turn_to_preview(folder: &mut ssbh_wgpu::ModelFolder) {
    let turn = root_transform(glam::Vec3::ZERO);
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
    if folder.find_skel().is_none() {
        folder.skels.retain(|(name, _)| name != "model.nusktb");
        folder.skels.push((
            "model.nusktb".to_owned(),
            Some(ssbh_data::skel_data::SkelData {
                major_version: 1,
                minor_version: 0,
                bones: Vec::new(),
            }),
        ));
    }
    for (_, skel) in folder.skels.iter_mut() {
        let Some(skel) = skel else { continue };
        for bone in skel.bones.iter_mut() {
            bone.parent_index = Some(bone.parent_index.map_or(0, |parent| parent + 1));
        }
        skel.bones.insert(
            0,
            ssbh_data::skel_data::BoneData {
                name: ROOT_BONE.to_owned(),
                transform: turn,
                parent_index: None,
                billboard_type: ssbh_data::skel_data::BillboardType::Disabled,
            },
        );
    }
    for (_, mesh) in folder.meshes.iter_mut() {
        let Some(mesh) = mesh else { continue };
        for object in mesh.objects.iter_mut() {
            if !object.parent_bone_name.is_empty() {
                continue;
            }
            if object.bone_influences.is_empty() {
                object.parent_bone_name = ROOT_BONE.to_owned();
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
    fn moves_are_done_where_the_game_does_them() {
        let ledges = [[-80.0, 0.0], [80.0, 0.0]];
        assert_eq!(spot("cliff_wait"), Spot::Ledge);
        assert_eq!(spot("attack_air_f"), Spot::Air);
        assert_eq!(spot("attack_s4_s"), Spot::Ground);
        assert_eq!(
            stage_shift(Spot::Ledge, &ledges),
            glam::Vec3::new(80.0, 0.0, 0.0)
        );
        assert_eq!(
            stage_shift(Spot::Air, &ledges),
            glam::Vec3::new(0.0, -AIR_HEIGHT, 0.0)
        );
        assert_eq!(stage_shift(Spot::Ground, &[]), glam::Vec3::ZERO);
        // The stage's ledge, turned into the preview, lands under the fighter.
        let ledge = root_transform(stage_shift(Spot::Ledge, &ledges))
            .transform_point3(glam::Vec3::new(-80.0, 0.0, 0.0));
        assert!(ledge.length() < 1e-4, "{ledge}");
    }

    /// Needs VISIONARY_DATA_ROOT with Battlefield extracted; skipped otherwise.
    #[test]
    fn battlefield_ledges_come_from_its_level_data() {
        let Some(root) = std::env::var_os("VISIONARY_DATA_ROOT") else {
            return;
        };
        let found = ledges(Path::new(&root), "battlefield");
        if found.is_empty() {
            return;
        }
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found.iter().any(|[x, _]| (x + 80.0).abs() < 0.1),
            "{found:?}"
        );
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
