//! Checks a model folder for the mistakes that make it render wrong in game.
//!
//! Ported from SSBH Editor's validation (`src/validation.rs`), with each finding sorted into
//! one of two kinds. **Blocking** findings break the model: a file the game can't read, a mesh
//! the game would leave invisible, a shader that doesn't exist, a cube map in a 2D slot or the
//! reverse. Reload asset preview refuses those. Everything else is a **warning**: a yellow
//! checkerboard, faceted normals, wrong gamma, clamped tiling, uneven skin weights.
//!
//! The crash-level problems (empty or missing required files, an oversized file, a skeleton
//! without the fighter's vanilla bones) are refused earlier, by the preview's own staging in
//! [`crate::project_hub::prepare_carrier_assets`].

use nutexb::{NutexbFile, NutexbFormat};
use ssbh_data::matl_data::{
    BlendFactor, MagFilter, MatlData, MaxAnisotropy, MinFilter, ParamId, WrapMode,
};
use ssbh_data::mesh_data::{MeshData, VectorData};
use ssbh_data::modl_data::ModlData;
use ssbh_wgpu::shader_database::ShaderDatabase;
use ssbh_wgpu::ModelFolder;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The model is broken in game; Reload asset preview refuses it.
    Blocking,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelIssue {
    pub severity: Severity,
    /// The file the problem is in (`model.numdlb`).
    pub file: String,
    pub message: String,
}

/// What [`check_files`] found, blocking issues first.
#[derive(Debug, Clone, Default)]
pub struct ModelReport {
    pub issues: Vec<ModelIssue>,
}

impl ModelReport {
    pub fn blocking(&self) -> impl Iterator<Item = &ModelIssue> {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Blocking)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &ModelIssue> {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
    }

    pub fn has_blocking(&self) -> bool {
        self.blocking().next().is_some()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    /// `2 problems that break the model · 5 warnings`, or `None` when there is nothing.
    pub fn summary(&self) -> Option<String> {
        let blocking = self.blocking().count();
        let warnings = self.warnings().count();
        let mut parts = Vec::new();
        if blocking > 0 {
            parts.push(format!(
                "{blocking} problem{} that break{} the model",
                plural(blocking),
                if blocking == 1 { "s" } else { "" }
            ));
        }
        if warnings > 0 {
            parts.push(format!("{warnings} warning{}", plural(warnings)));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// Check the model made of these `(name, path)` files.
pub fn check_files(files: impl IntoIterator<Item = (String, PathBuf)>) -> ModelReport {
    let model = ModelFolder::load_files(files);
    check_model(&model, shader_database())
}

/// Check `files`, leaving out every issue the `baseline` model (the costume as the game ships
/// it) already has. SSBH Editor's rules flag some of the game's own models (vanilla Pac-Man has
/// 23 warnings), and an issue the mod didn't introduce is not one it can act on.
pub fn check_files_against(
    files: impl IntoIterator<Item = (String, PathBuf)>,
    baseline: Option<Vec<(String, PathBuf)>>,
) -> ModelReport {
    let mut report = check_files(files);
    if let Some(baseline) = baseline.filter(|files| !files.is_empty()) {
        let known: HashSet<(String, String)> = check_files(baseline)
            .issues
            .into_iter()
            .map(|issue| (issue.file, issue.message))
            .collect();
        report
            .issues
            .retain(|issue| !known.contains(&(issue.file.clone(), issue.message.clone())));
    }
    report
}

/// The shader database is the same for every model and takes a moment to build.
fn shader_database() -> &'static ShaderDatabase {
    static DATABASE: OnceLock<ShaderDatabase> = OnceLock::new();
    DATABASE.get_or_init(ShaderDatabase::new)
}

pub fn check_model(model: &ModelFolder, database: &ShaderDatabase) -> ModelReport {
    let mut issues = Vec::new();
    unreadable_files(model, &mut issues);

    let mesh = named(&model.meshes, "model.numshb");
    let modl = named(&model.modls, "model.numdlb");
    let model_matl = named(&model.matls, "model.numatb");
    let adj = model.adjs.iter().find(|(name, _)| name == "model.adjb");

    if let Some(mesh) = mesh {
        mesh_subindices(mesh, &mut issues);
        mesh_weights(mesh, &mut issues);
    }
    if let Some(modl) = modl {
        modl_entries(modl, model_matl, mesh, &mut issues);
    }
    let nutexbs: Vec<(&str, &NutexbFile)> = model
        .nutexbs
        .iter()
        .filter_map(|(name, file)| Some((name.as_str(), file.as_ref()?)))
        .collect();
    let texture_names: Vec<&str> = model
        .nutexbs
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();

    for (file, matl) in model
        .matls
        .iter()
        .filter_map(|(name, matl)| Some((name.as_str(), matl.as_ref()?)))
    {
        let assigned = Assignments::new(matl, modl, mesh);
        shader_labels(file, matl, database, &mut issues);
        required_attributes(file, matl, database, &assigned, &mut issues);
        premultiplied_alpha(file, matl, database, &mut issues);
        wrap_mode_tiling(file, matl, &assigned, &mut issues);
        texture_formats(file, matl, &nutexbs, &mut issues);
        texture_dimensions(file, matl, &nutexbs, &mut issues);
        missing_textures(file, matl, &texture_names, &mut issues);
        renormal_entries(file, matl, adj, &assigned, mesh, &mut issues);
        duplicate_labels(file, matl, &mut issues);
        sampler_anisotropy(file, matl, &mut issues);
    }

    issues.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.file.cmp(&b.file))
    });
    ModelReport { issues }
}

fn named<'a, T>(files: &'a [(String, Option<T>)], name: &str) -> Option<&'a T> {
    files
        .iter()
        .find(|(file, _)| file == name)
        .and_then(|(_, data)| data.as_ref())
}

fn push(issues: &mut Vec<ModelIssue>, severity: Severity, file: &str, message: String) {
    issues.push(ModelIssue {
        severity,
        file: file.to_string(),
        message,
    });
}

type MeshObject = ssbh_data::mesh_data::MeshObjectData;

/// For each material label, the meshes `model.numdlb` assigns it to.
struct Assignments<'a>(HashMap<String, Vec<&'a MeshObject>>);

impl<'a> Assignments<'a> {
    fn new(matl: &MatlData, modl: Option<&ModlData>, mesh: Option<&'a MeshData>) -> Self {
        let mut map = HashMap::new();
        if let (Some(modl), Some(mesh)) = (modl, mesh) {
            for entry in &matl.entries {
                let label = &entry.material_label;
                let objects = mesh
                    .objects
                    .iter()
                    .filter(|object| {
                        modl.entries.iter().any(|assignment| {
                            assignment.material_label == *label
                                && assignment.mesh_object_name == object.name
                                && assignment.mesh_object_subindex == object.subindex
                        })
                    })
                    .collect();
                map.insert(label.clone(), objects);
            }
        }
        Self(map)
    }

    fn of(&self, label: &str) -> &[&'a MeshObject] {
        self.0.get(label).map_or(&[], Vec::as_slice)
    }
}

fn unreadable_files(model: &ModelFolder, issues: &mut Vec<ModelIssue>) {
    let names = model
        .meshes
        .iter()
        .map(|(n, f)| (n, f.is_none()))
        .chain(model.meshexes.iter().map(|(n, f)| (n, f.is_none())))
        .chain(model.skels.iter().map(|(n, f)| (n, f.is_none())))
        .chain(model.matls.iter().map(|(n, f)| (n, f.is_none())))
        .chain(model.modls.iter().map(|(n, f)| (n, f.is_none())))
        .chain(model.adjs.iter().map(|(n, f)| (n, f.is_none())))
        .chain(model.anims.iter().map(|(n, f)| (n, f.is_none())))
        .chain(model.hlpbs.iter().map(|(n, f)| (n, f.is_none())))
        .chain(model.nutexbs.iter().map(|(n, f)| (n, f.is_none())))
        .chain(model.xmbs.iter().map(|(n, f)| (n, f.is_none())));
    for (name, unreadable) in names {
        if unreadable {
            push(
                issues,
                Severity::Blocking,
                name,
                format!("{name} could not be read. It is damaged or not a valid file of its type, so the game can't load it either."),
            );
        }
    }
}

fn mesh_subindices(mesh: &MeshData, issues: &mut Vec<ModelIssue>) {
    let mut seen: HashMap<&str, HashSet<u64>> = HashMap::new();
    for object in &mesh.objects {
        if !seen
            .entry(&object.name)
            .or_default()
            .insert(object.subindex)
        {
            push(
                issues,
                Severity::Warning,
                "model.numshb",
                format!(
                    "Mesh {:?} repeats subindex {}. Meshes with the same name need unique subindices, or materials and weights can attach to the wrong one.",
                    object.name, object.subindex
                ),
            );
        }
    }
}

fn mesh_weights(mesh: &MeshData, issues: &mut Vec<ModelIssue>) {
    for object in &mesh.objects {
        let mut totals = BTreeMap::<u32, f32>::new();
        let mut counts = BTreeMap::<u32, u32>::new();
        for influence in &object.bone_influences {
            for weight in &influence.vertex_weights {
                *totals.entry(weight.vertex_index).or_default() += weight.vertex_weight;
                *counts.entry(weight.vertex_index).or_default() += 1;
            }
        }
        if totals.values().any(|total| (total - 1.0).abs() > 0.001) {
            push(
                issues,
                Severity::Warning,
                "model.numshb",
                format!("Vertex weights for mesh {:?} don't add up to 1.0. The game assumes they do, so it will deform unevenly.", object.name),
            );
        }
        if counts.values().any(|count| *count > 4) {
            push(
                issues,
                Severity::Warning,
                "model.numshb",
                format!("Mesh {:?} has vertices with more than 4 weights. The game ignores the extra weights, so it may not deform as expected.", object.name),
            );
        }
    }
}

fn modl_entries(
    modl: &ModlData,
    matl: Option<&MatlData>,
    mesh: Option<&MeshData>,
    issues: &mut Vec<ModelIssue>,
) {
    for entry in &modl.entries {
        if let Some(matl) = matl {
            if !matl
                .entries
                .iter()
                .any(|m| m.material_label == entry.material_label)
            {
                push(
                    issues,
                    Severity::Blocking,
                    "model.numdlb",
                    format!(
                        "Mesh {:?} is assigned material {:?}, which model.numatb doesn't have. The mesh will be invisible in game.",
                        entry.mesh_object_name, entry.material_label
                    ),
                );
            }
        }
        if let Some(mesh) = mesh {
            if !mesh.objects.iter().any(|o| {
                o.name == entry.mesh_object_name && o.subindex == entry.mesh_object_subindex
            }) {
                push(
                    issues,
                    Severity::Blocking,
                    "model.numdlb",
                    format!(
                        "model.numdlb assigns a material to mesh {:?} (subindex {}), which model.numshb doesn't have. Check the name and subindex; the mesh meant here will be invisible in game.",
                        entry.mesh_object_name, entry.mesh_object_subindex
                    ),
                );
            }
        }
    }
}

fn shader_labels(
    file: &str,
    matl: &MatlData,
    database: &ShaderDatabase,
    issues: &mut Vec<ModelIssue>,
) {
    for entry in &matl.entries {
        if database.get(&entry.shader_label).is_none() {
            push(
                issues,
                Severity::Blocking,
                file,
                format!(
                    "Material {:?} uses shader {:?}, which isn't one of the game's shaders.",
                    entry.material_label, entry.shader_label
                ),
            );
        }
    }
}

fn required_attributes(
    file: &str,
    matl: &MatlData,
    database: &ShaderDatabase,
    assigned: &Assignments,
    issues: &mut Vec<ModelIssue>,
) {
    for entry in &matl.entries {
        let Some(program) = database.get(&entry.shader_label) else {
            continue;
        };
        for &object in assigned.of(&entry.material_label) {
            let attributes: Vec<String> = object
                .texture_coordinates
                .iter()
                .chain(&object.color_sets)
                .map(|a| a.name.clone())
                .collect();
            let missing = program.missing_required_attributes(&attributes);
            if !missing.is_empty() {
                push(
                    issues,
                    Severity::Warning,
                    file,
                    format!(
                        "Mesh {:?} lacks {} needed by its material {:?}. It will show a yellow checkerboard in game.",
                        object.name,
                        missing.join(", "),
                        entry.material_label
                    ),
                );
            }
        }
    }
}

fn premultiplied_alpha(
    file: &str,
    matl: &MatlData,
    database: &ShaderDatabase,
    issues: &mut Vec<ModelIssue>,
) {
    for entry in &matl.entries {
        if let (Some(program), Some(blend)) = (
            database.get(&entry.shader_label),
            entry.blend_states.first(),
        ) {
            if program.premultiplied && blend.data.source_color == BlendFactor::SourceAlpha {
                push(
                    issues,
                    Severity::Warning,
                    file,
                    format!(
                        "Material {:?} blends with Source Color \"SourceAlpha\", but its shader already premultiplies alpha, so transparency is applied twice. Use \"One\".",
                        entry.material_label
                    ),
                );
            }
        }
    }
}

fn is_repeat(wrap: WrapMode) -> bool {
    matches!(wrap, WrapMode::Repeat | WrapMode::MirroredRepeat)
}

fn wrap_mode_tiling(
    file: &str,
    matl: &MatlData,
    assigned: &Assignments,
    issues: &mut Vec<ModelIssue>,
) {
    for entry in &matl.entries {
        for &object in assigned.of(&entry.material_label) {
            let clamped: Vec<String> = entry
                .samplers
                .iter()
                // Cube maps are meant to clamp.
                .filter(|s| {
                    !matches!(
                        s.param_id,
                        ParamId::Sampler2 | ParamId::Sampler7 | ParamId::Sampler8
                    )
                })
                .filter(|s| !is_repeat(s.data.wraps) || !is_repeat(s.data.wrapt))
                .filter(|s| {
                    // Only the first UV layer each sampler reads; extra layers such as an iris
                    // `uvSet` are normally clamped.
                    let uv = match s.param_id {
                        ParamId::Sampler0
                        | ParamId::Sampler4
                        | ParamId::Sampler5
                        | ParamId::Sampler6
                        | ParamId::Sampler10 => "map1",
                        ParamId::Sampler3 | ParamId::Sampler9 => "bake1",
                        _ => return false,
                    };
                    object
                        .texture_coordinates
                        .iter()
                        .filter(|a| a.name == uv)
                        .any(|a| uv_outside_first_quadrant(&a.data))
                })
                .map(|s| s.param_id.to_string())
                .collect();
            if !clamped.is_empty() {
                push(
                    issues,
                    Severity::Warning,
                    file,
                    format!(
                        "Material {:?} clamps {} on mesh {:?}, whose UVs go well outside 0–1. Use wrap mode Repeat if the texture should tile.",
                        entry.material_label,
                        clamped.join(", "),
                        object.name
                    ),
                );
            }
        }
    }
}

/// UVs slightly outside the first quadrant are normal; well outside means tiling is expected.
fn uv_outside_first_quadrant(data: &VectorData) -> bool {
    let outside = |u: f32, v: f32| !(-0.2..=1.2).contains(&u) || !(-0.2..=1.2).contains(&v);
    match data {
        VectorData::Vector2(values) => values.iter().any(|uv| outside(uv.x, uv.y)),
        VectorData::Vector3(values) => values.iter().any(|uv| outside(uv.x, uv.y)),
        VectorData::Vector4(values) => values.iter().any(|uv| outside(uv.x, uv.y)),
    }
}

/// A texture name the game supplies itself rather than the model folder.
fn is_game_texture(name: &str) -> bool {
    name.starts_with('#') || name.to_ascii_lowercase().starts_with("/common/")
}

fn find_texture<'a>(
    nutexbs: &[(&'a str, &'a NutexbFile)],
    name: &str,
) -> Option<(&'a str, &'a NutexbFile)> {
    nutexbs.iter().copied().find(|(file, _)| {
        Path::new(file)
            .with_extension("")
            .as_os_str()
            .eq_ignore_ascii_case(name)
    })
}

/// Textures that render wrong when sampled as sRGB: normal maps, PRM and similar data.
pub fn expects_srgb(param: ParamId) -> bool {
    !matches!(
        param,
        ParamId::Texture2
            | ParamId::Texture4
            | ParamId::Texture6
            | ParamId::Texture7
            | ParamId::Texture16
    )
}

fn is_srgb(format: NutexbFormat) -> bool {
    matches!(
        format,
        NutexbFormat::R8G8B8A8Srgb
            | NutexbFormat::B8G8R8A8Srgb
            | NutexbFormat::BC1Srgb
            | NutexbFormat::BC2Srgb
            | NutexbFormat::BC3Srgb
            | NutexbFormat::BC7Srgb
    )
}

fn texture_formats(
    file: &str,
    matl: &MatlData,
    nutexbs: &[(&str, &NutexbFile)],
    issues: &mut Vec<ModelIssue>,
) {
    for entry in &matl.entries {
        for texture in &entry.textures {
            let Some((name, nutexb)) = find_texture(nutexbs, &texture.data) else {
                continue;
            };
            let format = nutexb.footer.image_format;
            if expects_srgb(texture.param_id) != is_srgb(format) {
                push(
                    issues,
                    Severity::Warning,
                    file,
                    format!(
                        "{name} is {format:?}, but material {:?} uses it as {}, which {} an sRGB format. Its colors will look too {} in game.",
                        entry.material_label,
                        texture.param_id,
                        if expects_srgb(texture.param_id) { "expects" } else { "doesn't expect" },
                        if expects_srgb(texture.param_id) { "light" } else { "dark" }
                    ),
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dimension {
    Flat,
    Cube,
    Volume,
}

fn texture_dimensions(
    file: &str,
    matl: &MatlData,
    nutexbs: &[(&str, &NutexbFile)],
    issues: &mut Vec<ModelIssue>,
) {
    for entry in &matl.entries {
        for texture in &entry.textures {
            // The game's own textures are not in the folder to measure.
            if is_game_texture(&texture.data) {
                continue;
            }
            let Some((name, nutexb)) = find_texture(nutexbs, &texture.data) else {
                continue;
            };
            let actual = if nutexb.footer.depth > 1 {
                Dimension::Volume
            } else if nutexb.footer.layer_count == 6 {
                Dimension::Cube
            } else {
                Dimension::Flat
            };
            let expected = match texture.param_id {
                ParamId::Texture2 | ParamId::Texture7 | ParamId::Texture8 => Dimension::Cube,
                _ => Dimension::Flat,
            };
            if actual != expected {
                let describe = |d: Dimension| match d {
                    Dimension::Flat => "a 2D texture",
                    Dimension::Cube => "a cube map",
                    Dimension::Volume => "a 3D texture",
                };
                push(
                    issues,
                    Severity::Blocking,
                    file,
                    format!(
                        "Material {:?} puts {name}, {}, in {}, which needs {}.",
                        entry.material_label,
                        describe(actual),
                        texture.param_id,
                        describe(expected)
                    ),
                );
            }
        }
    }
}

fn missing_textures(
    file: &str,
    matl: &MatlData,
    texture_names: &[&str],
    issues: &mut Vec<ModelIssue>,
) {
    for entry in &matl.entries {
        let missing: Vec<&str> = entry
            .textures
            .iter()
            .map(|t| t.data.as_str())
            .filter(|name| !is_game_texture(name))
            .filter(|name| {
                !texture_names.iter().any(|file| {
                    Path::new(file)
                        .with_extension("")
                        .as_os_str()
                        .eq_ignore_ascii_case(name)
                })
            })
            .collect();
        if !missing.is_empty() {
            push(
                issues,
                Severity::Warning,
                file,
                format!(
                    "Material {:?} uses texture{} {} that the folder doesn't have.",
                    entry.material_label,
                    plural(missing.len()),
                    missing.join(", ")
                ),
            );
        }
    }
}

fn renormal_entries(
    file: &str,
    matl: &MatlData,
    adj: Option<&(String, Option<ssbh_data::adj_data::AdjData>)>,
    assigned: &Assignments,
    mesh: Option<&MeshData>,
    issues: &mut Vec<ModelIssue>,
) {
    for entry in matl
        .entries
        .iter()
        .filter(|e| e.material_label.contains("RENORMAL"))
    {
        let Some(adj) = adj else {
            push(
                issues,
                Severity::Warning,
                file,
                format!("Material {:?} is a RENORMAL material, but there is no model.adjb. Its meshes will look faceted in game.", entry.material_label),
            );
            continue;
        };
        let (Some(adj), Some(mesh)) = (adj.1.as_ref(), mesh) else {
            continue;
        };
        for &object in assigned.of(&entry.material_label) {
            let index = mesh.objects.iter().position(|o| std::ptr::eq(o, object));
            if index.is_some_and(|index| !adj.entries.iter().any(|a| a.mesh_object_index == index))
            {
                push(
                    issues,
                    Severity::Warning,
                    "model.adjb",
                    format!(
                        "Mesh {:?} has the RENORMAL material {:?} but no model.adjb entry. It will look faceted in game.",
                        object.name, entry.material_label
                    ),
                );
            }
        }
    }
}

fn duplicate_labels(file: &str, matl: &MatlData, issues: &mut Vec<ModelIssue>) {
    let mut seen = HashSet::new();
    for entry in &matl.entries {
        if !seen.insert(&entry.material_label) {
            push(
                issues,
                Severity::Warning,
                file,
                format!(
                    "Material name {:?} is used twice. model.numdlb assigns materials by name, so meshes can get the wrong one.",
                    entry.material_label
                ),
            );
        }
    }
}

fn sampler_anisotropy(file: &str, matl: &MatlData, issues: &mut Vec<ModelIssue>) {
    for entry in &matl.entries {
        for sampler in &entry.samplers {
            if sampler.data.max_anisotropy != MaxAnisotropy::One
                && (sampler.data.min_filter == MinFilter::Nearest
                    || sampler.data.mag_filter == MagFilter::Nearest)
            {
                push(
                    issues,
                    Severity::Warning,
                    file,
                    format!(
                        "Material {:?} enables anisotropic filtering on {} with Nearest filtering. Turn anisotropy off or use linear filtering.",
                        entry.material_label, sampler.param_id
                    ),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssbh_data::matl_data::{MatlEntryData, TextureParam};
    use ssbh_data::mesh_data::MeshObjectData;
    use ssbh_data::modl_data::ModlEntryData;

    fn folder() -> ModelFolder {
        ModelFolder::load_files(Vec::<(String, PathBuf)>::new())
    }

    fn material(label: &str, shader: &str) -> MatlEntryData {
        MatlEntryData {
            material_label: label.to_string(),
            shader_label: shader.to_string(),
            booleans: Vec::new(),
            floats: Vec::new(),
            vectors: Vec::new(),
            samplers: Vec::new(),
            textures: Vec::new(),
            blend_states: Vec::new(),
            rasterizer_states: Vec::new(),
            uv_transforms: Vec::new(),
        }
    }

    fn mesh_object(name: &str, subindex: u64) -> MeshObjectData {
        MeshObjectData {
            name: name.to_string(),
            subindex,
            ..Default::default()
        }
    }

    fn assignment(mesh: &str, subindex: u64, material: &str) -> ModlEntryData {
        ModlEntryData {
            mesh_object_name: mesh.to_string(),
            mesh_object_subindex: subindex,
            material_label: material.to_string(),
        }
    }

    fn model(
        matl: Vec<MatlEntryData>,
        objects: Vec<MeshObjectData>,
        modl: Vec<ModlEntryData>,
    ) -> ModelFolder {
        let mut folder = folder();
        folder.matls.push((
            "model.numatb".to_string(),
            Some(MatlData {
                major_version: 1,
                minor_version: 6,
                entries: matl,
            }),
        ));
        folder.meshes.push((
            "model.numshb".to_string(),
            Some(MeshData {
                major_version: 1,
                minor_version: 10,
                objects,
            }),
        ));
        folder.modls.push((
            "model.numdlb".to_string(),
            Some(ModlData {
                major_version: 1,
                minor_version: 0,
                model_name: String::new(),
                skeleton_file_name: String::new(),
                material_file_names: Vec::new(),
                animation_file_name: None,
                mesh_file_name: String::new(),
                entries: modl,
            }),
        ));
        folder
    }

    fn messages(report: &ModelReport, severity: Severity) -> Vec<String> {
        report
            .issues
            .iter()
            .filter(|i| i.severity == severity)
            .map(|i| i.message.clone())
            .collect()
    }

    #[test]
    fn a_valid_assignment_has_no_blocking_issues() {
        let model = model(
            vec![material("body", "SFX_PBS_0100000008008269_opaque")],
            vec![mesh_object("Body", 0)],
            vec![assignment("Body", 0, "body")],
        );
        let report = check_model(&model, shader_database());
        assert!(!report.has_blocking(), "{:?}", report.issues);
    }

    #[test]
    fn invisible_meshes_and_unknown_shaders_block() {
        let model = model(
            vec![material("body", "SFX_NOT_A_SHADER")],
            vec![mesh_object("Body", 0)],
            vec![
                assignment("Body", 0, "missing"),
                assignment("Ghost", 0, "body"),
            ],
        );
        let report = check_model(&model, shader_database());
        let blocking = messages(&report, Severity::Blocking);
        assert_eq!(blocking.len(), 3, "{blocking:?}");
        assert!(blocking
            .iter()
            .any(|m| m.contains("\"missing\"") && m.contains("invisible")));
        assert!(blocking.iter().any(|m| m.contains("\"Ghost\"")));
        assert!(blocking.iter().any(|m| m.contains("SFX_NOT_A_SHADER")));
        assert_eq!(
            report.summary().as_deref(),
            Some("3 problems that break the model")
        );
    }

    #[test]
    fn duplicates_and_missing_textures_only_warn() {
        let mut body = material("body", "SFX_PBS_0100000008008269_opaque");
        body.textures.push(TextureParam {
            param_id: ParamId::Texture0,
            data: "def_body_col".to_string(),
        });
        body.textures.push(TextureParam {
            param_id: ParamId::Texture10,
            data: "/common/shader/sfxpbs/default_white".to_string(),
        });
        let model = model(
            vec![body.clone(), body],
            vec![mesh_object("Body", 0), mesh_object("Body", 0)],
            vec![assignment("Body", 0, "body")],
        );
        let report = check_model(&model, shader_database());
        assert!(!report.has_blocking(), "{:?}", report.issues);
        let warnings = messages(&report, Severity::Warning);
        assert!(warnings.iter().any(|m| m.contains("used twice")));
        assert!(warnings.iter().any(|m| m.contains("repeats subindex 0")));
        assert!(warnings.iter().any(|m| m.contains("def_body_col")));
        assert!(
            !warnings.iter().any(|m| m.contains("default_white")),
            "game textures count as present"
        );
    }

    #[test]
    fn an_unreadable_file_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.numatb");
        std::fs::write(&path, b"not a material").unwrap();
        let report = check_files([("model.numatb".to_string(), path)]);
        assert_eq!(messages(&report, Severity::Blocking).len(), 1);
    }

    /// Every vanilla model in a dump passes without a blocking issue; a check that flags the
    /// game's own models would block every preview.
    #[test]
    #[ignore = "needs VISIONARY_VANILLA (an extracted data root)"]
    fn vanilla_body_models_have_no_blocking_issues() {
        let root = PathBuf::from(std::env::var_os("VISIONARY_VANILLA").unwrap());
        let fighters = std::env::var("VISIONARY_CHECK_FIGHTERS")
            .unwrap_or_else(|_| "mario,link,pacman,kirby,bayonetta,murabito".to_string());
        let mut checked = 0;
        for fighter in fighters.split(',') {
            for slot in 0..8 {
                let dir = root.join(format!("fighter/{fighter}/model/body/c{slot:02}"));
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                let files: Vec<(String, PathBuf)> = entries
                    .flatten()
                    .filter_map(|e| Some((e.file_name().to_str()?.to_string(), e.path())))
                    .collect();
                let report = check_files(files);
                let blocking: Vec<_> = report.blocking().collect();
                assert!(blocking.is_empty(), "{fighter} c{slot:02}: {blocking:#?}");
                eprintln!("{fighter} c{slot:02}: {:?}", report.summary());
                if std::env::var_os("VISIONARY_CHECK_VERBOSE").is_some() {
                    for issue in &report.issues {
                        eprintln!("  {:?} {}: {}", issue.severity, issue.file, issue.message);
                    }
                }
                checked += 1;
            }
        }
        assert!(checked > 0);
    }
}
