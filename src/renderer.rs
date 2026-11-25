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

/// A camera placement as Settings edits it and saves it as the default view. The view matrix
/// is translation * rotation (XYZ Euler angles), as in SSBH Editor and SSBH Studio.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CameraView {
    pub translation: [f32; 3],
    pub rotation: [f32; 3],
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for CameraView {
    /// The side view of a fighter standing at the origin.
    fn default() -> Self {
        Self {
            translation: [0.0, -8.0, -60.0],
            rotation: [0.0, std::f32::consts::FRAC_PI_2, 0.0],
            fov_y: 30f32.to_radians(),
            near: 1.0,
            far: 400_000.0,
        }
    }
}

pub struct Camera {
    pub translation: Vec3,
    pub rotation: Vec3,
    /// The point the camera keeps centred: the fighter's position over the ground, so a move
    /// that carries the fighter forward (a dash attack, a slide) stays in view.
    pub follow: Vec3,
    /// The point orbiting and view snaps turn about, relative to `follow`. Panning and
    /// framing move it, so whatever is centred stays centred while turning (SSBH Studio).
    pub target: Vec3,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self::from_view(&CameraView::default())
    }
}

/// The camera's rotation matrix for XYZ Euler angles.
pub fn camera_rotation(rotation: Vec3) -> Mat4 {
    Mat4::from_euler(glam::EulerRot::XYZ, rotation.x, rotation.y, rotation.z)
}

impl Camera {
    pub fn from_view(view: &CameraView) -> Self {
        let mut camera = Self {
            translation: Vec3::from(view.translation),
            rotation: Vec3::from(view.rotation),
            follow: Vec3::ZERO,
            target: Vec3::ZERO,
            fov_y: view.fov_y,
            near: view.near,
            far: view.far,
        };
        camera.target_centre();
        camera
    }

    /// This placement, without the fighter it follows.
    pub fn view(&self) -> CameraView {
        CameraView {
            translation: self.translation.to_array(),
            rotation: self.rotation.to_array(),
            fov_y: self.fov_y,
            near: self.near,
            far: self.far,
        }
    }

    /// The point straight ahead at the depth of the view origin, relative to `follow`.
    fn centred_point(&self) -> Vec3 {
        camera_rotation(self.rotation)
            .inverse()
            .transform_vector3(Vec3::new(-self.translation.x, -self.translation.y, 0.0))
    }

    /// Turns to `rotation` while keeping `target` where it appears on screen.
    pub fn turn_to(&mut self, rotation: Vec3) {
        let view = camera_rotation(self.rotation).transform_vector3(self.target) + self.translation;
        self.rotation = rotation;
        self.translation = view - camera_rotation(rotation).transform_vector3(self.target);
    }

    /// Turn the camera around the fighter: sideways drags turn it around, vertical drags tilt
    /// it up or down, short of looking straight down. A view already past that, such as the
    /// top view, is not pulled back to it.
    pub fn orbit(&mut self, delta_x: f32, delta_y: f32) {
        let limit = self.rotation.x.abs().max(1.4);
        self.rotation.y += delta_x * 0.01;
        self.rotation.x = (self.rotation.x + delta_y * 0.01).clamp(-limit, limit);
    }

    /// Orbit like dragging the scene: right turns right, up tilts up.
    pub fn rotate(&mut self, delta_x: f32, delta_y: f32) {
        self.turn_to(self.rotation + Vec3::new(delta_y * 0.01, delta_x * 0.01, 0.0));
    }

    /// Moves the view by `shift` in view space, keeping `target` at the view centre.
    pub fn pan_view(&mut self, shift: Vec3) {
        self.translation += shift;
        self.target -= camera_rotation(self.rotation)
            .inverse()
            .transform_vector3(shift);
    }

    /// Pan in the camera plane (left/right and up/down) while keeping rotation fixed.
    /// Moves in world X (left/right) and world Y (up/down) regardless of camera rotation.
    pub fn pan_camera(&mut self, delta_x: f32, delta_y: f32) {
        let speed = self.translation.z.abs() * 0.001;
        self.translation.x -= delta_x * speed;
        self.translation.y += delta_y * speed;
    }

    /// Make whatever is straight ahead what orbiting and view snaps turn about.
    pub fn target_centre(&mut self) {
        self.target = self.centred_point();
    }

    /// Pan in the view plane so the model follows the pointer. The viewport height and
    /// vertical field of view convert pixels to world units at the model's distance.
    pub fn pan(&mut self, delta_x: f32, delta_y: f32, viewport_height: f32) {
        let factor = self.fov_y.sin() * self.translation.z.abs() / viewport_height.max(1.0);
        self.pan_view(Vec3::new(delta_x * factor, -delta_y * factor, 0.0));
    }

    /// Zooms in by `amount`, scaled by distance so far views zoom as quickly as near ones.
    /// Never zooms through the point being looked at.
    pub fn zoom(&mut self, amount: f32) {
        let delta = amount * self.translation.z.abs();
        self.translation.z = (self.translation.z + delta).min(-1.0);
    }

    /// Points the camera at a sphere (in world space) without changing its angle.
    pub fn frame(&mut self, center: Vec3, radius: f32) {
        let center = center - self.follow;
        let half_fov = (self.fov_y * 0.5).max(0.05);
        let distance = radius.max(1.0) / half_fov.sin() * 1.1;
        self.translation = Vec3::new(0.0, 0.0, -distance)
            - camera_rotation(self.rotation).transform_vector3(center);
        self.target = center;
    }

    /// Whether the camera sits at `view`.
    pub fn is_at(&self, view: &CameraView) -> bool {
        self.translation
            .abs_diff_eq(Vec3::from(view.translation), 1e-4)
            && self.rotation.abs_diff_eq(Vec3::from(view.rotation), 1e-4)
            && (self.fov_y - view.fov_y).abs() < 1e-4
    }

    /// Back to `view`'s angle, keeping the zoom and what is centred.
    pub fn reset_turn(&mut self, view: &CameraView) {
        self.turn_to(Vec3::from(view.rotation));
    }

    /// Takes one frame of a camera animation. Its view matrix is rotation * translation *
    /// scale; it is decomposed into this camera's translation * rotation, as SSBH Studio does,
    /// so the overlays and effects project through the same camera.
    pub fn apply_animation(&mut self, values: &ssbh_wgpu::animation::camera::CameraAnimValues) {
        let model_view = Mat4::from_quat(values.rotation)
            * Mat4::from_translation(values.translation)
            * Mat4::from_scale(values.scale);
        let (_, rotation, translation) = model_view.to_scale_rotation_translation();
        self.translation = translation;
        self.rotation = rotation.to_euler(glam::EulerRot::XYZ).into();
        self.fov_y = values.fov_y_radians;
        self.near = values.near_clip;
        self.far = values.far_clip;
        self.follow = Vec3::ZERO;
        self.target_centre();
    }

    pub fn transforms(&self, width: f32, height: f32) -> CameraTransforms {
        let aspect = if height > 0.0 { width / height } else { 1.0 };
        let model_view = Mat4::from_translation(self.translation)
            * camera_rotation(self.rotation)
            * Mat4::from_translation(-self.follow);
        let projection =
            glam::camera::rh::proj::directx::perspective(self.fov_y, aspect, self.near, self.far);
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
    /// What the renderer draws with (bloom on, shadows off); models are built for it.
    render_settings: RenderSettings,
    pub shared_data: SharedRenderData,
    pub render_models: Vec<RenderModel>,
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
    /// The loaded model's `model.nuhlpb`, which bends the mesh while [`Self::helper_bones`] is
    /// on. Not the overlay's bone matrices: the game's `ModelModule` joints, which hitboxes and
    /// effects sit on, leave helper bones as the animation has them.
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
    /// The bone matrices of frames asked for before, while the animation, skeletons and scale
    /// they were evaluated with stand.
    poses: PoseCache,
    /// Weapon skeletons: (weapon_name, skel, attach_bone_name)
    /// attach_bone_name is the character bone the weapon root attaches to (e.g. "haver")
    weapon_skels: Vec<(String, ssbh_data::skel_data::SkelData, String)>,
    /// Track last rendered state to skip redundant GPU work
    last_frame: f32,
    last_anim_path: Option<std::path::PathBuf>,
    last_default_anim_path: Option<std::path::PathBuf>,
    last_skel_path: Option<std::path::PathBuf>,
    /// Model folder of each object model after the body in `render_models`.
    object_dirs: Vec<std::path::PathBuf>,
    /// Effects drawn with the game's shaders, after the models.
    pub vfx: crate::vfx_preview::VfxPreview,
    /// The scene's colour and depth as the effects find it, for refraction and floor decals.
    scene_copy: eff_wgpu::scene::SceneCopy,
    distortion: eff_wgpu::scene::Distortion,
    /// Weapon swing trails.
    trails: crate::preview_trail::PreviewTrails,
    /// The data folder and stage (`None`: no stage, training lighting) the preview shows.
    stage_root: Option<(std::path::PathBuf, Option<String>)>,
    /// The picked stage's models, drawn after the fighter and its objects.
    stage_models: Vec<RenderModel>,
    /// What each stage model was placed from, to place it again when the stage shifts.
    stage_parts: Vec<StageModelLoad>,
    /// How far the stage is shifted under the fighter, in game space.
    stage_shift: glam::Vec3,
    /// Stage model folders being read on a thread, arriving one at a time.
    stage_loading: Option<std::sync::Mutex<std::sync::mpsc::Receiver<StageModelLoad>>>,
    /// The stage floor under grounded moves, and its colour as stored in the scene.
    floor: crate::preview_floor::PreviewFloor,
    floor_color: [f32; 4],
    /// The lines on the ground, and which of them Settings shows.
    axes: crate::preview_axes::PreviewAxes,
    axis_lines: crate::preview_axes::Lines,
    /// Body meshes hidden for a visibility set, to show again when the set switches back.
    set_hidden: std::collections::HashSet<usize>,
    /// Where the models and effects are drawn before going on screen.
    pub target: crate::viewport_target::ViewportTarget,
    /// Cached GPU handles for use in paint()
    pub wgpu_device: Option<wgpu::Device>,
    pub wgpu_queue: Option<wgpu::Queue>,
    /// What [`Self::apply_preferences`] last applied.
    preferences: Option<crate::preferences::Preferences>,
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
            [0.0, 0.0, 0.0, 1.0],
            surface_format,
        );
        let shared_data = SharedRenderData::new(device, queue);
        // The game's post pass: bloom, then the stage's colour grading. Effects draw into the
        // scene before it (see `prepare`), so they glow as they do in game. The backdrop is
        // part of the scene, as a stage is.
        renderer.update_color_lut_rgba(device, queue, &crate::post_grading::training_lut());
        // Use Shaded mode for full PBR rendering with textures
        let render_settings = RenderSettings {
            render_bloom: true,
            render_shadows: false,
            ..RenderSettings::default()
        };
        renderer.update_render_settings(queue, &render_settings);
        let mut state = Self {
            renderer,
            render_settings,
            shared_data,
            render_models: Vec::new(),
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
            poses: PoseCache::default(),
            weapon_skels: Vec::new(),
            last_frame: -1.0,
            last_anim_path: None,
            last_default_anim_path: None,
            last_skel_path: None,
            object_dirs: Vec::new(),
            vfx: crate::vfx_preview::VfxPreview::new(
                ssbh_wgpu::RGBA_COLOR_FORMAT,
                Some(ssbh_wgpu::DEPTH_FORMAT),
                ssbh_wgpu::MSAA_SAMPLE_COUNT,
            ),
            scene_copy: eff_wgpu::scene::SceneCopy::new(
                device,
                ssbh_wgpu::RGBA_COLOR_FORMAT,
                ssbh_wgpu::MSAA_SAMPLE_COUNT,
            ),
            distortion: eff_wgpu::scene::Distortion::new(
                device,
                ssbh_wgpu::RGBA_COLOR_FORMAT,
                ssbh_wgpu::MSAA_SAMPLE_COUNT,
            ),
            stage_root: None,
            stage_models: Vec::new(),
            stage_parts: Vec::new(),
            stage_shift: glam::Vec3::ZERO,
            stage_loading: None,
            trails: crate::preview_trail::PreviewTrails::new(
                device,
                ssbh_wgpu::RGBA_COLOR_FORMAT,
                ssbh_wgpu::DEPTH_FORMAT,
                ssbh_wgpu::MSAA_SAMPLE_COUNT,
            ),
            floor: crate::preview_floor::PreviewFloor::new(
                device,
                ssbh_wgpu::RGBA_COLOR_FORMAT,
                ssbh_wgpu::DEPTH_FORMAT,
                ssbh_wgpu::MSAA_SAMPLE_COUNT,
            ),
            axes: crate::preview_axes::PreviewAxes::new(
                device,
                ssbh_wgpu::RGBA_COLOR_FORMAT,
                ssbh_wgpu::DEPTH_FORMAT,
                ssbh_wgpu::MSAA_SAMPLE_COUNT,
            ),
            axis_lines: Default::default(),
            set_hidden: std::collections::HashSet::new(),
            floor_color: [0.0; 4],
            target: crate::viewport_target::ViewportTarget::new(device, surface_format),
            wgpu_device: Some(device.clone()),
            wgpu_queue: Some(queue.clone()),
            preferences: None,
        };
        state.apply_preferences(queue, &crate::preferences::Preferences::default());
        state
    }

    /// Viewport preferences from the Settings window: background, floor grid, world axes and
    /// bloom. Called every paint; only a change does anything.
    pub fn apply_preferences(
        &mut self,
        queue: &wgpu::Queue,
        preferences: &crate::preferences::Preferences,
    ) {
        if self.preferences.as_ref() == Some(preferences) {
            return;
        }
        self.preferences = Some(preferences.clone());
        let (backdrop, floor) = preferences.viewport_colors();
        self.renderer.set_clear_color([
            backdrop[0] as f64,
            backdrop[1] as f64,
            backdrop[2] as f64,
            1.0,
        ]);
        // The backdrop and the floor are part of the scene and go through the post pass, so
        // each is stored as the value that comes out as its colour.
        let scene = crate::post_grading::scene_value_for(backdrop);
        self.renderer.set_scene_clear_color(wgpu::Color {
            r: scene[0] as f64,
            g: scene[1] as f64,
            b: scene[2] as f64,
            a: 1.0,
        });
        let [r, g, b] = crate::post_grading::scene_value_for(floor);
        self.floor_color = [r, g, b, 1.0];
        self.axis_lines = crate::preview_axes::Lines {
            grid: preferences.floor_grid,
            axes: preferences.world_axes,
        };
        if self.render_settings.render_bloom != preferences.bloom {
            self.render_settings.render_bloom = preferences.bloom;
            self.renderer
                .update_render_settings(queue, &self.render_settings);
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if width == self.current_width && height == self.current_height {
            return;
        }
        self.renderer.resize(device, queue, width, height, 1.0);
        self.target.resize(device, width, height);
        self.scene_copy.resize(device, width, height);
        self.current_width = width;
        self.current_height = height;
    }

    /// Upload a model already read from `model_dir` (reading it is slow, so callers do that
    /// off the GUI thread).
    pub fn load_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model_dir: &Path,
        folder: ModelFolder,
    ) {
        self.render_models.clear();
        self.set_hidden.clear();
        self.object_dirs.clear();
        self.weapon_skels.clear();
        self.poses.clear();
        // `folder` holds the files ARCropolis would load here (see `read_costume_model`).
        if folder.meshes.is_empty() && folder.skels.is_empty() {
            eprintln!("[MODEL] no model files for {:?}", model_dir);
            return;
        }
        let render_model = RenderModel::from_folder(
            device,
            queue,
            &folder,
            &self.shared_data,
            &self.render_settings,
        );
        eprintln!("[MODEL] loaded from {:?}", model_dir);
        self.model_hlpb = folder.find_hlpb().cloned();
        self.model_matl = folder.find_matl().cloned();
        self.render_models.push(render_model);

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
            self.poses.clear();
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
                self.poses.clear();
                self.cached_anim =
                    ssbh_data::anim_data::AnimData::from_file(path)
                        .ok()
                        .map(|mut a| {
                            exact_visibility_tracks(&mut a, &meshes);
                            (path.to_path_buf(), a)
                        });
            }
        } else if self.cached_anim.take().is_some() {
            self.poses.clear();
        }

        // Reload skel only when path changes
        if let Some(path) = skel_path {
            let needs_load = self
                .cached_skel
                .as_ref()
                .map(|(p, _)| p != path)
                .unwrap_or(true);
            if needs_load {
                self.poses.clear();
                self.cached_skel = ssbh_data::skel_data::SkelData::from_file(path)
                    .ok()
                    .map(|s| (path.to_path_buf(), s));
            }
        } else if self.cached_skel.take().is_some() {
            self.poses.clear();
        }

        let anim = self.cached_anim.as_ref().map(|(_, a)| a);
        let skel = self.cached_skel.as_ref().map(|(_, s)| s);
        let hlpb = self.model_hlpb.as_ref().filter(|_| self.helper_bones);
        let matl = self.model_matl.as_ref();

        // Only the body; object models after it are animated in `update_objects`.
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
                None,
                None,
                None,
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
    }

    /// Light the models as the training stage does, from `data_root`'s
    /// `stage/training/normal`: its light set (the directional light, ambient and colours the
    /// fighter shaders read) and its colour grading LUT. Without the stage files the renderer's
    /// default lighting and the LUT read from a GPU capture stay.
    pub fn use_stage(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data_root: Option<&Path>,
        stage: Option<&str>,
    ) {
        let key = data_root.map(|root| (root.to_path_buf(), stage.map(str::to_owned)));
        if self.stage_root == key {
            return;
        }
        self.stage_root = key;
        self.stage_models.clear();
        self.stage_parts.clear();
        self.stage_loading = None;
        let Some(root) = data_root else {
            return;
        };
        // Lit by the picked stage, or by the training stage with no stage shown.
        let picked = stage.map(|name| crate::preview_stage::files(root, name));
        let training = crate::preview_stage::files(root, "training");
        let light = picked
            .as_ref()
            .and_then(|f| f.light.clone())
            .or(training.light.clone());
        match light.and_then(|path| ssbh_data::anim_data::AnimData::from_file(path).ok()) {
            Some(light) => self.renderer.update_stage_uniforms(queue, &light, 0.0),
            None => self.renderer.reset_stage_uniforms(queue),
        }
        let lut = picked.as_ref().and_then(|f| f.lut.clone()).or(training.lut);
        if let Some(lut) = lut.and_then(|path| nutexb::NutexbFile::read_from_file(path).ok()) {
            self.renderer.update_color_lut(device, queue, &lut);
        }
        // The models are large (tens of megabytes of textures): read them off the GUI thread.
        if let Some(picked) = picked {
            let (send, receive) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for (dir, anim) in picked.models {
                    let mut folder = ModelFolder::load_folder(&dir);
                    crate::preview_stage::turn_to_preview(&mut folder);
                    let anim =
                        anim.and_then(|path| ssbh_data::anim_data::AnimData::from_file(path).ok());
                    if send.send(StageModelLoad { folder, anim }).is_err() {
                        return;
                    }
                }
            });
            self.stage_loading = Some(std::sync::Mutex::new(receive));
        }
    }

    /// Upload the stage models read since the last frame. Returns whether more are coming.
    fn collect_stage_models(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let Some(loading) = &self.stage_loading else {
            return false;
        };
        let loading = loading.lock().unwrap_or_else(|e| e.into_inner());
        let mut arrived = Vec::new();
        let more = loop {
            match loading.try_recv() {
                Ok(load) => arrived.push(load),
                Err(std::sync::mpsc::TryRecvError::Empty) => break true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break false,
            }
        };
        drop(loading);
        if !more {
            self.stage_loading = None;
        }
        for load in arrived {
            let mut model = RenderModel::from_folder(
                device,
                queue,
                &load.folder,
                &self.shared_data,
                &self.render_settings,
            );
            Self::place_stage_model(
                &mut model,
                &load,
                self.stage_shift,
                queue,
                &self.shared_data,
            );
            // The layer Inkling's ink paints onto the stage draws black with no ink on it.
            for mesh in model.meshes.iter_mut() {
                if mesh.name.to_ascii_lowercase().starts_with("inkmesh") {
                    mesh.is_visible = false;
                }
            }
            self.stage_models.push(model);
            // Only what placing needs is kept; the meshes and textures are on the GPU.
            self.stage_parts.push(StageModelLoad {
                folder: ModelFolder {
                    skels: load.folder.skels,
                    matls: load.folder.matls,
                    hlpbs: load.folder.hlpbs,
                    meshes: Vec::new(),
                    meshexes: Vec::new(),
                    modls: Vec::new(),
                    adjs: Vec::new(),
                    anims: Vec::new(),
                    nutexbs: Vec::new(),
                    xmbs: Vec::new(),
                },
                anim: load.anim,
            });
        }
        more
    }

    /// Place a stage model: its own placing animation, with its root holding the turn into
    /// preview space and the shift for the move.
    fn place_stage_model(
        model: &mut RenderModel,
        parts: &StageModelLoad,
        shift: glam::Vec3,
        queue: &wgpu::Queue,
        shared_data: &SharedRenderData,
    ) {
        let root = crate::preview_stage::root_anim(crate::preview_stage::root_transform(shift));
        model.apply_anims(
            queue,
            parts.anim.iter().chain(std::iter::once(&root)),
            parts.folder.find_skel(),
            parts.folder.find_matl(),
            None,
            None,
            None,
            parts.folder.find_hlpb(),
            shared_data,
            0.0,
        );
    }

    /// Shift the stage under the fighter (game space), placing its models again if it moved.
    fn shift_stage(&mut self, shift: glam::Vec3, queue: &wgpu::Queue) {
        if (shift - self.stage_shift).length() < 1e-4 {
            return;
        }
        self.stage_shift = shift;
        for (model, parts) in self.stage_models.iter_mut().zip(&self.stage_parts) {
            Self::place_stage_model(model, parts, shift, queue, &self.shared_data);
        }
    }

    /// Whether a stage is shown, so the preview's own floor stays away.
    pub fn shows_stage(&self) -> bool {
        self.stage_root
            .as_ref()
            .is_some_and(|(_, stage)| stage.is_some())
    }

    /// Hide the body meshes of the visibility groups not showing, and show again the ones
    /// hidden earlier whose group is back. Runs after the animation, which sets visibility
    /// only for the meshes it has tracks for.
    fn apply_hidden_nodes(&mut self, hidden: &std::collections::HashSet<u64>) {
        let Some(model) = self.render_models.first_mut() else {
            return;
        };
        for (index, mesh) in model.meshes.iter_mut().enumerate() {
            if hidden.contains(&crate::preview_visibility::mesh_node(&mesh.name)) {
                if mesh.is_visible || self.set_hidden.contains(&index) {
                    mesh.is_visible = false;
                    self.set_hidden.insert(index);
                }
            } else if self.set_hidden.remove(&index) {
                mesh.is_visible = true;
            }
        }
    }

    /// Every mesh name of the fighter's own model.
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

    /// Place each mesh hung on a bone for this frame's pose and camera: the fighter's, its
    /// objects' and the stage's. ssbh_wgpu draws such meshes from this per-mesh transform.
    fn update_per_world(&mut self, queue: &wgpu::Queue, objects: &[ObjectDraw]) {
        let camera = self.renderer.camera_transforms();
        let body_skel = self.cached_skel.as_ref().map(|(_, s)| s);
        for (index, model) in self.render_models.iter_mut().enumerate() {
            let skel = match index {
                0 => body_skel,
                n => objects.get(n - 1).map(|object| object.skel.as_ref()),
            };
            model.update_per_world(queue, &camera, skel);
        }
        for (model, parts) in self.stage_models.iter_mut().zip(&self.stage_parts) {
            model.update_per_world(queue, &camera, parts.folder.find_skel());
        }
    }

    /// Shows the objects out this frame after the body model, loading each model once.
    fn update_objects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        objects: &[ObjectDraw],
    ) {
        if self.render_models.is_empty() {
            return;
        }
        for (index, object) in objects.iter().enumerate() {
            if self.object_dirs.get(index) != Some(&object.model_dir) {
                let folder = ModelFolder::load_folder(&object.model_dir);
                let model = RenderModel::from_folder(
                    device,
                    queue,
                    &folder,
                    &self.shared_data,
                    &self.render_settings,
                );
                if index < self.object_dirs.len() {
                    self.render_models[index + 1] = model;
                    self.object_dirs[index] = object.model_dir.clone();
                } else {
                    self.render_models.push(model);
                    self.object_dirs.push(object.model_dir.clone());
                }
            }
            let model = &mut self.render_models[index + 1];
            model.is_visible = true;
            let layers: Vec<&ssbh_data::anim_data::AnimData> = object
                .anim
                .as_deref()
                .into_iter()
                .chain(std::iter::once(&object.placement))
                .collect();
            model.apply_anims(
                queue,
                layers.iter().copied(),
                Some(&object.skel),
                None,
                None,
                None,
                None,
                None,
                &self.shared_data,
                object.frame,
            );
        }
        for model in self.render_models.iter_mut().skip(objects.len() + 1) {
            model.is_visible = false;
        }
    }

    pub fn weapon_skel_count(&self) -> usize {
        self.weapon_skels.len()
    }

    /// Whether the loaded model has a `model.nuhlpb` for the helper-bones toggle to act on.
    pub fn has_helper_bones(&self) -> bool {
        self.model_hlpb.is_some()
    }

    /// Turn helper bone constraints on or off.
    pub fn set_helper_bones(&mut self, enabled: bool) {
        if self.helper_bones != enabled {
            self.helper_bones = enabled;
            // Force the next prepare() to re-skin with the new constraint set.
            self.last_frame = -1.0;
        }
    }

    /// Resize the fighter, mesh and overlay alike. See [`Self::model_scale`].
    pub fn set_model_scale(&mut self, scale: f32) {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        if self.model_scale != scale {
            self.model_scale = scale;
            self.poses.clear();
            self.last_frame = -1.0;
        }
    }

    /// The scale the mesh and [`Self::bone_world_matrices_at`] are drawn at.
    pub fn model_scale(&self) -> f32 {
        self.model_scale
    }

    /// Eagerly load skeleton data so bone_world_matrices_at() returns valid results
    /// immediately, without waiting for the first prepare() call.
    pub fn load_skeleton(&mut self, skel_path: &Path) {
        self.poses.clear();
        self.cached_skel = ssbh_data::skel_data::SkelData::from_file(skel_path)
            .ok()
            .map(|s| (skel_path.to_path_buf(), s));
    }

    /// Bone name -> world matrix (body and weapon skeletons) at an explicit `frame`.
    /// The offset from ACMD should be transformed by this matrix (not just added). Use this from the simulation code so bone
    /// positions are correct on the very first frame (no 1-frame delay).
    pub fn bone_world_matrices_at(&self, frame: f32) -> Pose {
        let Some((_, skel)) = self.cached_skel.as_ref() else {
            if crate::debug_enabled() {
                eprintln!("[BONE] cached_skel is NONE — returning empty map");
            }
            return Default::default();
        };
        self.poses.at(frame, true, || {
            let anim = self.cached_anim.as_ref().map(|(_, a)| a);
            let mut matrices =
                compute_bone_world_matrices(skel, anim, None, &self.weapon_skels, frame);
            // The same as scaling the root bone, which is what the mesh gets: in game every
            // joint sits at exactly the model scale times its unscaled place.
            let scale = glam::Mat4::from_scale(glam::Vec3::splat(self.model_scale));
            for matrix in matrices.values_mut() {
                *matrix = scale * *matrix;
            }
            matrices
        })
    }

    /// [`Self::bone_world_matrices_at`] with every axis at its unscaled length: where the scaled
    /// fighter's bones are, for what is placed on them without taking its scale.
    pub fn joint_matrices_at(&self, frame: f32) -> Pose {
        let scaled = self.bone_world_matrices_at(frame);
        if self.model_scale == 1.0 {
            return scaled;
        }
        self.poses.at(frame, false, || {
            let unscale = glam::Mat4::from_scale(glam::Vec3::splat(self.model_scale.recip()));
            let joint = |(name, matrix): (&String, &glam::Mat4)| (name.clone(), *matrix * unscale);
            scaled.iter().map(joint).collect()
        })
    }
}

/// Bone matrices by frame. Evaluating a skeleton sorts its bones and builds a map of their
/// names, and one paint asks for the same frames many times over: the overlays for the open
/// one, and a weapon trail for a dozen samples of each frame it covers.
#[derive(Default)]
struct PoseCache {
    poses: std::sync::Mutex<Poses>,
}

/// A frame's bone matrices by name.
pub type Pose = std::sync::Arc<std::collections::HashMap<String, glam::Mat4>>;
/// By frame, and whether the axes carry the fighter's scale.
type Poses = std::collections::HashMap<(u32, bool), Pose>;

impl PoseCache {
    /// Enough for every sample of a long trail and the frames around it.
    const MOST: usize = 512;

    fn at(
        &self,
        frame: f32,
        scaled: bool,
        evaluate: impl FnOnce() -> std::collections::HashMap<String, glam::Mat4>,
    ) -> Pose {
        let key = (frame.to_bits(), scaled);
        if let Some(pose) = self.poses.lock().unwrap().get(&key) {
            return pose.clone();
        }
        let pose = Pose::new(evaluate());
        let mut poses = self.poses.lock().unwrap();
        if poses.len() >= Self::MOST {
            poses.clear();
        }
        poses.insert(key, pose.clone());
        pose
    }

    fn clear(&mut self) {
        self.poses.get_mut().unwrap().clear();
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
            let overreaches = meshes.iter().any(|mesh| {
                mesh.starts_with(&node.name) && !mesh.starts_with(&exact) && *mesh != node.name
            });
            let named_bare = meshes.contains(&node.name);
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
            let bind_mat = bind_world;
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
            transform: glam::Mat4::from_translation(translation),
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
                            scale: glam::Vec3::ONE,
                            rotation,
                            translation,
                        })
                        .collect(),
                ),
            }],
        }
    }

    /// Settings' floor grid is drawn with the ground lines, before the effects. ssbh_wgpu's own
    /// grid is drawn after them, over particles, so it stays off whatever is picked; and with
    /// nothing picked the view draws no lines and keeps its bloom, backdrop and floor.
    #[test]
    fn the_floor_grid_is_never_drawn_over_the_effects() {
        let Some((device, queue)) = crate::preview_trail::tests::gpu() else {
            eprintln!("no GPU; skipped");
            return;
        };
        let mut state =
            super::HitboxRenderState::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        assert!(!state.axis_lines.any());
        assert!(state.render_settings.render_bloom);
        let [r, g, b] = crate::post_grading::scene_value_for([0.028, 0.028, 0.055]);
        assert_eq!(state.floor_color, [r, g, b, 1.0]);
        let all = crate::preferences::Preferences {
            floor_grid: true,
            world_axes: true,
            bloom: false,
            viewport_color: Some([200, 40, 90]),
            ..Default::default()
        };
        state.apply_preferences(&queue, &all);
        assert!(state.axis_lines.grid && state.axis_lines.axes);
        assert!(!state.model_render_options.draw_floor_grid);
        assert!(!state.render_settings.render_bloom);
        assert_ne!(state.floor_color, [r, g, b, 1.0]);
        state.apply_preferences(&queue, &Default::default());
        assert!(!state.axis_lines.any());
        assert_eq!(state.floor_color, [r, g, b, 1.0]);
    }

    /// A frame's bones are evaluated once, however often a paint asks for them, and again after
    /// anything they depend on changes.
    #[test]
    fn a_frames_bones_are_evaluated_once() {
        let evaluated = std::cell::Cell::new(0);
        let mut poses = super::PoseCache::default();
        // One paint of an eight frame trail: the overlays at the open frame, twelve samples of
        // each trail frame for the ribbon and three for its outline. Sixty paints of it.
        let paint = |poses: &super::PoseCache| {
            let ask = |frame: f32| {
                poses.at(frame, true, || {
                    evaluated.set(evaluated.get() + 1);
                    std::collections::HashMap::from([(
                        "top".to_owned(),
                        glam::Mat4::from_translation(glam::Vec3::splat(frame)),
                    )])
                })
            };
            for _ in 0..4 {
                assert_eq!(ask(8.0)["top"].w_axis.x, 8.0);
            }
            for i in 0..=8 * 12 {
                ask(i as f32 / 12.0);
            }
            for i in 0..=8 * 3 {
                ask(i as f32 * 4.0 / 12.0);
            }
        };
        for _ in 0..60 {
            paint(&poses);
        }
        assert_eq!(evaluated.get(), 97);
        poses.clear();
        paint(&poses);
        assert_eq!(evaluated.get(), 2 * 97);
        // It never grows past its limit.
        for i in 0..5000 {
            poses.at(i as f32, true, Default::default);
        }
        assert!(poses.poses.lock().unwrap().len() <= super::PoseCache::MOST);
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

    /// Helper bones bend the mesh only. The game's joints leave them as the animation has them
    /// (Wolf's `h_elbowr` in wait: 0.4 degrees from the animation's turn, 53 from the helper's),
    /// so nothing the view draws on a bone may turn with them.
    #[test]
    fn helper_bones_leave_the_overlays_bones_alone() {
        use glam::{Quat, Vec3};
        use ssbh_data::hlpb_data::{HlpbData, OrientConstraintData};
        let Some((device, queue)) = crate::preview_trail::tests::gpu() else {
            eprintln!("no GPU; skipped");
            return;
        };
        let skel = ssbh_data::skel_data::SkelData {
            major_version: 1,
            minor_version: 0,
            bones: vec![
                bone("ShoulderR", None, Vec3::ZERO),
                bone("ArmR", Some(0), Vec3::X),
                bone("H_ElbowR", Some(0), Vec3::X),
            ],
        };
        let anim = ssbh_data::anim_data::AnimData {
            major_version: 2,
            minor_version: 0,
            final_frame_index: 0.0,
            groups: vec![ssbh_data::anim_data::GroupData {
                group_type: ssbh_data::anim_data::GroupType::Transform,
                nodes: vec![transform_track(
                    "ArmR",
                    vec![(Vec3::X, Quat::from_rotation_z(1.0))],
                )],
            }],
        };
        let hlpb = HlpbData {
            major_version: 1,
            minor_version: 1,
            aim_constraints: Vec::new(),
            orient_constraints: vec![OrientConstraintData {
                name: "elbow".into(),
                parent_bone_name1: "ShoulderR".into(),
                parent_bone_name2: "ShoulderR".into(),
                source_bone_name: "ArmR".into(),
                target_bone_name: "H_ElbowR".into(),
                unk_type: 1,
                constraint_axes: Vec3::splat(0.5),
                quat1: Quat::IDENTITY,
                quat2: Quat::IDENTITY,
                range_min: Vec3::splat(-180.0),
                range_max: Vec3::splat(180.0),
            }],
        };
        let evaluate =
            |hlpb| super::compute_bone_world_matrices(&skel, Some(&anim), hlpb, &[], 0.0);
        let plain = evaluate(None);
        let bent = evaluate(Some(&hlpb));
        assert!(!bent["H_ElbowR"].abs_diff_eq(plain["H_ElbowR"], 0.1));
        let mut state =
            super::HitboxRenderState::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        state.cached_skel = Some((Default::default(), skel.clone()));
        state.cached_anim = Some((Default::default(), anim.clone()));
        state.model_hlpb = Some(hlpb.clone());
        state.set_helper_bones(true);
        let drawn = state.bone_world_matrices_at(0.0);
        assert!(drawn["H_ElbowR"].abs_diff_eq(plain["H_ElbowR"], 1e-5));
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
        use ssbh_data::anim_data::{
            AnimData, GroupData, GroupType, NodeData, TrackData, TrackValues,
        };

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
                nodes: vec![
                    track("mace_spikeFLIP", false),
                    track("mace_spike", true),
                    track("Eye", true),
                ],
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
    /// Fighter objects out this frame.
    pub objects: Vec<ObjectDraw>,
    /// Effect sets out this frame.
    pub effects: Vec<crate::vfx_preview::EffectDraw>,
    /// Effect files to have loaded and built ahead of use (the fighter's own and the shared).
    pub effect_files: Vec<std::path::PathBuf>,
    /// Effect files to draw from edited bytes instead of the file on disk (`None` reverts).
    pub effect_overrides: Vec<(std::path::PathBuf, Option<std::sync::Arc<Vec<u8>>>)>,
    /// For painting again once effects still loading are ready.
    pub repaint: egui::Context,
    /// Draw the stage floor: the move is done on the ground.
    pub floor: bool,
    /// Weapon swing trails out this frame.
    pub trails: Vec<crate::preview_trail::TrailDraw>,
    /// Body mesh nodes (hash40) the fighter's visibility sets hide at this frame.
    pub hidden_nodes: std::collections::HashSet<u64>,
    /// The fighter has turned into one of its objects (Inkling's squid): its body is not drawn.
    pub hide_body: bool,
    /// How far the stage moves, in game space, to put the fighter where the move is done.
    pub stage_shift: glam::Vec3,
}

/// One object model to draw, already animated and placed.
pub struct ObjectDraw {
    pub model_dir: std::path::PathBuf,
    pub skel: std::sync::Arc<ssbh_data::skel_data::SkelData>,
    pub anim: Option<std::sync::Arc<ssbh_data::anim_data::AnimData>>,
    pub frame: f32,
    /// Puts the root bone in the world; layered over `anim`.
    pub placement: ssbh_data::anim_data::AnimData,
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

            state.resize(device, queue, w, h);
            state.update_camera(queue, self.width, self.height);
            state.wgpu_device = Some(device.clone());
            state.wgpu_queue = Some(queue.clone());

            let t_anim = std::time::Instant::now();
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

            state.apply_hidden_nodes(&self.hidden_nodes);
            if let Some(body) = state.render_models.first_mut() {
                body.is_visible = !self.hide_body;
            }
            let anim_ms = t_anim.elapsed().as_secs_f64() * 1000.0;
            let t_obj = std::time::Instant::now();
            state.update_objects(device, queue, &self.objects);
            state.shift_stage(self.stage_shift, queue);
            if state.collect_stage_models(device, queue) {
                self.repaint
                    .request_repaint_after(std::time::Duration::from_millis(30));
            }
            state.update_per_world(queue, &self.objects);
            let obj_ms = t_obj.elapsed().as_secs_f64() * 1000.0;
            if std::env::var_os("VFX_TIMING").is_some() && anim_ms + obj_ms > 8.0 {
                eprintln!("[vfx timing] paint anim {anim_ms:.1} ms objects {obj_ms:.1} ms");
            }

            let t = state.camera.transforms(self.width, self.height);
            let eye = t.camera_pos;
            for (file, bytes) in &self.effect_overrides {
                state.vfx.set_override(device, queue, file, bytes.clone());
            }
            for file in &self.effect_files {
                state.vfx.preload(device, queue, file);
            }
            let timing = std::env::var_os("VFX_TIMING")
                .is_some()
                .then(std::time::Instant::now);
            state.vfx.set_scene(&state.scene_copy);
            state.vfx.prepare(
                device,
                queue,
                t.model_view_matrix,
                t.projection_matrix,
                [eye.x, eye.y, eye.z],
                state.camera.near,
                state.camera.far,
                &self.effects,
            );
            if let Some(start) = timing {
                eprintln!(
                    "[vfx timing] prepare {:?} for {} draws",
                    start.elapsed(),
                    self.effects.len()
                );
            }
            if state.vfx.waiting() {
                self.repaint
                    .request_repaint_after(std::time::Duration::from_millis(4));
            }
            // A program wgpu rejects is skipped rather than fatal; say which, once each.
            for failure in state.vfx.take_failures() {
                eprintln!("[vfx] effect preview skipped a shader: {failure}");
            }
            // Always re-render (camera may have changed, or egui needs a fresh frame). Effects
            // draw into the scene right after the models and the floor, with their depth, so
            // bloom and colour grading treat them as the game's post pass does. Before they
            // draw, the scene is copied for the shaders that read what is behind them.
            state
                .trails
                .prepare(device, queue, t.mvp_matrix, &self.trails);
            let vfx = &state.vfx;
            if vfx.has_offsets() {
                state
                    .distortion
                    .fit(device, state.current_width, state.current_height);
            }
            let distortion = &state.distortion;
            let scene_copy = &state.scene_copy;
            let trails = &state.trails;
            // An effect alone has no floor, except one with floor decals to show on it.
            let floor = ((self.floor || vfx.looped_on_floor()) && !state.shows_stage())
                .then_some(&state.floor);
            if let Some(floor) = floor {
                floor.update(queue, t.mvp_matrix, state.floor_color);
            }
            let axes = state.axis_lines.any().then_some(&state.axes);
            if let Some(axes) = axes {
                axes.update(queue, t.mvp_matrix, t.camera_pos, state.axis_lines);
            }
            // The stage's models are drawn with the fighter's, in one list for the renderer;
            // they go back to their own list afterwards.
            let fighter_models = state.render_models.len();
            state.render_models.append(&mut state.stage_models);
            state.renderer.begin_render_models_with_scene(
                egui_encoder,
                &state.render_models,
                state.shared_data.database(),
                &state.model_render_options,
                &mut |encoder, scene| {
                    if let Some(floor) = floor {
                        floor.draw(&mut scene_pass(encoder, &scene, "viewport floor"));
                    }
                    scene_copy.copy(device, encoder, scene.resolve, scene.depth);
                    let mut pass = scene_pass(encoder, &scene, "viewport effects");
                    if let Some(axes) = axes {
                        axes.draw(&mut pass);
                    }
                    vfx.paint(&mut pass);
                    trails.draw(&mut pass);
                    drop(pass);
                    // What moves the picture moves all of it, so it comes last, as in the
                    // game's final pass.
                    if vfx.has_offsets() {
                        if let Some(mut pass) = distortion.begin(encoder, scene.depth) {
                            vfx.paint_offsets(&mut pass);
                        }
                        distortion.bend(device, encoder, scene.color, scene.resolve);
                    }
                },
            );
            state.stage_models = state.render_models.split_off(fighter_models);
            state.target.resize(device, w.max(1), h.max(1));
            if let Some(plain) = &state.target.plain {
                let mut pass = egui_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("viewport models"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: plain,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                state.renderer.end_render_models(&mut pass);
            }
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
            state.target.paint(render_pass);
        } else {
            eprintln!("[VIEWPORT] paint skipped: HitboxRenderState missing (load a model first)");
        }
    }
}

/// A pass that draws on into the scene the models left, colour and depth.
pub(crate) fn scene_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    scene: &ssbh_wgpu::SceneTarget<'_>,
    label: &'static str,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: scene.color,
            resolve_target: Some(scene.resolve),
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: scene.depth,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        ..Default::default()
    })
}

/// One stage model folder read off the GUI thread, with the animation that places it.
struct StageModelLoad {
    folder: ModelFolder,
    anim: Option<ssbh_data::anim_data::AnimData>,
}
