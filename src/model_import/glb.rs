//! Binary glTF: everything an effect model's vertices have, with opacity.

use anyhow::{anyhow, bail, Context, Result};
use glam::{Mat3, Mat4, Quat, Vec3};
use serde_json::{json, Value};

use super::{Corners, Model, Vertex};

const MAGIC: u32 = 0x4654_6c67;
const JSON: u32 = 0x4e4f_534a;
const BIN: u32 = 0x004e_4942;

const FLOAT: u64 = 5126;
const UNSIGNED_INT: u64 = 5125;

pub(super) fn write(name: &str, model: &Model) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let (mut views, mut accessors) = (Vec::new(), Vec::new());
    let mut accessor = |data: Vec<f32>, width: usize, bounds: bool| -> usize {
        let (at, count) = (bin.len(), data.len() / width);
        bin.extend(data.iter().flat_map(|v| v.to_le_bytes()));
        views.push(json!({"buffer": 0, "byteOffset": at, "byteLength": data.len() * 4}));
        let kind = ["SCALAR", "VEC2", "VEC3", "VEC4"][width - 1];
        let mut entry = json!({
            "bufferView": views.len() - 1, "componentType": FLOAT, "count": count, "type": kind
        });
        if bounds {
            let edge = |pick: fn(f32, f32) -> f32, start: f32| -> Vec<f32> {
                (0..width)
                    .map(|k| {
                        data.iter()
                            .skip(k)
                            .step_by(width)
                            .copied()
                            .fold(start, pick)
                    })
                    .collect()
            };
            entry["min"] = json!(edge(f32::min, f32::MAX));
            entry["max"] = json!(edge(f32::max, f32::MIN));
        }
        accessors.push(entry);
        accessors.len() - 1
    };
    let v = &model.vertices;
    let mut attributes = serde_json::Map::new();
    let mut put = |key: &str, at: usize| attributes.insert(key.into(), json!(at));
    put(
        "POSITION",
        accessor(v.iter().flat_map(|v| v.position).collect(), 3, true),
    );
    put(
        "NORMAL",
        accessor(v.iter().flat_map(|v| v.normal).collect(), 3, false),
    );
    put(
        "TEXCOORD_0",
        accessor(v.iter().flat_map(|v| v.uv).collect(), 2, false),
    );
    if v.iter().all(|v| v.uv2.is_some()) {
        let uv2 = v.iter().filter_map(|v| v.uv2).flatten().collect();
        put("TEXCOORD_1", accessor(uv2, 2, false));
    }
    put(
        "COLOR_0",
        accessor(v.iter().flat_map(|v| v.colour).collect(), 4, false),
    );
    let at = bin.len();
    bin.extend(
        model
            .triangles
            .iter()
            .flatten()
            .flat_map(|i| i.to_le_bytes()),
    );
    views.push(json!({"buffer": 0, "byteOffset": at, "byteLength": bin.len() - at}));
    accessors.push(json!({
        "bufferView": views.len() - 1, "componentType": UNSIGNED_INT,
        "count": model.triangles.len() * 3, "type": "SCALAR"
    }));
    let document = json!({
        "asset": {"version": "2.0", "generator": "Visionary"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": name}],
        "meshes": [{
            "name": name,
            "primitives": [{"attributes": attributes, "indices": accessors.len() - 1}]
        }],
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": bin.len()}],
    });
    let mut text = document.to_string().into_bytes();
    text.resize(text.len().next_multiple_of(4), b' ');
    bin.resize(bin.len().next_multiple_of(4), 0);
    let mut out = Vec::new();
    let total = 12 + 8 + text.len() + 8 + bin.len();
    for word in [MAGIC, 2, total as u32, text.len() as u32, JSON] {
        out.extend(word.to_le_bytes());
    }
    out.extend(text);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(BIN.to_le_bytes());
    out.extend(bin);
    out
}

struct Document<'a> {
    json: Value,
    bin: &'a [u8],
}

impl Document<'_> {
    fn item(&self, list: &str, index: u64) -> Result<&Value> {
        self.json[list]
            .get(index as usize)
            .ok_or_else(|| anyhow!("the file names {list} {index}, which it does not have"))
    }

    /// An accessor's numbers, `width` to a row.
    fn numbers(&self, index: u64) -> Result<(Vec<f64>, usize)> {
        let accessor = self.item("accessors", index)?;
        if !accessor["sparse"].is_null() {
            bail!("sparse accessors can't be read");
        }
        let width = match accessor["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            _ => bail!("an accessor of matrices can't be read"),
        };
        let kind = accessor["componentType"].as_u64().unwrap_or(0);
        let size = match kind {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            FLOAT | UNSIGNED_INT => 4,
            _ => bail!("numbers of type {kind} can't be read"),
        };
        let normalized = accessor["normalized"].as_bool().unwrap_or(false);
        let count = accessor["count"].as_u64().unwrap_or(0) as usize;
        let view = accessor["bufferView"].as_u64().context(
            "the file is compressed, which can't be read; export it without compression",
        )?;
        let view = self.item("bufferViews", view)?;
        if view["buffer"].as_u64().unwrap_or(0) != 0 {
            bail!("the file keeps its numbers in another file");
        }
        let offset = |v: &Value| v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let start = offset(view) + offset(accessor);
        let stride = view["byteStride"]
            .as_u64()
            .map_or(size * width, |s| s as usize);
        let mut out = Vec::new();
        for row in 0..count {
            for column in 0..width {
                let at = start + row * stride + column * size;
                let b = self
                    .bin
                    .get(at..at + size)
                    .context("the file ends before its numbers do")?;
                let (value, full) = match kind {
                    5120 => (f64::from(b[0] as i8), 127.0),
                    5121 => (f64::from(b[0]), 255.0),
                    5122 => (f64::from(i16::from_le_bytes([b[0], b[1]])), 32767.0),
                    5123 => (f64::from(u16::from_le_bytes([b[0], b[1]])), 65535.0),
                    FLOAT => (f64::from(f32::from_le_bytes(b.try_into()?)), 1.0),
                    _ => (f64::from(u32::from_le_bytes(b.try_into()?)), 1.0),
                };
                let value = if normalized {
                    (value / full).max(-1.0)
                } else {
                    value
                };
                if !value.is_finite() {
                    bail!("the file has a number that is not one");
                }
                out.push(value);
            }
        }
        Ok((out, width))
    }

    fn rows<const N: usize>(&self, index: &Value, fill: f32) -> Result<Option<Vec<[f32; N]>>> {
        let Some(index) = index.as_u64() else {
            return Ok(None);
        };
        let (numbers, width) = self.numbers(index)?;
        Ok(Some(
            numbers
                .chunks_exact(width)
                .map(|row| std::array::from_fn(|k| row.get(k).map_or(fill, |v| *v as f32)))
                .collect(),
        ))
    }
}

/// Where a node puts what is in it.
fn placement(node: &Value) -> Mat4 {
    let numbers = |key: &str| -> Option<Vec<f32>> {
        node[key]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
    };
    if let Some(m) = numbers("matrix").and_then(|m| <[f32; 16]>::try_from(m).ok()) {
        return Mat4::from_cols_array(&m);
    }
    let vec3 = |key: &str, rest: f32| {
        numbers(key)
            .and_then(|v| <[f32; 3]>::try_from(v).ok())
            .map_or(Vec3::splat(rest), Vec3::from)
    };
    let turn = numbers("rotation")
        .and_then(|v| <[f32; 4]>::try_from(v).ok())
        .map_or(Quat::IDENTITY, Quat::from_array);
    Mat4::from_scale_rotation_translation(vec3("scale", 1.0), turn, vec3("translation", 0.0))
}

pub(super) fn read(bytes: &[u8]) -> Result<Model> {
    let word = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    if word(0) != Some(MAGIC) || word(4) != Some(2) {
        bail!("the file is not a binary glTF 2.0 model");
    }
    let (mut json, mut bin) = (None, &[][..]);
    let mut at = 12;
    while let (Some(size), Some(kind)) = (word(at), word(at + 4)) {
        let chunk = bytes
            .get(at + 8..at + 8 + size as usize)
            .context("the file is cut short")?;
        match kind {
            JSON => json = Some(serde_json::from_slice(chunk).context("the file's contents")?),
            BIN => bin = chunk,
            _ => {}
        }
        at += 8 + size as usize;
    }
    let document = Document {
        json: json.context("the file has no contents")?,
        bin,
    };
    let mut corners = Corners::default();
    let scene = document.json["scene"].as_u64().unwrap_or(0);
    let mut open: Vec<(u64, Mat4, usize)> = match document.json["scenes"].get(scene as usize) {
        Some(scene) => scene["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| Some((n.as_u64()?, Mat4::IDENTITY, 0)))
            .collect(),
        None => Vec::new(),
    };
    open.reverse();
    while let Some((index, parent, depth)) = open.pop() {
        if depth > 64 {
            bail!("the file's nodes are in each other");
        }
        let node = document.item("nodes", index)?;
        let place = parent * placement(node);
        if let Some(mesh) = node["mesh"].as_u64() {
            let mesh = document.item("meshes", mesh)?;
            for primitive in mesh["primitives"].as_array().into_iter().flatten() {
                read_primitive(&document, primitive, place, &mut corners)?;
            }
        }
        for child in node["children"].as_array().into_iter().flatten().rev() {
            if let Some(child) = child.as_u64() {
                open.push((child, place, depth + 1));
            }
        }
    }
    Ok(corners.finish())
}

fn read_primitive(
    document: &Document,
    primitive: &Value,
    place: Mat4,
    corners: &mut Corners,
) -> Result<()> {
    if primitive["mode"].as_u64().unwrap_or(4) != 4 {
        return Ok(());
    }
    let attributes = &primitive["attributes"];
    let Some(positions) = document.rows::<3>(&attributes["POSITION"], 0.0)? else {
        return Ok(());
    };
    let normals = document.rows::<3>(&attributes["NORMAL"], 0.0)?;
    let uvs = document.rows::<2>(&attributes["TEXCOORD_0"], 0.0)?;
    let uvs2 = document.rows::<2>(&attributes["TEXCOORD_1"], 0.0)?;
    let colours = document.rows::<4>(&attributes["COLOR_0"], 1.0)?;
    let indices: Vec<usize> = match primitive["indices"].as_u64() {
        Some(index) => document
            .numbers(index)?
            .0
            .into_iter()
            .map(|v| v as usize)
            .collect(),
        None => (0..positions.len()).collect(),
    };
    let turn = Mat3::from_mat4(place);
    // A mirrored part is inside out: its corners run the other way round.
    let mirrored = turn.determinant() < 0.0;
    let turn = if turn.determinant().abs() > 1e-12 {
        turn.inverse().transpose()
    } else {
        Mat3::IDENTITY
    };
    let mut faces = Vec::new();
    for &i in &indices {
        let position = positions
            .get(i)
            .context("a triangle names a vertex the file does not have")?;
        let normal = normals.as_ref().and_then(|n| n.get(i));
        let vertex = Vertex {
            position: place.transform_point3(Vec3::from(*position)).into(),
            normal: normal.map_or([0.0; 3], |n| {
                (turn * Vec3::from(*n)).normalize_or_zero().into()
            }),
            uv: uvs
                .as_ref()
                .and_then(|u| u.get(i))
                .copied()
                .unwrap_or_default(),
            uv2: uvs2.as_ref().and_then(|u| u.get(i)).copied(),
            colour: colours
                .as_ref()
                .and_then(|c| c.get(i))
                .copied()
                .unwrap_or([1.0; 4]),
        };
        faces.push(corners.vertex(vertex, normal.is_some()));
    }
    for t in faces.chunks_exact(3) {
        corners.model.triangles.push(if mirrored {
            [t[0], t[2], t[1]]
        } else {
            [t[0], t[1], t[2]]
        });
    }
    Ok(())
}
