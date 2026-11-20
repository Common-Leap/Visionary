//! The `param_<name>` struct an article reads from its owner fighter's `vl.prc` (or a moveset's
//! `vl.prcxml`).
//!
//! A cloned article's params come from the NEW owner's file under its own `param_<new_name>`;
//! smashline never falls back to the source article's block, so neither does this.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Float(f32),
    Int(i32),
    /// Anything that is not a plain number (hashes, strings, bools), as text.
    Other(String),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamBlock {
    /// Some layer holds a `param_<name>` struct.
    pub present: bool,
    /// Field name (label, else `0x…` hash) to its final value and the file it came from
    /// (an empty path for a `param_config` override).
    pub fields: BTreeMap<String, (ParamValue, PathBuf)>,
}

fn is_xml(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("prcxml"))
}

/// The hash of a label, or of a `0x…` hex name as written for unlabelled hashes.
fn hash_of(name: &str) -> u64 {
    match name.strip_prefix("0x").map(|h| u64::from_str_radix(h, 16)) {
        Some(Ok(h)) => h,
        _ => hash40::hash40(name).0,
    }
}

fn name_of(hash: u64, labels: &HashMap<u64, String>) -> String {
    labels.get(&hash).cloned().unwrap_or_else(|| format!("{hash:#x}"))
}

/// The merged `param_<name>` block of `layers` (lowest first; later layers win per field), with
/// `param_config` `overrides` (block, field, value) applied on top when some layer has the block.
pub fn param_block(
    layers: &[PathBuf],
    overrides: &[(String, String, f64)],
    name: &str,
    labels: &HashMap<u64, String>,
) -> ParamBlock {
    let full = format!("param_{name}");
    let target = hash_of(&full);
    let mut out = ParamBlock::default();
    for layer in layers {
        let Some(fields) = (if is_xml(layer) {
            read_xml_block(layer, target)
        } else {
            read_prc_block(layer, target, labels)
        }) else {
            continue;
        };
        out.present = true;
        for (field, value) in fields {
            out.fields.insert(field, (value, layer.clone()));
        }
    }
    if out.present {
        for (block, field, value) in overrides {
            if block != &full && block != name {
                continue;
            }
            let key = name_of(hash_of(field), labels);
            let existing_int = matches!(out.fields.get(&key), Some((ParamValue::Int(_), _)));
            let value = if existing_int && value.fract() == 0.0 {
                ParamValue::Int(*value as i32)
            } else {
                ParamValue::Float(*value as f32)
            };
            out.fields.insert(key, (value, PathBuf::new()));
        }
    }
    out
}

/// The fields of a block value. Vanilla blocks are usually a list of structs, one per variant, and
/// the first is the article's own; a plain struct is its own fields.
fn block_struct(value: &prc::ParamKind) -> Option<&prc::ParamStruct> {
    match value {
        prc::ParamKind::Struct(inner) => Some(inner),
        prc::ParamKind::List(list) => match list.0.first()? {
            prc::ParamKind::Struct(inner) => Some(inner),
            _ => None,
        },
        _ => None,
    }
}

fn block_struct_mut(value: &mut prc::ParamKind) -> Option<&mut prc::ParamStruct> {
    match value {
        prc::ParamKind::Struct(inner) => Some(inner),
        prc::ParamKind::List(list) => match list.0.first_mut()? {
            prc::ParamKind::Struct(inner) => Some(inner),
            _ => None,
        },
        _ => None,
    }
}

fn read_prc_block(
    path: &Path,
    target: u64,
    labels: &HashMap<u64, String>,
) -> Option<Vec<(String, ParamValue)>> {
    let root = prc::open(path).ok()?;
    let block = root
        .0
        .iter()
        .find_map(|(key, value)| (key.0 == target).then(|| block_struct(value)).flatten())?;
    Some(
        block
            .0
            .iter()
            .filter_map(|(key, value)| {
                let value = match value {
                    prc::ParamKind::Float(v) => ParamValue::Float(*v),
                    prc::ParamKind::I8(v) => ParamValue::Int(*v as i32),
                    prc::ParamKind::U8(v) => ParamValue::Int(*v as i32),
                    prc::ParamKind::I16(v) => ParamValue::Int(*v as i32),
                    prc::ParamKind::U16(v) => ParamValue::Int(*v as i32),
                    prc::ParamKind::I32(v) => ParamValue::Int(*v),
                    prc::ParamKind::U32(v) => ParamValue::Int(*v as i32),
                    prc::ParamKind::Bool(v) => ParamValue::Other(v.to_string()),
                    prc::ParamKind::Hash(h) => ParamValue::Other(name_of(h.0, labels)),
                    prc::ParamKind::Str(s) => ParamValue::Other(s.clone()),
                    _ => return None,
                };
                Some((name_of(key.0, labels), value))
            })
            .collect(),
    )
}

/// One tag of an XML file, with its byte range in the source.
struct Tag {
    name: String,
    hash: Option<String>,
    open: bool,
    self_closing: bool,
    start: usize,
    end: usize,
}

/// The tags of `src` in order; comments, declarations and text are skipped.
fn scan_tags(src: &str) -> Vec<Tag> {
    let bytes = src.as_bytes();
    let mut tags = Vec::new();
    let mut i = 0;
    while let Some(off) = src[i..].find('<') {
        let start = i + off;
        if src[start..].starts_with("<!--") {
            i = src[start..].find("-->").map_or(src.len(), |e| start + e + 3);
            continue;
        }
        // The end of the tag is the first `>` outside quotes.
        let mut quote = None;
        let mut end = None;
        for (j, &b) in bytes.iter().enumerate().skip(start + 1) {
            match (quote, b) {
                (Some(q), _) if b == q => quote = None,
                (Some(_), _) => {}
                (None, b'"' | b'\'') => quote = Some(b),
                (None, b'>') => {
                    end = Some(j + 1);
                    break;
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        let inner = &src[start + 1..end - 1];
        i = end;
        if inner.starts_with('?') || inner.starts_with('!') {
            continue;
        }
        let (open, body) = match inner.strip_prefix('/') {
            Some(rest) => (false, rest),
            None => (true, inner),
        };
        let name_len = body.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(body.len());
        tags.push(Tag {
            name: body[..name_len].to_string(),
            hash: hash_attr(&body[name_len..]),
            open,
            self_closing: open && inner.ends_with('/'),
            start,
            end,
        });
    }
    tags
}

fn hash_attr(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(pos) = rest.find("hash") {
        let after = &rest[pos + 4..];
        if rest[..pos].chars().next_back().is_none_or(char::is_whitespace) {
            if let Some(value) = after.trim_start().strip_prefix('=') {
                let value = value.trim_start();
                let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
                let inner = &value[1..];
                return inner.find(quote).map(|e| unescape(&inner[..e]));
            }
        }
        rest = after;
    }
    None
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// A leaf element of a block: its `hash` attribute, its element name, and the byte range of its text.
type Leaf = (String, String, std::ops::Range<usize>);

/// The leaf children of the struct (or first struct of the list) `target` directly under the root; `None` if there is no such
/// struct. Nested structs and lists are skipped.
fn xml_block_leaves(src: &str, target: u64) -> Option<Vec<Leaf>> {
    let tags = scan_tags(src);
    let mut depth = 0usize;
    let mut idx = None;
    for (n, tag) in tags.iter().enumerate() {
        if tag.open {
            if depth == 1
                && (tag.name == "struct" || tag.name == "list")
                && tag.hash.as_deref().is_some_and(|h| hash_of(h) == target)
            {
                idx = Some(n);
                break;
            }
            if !tag.self_closing {
                depth += 1;
            }
        } else {
            depth = depth.saturating_sub(1);
        }
    }
    let mut idx = idx?;
    let mut leaves = Vec::new();
    if tags[idx].self_closing {
        return Some(leaves);
    }
    if tags[idx].name == "list" {
        // The first struct of the list, as with the binary form.
        match tags.get(idx + 1) {
            Some(t) if t.open && !t.self_closing && t.name == "struct" => idx += 1,
            _ => return Some(leaves),
        }
    }
    let mut depth = 1usize;
    let mut n = idx + 1;
    while n < tags.len() && depth > 0 {
        let tag = &tags[n];
        if tag.open {
            let leaf = depth == 1
                && !tag.self_closing
                && tags.get(n + 1).is_some_and(|c| !c.open && c.name == tag.name);
            if let (true, Some(h)) = (leaf, &tag.hash) {
                leaves.push((h.clone(), tag.name.clone(), tag.end..tags[n + 1].start));
            }
            if !tag.self_closing {
                depth += 1;
            }
        } else {
            depth -= 1;
        }
        n += 1;
    }
    Some(leaves)
}

fn read_xml_block(path: &Path, target: u64) -> Option<Vec<(String, ParamValue)>> {
    let src = std::fs::read_to_string(path).ok()?;
    let leaves = xml_block_leaves(&src, target)?;
    Some(
        leaves
            .into_iter()
            .map(|(field, element, range)| {
                let text = unescape(src[range].trim());
                let value = match element.as_str() {
                    "float" => text.parse().map(ParamValue::Float).unwrap_or(ParamValue::Other(text)),
                    "sbyte" | "byte" | "short" | "ushort" | "int" | "uint" => text
                        .parse::<i64>()
                        .map(|v| ParamValue::Int(v as i32))
                        .unwrap_or(ParamValue::Other(text)),
                    _ => ParamValue::Other(text),
                };
                (field, value)
            })
            .collect(),
    )
}

/// Set `block.field` (`block` a full `param_…` name; `field` a label or `0x…` hash) to `value` in
/// `file`, keeping the field's type. A `.prcxml` has only that element's text replaced, so every
/// other byte (CRLF included) is kept and no binary `.prc` appears beside it; anything else is
/// rewritten through `prc::save`. The block and field must already exist.
pub fn write_param_field(
    file: &Path,
    block: &str,
    field: &str,
    value: f64,
    _labels: &HashMap<u64, String>,
) -> Result<()> {
    let (block_h, field_h) = (hash_of(block), hash_of(field));
    if is_xml(file) {
        let src = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        let leaves = xml_block_leaves(&src, block_h)
            .ok_or_else(|| anyhow!("{block} is not in {}", file.display()))?;
        let (_, element, range) = leaves
            .into_iter()
            .find(|(name, _, _)| hash_of(name) == field_h)
            .ok_or_else(|| anyhow!("{field} is not in {block}"))?;
        let text = match element.as_str() {
            "float" => (value as f32).to_string(),
            "sbyte" | "byte" | "short" | "ushort" | "int" | "uint" => (value.round() as i64).to_string(),
            other => bail!("{field} is a {other}, not a number"),
        };
        let out = format!("{}{text}{}", &src[..range.start], &src[range.end..]);
        return std::fs::write(file, out).with_context(|| format!("writing {}", file.display()));
    }
    let mut root = prc::open(file).map_err(|e| anyhow!("reading {}: {e}", file.display()))?;
    let inner = root
        .0
        .iter_mut()
        .find_map(|(key, v)| (key.0 == block_h).then(|| block_struct_mut(v)).flatten())
        .ok_or_else(|| anyhow!("{block} is not in {}", file.display()))?;
    let slot = inner
        .0
        .iter_mut()
        .find_map(|(key, v)| (key.0 == field_h).then_some(v))
        .ok_or_else(|| anyhow!("{field} is not in {block}"))?;
    let int = value.round();
    match slot {
        prc::ParamKind::Float(v) => *v = value as f32,
        prc::ParamKind::I8(v) => *v = int as i8,
        prc::ParamKind::U8(v) => *v = int as u8,
        prc::ParamKind::I16(v) => *v = int as i16,
        prc::ParamKind::U16(v) => *v = int as u16,
        prc::ParamKind::I32(v) => *v = int as i32,
        prc::ParamKind::U32(v) => *v = int as u32,
        _ => bail!("{field} is not a number"),
    }
    prc::save(file, &root).map_err(|e| anyhow!("writing {}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prc_file(dir: &Path, name: &str, block: &str, speed: f32) -> PathBuf {
        let path = dir.join(name);
        let inner = prc::ParamStruct(vec![
            (hash40::hash40("speed"), prc::ParamKind::Float(speed)),
            (hash40::hash40("life"), prc::ParamKind::I32(60)),
        ]);
        let root = prc::ParamStruct(vec![(hash40::hash40(block), prc::ParamKind::Struct(inner))]);
        prc::save(&path, &root).unwrap();
        path
    }

    fn labels() -> HashMap<u64, String> {
        ["speed", "life"].iter().map(|n| (hash40::hash40(n).0, n.to_string())).collect()
    }

    #[test]
    fn clone_block_present_latios() {
        let dir = tempfile::tempdir().unwrap();
        let file = prc_file(dir.path(), "vl.prc", "param_lativolt", 1.6);
        let block = param_block(&[file], &[], "lativolt", &labels());
        assert!(block.present);
        assert_eq!(block.fields["speed"].0, ParamValue::Float(1.6));
        assert_eq!(block.fields["life"].0, ParamValue::Int(60));
    }

    #[test]
    fn absent_block_is_not_borrowed() {
        let dir = tempfile::tempdir().unwrap();
        let file = prc_file(dir.path(), "vl.prc", "param_fireball", 1.0);
        let block = param_block(&[file], &[], "lativolt", &labels());
        assert!(!block.present);
        assert!(block.fields.is_empty());
    }

    #[test]
    fn later_layer_wins() {
        let dir = tempfile::tempdir().unwrap();
        let dump = prc_file(dir.path(), "dump.prc", "param_x", 1.0);
        let romfs = prc_file(dir.path(), "romfs.prc", "param_x", 2.0);
        let block = param_block(&[dump, romfs.clone()], &[], "x", &labels());
        assert_eq!(block.fields["speed"], (ParamValue::Float(2.0), romfs));
    }

    #[test]
    fn param_config_override_wins() {
        let dir = tempfile::tempdir().unwrap();
        let file = prc_file(dir.path(), "vl.prc", "param_x", 1.0);
        let over = vec![("param_x".to_string(), "speed".to_string(), 3.0)];
        let block = param_block(&[file], &over, "x", &labels());
        assert_eq!(block.fields["speed"].0, ParamValue::Float(3.0));
    }

    #[test]
    fn list_of_structs_block_uses_its_first_struct() {
        let dir = tempfile::tempdir().unwrap();
        let variant = |speed: f32| {
            prc::ParamKind::Struct(prc::ParamStruct(vec![(hash40::hash40("speed"), prc::ParamKind::Float(speed))]))
        };
        let root = prc::ParamStruct(vec![(
            hash40::hash40("param_x"),
            prc::ParamKind::List(prc::ParamList(vec![variant(2.8), variant(0.0)])),
        )]);
        let file = dir.path().join("vl.prc");
        prc::save(&file, &root).unwrap();
        assert_eq!(param_block(&[file.clone()], &[], "x", &labels()).fields["speed"].0, ParamValue::Float(2.8));
        write_param_field(&file, "param_x", "speed", 3.5, &labels()).unwrap();
        assert_eq!(param_block(&[file], &[], "x", &labels()).fields["speed"].0, ParamValue::Float(3.5));

        let xml = dir.path().join("vl.prcxml");
        std::fs::write(&xml, "<struct>
<list size=\"2\" hash=\"param_x\">
<struct index=\"0\"><float hash=\"speed\">2.8</float></struct>
<struct index=\"1\"><float hash=\"speed\">0</float></struct>
</list>
</struct>").unwrap();
        assert_eq!(param_block(&[xml.clone()], &[], "x", &labels()).fields["speed"].0, ParamValue::Float(2.8));
        write_param_field(&xml, "param_x", "speed", 1.0, &labels()).unwrap();
        assert!(std::fs::read_to_string(&xml).unwrap().contains("<float hash=\"speed\">1</float></struct>
<struct index=\"1\"><float hash=\"speed\">0<"));
    }

    #[test]
    fn prc_write_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let file = prc_file(dir.path(), "vl.prc", "param_x", 1.0);
        write_param_field(&file, "param_x", "speed", 4.5, &labels()).unwrap();
        let block = param_block(&[file], &[], "x", &labels());
        assert_eq!(block.fields["speed"].0, ParamValue::Float(4.5));
    }

    #[test]
    fn prcxml_layer_read_and_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("vl.prcxml");
        let before = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<struct>\r\n  <!-- keep <me> -->\r\n  <struct hash=\"param_other\">\r\n    <float hash=\"speed\">9</float>\r\n  </struct>\r\n  <struct hash=\"param_lativolt\">\r\n    <float hash=\"speed\">1.6</float>\r\n    <int hash=\"life\">60</int>\r\n  </struct>\r\n</struct>\r\n";
        std::fs::write(&file, before).unwrap();
        let block = param_block(&[file.clone()], &[], "lativolt", &labels());
        assert_eq!(block.fields["speed"].0, ParamValue::Float(1.6));
        assert_eq!(block.fields["life"].0, ParamValue::Int(60));

        write_param_field(&file, "param_lativolt", "speed", 2.5, &labels()).unwrap();
        let after = std::fs::read_to_string(&file).unwrap();
        assert_eq!(after, before.replace("<float hash=\"speed\">1.6<", "<float hash=\"speed\">2.5<"));
        assert!(!dir.path().join("vl.prc").exists());
    }

    #[test]
    #[ignore = "reads a real moveset checkout"]
    fn real_latios_file() {
        let file = PathBuf::from("C:/Users/notja/Documents/Coding/SSBU Modding/Movesets/Latios-Moveset/romfs/fighter/samus/param/vl.prc");
        let block = param_block(&[file], &[], "lativolt", &HashMap::new());
        assert!(block.present);
    }
}
