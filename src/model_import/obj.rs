//! Wavefront OBJ: positions with the colours some programs write after them, texture coordinates
//! and normals. It holds no opacity and no second texture coordinate.

use std::fmt::Write;

use anyhow::{anyhow, bail, Result};

use super::{Corners, Model, Vertex};

pub(super) fn write(name: &str, model: &Model) -> String {
    let mut out = format!("o {name}\n");
    for v in &model.vertices {
        let ([x, y, z], [r, g, b, _]) = (v.position, v.colour);
        let _ = writeln!(out, "v {x} {y} {z} {r} {g} {b}");
    }
    for v in &model.vertices {
        // The file counts up the picture, the game down it.
        let _ = writeln!(out, "vt {} {}", v.uv[0], 1.0 - v.uv[1]);
    }
    for v in &model.vertices {
        let [x, y, z] = v.normal;
        let _ = writeln!(out, "vn {x} {y} {z}");
    }
    for t in &model.triangles {
        let [a, b, c] = t.map(|i| i + 1);
        let _ = writeln!(out, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}");
    }
    out
}

pub(super) fn read(text: &str) -> Result<Model> {
    let (mut positions, mut uvs, mut normals) = (Vec::new(), Vec::new(), Vec::new());
    let mut corners = Corners::default();
    for (number, line) in text.lines().enumerate() {
        let mut words = line.split_whitespace();
        let kind = words.next().unwrap_or_default();
        let words: Vec<&str> = words.collect();
        let numbers = || -> Result<Vec<f32>> {
            words
                .iter()
                .map(|w| {
                    w.parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| anyhow!("line {}: '{w}' is not a number", number + 1))
                })
                .collect()
        };
        let need = |v: &[f32], n: usize| -> Result<()> {
            if v.len() < n {
                bail!("line {}: too few numbers", number + 1);
            }
            Ok(())
        };
        match kind {
            "v" => {
                let v = numbers()?;
                need(&v, 3)?;
                let colour = if v.len() >= 6 {
                    [v[3], v[4], v[5], 1.0]
                } else {
                    [1.0; 4]
                };
                positions.push(([v[0], v[1], v[2]], colour));
            }
            "vt" => {
                let v = numbers()?;
                need(&v, 2)?;
                uvs.push([v[0], 1.0 - v[1]]);
            }
            "vn" => {
                let v = numbers()?;
                need(&v, 3)?;
                normals.push([v[0], v[1], v[2]]);
            }
            "f" => {
                let mut face = Vec::new();
                for word in &words {
                    let mut parts = word.split('/');
                    let mut at = |count: usize| -> Result<Option<usize>> {
                        let Some(text) = parts.next().filter(|t| !t.is_empty()) else {
                            return Ok(None);
                        };
                        let index: i64 = text
                            .parse()
                            .map_err(|_| anyhow!("line {}: '{word}' is no corner", number + 1))?;
                        // Counted from one, or back from the last.
                        let index = if index < 0 {
                            count as i64 + index
                        } else {
                            index - 1
                        };
                        usize::try_from(index)
                            .ok()
                            .filter(|i| *i < count)
                            .map(Some)
                            .ok_or_else(|| anyhow!("line {}: '{word}' names no vertex", number + 1))
                    };
                    let position = at(positions.len())?
                        .ok_or_else(|| anyhow!("line {}: '{word}' is no corner", number + 1))?;
                    let uv = at(uvs.len())?;
                    let normal = at(normals.len())?;
                    let (position, colour) = positions[position];
                    let vertex = Vertex {
                        position,
                        normal: normal.map_or([0.0; 3], |i| normals[i]),
                        uv: uv.map_or([0.0; 2], |i| uvs[i]),
                        uv2: None,
                        colour,
                    };
                    face.push(corners.vertex(vertex, normal.is_some()));
                }
                // A face of more corners is a fan of triangles.
                for k in 2..face.len() {
                    corners
                        .model
                        .triangles
                        .push([face[0], face[k - 1], face[k]]);
                }
            }
            _ => {}
        }
    }
    Ok(corners.finish())
}
