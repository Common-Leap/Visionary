//! Effect models as files a person can edit: a model of an effect file's pool out to GLB or OBJ,
//! and one of their own in its place.
//!
//! Every model of the game's effect files is one interleaved vertex buffer and one list of
//! triangles. A replacement keeps the numbers each vertex has (a part's shader was made for
//! them), and stores its positions and texture coordinates in formats that hold any value.

use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};
use effect_library::ptcl_file::PrimitiveDescriptor;

mod glb;
mod obj;

/// What one vertex of a model is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Across and down the texture.
    pub uv: [f32; 2],
    pub uv2: Option<[f32; 2]>,
    /// Red, green, blue and opacity.
    pub colour: [f32; 4],
}

/// A model as it is edited: its vertices, and three of them to a triangle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<[u32; 3]>,
}

/// A model of an effect file, for its row.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    /// What parts name it by.
    pub id: u64,
    pub name: String,
    pub vertices: usize,
    pub triangles: usize,
    /// Which numbers its vertices have. A part's shader is made for one set of them.
    pub attributes: Vec<String>,
}

/// The models of one effect file.
#[derive(Debug, Clone)]
pub struct ModelPool {
    binary: Vec<u8>,
    descriptors: Vec<PrimitiveDescriptor>,
    infos: std::sync::OnceLock<Vec<ModelInfo>>,
}

impl ModelPool {
    pub fn of(ptcl: &effect_library::PtclFile) -> Option<Self> {
        let info = ptcl.primitive_info.as_ref()?;
        Some(Self {
            binary: info.binary_data.clone().filter(|b| !b.is_empty())?,
            descriptors: info.descriptors.clone(),
            infos: std::sync::OnceLock::new(),
        })
    }

    pub fn infos(&self) -> &[ModelInfo] {
        self.infos.get_or_init(|| {
            let Ok(file) = effect_library::bfres::load::load_from_bytes(&self.binary) else {
                return Vec::new();
            };
            self.descriptors
                .iter()
                .zip(file.models.values())
                .filter_map(|(descriptor, model)| {
                    let buffer = model.vertex_buffers.first()?;
                    let mesh = model.shapes.values().next()?.meshes.first()?;
                    Some(ModelInfo {
                        id: descriptor.id,
                        name: model.name.clone(),
                        vertices: buffer.vertex_count as usize,
                        triangles: mesh.index_count as usize / 3,
                        attributes: buffer.attributes.keys().cloned().collect(),
                    })
                })
                .collect()
        })
    }

    pub fn info(&self, id: u64) -> Option<&ModelInfo> {
        self.infos().iter().find(|info| info.id == id)
    }

    /// A model of the pool, to edit.
    pub fn model(&self, id: u64) -> Result<Model> {
        read(&self.binary, &self.descriptors, id)
    }

    /// Whether `model` can take a model's place, and what it would be.
    pub fn check(&self, id: u64, model: &Model) -> Result<ModelInfo> {
        let binary = replace(&self.binary, &self.descriptors, id, model)?;
        let pool = Self {
            binary,
            descriptors: self.descriptors.clone(),
            infos: std::sync::OnceLock::new(),
        };
        pool.info(id)
            .cloned()
            .ok_or_else(|| anyhow!("the model could not be read back"))
    }
}

/// A model file, read by its extension.
pub fn load(path: &std::path::Path) -> Result<Model> {
    let bytes = std::fs::read(path).with_context(|| format!("couldn't read {}", path.display()))?;
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let model = match extension.as_str() {
        "obj" => obj::read(&String::from_utf8_lossy(&bytes))?,
        "glb" => glb::read(&bytes)?,
        _ => bail!("only .glb and .obj models can be read"),
    };
    if model.triangles.is_empty() {
        bail!("the file has no triangles");
    }
    Ok(model)
}

/// `model` as a file's bytes, by the file's extension.
pub fn save(path: &std::path::Path, name: &str, model: &Model) -> Result<()> {
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let bytes = match extension.as_str() {
        "obj" => obj::write(name, model).into_bytes(),
        "glb" => glb::write(name, model),
        _ => bail!("a model is saved as .glb or .obj"),
    };
    std::fs::write(path, bytes).with_context(|| format!("couldn't write {}", path.display()))
}

fn index_of(descriptors: &[PrimitiveDescriptor], id: u64) -> Result<usize> {
    descriptors
        .iter()
        .position(|d| d.id == id)
        .ok_or_else(|| anyhow!("the effect file has no such model"))
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let man = f32::from(bits & 0x3ff);
    sign * match exp {
        0 => man * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + man / 1024.0) * 2f32.powi(exp - 15),
    }
}

fn f32_to_half(v: f32) -> u16 {
    let sign = if v.is_sign_negative() { 0x8000 } else { 0 };
    let v = v.abs();
    if v.is_nan() || v >= 65520.0 {
        return sign | 0x7bff;
    }
    if v < 2f32.powi(-14) {
        return sign | (v * 2f32.powi(24)).round() as u16;
    }
    let exp = v.log2().floor() as i32;
    let man = ((v / 2f32.powi(exp) - 1.0) * 1024.0).round() as u16;
    sign | ((((exp + 15) as u16) << 10) + man)
}

/// Vertex formats the game's effect models have.
const FLOAT3: u16 = 0x1805;
const FLOAT2: u16 = 0x1705;
const HALF4: u16 = 0x1505;
const HALF2: u16 = 0x1205;
const UNORM16_2: u16 = 0x1201;
const SNORM16_2: u16 = 0x1202;
const UNORM8_2: u16 = 0x0901;
const SNORM8_2: u16 = 0x0902;
const UNORM8_4: u16 = 0x0b01;
const SNORM8_4: u16 = 0x0b02;
const SNORM10: u16 = 0x0e02;

fn format_size(format: u16) -> Option<usize> {
    Some(match format {
        FLOAT3 => 12,
        FLOAT2 | HALF4 => 8,
        HALF2 | UNORM16_2 | SNORM16_2 | UNORM8_4 | SNORM8_4 | SNORM10 => 4,
        UNORM8_2 | SNORM8_2 => 2,
        _ => return None,
    })
}

fn decode(format: u16, b: &[u8]) -> Option<[f32; 4]> {
    let b = b.get(..format_size(format)?)?;
    let f = |i: usize| f32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let h = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let i8n = |i: usize| (f32::from(b[i] as i8) / 127.0).max(-1.0);
    let u8n = |i: usize| f32::from(b[i]) / 255.0;
    Some(match format {
        FLOAT3 => [f(0), f(4), f(8), 1.0],
        FLOAT2 => [f(0), f(4), 0.0, 0.0],
        HALF4 => [0, 2, 4, 6].map(|i| half_to_f32(h(i))),
        HALF2 => [half_to_f32(h(0)), half_to_f32(h(2)), 0.0, 0.0],
        UNORM16_2 => [
            f32::from(h(0)) / 65535.0,
            f32::from(h(2)) / 65535.0,
            0.0,
            0.0,
        ],
        SNORM16_2 => {
            let n = |i: usize| (f32::from(h(i) as i16) / 32767.0).max(-1.0);
            [n(0), n(2), 0.0, 0.0]
        }
        UNORM8_2 => [u8n(0), u8n(1), 0.0, 0.0],
        SNORM8_2 => [i8n(0), i8n(1), 0.0, 0.0],
        UNORM8_4 => [u8n(0), u8n(1), u8n(2), u8n(3)],
        SNORM8_4 => [i8n(0), i8n(1), i8n(2), i8n(3)],
        SNORM10 => {
            let w = u32::from_le_bytes(b.try_into().ok()?);
            let n = |shift: u32| (((w >> shift) << 22) as i32 >> 22) as f32 / 511.0;
            [n(0), n(10), n(20), 0.0]
        }
        _ => return None,
    })
}

fn encode(format: u16, v: [f32; 4], out: &mut Vec<u8>) -> Result<()> {
    let i8n = |v: f32| (v.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8;
    let u8n = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    match format {
        FLOAT3 => v[..3]
            .iter()
            .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
        FLOAT2 => v[..2]
            .iter()
            .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
        HALF4 => v
            .iter()
            .for_each(|v| out.extend_from_slice(&f32_to_half(*v).to_le_bytes())),
        HALF2 => v[..2]
            .iter()
            .for_each(|v| out.extend_from_slice(&f32_to_half(*v).to_le_bytes())),
        UNORM8_4 => out.extend(v.map(u8n)),
        SNORM8_4 => out.extend(v.map(i8n)),
        SNORM10 => {
            let n = |v: f32| ((v.clamp(-1.0, 1.0) * 511.0).round() as i32 as u32) & 0x3ff;
            let w = n(v[0]) | n(v[1]) << 10 | n(v[2]) << 20;
            out.extend_from_slice(&w.to_le_bytes());
        }
        _ => bail!("the model stores its vertices in a way Visionary does not know ({format:#x})"),
    }
    Ok(())
}

fn read(binary: &[u8], descriptors: &[PrimitiveDescriptor], id: u64) -> Result<Model> {
    let index = index_of(descriptors, id)?;
    let file = effect_library::bfres::load::load_from_bytes(binary)
        .map_err(|e| anyhow!("reading the models: {e}"))?;
    let (_, model) = file
        .models
        .get_index(index)
        .context("the effect file has no such model")?;
    let buffer = model
        .vertex_buffers
        .first()
        .context("the model has no vertices")?;
    let mesh = model
        .shapes
        .values()
        .next()
        .and_then(|s| s.meshes.first())
        .context("the model has no triangles")?;
    let value = |name: &str, vertex: usize| -> Option<[f32; 4]> {
        let attribute = buffer.attributes.get(name)?;
        let at = usize::from(attribute.buffer_index);
        let stride = *buffer.buffer_strides.get(at)? as usize;
        let bytes = buffer
            .buffers
            .get(at)?
            .get(vertex * stride + usize::from(attribute.offset)..)?;
        decode(attribute.format, bytes)
    };
    let vertices = (0..buffer.vertex_count as usize)
        .map(|i| {
            let p = value("_p0", i).context("the model has no positions")?;
            let n = value("_n0", i).unwrap_or([0.0, 0.0, 1.0, 0.0]);
            let uv = value("_u0", i).unwrap_or_default();
            Ok(Vertex {
                position: [p[0], p[1], p[2]],
                normal: [n[0], n[1], n[2]],
                uv: [uv[0], uv[1]],
                uv2: value("_u1", i).map(|uv| [uv[0], uv[1]]),
                colour: value("_c0", i).unwrap_or([1.0; 4]),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let wide = mesh.index_format == 2;
    let indices: Vec<u32> = if wide {
        mesh.index_data
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    } else {
        mesh.index_data
            .chunks_exact(2)
            .map(|c| u32::from(u16::from_le_bytes([c[0], c[1]])))
            .collect()
    };
    let triangles = indices
        .chunks_exact(3)
        .take(mesh.index_count as usize / 3)
        .map(|t| [t[0], t[1], t[2]].map(|i| i + mesh.first_vertex))
        .collect();
    Ok(Model {
        vertices,
        triangles,
    })
}

/// `binary` with `model` in place of one of its models.
fn replace(
    binary: &[u8],
    descriptors: &[PrimitiveDescriptor],
    id: u64,
    model: &Model,
) -> Result<Vec<u8>> {
    if model.vertices.len() > usize::from(u16::MAX) {
        bail!(
            "the model has {} vertices; an effect model has 65,535 at most",
            model.vertices.len()
        );
    }
    let index = index_of(descriptors, id)?;
    let mut models: Vec<(String, effect_library::bfres::Model)> =
        effect_library::bfres::load::load_from_bytes(binary)
            .map_err(|e| anyhow!("reading the models: {e}"))?
            .models
            .into_iter()
            .collect();
    let (_, target) = models
        .get_mut(index)
        .context("the effect file has no such model")?;
    if target.vertex_buffers.len() != 1 || target.shapes.len() != 1 {
        bail!("the model is in several pieces, which Visionary can't replace");
    }
    let buffer = &mut target.vertex_buffers[0];
    let shape = target
        .shapes
        .values_mut()
        .next()
        .context("the model has no triangles")?;
    let mesh = shape
        .meshes
        .first_mut()
        .context("the model has no triangles")?;

    // The same numbers a vertex as before, in the same order.
    let mut stride = 0;
    let mut layout: Vec<(String, u16)> = Vec::new();
    for attribute in buffer.attributes.values_mut() {
        let format = match attribute.format {
            HALF4 if attribute.name == "_p0" => FLOAT3,
            UNORM16_2 | SNORM16_2 | UNORM8_2 | SNORM8_2 => HALF2,
            other => other,
        };
        let size = format_size(format).with_context(|| {
            format!("the model stores its vertices in a way Visionary does not know ({format:#x})")
        })?;
        attribute.buffer_index = 0;
        attribute.offset = stride as u16;
        attribute.format = format;
        layout.push((attribute.name.clone(), format));
        stride += size;
    }
    let tangents = layout
        .iter()
        .any(|(name, _)| name == "_t0" || name == "_b0")
        .then(|| tangents(model));
    let mut data = Vec::with_capacity(stride * model.vertices.len());
    for (i, vertex) in model.vertices.iter().enumerate() {
        for (name, format) in &layout {
            let [x, y, z] = vertex.position;
            let [nx, ny, nz] = vertex.normal;
            let value = match name.as_str() {
                "_p0" => [x, y, z, 1.0],
                "_n0" => [nx, ny, nz, 0.0],
                "_u0" => [vertex.uv[0], vertex.uv[1], 0.0, 0.0],
                "_u1" => {
                    let uv = vertex.uv2.unwrap_or(vertex.uv);
                    [uv[0], uv[1], 0.0, 0.0]
                }
                "_c0" => vertex.colour,
                "_t0" => tangents.as_ref().map_or([0.0; 4], |t| t[i].0),
                "_b0" => tangents.as_ref().map_or([0.0; 4], |t| t[i].1),
                _ => [0.0; 4],
            };
            encode(*format, value, &mut data)?;
        }
    }
    buffer.vertex_count = model.vertices.len() as u32;
    buffer.buffer_strides = vec![stride as u32];
    buffer.buffer_sizes = vec![data.len() as u32];
    buffer.buffers = vec![data];

    mesh.index_format = 1;
    mesh.first_vertex = 0;
    mesh.index_count = model.triangles.len() as u32 * 3;
    mesh.index_data = model
        .triangles
        .iter()
        .flatten()
        .flat_map(|i| (*i as u16).to_le_bytes())
        .collect();
    for (k, part) in mesh.sub_meshes.iter_mut().enumerate() {
        part.offset = 0;
        part.count = if k == 0 { mesh.index_count } else { 0 };
    }

    let (mut low, mut high) = ([f32::MAX; 3], [f32::MIN; 3]);
    for vertex in &model.vertices {
        for k in 0..3 {
            low[k] = low[k].min(vertex.position[k]);
            high[k] = high[k].max(vertex.position[k]);
        }
    }
    let centre: [f32; 3] = std::array::from_fn(|k| (low[k] + high[k]) / 2.0);
    for bound in &mut shape.sub_mesh_boundings {
        let (c, e) = (&mut bound.center, &mut bound.extent);
        (c.x, c.y, c.z) = centre.into();
        let half: [f32; 3] = std::array::from_fn(|k| (high[k] - low[k]) / 2.0);
        (e.x, e.y, e.z) = half.into();
    }
    let radius = model
        .vertices
        .iter()
        .map(|v| {
            (0..3)
                .map(|k| (v.position[k] - centre[k]).powi(2))
                .sum::<f32>()
                .sqrt()
        })
        .fold(0.0, f32::max);
    shape.radius_array.iter_mut().for_each(|r| *r = radius);
    effect_library::bfres::ResFile::rebuild_from_base_and_models(binary, None, &models)
        .context("writing the models")
}

/// Each vertex's tangent and bitangent, from how its triangles' textures run.
fn tangents(model: &Model) -> Vec<([f32; 4], [f32; 4])> {
    use glam::{Vec2, Vec3};
    let mut along = vec![(Vec3::ZERO, Vec3::ZERO); model.vertices.len()];
    for triangle in &model.triangles {
        let [a, b, c] = triangle.map(|i| &model.vertices[i as usize]);
        let p = |v: &Vertex| Vec3::from(v.position);
        let t = |v: &Vertex| Vec2::from(v.uv);
        let (e1, e2) = (p(b) - p(a), p(c) - p(a));
        let (d1, d2) = (t(b) - t(a), t(c) - t(a));
        let area = d1.x * d2.y - d2.x * d1.y;
        if area.abs() < 1e-12 {
            continue;
        }
        let tangent = (e1 * d2.y - e2 * d1.y) / area;
        let bitangent = (e2 * d1.x - e1 * d2.x) / area;
        for i in triangle {
            along[*i as usize].0 += tangent;
            along[*i as usize].1 += bitangent;
        }
    }
    along
        .into_iter()
        .zip(&model.vertices)
        .map(|((t, b), vertex)| {
            let n = Vec3::from(vertex.normal);
            let t = (t - n * n.dot(t)).normalize_or(n.any_orthonormal_vector());
            let side = if n.cross(t).dot(b) < 0.0 { -1.0 } else { 1.0 };
            let b = n.cross(t) * side;
            ([t.x, t.y, t.z, side], [b.x, b.y, b.z, side])
        })
        .collect()
}

/// Normals for the vertices that have none, from the triangles they are in.
fn fill_normals(model: &mut Model, missing: &[bool]) {
    use glam::Vec3;
    let mut sums = vec![Vec3::ZERO; model.vertices.len()];
    for triangle in &model.triangles {
        let [a, b, c] = triangle.map(|i| Vec3::from(model.vertices[i as usize].position));
        let normal = (b - a).cross(c - a);
        for i in triangle {
            sums[*i as usize] += normal;
        }
    }
    for ((vertex, sum), missing) in model.vertices.iter_mut().zip(sums).zip(missing) {
        if *missing {
            vertex.normal = sum.normalize_or(Vec3::Z).into();
        }
    }
}

/// Vertices gathered one corner at a time, the same ones once.
#[derive(Default)]
struct Corners {
    model: Model,
    missing_normals: Vec<bool>,
    seen: HashMap<Vec<u32>, u32>,
}

impl Corners {
    fn vertex(&mut self, vertex: Vertex, has_normal: bool) -> u32 {
        let normal = if has_normal { vertex.normal } else { [0.0; 3] };
        let key: Vec<u32> = vertex
            .position
            .iter()
            .chain(&normal)
            .chain(&vertex.uv)
            .chain(vertex.uv2.iter().flatten())
            .chain(&vertex.colour)
            .map(|v| v.to_bits())
            .collect();
        let next = self.model.vertices.len() as u32;
        let at = *self.seen.entry(key).or_insert(next);
        if at == next {
            self.model.vertices.push(vertex);
            self.missing_normals.push(!has_normal);
        }
        at
    }

    fn finish(mut self) -> Model {
        fill_normals(&mut self.model, &self.missing_normals);
        self.model
    }
}

/// Put the models the user replaced in an effect file's pool. A model the file does not have is
/// a warning, as a texture it does not have is.
pub fn apply(
    ptcl: Option<&mut effect_library::PtclFile>,
    imports: &[crate::mod_project::ModelImport],
    warnings: &mut Vec<String>,
) -> Result<()> {
    let Some(info) = ptcl.and_then(|ptcl| ptcl.primitive_info.as_mut()) else {
        warnings.extend(imports.iter().map(|i| format!("model:{}", i.name)));
        return Ok(());
    };
    for import in imports {
        let has = info.descriptors.iter().any(|d| d.id == import.model_id);
        let Some(binary) = info.binary_data.as_ref().filter(|_| has) else {
            warnings.push(format!("model:{}", import.name));
            continue;
        };
        let model = load(std::path::Path::new(&import.path))
            .with_context(|| format!("model '{}'", import.name))?;
        let replaced = replace(binary, &info.descriptors, import.model_id, &model)
            .with_context(|| format!("putting {} in place of '{}'", import.path, import.name))?;
        info.binary_data = Some(replaced);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
