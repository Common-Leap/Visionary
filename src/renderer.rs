use glam::{Mat4, Vec3, Vec4};
use ssbh_wgpu::{
    CameraTransforms, ModelFolder, ModelRenderOptions, RenderModel, RenderSettings,
    SharedRenderData, SsbhRenderer,
};
/// ssbh_wgpu rendering integration for Visionary's character viewport.
/// Uses egui-wgpu paint callbacks to render directly into the egui surface.
use std::path::Path;

fn synthetic_top_matrix(bones: &std::collections::HashMap<String, glam::Mat4>) -> glam::Mat4 {
    bones
        .get("Trans")
        .or_else(|| bones.get("trans"))
        .map(|matrix| glam::Mat4::from_translation(matrix.col(3).truncate()))
        .unwrap_or(glam::Mat4::IDENTITY)
}

#[allow(dead_code)]
pub struct Camera {
    pub translation: Vec3,
    pub rotation: Vec3,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            translation: Vec3::new(0.0, -8.0, -60.0),
            rotation: Vec3::new(0.0, std::f32::consts::FRAC_PI_2, 0.0),
            fov_y: 30f32.to_radians(),
            near: 1.0,
            far: 400_000.0,
        }
    }
}

impl Camera {
    /// Pan in the camera plane (left/right and up/down) while keeping rotation fixed.
    /// Moves in world X (left/right) and world Y (up/down) regardless of camera rotation.
    pub fn pan(&mut self, delta_x: f32, delta_y: f32) {
        let speed = self.translation.z.abs() * 0.001;
        self.translation.x -= delta_x * speed;
        self.translation.y += delta_y * speed;
    }

    /// Zoom: move along Z axis.
    pub fn zoom(&mut self, delta: f32) {
        let speed = self.translation.z.abs() * 0.1;
        self.translation.z += delta * speed;
        self.translation.z = self.translation.z.min(-1.0);
    }

    pub fn transforms(&self, width: f32, height: f32) -> CameraTransforms {
        let aspect = if height > 0.0 { width / height } else { 1.0 };
        let rotation = Mat4::from_euler(
            glam::EulerRot::XYZ,
            self.rotation.x,
            self.rotation.y,
            self.rotation.z,
        );
        let model_view = Mat4::from_translation(self.translation) * rotation;
        let projection = Mat4::perspective_rh(self.fov_y, aspect, self.near, self.far);
        let mvp = projection * model_view;
        CameraTransforms {
            model_view_matrix: model_view,
            mvp_matrix: mvp,
            projection_matrix: projection,
            mvp_inv_matrix: mvp.inverse(),
            camera_pos: model_view.inverse().col(3),
            screen_dimensions: Vec4::new(width, height, 1.0, 0.0),
        }
    }
}

/// All wgpu rendering resources stored in egui's callback_resources.
pub struct HitboxRenderState {
    pub renderer: SsbhRenderer,
    pub shared_data: SharedRenderData,
    pub render_models: Vec<RenderModel>,
    pub render_settings: RenderSettings,
    pub model_render_options: ModelRenderOptions,
    pub camera: Camera,
    pub current_width: u32,
    pub current_height: u32,
    /// Cached selected-move animation data — reloaded only when the path changes.
    cached_anim: Option<(std::path::PathBuf, ssbh_data::anim_data::AnimData)>,
    /// The mesh names both cached animations' visibility tracks were matched against (see
    /// [`exact_visibility_tracks`]); a different set of loaded meshes re-reads them.
    cached_vis_meshes: u64,
    /// Cached default eyelid animation data. This is kept separately from the selected move
    /// animation because the eyelid asset is a visibility baseline, not a second source of bone
    /// or material animation.
    cached_default_anim: Option<(std::path::PathBuf, Option<ssbh_data::anim_data::AnimData>)>,
    /// Cached skeleton data — reloaded only when the path changes
    cached_skel: Option<(std::path::PathBuf, ssbh_data::skel_data::SkelData)>,
    /// The loaded model's `model.nuhlpb`. Applied to the mesh and to the overlay's bone
    /// matrices while [`Self::helper_bones`] is on, as the game applies it every frame.
    model_hlpb: Option<ssbh_data::hlpb_data::HlpbData>,
    /// The loaded model's `model.numatb`, which material tracks in a move's `.nuanmb` animate.
    model_matl: Option<ssbh_data::matl_data::MatlData>,
    /// Viewport preference: evaluate helper bone constraints. Set through
    /// [`Self::set_helper_bones`] so a change re-skins the model.
    helper_bones: bool,
    /// The fighter's model scale (`ModelModule::scale`): its `fighter_param` `scale`, or what
    /// the moveset's frame callback sets. Applied at the skeleton's root, so the mesh and every
    /// bone the overlay reads grow together. Set through [`Self::set_model_scale`].
    model_scale: f32,
    /// A second fighter held by this one's `throw` bone, drawn as `render_models[1]`.
    victim: Option<ThrownVictim>,
    /// Weapon skeletons: (weapon_name, skel, attach_bone_name)
    /// attach_bone_name is the character bone the weapon root attaches to (e.g. "haver")
    weapon_skels: Vec<(String, ssbh_data::skel_data::SkelData, String)>,
    /// Track last rendered state to skip redundant GPU work
    last_frame: f32,
    last_anim_path: Option<std::path::PathBuf>,
    last_default_anim_path: Option<std::path::PathBuf>,
    last_skel_path: Option<std::path::PathBuf>,
    /// Cached GPU handles for use in paint()
    pub wgpu_device: Option<wgpu::Device>,
    pub wgpu_queue: Option<wgpu::Queue>,
}

impl HitboxRenderState {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let mut renderer = SsbhRenderer::new(
            device,
            queue,
            1,
            1,
            1.0,
            [0.05, 0.05, 0.1, 1.0],
            surface_format,
        );
        let shared_data = SharedRenderData::new(device, queue);
        // Use Shaded mode for full PBR rendering with textures
        let render_settings = RenderSettings {
            render_bloom: false,
            render_shadows: false,
            ..RenderSettings::default()
        };
        renderer.update_render_settings(queue, &render_settings);
        Self {
            renderer,
            shared_data,
            render_models: Vec::new(),
            render_settings,
            model_render_options: ModelRenderOptions::default(),
            camera: Camera::default(),
            current_width: 0,
            current_height: 0,
            cached_anim: None,
            cached_vis_meshes: 0,
            cached_default_anim: None,
            cached_skel: None,
            model_hlpb: None,
            model_matl: None,
            helper_bones: true,
            model_scale: 1.0,
            victim: None,
            weapon_skels: Vec::new(),
            last_frame: -1.0,
            last_anim_path: None,
            last_default_anim_path: None,
            last_skel_path: None,
            wgpu_device: Some(device.clone()),
            wgpu_queue: Some(queue.clone()),
        }
    }

    /// Viewport preferences from the Settings window: background, floor grid, and bloom.
    pub fn apply_preferences(
        &mut self,
        queue: &wgpu::Queue,
        preferences: &crate::preferences::Preferences,
    ) {
        self.renderer
            .set_clear_color(preferences.viewport_clear_color());
        self.model_render_options.draw_floor_grid = preferences.floor_grid;
        if self.render_settings.render_bloom != preferences.bloom {
            self.render_settings.render_bloom = preferences.bloom;
            self.renderer
                .update_render_settings(queue, &self.render_settings);
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if width == self.current_width && height == self.current_height {
            return;
        }
        self.renderer.resize(device, width, height, 1.0);
        self.current_width = width;
        self.current_height = height;
    }

    pub fn load_model(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, model_dir: &Path) {
        self.render_models.clear();
        self.weapon_skels.clear();
        // The files ARCropolis would load here: the folder's own, its config.json shares, and
        // the game's files for a vanilla slot.
        let costume = crate::costume_files::CostumeFolder::resolve(model_dir);
        if costume.is_empty() {
            eprintln!("[MODEL] no model files for {:?}", model_dir);
            return;
        }
        let folder = ModelFolder::load_files(costume.model_files());
        let render_model = RenderModel::from_folder(device, queue, &folder, &self.shared_data);
        eprintln!("[MODEL] loaded from {:?}", model_dir);
        self.model_hlpb = folder.find_hlpb().cloned();
        self.model_matl = folder.find_matl().cloned();
        self.render_models.push(render_model);
        // `clear()` above dropped the victim's render model with the body's.
        if let Some(victim) = &self.victim {
            let folder = ModelFolder::load_files(victim.files.clone());
            self.render_models.push(RenderModel::from_folder(
                device,
                queue,
                &folder,
                &self.shared_data,
            ));
        }

        // Scan sibling directories for weapon skeletons.
        // model_dir is e.g. fighter/link/model/body/c00
        // Weapon dirs are e.g. fighter/link/model/sword/c00
        // Prefer the slot the body model was loaded from; a modded fighter may ship no c00,
        // and hardcoding it rendered those fighters with no weapons at all.
        let body_slot = model_dir
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(crate::data::parse_costume_dir)
            .unwrap_or(0);
        if let Some(model_root) = model_dir.parent().and_then(|p| p.parent()) {
            for part_dir in crate::costume_files::sibling_dirs(model_root) {
                let Some(dir_name) = part_dir.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if dir_name.eq_ignore_ascii_case("body") {
                    continue;
                }
                let Some(weapon_skel) = crate::data::find_part_skel(&part_dir, body_slot) else {
                    continue;
                };
                if let Ok(skel) = ssbh_data::skel_data::SkelData::from_file(&weapon_skel) {
                    // Determine the attach bone: prefer "haver" (right hand), then "havel" (left hand)
                    // This is the character body bone the weapon root is parented to at runtime.
                    let attach = weapon_attach_bone(dir_name);
                    self.weapon_skels.push((dir_name.to_string(), skel, attach));
                }
            }
        }

        // Force re-skin on next frame
        self.last_frame = -1.0;
        self.last_anim_path = None;
        self.last_default_anim_path = None;
        self.last_skel_path = None;
    }

    /// Apply a selected move animation over an optional default eyelid visibility animation.
    ///
    /// Default assets are parsed and cached independently from the selected move. Only
    /// [`ssbh_data::anim_data::GroupType::Visibility`] groups survive the default-asset filter;
    /// this prevents a malformed or customized `a00defaulteyelid.nuanmb` from changing bones or
    /// materials while still restoring the game's eyelid state.
    pub fn apply_animation_with_default(
        &mut self,
        queue: &wgpu::Queue,
        default_anim_path: Option<&Path>,
        anim_path: Option<&Path>,
        skel_path: Option<&Path>,
        frame: f32,
    ) {
        let meshes = self.mesh_names();
        let meshes_key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            meshes.hash(&mut hasher);
            hasher.finish()
        };
        if meshes_key != self.cached_vis_meshes {
            // Both animations' track names were fitted to the previous meshes.
            self.cached_anim = None;
            self.cached_default_anim = None;
            self.cached_vis_meshes = meshes_key;
        }
        self.update_default_animation(default_anim_path, &meshes);

        // Reload anim only when path changes
        if let Some(path) = anim_path {
            let needs_load = self
                .cached_anim
                .as_ref()
                .map(|(p, _)| p != path)
                .unwrap_or(true);
            if needs_load {
                self.cached_anim =
                    ssbh_data::anim_data::AnimData::from_file(path)
                        .ok()
                        .map(|mut a| {
                            exact_visibility_tracks(&mut a, &meshes);
                            (path.to_path_buf(), a)
                        });
            }
        } else {
            self.cached_anim = None;
        }

        // Reload skel only when path changes
        if let Some(path) = skel_path {
            let needs_load = self
                .cached_skel
                .as_ref()
                .map(|(p, _)| p != path)
                .unwrap_or(true);
            if needs_load {
                self.cached_skel = ssbh_data::skel_data::SkelData::from_file(path)
                    .ok()
                    .map(|s| (path.to_path_buf(), s));
            }
        } else {
            self.cached_skel = None;
        }

        let anim = self.cached_anim.as_ref().map(|(_, a)| a);
        let skel = self.cached_skel.as_ref().map(|(_, s)| s);
        // Field borrows rather than `active_hlpb()`: the loop below borrows `render_models`.
        let hlpb = self.model_hlpb.as_ref().filter(|_| self.helper_bones);
        let matl = self.model_matl.as_ref();

        if let Some(render_model) = self.render_models.first_mut() {
            // RenderModel applies animations in iterator order. Put the default eyelid layer
            // first so a selected move's visibility tracks take precedence over the baseline.
            let animations = animation_layers(
                self.cached_default_anim
                    .as_ref()
                    .and_then(|(_, a)| a.as_ref()),
                anim,
            );
            let model_scale = self.model_scale;
            render_model.apply_anims_with_skel_edit(
                queue,
                animations.iter().copied(),
                skel,
                matl,
                hlpb,
                &self.shared_data,
                frame,
                |bones| {
                    if let Some(skel) = skel {
                        scale_root_bones(bones, skel, model_scale);
                    }
                },
            );
        }
        self.pose_victim(queue, frame);
    }

    /// Show `victim` held by this fighter's `throw` bone, or stop showing one.
    pub fn set_victim(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        victim: Option<VictimSpec>,
    ) {
        let Some(spec) = victim else {
            if self.victim.take().is_some() {
                self.render_models.truncate(1);
                self.last_frame = -1.0;
            }
            return;
        };
        let same_model = self
            .victim
            .as_ref()
            .is_some_and(|current| current.files == spec.model_files);
        if !same_model {
            let folder = ModelFolder::load_files(spec.model_files.clone());
            let render_model =
                RenderModel::from_folder(device, queue, &folder, &self.shared_data);
            self.render_models.truncate(1);
            self.render_models.push(render_model);
            self.victim = Some(ThrownVictim {
                files: spec.model_files,
                scale: spec.scale,
                skel: folder.find_skel().cloned(),
                hlpb: folder.find_hlpb().cloned(),
                matl: folder.find_matl().cloned(),
                anim: None,
            });
            self.last_frame = -1.0;
        }
        let victim = self.victim.as_mut().expect("set above");
        if victim.scale != spec.scale {
            victim.scale = spec.scale;
            self.last_frame = -1.0;
        }
        if victim.anim.as_ref().map(|(path, _)| path) != spec.anim_path.as_ref() {
            victim.anim = spec.anim_path.and_then(|path| {
                let anim = ssbh_data::anim_data::AnimData::from_file(&path).ok()?;
                Some((path, anim))
            });
            self.last_frame = -1.0;
        }
    }

    /// Pose the victim for `frame`: its own thrown animation, with its `rot` bone placed on
    /// this fighter's `throw` bone, which is how the game carries a thrown fighter.
    fn pose_victim(&mut self, queue: &wgpu::Queue, frame: f32) {
        // The throw bone carries this fighter's model scale; the victim keeps its own.
        let target = self.victim.as_ref().and_then(|victim| {
            let throw = self.bone_world_matrices_at(frame).get("throw").copied()?;
            Some(throw * glam::Mat4::from_scale(glam::Vec3::splat(victim.scale / self.model_scale)))
        });
        let (Some(victim), Some(render_model)) = (&self.victim, self.render_models.get_mut(1))
        else {
            return;
        };
        let Some(skel) = victim.skel.as_ref() else {
            return;
        };
        let rot = skel
            .bones
            .iter()
            .position(|bone| bone.name.eq_ignore_ascii_case("rot"));
        let hlpb = victim.hlpb.as_ref().filter(|_| self.helper_bones);
        render_model.apply_anims_with_skel_edit(
            queue,
            victim.anim.iter().map(|(_, anim)| anim),
            Some(skel),
            victim.matl.as_ref(),
            hlpb,
            &self.shared_data,
            frame,
            |bones| {
                if let (Some(rot), Some(target)) = (rot, target) {
                    constrain_bone_to_world(bones, skel, rot, target);
                }
            },
        );
    }

    /// Every mesh name of the fighter's own model. The victim's visibility tracks are its own.
    fn mesh_names(&self) -> Vec<String> {
        self.render_models
            .iter()
            .take(1)
            .flat_map(|model| model.meshes.iter().map(|mesh| mesh.name.clone()))
            .collect()
    }

    fn update_default_animation(&mut self, path: Option<&Path>, meshes: &[String]) {
        if let Some(path) = path {
            let needs_load = self
                .cached_default_anim
                .as_ref()
                .map(|(cached_path, _)| cached_path != path)
                .unwrap_or(true);
            if needs_load {
                self.cached_default_anim = Some((
                    path.to_path_buf(),
                    ssbh_data::anim_data::AnimData::from_file(path)
                        .ok()
                        .map(|anim| {
                            let mut anim = visibility_only_animation(&anim);
                            exact_visibility_tracks(&mut anim, meshes);
                            anim
                        }),
                ));
            }
        } else {
            self.cached_default_anim = None;
        }
    }

    pub fn update_camera(&mut self, queue: &wgpu::Queue, width: f32, height: f32) {
        let transforms = self.camera.transforms(width, height);
        self.renderer.update_camera(queue, transforms);
    }

    /// Returns all bone names from the cached skeleton.
    #[allow(dead_code)]
    pub fn bone_names(&self) -> Vec<String> {
        self.cached_skel
            .as_ref()
            .map(|(_, s)| s.bones.iter().map(|b| b.name.clone()).collect())
            .unwrap_or_default()
    }

    pub fn weapon_skel_count(&self) -> usize {
        self.weapon_skels.len()
    }

    /// Whether the loaded model has a `model.nuhlpb` for the helper-bones toggle to act on.
    pub fn has_helper_bones(&self) -> bool {
        self.model_hlpb.is_some()
    }

    /// Turn helper bone constraints on or off for both the mesh and the overlay.
    pub fn set_helper_bones(&mut self, enabled: bool) {
        if self.helper_bones != enabled {
            self.helper_bones = enabled;
            // Force the next prepare() to re-skin with the new constraint set.
            self.last_frame = -1.0;
        }
    }

    /// Resize the fighter, mesh and overlay alike. See [`Self::model_scale`].
    pub fn set_model_scale(&mut self, scale: f32) {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        if self.model_scale != scale {
            self.model_scale = scale;
            self.last_frame = -1.0;
        }
    }

    fn active_hlpb(&self) -> Option<&ssbh_data::hlpb_data::HlpbData> {
        self.model_hlpb.as_ref().filter(|_| self.helper_bones)
    }

    /// Eagerly load skeleton data so bone_world_matrices() returns valid results
    /// immediately, without waiting for the first prepare() call.
    pub fn load_skeleton(&mut self, skel_path: &Path) {
        self.cached_skel = ssbh_data::skel_data::SkelData::from_file(skel_path)
            .ok()
            .map(|s| (skel_path.to_path_buf(), s));
    }

    /// Returns a map of bone name -> world matrix for the current frame.
    /// Includes both body skeleton bones and weapon skeleton bones.
    /// The offset from ACMD should be transformed by this matrix (not just added).
    pub fn bone_world_matrices(&self) -> std::collections::HashMap<String, glam::Mat4> {
        self.bone_world_matrices_at(self.last_frame.max(0.0))
    }

    /// Like `bone_world_matrices()` but evaluates animation at an explicit `frame`
    /// instead of `self.last_frame`. Use this from the simulation code so bone
    /// positions are correct on the very first frame (no 1-frame delay).
    pub fn bone_world_matrices_at(
        &self,
        frame: f32,
    ) -> std::collections::HashMap<String, glam::Mat4> {
        let skel = match self.cached_skel.as_ref() {
            Some((_, s)) => s,
            None => {
                if crate::debug_enabled() {
                    eprintln!("[BONE] cached_skel is NONE — returning empty map");
                }
                return std::collections::HashMap::new();
            }
        };
        let anim = self.cached_anim.as_ref().map(|(_, a)| a);
        let mut matrices =
            compute_bone_world_matrices(skel, anim, self.active_hlpb(), &self.weapon_skels, frame);
        if self.model_scale != 1.0 {
            // The same as scaling the root bone, which is what the mesh gets.
            let scale = glam::Mat4::from_scale(glam::Vec3::splat(self.model_scale));
            for matrix in matrices.values_mut() {
                *matrix = scale * *matrix;
            }
        }
        matrices
    }

    /// The model scale the mesh and [`Self::bone_world_matrices_at`] are drawn at.
    pub fn model_scale(&self) -> f32 {
        self.model_scale
    }
}

/// What [`HitboxRenderState::set_victim`] shows.
pub struct VictimSpec {
    /// The victim costume's model, as [`crate::costume_files::CostumeFolder::model_files`].
    pub model_files: Vec<(String, std::path::PathBuf)>,
    /// The victim's thrown animation; `None` holds it in its rest pose.
    pub anim_path: Option<std::path::PathBuf>,
    /// The victim's own model scale, its `fighter_param` `scale`.
    pub scale: f32,
}

struct ThrownVictim {
    files: Vec<(String, std::path::PathBuf)>,
    scale: f32,
    skel: Option<ssbh_data::skel_data::SkelData>,
    hlpb: Option<ssbh_data::hlpb_data::HlpbData>,
    matl: Option<ssbh_data::matl_data::MatlData>,
    anim: Option<(std::path::PathBuf, ssbh_data::anim_data::AnimData)>,
}

/// Set bone `index`'s local transform so its world transform is `target`, given the rest of
/// the sampled pose. Ported from SSBH Editor's thrown-character link.
fn constrain_bone_to_world(
    bones: &mut [(usize, ssbh_wgpu::animation::AnimatedBone)],
    skel: &ssbh_data::skel_data::SkelData,
    index: usize,
    target: glam::Mat4,
) {
    use ssbh_wgpu::animation::{animate_skel_inner, AnimationTransforms};
    let mut sampled = Box::new(AnimationTransforms::from_skel(skel));
    animate_skel_inner(&mut sampled, bones, &skel.bones, None);
    let parent = skel.bones[index]
        .parent_index
        .map(|parent| sampled.world_transforms[parent])
        .unwrap_or(glam::Mat4::IDENTITY);
    let (scale, rotation, translation) = (parent.inverse() * target).to_scale_rotation_translation();
    if let Some((_, bone)) = bones.iter_mut().find(|(i, _)| *i == index) {
        bone.set_current_trs(translation, rotation, scale);
    }
}

/// Scale the skeleton's root bones by `scale`, which scales the whole fighter about its origin:
/// what `ModelModule::set_scale` does to a fighter in game.
fn scale_root_bones(
    bones: &mut [(usize, ssbh_wgpu::animation::AnimatedBone)],
    skel: &ssbh_data::skel_data::SkelData,
    scale: f32,
) {
    use ssbh_wgpu::animation::{animate_skel_inner, AnimationTransforms};
    if scale == 1.0 {
        return;
    }
    // A root's world transform is its animated local one.
    let mut sampled = Box::new(AnimationTransforms::from_skel(skel));
    animate_skel_inner(&mut sampled, bones, &skel.bones, None);
    for (index, bone) in bones.iter_mut() {
        if bone.parent_index().is_none() {
            let (bone_scale, rotation, translation) =
                sampled.world_transforms[*index].to_scale_rotation_translation();
            bone.set_current_trs(translation * scale, rotation, bone_scale * scale);
        }
    }
}

/// Keep only visibility groups from a default eyelid animation.
///
/// Default eyelid files are intended to provide a baseline for mesh visibility. Filtering at
/// load time makes that contract explicit and protects the renderer from accidentally applying
/// transform or material tracks found in a customized asset.
fn visibility_only_animation(
    animation: &ssbh_data::anim_data::AnimData,
) -> ssbh_data::anim_data::AnimData {
    use ssbh_data::anim_data::GroupType;

    ssbh_data::anim_data::AnimData {
        major_version: animation.major_version,
        minor_version: animation.minor_version,
        final_frame_index: animation.final_frame_index,
        groups: animation
            .groups
            .iter()
            .filter(|group| group.group_type == GroupType::Visibility)
            .cloned()
            .collect(),
    }
}

/// Narrow each visibility track to the meshes the game gives it.
///
/// A visibility track names a mesh without its `_VIS_O_OBJShape` suffix, and ssbh_wgpu applies
/// it to every mesh whose name merely *starts with* the track's name. That reaches too far
/// whenever one mesh's name extends another's: Shy Guy's `mace_spike` track (on) also switched
/// on `mace_spikeFLIP_VIS_O_OBJShape`, the left-hand mace its `mace_spikeFLIP` track keeps off.
/// Which won depended on the order the tracks are stored in, so the forward smash showed one
/// mace and its reverse showed both.
///
/// A track that has an exact match — a mesh named `<track>_VIS…` — is renamed to
/// `<track>_VIS`, which the prefix rule can only match exactly. A track with no exact match is
/// left alone, so any model that does rely on prefix matching renders as before.
fn exact_visibility_tracks(anim: &mut ssbh_data::anim_data::AnimData, meshes: &[String]) {
    use ssbh_data::anim_data::GroupType;
    for group in &mut anim.groups {
        if group.group_type != GroupType::Visibility {
            continue;
        }
        for node in &mut group.nodes {
            let exact = format!("{}_VIS", node.name);
            let has_exact = meshes.iter().any(|mesh| mesh.starts_with(&exact));
            let overreaches = meshes
                .iter()
                .any(|mesh| mesh.starts_with(&node.name) && !mesh.starts_with(&exact) && *mesh != node.name);
            let named_bare = meshes.iter().any(|mesh| *mesh == node.name);
            if has_exact && overreaches && !named_bare {
                node.name = exact;
            }
        }
    }
}

/// Build the animation stack in the same order used by [`RenderModel::apply_anims`]. Keeping this
/// small piece pure makes the baseline-before-selected precedence easy to regression-test.
fn animation_layers<'a>(
    default_animation: Option<&'a ssbh_data::anim_data::AnimData>,
    selected_animation: Option<&'a ssbh_data::anim_data::AnimData>,
) -> Vec<&'a ssbh_data::anim_data::AnimData> {
    default_animation
        .into_iter()
        .chain(selected_animation)
        .collect()
}

/// Evaluates the bone hierarchy for `frame` and returns `bone name -> world matrix`.
///
/// Split out of [`HitboxRenderState`] so it can be benchmarked and tested without a GPU
/// device — this is the single hottest piece of per-frame CPU work in the editor, so it
/// needs to stay measurable (see `src/bin/bone_bench.rs`).
///
/// Every bone is keyed under both its original and its lowercased name because ACMD bone
/// names are inconsistently cased; callers look up either spelling.
///
/// The body skeleton is evaluated by ssbh_wgpu's own [`animate_skel`], the same evaluation
/// that skins the mesh, so every collision sits where the rendered model puts its bone. That
/// includes helper bones when `hlpb` is given, and bones a skeleton lists before their parent
/// (Mario's `Jaw` and lip bones), which an in-order walk left at the origin.
///
/// [`animate_skel`]: ssbh_wgpu::animation::animate_skel
pub fn compute_bone_world_matrices(
    skel: &ssbh_data::skel_data::SkelData,
    anim: Option<&ssbh_data::anim_data::AnimData>,
    hlpb: Option<&ssbh_data::hlpb_data::HlpbData>,
    weapon_skels: &[(String, ssbh_data::skel_data::SkelData, String)],
    frame: f32,
) -> std::collections::HashMap<String, glam::Mat4> {
    let world = body_world(skel, anim, hlpb, frame);
    bone_world_map(skel, &world, weapon_skels)
}

/// The body skeleton's world matrices through ssbh_wgpu, with any helper bone constraints.
fn body_world(
    skel: &ssbh_data::skel_data::SkelData,
    anim: Option<&ssbh_data::anim_data::AnimData>,
    hlpb: Option<&ssbh_data::hlpb_data::HlpbData>,
    frame: f32,
) -> Vec<glam::Mat4> {
    use ssbh_wgpu::animation::{animate_skel, AnimationTransforms, MAX_BONE_COUNT};
    // Three 512-matrix arrays; boxed to keep them off the UI thread's stack.
    let mut transforms = Box::new(AnimationTransforms::identity());
    animate_skel(&mut transforms, skel, anim.into_iter(), hlpb, frame);
    let evaluated = skel.bones.len().min(MAX_BONE_COUNT);
    let mut world = transforms.world_transforms[..evaluated].to_vec();
    // The renderer stops at its uniform-buffer limit too; keep any further bones at rest so
    // their names still resolve rather than vanishing from the map.
    for bone in &skel.bones[evaluated..] {
        world.push(
            skel.calculate_world_transform(bone)
                .map(|m| glam::Mat4::from_cols_array_2d(&m))
                .unwrap_or(glam::Mat4::IDENTITY),
        );
    }
    world
}

/// Key the body's world matrices by bone name, then add the synthetic `top` joint and the
/// weapon skeletons attached to their body bones.
fn bone_world_map(
    skel: &ssbh_data::skel_data::SkelData,
    world: &[glam::Mat4],
    weapon_skels: &[(String, ssbh_data::skel_data::SkelData, String)],
) -> std::collections::HashMap<String, glam::Mat4> {
    let mut result = std::collections::HashMap::new();
    for (bone, matrix) in skel.bones.iter().zip(world) {
        result.insert(bone.name.clone(), *matrix);
        result.insert(bone.name.to_lowercase(), *matrix);
    }

    if crate::debug_enabled() {
        for (name, mat) in &result {
            let pos = mat.col(3).truncate();
            eprintln!(
                "[BONE_MAT] '{}' pos=({:.3},{:.3},{:.3})",
                name, pos.x, pos.y, pos.z
            );
        }
    }

    // ACMD's synthetic `top` joint is the character origin. It is not present in nusktb;
    // offline animations instead carry root motion on `Trans`. Using identity left
    // top-bound hitboxes (most grabs) behind while the rendered fighter moved.
    let top = synthetic_top_matrix(&result);
    result.insert("top".to_string(), top);
    result.insert("Top".to_string(), top);

    // ── Weapon skeletons ──────────────────────────────────────────────────
    for (_, weapon_skel, attach_bone) in weapon_skels {
        let attach_world = skel
            .bones
            .iter()
            .enumerate()
            .find(|(_, b)| b.name.eq_ignore_ascii_case(attach_bone))
            .map(|(i, _)| world[i])
            .unwrap_or(glam::Mat4::IDENTITY);

        for bone in &weapon_skel.bones {
            let Ok(bind_world) = weapon_skel.calculate_world_transform(bone) else {
                continue;
            };
            let bind_mat = glam::Mat4::from_cols_array_2d(&bind_world);
            let final_mat = attach_world * bind_mat;
            // Only insert if this bone name isn't already in the body skeleton —
            // body skeleton bones always take priority over weapon skeleton bones.
            result.entry(bone.name.clone()).or_insert(final_mat);
            result.entry(bone.name.to_lowercase()).or_insert(final_mat);
        }
    }

    result
}

impl HitboxRenderState {
    /// Returns a map of bone name -> world position (convenience wrapper).
    #[allow(dead_code)]
    pub fn bone_world_positions(&self) -> std::collections::HashMap<String, glam::Vec3> {
        self.bone_world_matrices()
            .into_iter()
            .map(|(k, m)| (k, m.col(3).truncate()))
            .collect()
    }

    /// Projects a 3D world position to normalized device coordinates (NDC),
    /// then to pixel coordinates within the given viewport rect.
    pub fn world_to_screen(
        &self,
        world_pos: glam::Vec3,
        viewport: egui::Rect,
    ) -> Option<egui::Pos2> {
        let transforms = self.camera.transforms(viewport.width(), viewport.height());
        let clip =
            transforms.mvp_matrix * glam::Vec4::new(world_pos.x, world_pos.y, world_pos.z, 1.0);
        clip_to_screen(clip, viewport)
    }

    /// Computes the screen-space radius for a sphere of `world_radius` centered at `world_pos`.
    /// Uses the camera-right vector so it's correct regardless of camera orientation.
    pub fn world_radius_to_screen(
        &self,
        world_pos: glam::Vec3,
        world_radius: f32,
        viewport: egui::Rect,
    ) -> Option<f32> {
        let transforms = self.camera.transforms(viewport.width(), viewport.height());
        let cam_right = transforms
            .model_view_matrix
            .inverse()
            .col(0)
            .truncate()
            .normalize();
        let edge = world_pos + cam_right * world_radius;
        let center_screen = self.world_to_screen(world_pos, viewport)?;
        let edge_screen = self.world_to_screen(edge, viewport)?;
        Some((edge_screen - center_screen).length())
    }
}

fn clip_to_screen(clip: glam::Vec4, viewport: egui::Rect) -> Option<egui::Pos2> {
    if !clip.is_finite() || clip.w <= 0.0001 {
        return None;
    }
    // Do not reject X/Y outside NDC here. A large sphere can overlap the viewport while
    // its center is outside it, and a capsule/wind polygon must retain off-screen endpoints
    // so its connected in-frame portion can still be painted. `painter_at(viewport)` clips
    // the finished primitive to the viewport safely.
    let ndc = clip.truncate() / clip.w;
    let sx = (ndc.x * 0.5 + 0.5) * viewport.width() + viewport.left();
    let sy = (-ndc.y * 0.5 + 0.5) * viewport.height() + viewport.top();
    (sx.is_finite() && sy.is_finite()).then(|| egui::pos2(sx, sy))
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)] // Rendering helpers remain grouped before callbacks.
mod tests {
    fn bone(
        name: &str,
        parent: Option<usize>,
        translation: glam::Vec3,
    ) -> ssbh_data::skel_data::BoneData {
        ssbh_data::skel_data::BoneData {
            name: name.to_string(),
            transform: glam::Mat4::from_translation(translation).to_cols_array_2d(),
            parent_index: parent,
            billboard_type: ssbh_data::skel_data::BillboardType::Disabled,
        }
    }

    fn transform_track(
        name: &str,
        frames: Vec<(glam::Vec3, glam::Quat)>,
    ) -> ssbh_data::anim_data::NodeData {
        use ssbh_data::anim_data::*;
        NodeData {
            name: name.to_string(),
            tracks: vec![TrackData {
                name: "Transform".to_string(),
                compensate_scale: false,
                transform_flags: TransformFlags::default(),
                values: TrackValues::Transform(
                    frames
                        .into_iter()
                        .map(|(translation, rotation)| Transform {
                            scale: ssbh_data::Vector3::new(1.0, 1.0, 1.0),
                            rotation: ssbh_data::Vector4::new(
                                rotation.x, rotation.y, rotation.z, rotation.w,
                            ),
                            translation: ssbh_data::Vector3::new(
                                translation.x,
                                translation.y,
                                translation.z,
                            ),
                        })
                        .collect(),
                ),
            }],
        }
    }

    /// The thrown fighter's `rot` bone lands exactly on the target, whatever its animation
    /// does to its parents.
    #[test]
    fn a_constrained_bone_reaches_its_world_target() {
        use glam::{Mat4, Quat, Vec3};
        let skel = ssbh_data::skel_data::SkelData {
            major_version: 1,
            minor_version: 0,
            bones: vec![
                bone("Trans", None, Vec3::ZERO),
                bone("Rot", Some(0), Vec3::new(0.0, 1.0, 0.0)),
                bone("Hip", Some(1), Vec3::new(0.0, 5.0, 0.0)),
            ],
        };
        let anim = ssbh_data::anim_data::AnimData {
            major_version: 2,
            minor_version: 0,
            final_frame_index: 0.0,
            groups: vec![ssbh_data::anim_data::GroupData {
                group_type: ssbh_data::anim_data::GroupType::Transform,
                nodes: vec![transform_track(
                    "Trans",
                    vec![(Vec3::new(3.0, 0.0, 2.0), Quat::from_rotation_y(0.7))],
                )],
            }],
        };
        let target = Mat4::from_rotation_translation(
            Quat::from_rotation_z(1.1),
            Vec3::new(10.0, 12.0, -4.0),
        );
        let mut transforms = Box::new(ssbh_wgpu::animation::AnimationTransforms::identity());
        ssbh_wgpu::animation::animate_skel_with_edit(
            &mut transforms,
            &skel,
            std::iter::once(&anim),
            None,
            0.0,
            |bones| super::constrain_bone_to_world(bones, &skel, 1, target),
        );
        assert!(transforms.world_transforms[1].abs_diff_eq(target, 1e-4));
        // Its children follow it.
        let hip = transforms.world_transforms[2].col(3).truncate();
        assert!(hip.abs_diff_eq(target.transform_point3(Vec3::new(0.0, 5.0, 0.0)), 1e-4));
    }

    /// Bone positions are evaluated through the hierarchy, whatever order the skeleton lists
    /// bones in, and a helper bone file without constraints moves nothing.
    #[test]
    fn bone_world_matrices_follow_the_hierarchy() {
        use glam::{Quat, Vec3};
        let skel = ssbh_data::skel_data::SkelData {
            major_version: 1,
            minor_version: 0,
            bones: vec![
                bone("Trans", None, Vec3::ZERO),
                bone("Hip", Some(0), Vec3::new(0.0, 8.0, 0.0)),
                // Listed before its parent, like Mario's lip bones.
                bone("H_ArmR", Some(3), Vec3::new(1.0, 0.0, 0.0)),
                bone("ArmR", Some(1), Vec3::new(2.0, 3.0, 0.0)),
            ],
        };
        let anim = ssbh_data::anim_data::AnimData {
            major_version: 2,
            minor_version: 0,
            final_frame_index: 1.0,
            groups: vec![ssbh_data::anim_data::GroupData {
                group_type: ssbh_data::anim_data::GroupType::Transform,
                nodes: vec![
                    transform_track(
                        "Trans",
                        vec![
                            (Vec3::ZERO, Quat::IDENTITY),
                            (Vec3::new(4.0, 0.0, 0.0), Quat::IDENTITY),
                        ],
                    ),
                    transform_track(
                        "ArmR",
                        vec![
                            (Vec3::new(2.0, 3.0, 0.0), Quat::IDENTITY),
                            (Vec3::new(2.0, 3.0, 0.0), Quat::from_rotation_z(1.2)),
                        ],
                    ),
                ],
            }],
        };
        let hlpb = ssbh_data::hlpb_data::HlpbData {
            major_version: 1,
            minor_version: 0,
            aim_constraints: Vec::new(),
            orient_constraints: Vec::new(),
        };

        let at = |name: &str, hlpb| {
            super::compute_bone_world_matrices(&skel, Some(&anim), hlpb, &[], 1.0)[name]
                .col(3)
                .truncate()
        };
        let (sin, cos) = 1.2f32.sin_cos();
        let expected = [
            ("Trans", Vec3::new(4.0, 0.0, 0.0)),
            ("Hip", Vec3::new(4.0, 8.0, 0.0)),
            ("ArmR", Vec3::new(6.0, 11.0, 0.0)),
            ("H_ArmR", Vec3::new(6.0 + cos, 11.0 + sin, 0.0)),
            ("top", Vec3::new(4.0, 0.0, 0.0)),
        ];
        for (name, position) in expected {
            for hlpb in [None, Some(&hlpb)] {
                let got = at(name, hlpb);
                assert!(
                    got.abs_diff_eq(position, 1e-4),
                    "{name}: {got:?}, expected {position:?}"
                );
            }
        }
    }

    /// Against real data: a fighter's helper bones move once their constraints apply, and the
    /// bones the constraints don't target stay where the plain evaluation put them.
    #[test]
    #[ignore = "needs a real extracted data root; see VISIONARY_BENCH_FIGHTER"]
    fn helper_bones_move_only_constrained_bones() {
        let root = std::path::PathBuf::from(
            std::env::var_os("VISIONARY_BENCH_FIGHTER").expect("set VISIONARY_BENCH_FIGHTER"),
        );
        let body = root.join("model/body/c00");
        let skel = ssbh_data::skel_data::SkelData::from_file(body.join("model.nusktb")).unwrap();
        let hlpb = ssbh_data::hlpb_data::HlpbData::from_file(body.join("model.nuhlpb")).unwrap();
        let anim =
            ssbh_data::anim_data::AnimData::from_file(root.join("motion/body/c00/a00wait1.nuanmb"))
                .unwrap();
        let constrained: std::collections::HashSet<String> = hlpb
            .orient_constraints
            .iter()
            .map(|c| c.target_bone_name.clone())
            .chain(
                hlpb.aim_constraints
                    .iter()
                    .map(|c| c.target_bone_name1.clone()),
            )
            .collect();
        let plain = super::compute_bone_world_matrices(&skel, Some(&anim), None, &[], 20.0);
        let helpers =
            super::compute_bone_world_matrices(&skel, Some(&anim), Some(&hlpb), &[], 20.0);
        // Mario lists `Jaw` and the lip bones after their `_offset` parents' children; an
        // in-order walk used to leave them at the origin.
        assert!(plain["Jaw"].col(3).truncate().length() > 1.0);
        let mut moved = 0;
        for bone in &skel.bones {
            let same = plain[&bone.name].abs_diff_eq(helpers[&bone.name], 1e-3);
            if constrained.contains(&bone.name) {
                moved += usize::from(!same);
            } else if !bone_has_constrained_ancestor(&skel, bone, &constrained) {
                assert!(same, "{} moved without a constraint", bone.name);
            }
        }
        eprintln!("{moved} of {} constrained bones moved", constrained.len());
        assert!(moved > 0);
    }

    fn bone_has_constrained_ancestor(
        skel: &ssbh_data::skel_data::SkelData,
        bone: &ssbh_data::skel_data::BoneData,
        constrained: &std::collections::HashSet<String>,
    ) -> bool {
        let mut parent = bone.parent_index;
        while let Some(index) = parent {
            if constrained.contains(&skel.bones[index].name) {
                return true;
            }
            parent = skel.bones[index].parent_index;
        }
        false
    }

    #[test]
    fn offscreen_endpoints_stay_projectable_for_partially_visible_shapes() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0));
        let offscreen = super::clip_to_screen(glam::Vec4::new(2.5, 0.0, 0.0, 1.0), viewport)
            .expect("front-facing points stay projectable outside NDC");
        assert!(offscreen.x > viewport.right());
        assert!(super::clip_to_screen(glam::Vec4::new(0.0, 0.0, 0.0, -1.0), viewport).is_none());
    }

    #[test]
    fn synthetic_top_uses_animated_trans_translation() {
        let expected = glam::Vec3::new(8.0, 1.5, -3.0);
        let bones = std::collections::HashMap::from([(
            "Trans".to_string(),
            glam::Mat4::from_scale_rotation_translation(
                glam::Vec3::splat(2.0),
                glam::Quat::from_rotation_y(1.0),
                expected,
            ),
        )]);
        let top = super::synthetic_top_matrix(&bones);
        assert_eq!(top.col(3).truncate(), expected);
        assert_eq!(top.x_axis.truncate(), glam::Vec3::X);
    }

    #[test]
    fn default_eyelid_animation_keeps_visibility_groups_only() {
        use ssbh_data::anim_data::{AnimData, GroupData, GroupType};

        let animation = AnimData {
            major_version: 2,
            minor_version: 1,
            final_frame_index: 12.0,
            groups: vec![
                GroupData {
                    group_type: GroupType::Transform,
                    nodes: Vec::new(),
                },
                GroupData {
                    group_type: GroupType::Visibility,
                    nodes: Vec::new(),
                },
                GroupData {
                    group_type: GroupType::Material,
                    nodes: Vec::new(),
                },
            ],
        };

        let filtered = super::visibility_only_animation(&animation);

        assert_eq!(filtered.major_version, animation.major_version);
        assert_eq!(filtered.minor_version, animation.minor_version);
        assert_eq!(filtered.final_frame_index, animation.final_frame_index);
        assert_eq!(filtered.groups.len(), 1);
        assert_eq!(filtered.groups[0].group_type, GroupType::Visibility);
    }

    #[test]
    fn a_visibility_track_reaches_only_its_own_mesh() {
        use ssbh_data::anim_data::{AnimData, GroupData, GroupType, NodeData, TrackData, TrackValues};

        let track = |name: &str, on: bool| NodeData {
            name: name.into(),
            tracks: vec![TrackData {
                name: "Visibility".into(),
                compensate_scale: false,
                transform_flags: Default::default(),
                values: TrackValues::Boolean(vec![on]),
            }],
        };
        // The reverse forward smash stores the FLIP track first, so the plain one won last.
        let mut anim = AnimData {
            major_version: 2,
            minor_version: 1,
            final_frame_index: 0.0,
            groups: vec![GroupData {
                group_type: GroupType::Visibility,
                nodes: vec![track("mace_spikeFLIP", false), track("mace_spike", true), track("Eye", true)],
            }],
        };
        let meshes: Vec<String> = [
            "mace_spike_VIS_O_OBJShape",
            "mace_spikeFLIP_VIS_O_OBJShape",
            "EyeL_VIS_O_OBJShape",
            "EyeR_VIS_O_OBJShape",
        ]
        .map(String::from)
        .to_vec();
        super::exact_visibility_tracks(&mut anim, &meshes);

        let mut visible: Vec<(String, bool)> = meshes.iter().map(|m| (m.clone(), true)).collect();
        ssbh_wgpu::animation::animate_visibility(&anim, 0.0, &mut visible);
        assert_eq!(
            visible,
            [
                ("mace_spike_VIS_O_OBJShape".to_string(), true),
                ("mace_spikeFLIP_VIS_O_OBJShape".to_string(), false),
                // No exact `Eye_VIS` mesh: the prefix rule is left to work as it did.
                ("EyeL_VIS_O_OBJShape".to_string(), true),
                ("EyeR_VIS_O_OBJShape".to_string(), true),
            ]
        );
    }

    #[test]
    fn default_eyelid_layer_precedes_selected_animation() {
        use ssbh_data::anim_data::AnimData;

        let default = AnimData {
            major_version: 2,
            minor_version: 1,
            final_frame_index: 0.0,
            groups: Vec::new(),
        };
        let selected = default.clone();
        let layers = super::animation_layers(Some(&default), Some(&selected));

        assert_eq!(layers.len(), 2);
        assert!(std::ptr::eq(layers[0], &default));
        assert!(std::ptr::eq(layers[1], &selected));
    }

    /// Benchmark for the editor's dominant per-frame CPU cost. Ignored by default because it
    /// needs real game files; run it against an extracted data root with:
    ///
    /// ```text
    /// VISIONARY_BENCH_FIGHTER=/path/to/export/fighter/mario \
    ///   cargo test --release -- --ignored --nocapture bone_matrix_cost
    /// ```
    #[test]
    #[ignore = "needs a real extracted data root; see VISIONARY_BENCH_FIGHTER"]
    fn bone_matrix_cost() {
        use std::time::Instant;
        let Some(root) = std::env::var_os("VISIONARY_BENCH_FIGHTER").map(std::path::PathBuf::from)
        else {
            panic!("set VISIONARY_BENCH_FIGHTER to a fighter dir, e.g. .../export/fighter/mario");
        };
        let model_root = root.join("model");
        let skel_path = model_root.join("body").join("c00").join("model.nusktb");
        let skel = ssbh_data::skel_data::SkelData::from_file(&skel_path)
            .unwrap_or_else(|e| panic!("read {skel_path:?}: {e}"));

        // Any body motion will do — pick the first one on disk for a stable, real workload.
        let anim_dir = root.join("motion").join("body").join("c00");
        let anim_path = std::fs::read_dir(&anim_dir)
            .ok()
            .and_then(|d| {
                let mut v: Vec<_> = d
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "nuanmb"))
                    .collect();
                v.sort();
                v.into_iter().next()
            })
            .expect("no .nuanmb under motion/body/c00");
        let anim = ssbh_data::anim_data::AnimData::from_file(&anim_path).ok();

        // Load weapon skeletons the way `load_model` does.
        let mut weapons: Vec<(String, ssbh_data::skel_data::SkelData, String)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&model_root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "body" {
                    continue;
                }
                let Some(p) = (0..8)
                    .map(|s| entry.path().join(format!("c{s:02}")).join("model.nusktb"))
                    .find(|p| p.exists())
                else {
                    continue;
                };
                if let Ok(s) = ssbh_data::skel_data::SkelData::from_file(&p) {
                    let attach = super::weapon_attach_bone(&name);
                    weapons.push((name, s, attach));
                }
            }
        }

        eprintln!(
            "\n[bench] {} body bones, anim {:?}, {} weapon skeletons ({} bones)",
            skel.bones.len(),
            anim_path.file_name().unwrap(),
            weapons.len(),
            weapons.iter().map(|(_, s, _)| s.bones.len()).sum::<usize>(),
        );

        const N: usize = 200;
        // Warm up so we time steady state rather than first-touch page faults.
        for f in 0..10 {
            std::hint::black_box(super::compute_bone_world_matrices(
                &skel,
                anim.as_ref(),
                None,
                &weapons,
                f as f32,
            ));
        }

        // Case A: a different frame each call (animation playing) — always real work.
        let t = Instant::now();
        let mut entries = 0;
        for i in 0..N {
            let m = super::compute_bone_world_matrices(
                &skel,
                anim.as_ref(),
                None,
                &weapons,
                (i % 60) as f32,
            );
            entries += m.len();
        }
        let moving = t.elapsed();

        // Case B: the same frame every call — the common editor case (idle repaints, mouse
        // moves, slider drags all recompute an identical result).
        let t = Instant::now();
        for _ in 0..N {
            std::hint::black_box(super::compute_bone_world_matrices(
                &skel,
                anim.as_ref(),
                None,
                &weapons,
                7.0,
            ));
        }
        let still = t.elapsed();

        eprintln!(
            "[bench] moving frame: {:.3} ms/call\n[bench] same frame:   {:.3} ms/call\n[bench] {} map entries per call\n",
            moving.as_secs_f64() * 1000.0 / N as f64,
            still.as_secs_f64() * 1000.0 / N as f64,
            entries / N,
        );
    }
}

/// Determine which character body bone a weapon model attaches to at runtime.
/// This is based on the weapon folder name convention used in Smash Ultimate.
fn weapon_attach_bone(weapon_dir: &str) -> String {
    // Most right-hand weapons attach to "haver" (right hand helper bone).
    // Left-hand weapons (shields, off-hand items) attach to "havel".
    // Hammers, bats, and other two-handed weapons also use "haver".
    match weapon_dir {
        "shield" | "shieldl" => "havel".to_string(),
        _ => "haver".to_string(),
    }
}
/// Paint callback for the animated character model. Hitbox, windbox, grab, and effect-spawn
/// circles are drawn by egui on top of this viewport in `app.rs`.
pub struct ViewportCallback {
    pub width: f32,
    pub height: f32,
    /// Zero-based `.nuanmb` frame index. The app converts its one-based game-frame playhead
    /// before constructing the callback.
    pub animation_frame: f32,
    pub anim_path: Option<std::path::PathBuf>,
    /// Optional default eyelid visibility baseline for the selected fighter/costume.
    pub default_anim_path: Option<std::path::PathBuf>,
    pub skel_path: Option<std::path::PathBuf>,
}

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        // Set up uncaptured error handler to surface wgpu validation errors
        use std::sync::atomic::{AtomicBool, Ordering};
        static ERROR_HANDLER_SET: AtomicBool = AtomicBool::new(false);
        if !ERROR_HANDLER_SET.swap(true, Ordering::Relaxed) {
            device.on_uncaptured_error(std::sync::Arc::new(|e: wgpu::Error| {
                eprintln!("[wgpu ERROR] {:?}", e);
            }));
        }

        if let Some(state) = resources.get_mut::<HitboxRenderState>() {
            let w = self.width as u32;
            let h = self.height as u32;

            let frame_changed = (self.animation_frame - state.last_frame).abs() > f32::EPSILON;
            let anim_changed = self.anim_path != state.last_anim_path;
            let default_anim_changed = self.default_anim_path != state.last_default_anim_path;
            let skel_changed = self.skel_path != state.last_skel_path;

            state.resize(device, w, h);
            state.update_camera(queue, self.width, self.height);
            state.wgpu_device = Some(device.clone());
            state.wgpu_queue = Some(queue.clone());

            // Re-skin when frame, animation, or skeleton changes
            if frame_changed || anim_changed || default_anim_changed || skel_changed {
                state.apply_animation_with_default(
                    queue,
                    self.default_anim_path.as_deref(),
                    self.anim_path.as_deref(),
                    self.skel_path.as_deref(),
                    self.animation_frame,
                );
                state.last_frame = self.animation_frame;
                state.last_anim_path = self.anim_path.clone();
                state.last_default_anim_path = self.default_anim_path.clone();
                state.last_skel_path = self.skel_path.clone();
            }

            // Always re-render (camera may have changed, or egui needs a fresh frame)
            state.renderer.begin_render_models(
                egui_encoder,
                &state.render_models,
                state.shared_data.database(),
                &state.model_render_options,
            );
        }
        Vec::new()
    }

    fn finish_prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(state) = resources.get::<HitboxRenderState>() {
            state.renderer.end_render_models(render_pass);
        } else {
            eprintln!("[VIEWPORT] paint skipped: HitboxRenderState missing (load a model first)");
        }
    }
}
