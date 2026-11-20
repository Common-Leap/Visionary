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

type Fields = BTreeMap<String, (ParamValue, PathBuf)>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamBlock {
    /// Some layer holds a `param_<name>` struct or list.
    pub present: bool,
    /// Variant 0 of a list block, or the plain struct. Field names are the label, else `0x…`; the
    /// path is the layer the value came from (empty for a `param_config` override).
    pub fields: Fields,
    /// Every variant of a list block, variant 0 included; empty for a plain struct.
    pub variants: Vec<Fields>,
    /// What was not read, as (field path or "variant N" or a file path, one-sentence reason).
    pub skipped: Vec<(String, String)>,
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

/// One layer's view of a block.
struct LayerBlock {
    is_list: bool,
    variants: Vec<Vec<(String, ParamValue)>>,
    skipped: Vec<(String, String)>,
}

fn skip_path(is_list: bool, variant: usize, field: &str) -> String {
    if is_list {
        format!("variant {variant}/{field}")
    } else {
        field.to_string()
    }
}

fn nested_reason(count: usize) -> String {
    format!("a nested group of {count} values Visionary doesn't edit")
}

/// The merged `param_<name>` block of `layers` (lowest first; later layers win per field and per
/// variant), with `param_config` `overrides` (block, field, value) applied to variant 0 when some
/// layer has the block. A layer that cannot be read is recorded in `skipped` and the rest still
/// merge.
pub fn param_block(
    layers: &[PathBuf],
    overrides: &[(String, String, f64)],
    name: &str,
    labels: &HashMap<u64, String>,
) -> ParamBlock {
    let full = format!("param_{name}");
    let target = hash_of(&full);
    let mut out = ParamBlock::default();
    let mut is_list = false;
    let mut variants: Vec<Fields> = Vec::new();
    for layer in layers {
        if !layer.exists() {
            continue;
        }
        let read = if is_xml(layer) {
            read_xml_block(layer, target)
        } else {
            read_prc_block(layer, target, labels)
        };
        let found = match read {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(error) => {
                out.skipped.push((
                    layer.display().to_string(),
                    format!("Visionary could not read this param file: {error}"),
                ));
                continue;
            }
        };
        out.present = true;
        is_list |= found.is_list;
        for entry in found.skipped {
            if !out.skipped.contains(&entry) {
                out.skipped.push(entry);
            }
        }
        for (index, fields) in found.variants.into_iter().enumerate() {
            if variants.len() <= index {
                variants.resize_with(index + 1, Fields::new);
            }
            for (field, value) in fields {
                variants[index].insert(field, (value, layer.clone()));
            }
        }
    }
    if out.present {
        if variants.is_empty() {
            variants.push(Fields::new());
        }
        for (block, field, value) in overrides {
            if block != &full && block != name {
                continue;
            }
            let key = name_of(hash_of(field), labels);
            let existing_int = matches!(variants[0].get(&key), Some((ParamValue::Int(_), _)));
            let value = if existing_int && value.fract() == 0.0 {
                ParamValue::Int(*value as i32)
            } else {
                ParamValue::Float(*value as f32)
            };
            variants[0].insert(key, (value, PathBuf::new()));
        }
        out.fields = variants[0].clone();
        if is_list {
            out.variants = variants;
        }
    }
    out
}

fn prc_number(value: &prc::ParamKind, labels: &HashMap<u64, String>) -> Option<ParamValue> {
    Some(match value {
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
    })
}

fn read_prc_block(
    path: &Path,
    target: u64,
    labels: &HashMap<u64, String>,
) -> std::result::Result<Option<LayerBlock>, String> {
    let root = prc::open(path).map_err(|e| e.to_string())?;
    let Some(block) = root.0.iter().find_map(|(key, value)| (key.0 == target).then_some(value)) else {
        return Ok(None);
    };
    let (is_list, structs): (bool, Vec<Option<&prc::ParamStruct>>) = match block {
        prc::ParamKind::Struct(inner) => (false, vec![Some(inner)]),
        prc::ParamKind::List(list) => (
            true,
            list.0
                .iter()
                .map(|v| match v {
                    prc::ParamKind::Struct(inner) => Some(inner),
                    _ => None,
                })
                .collect(),
        ),
        _ => return Ok(None),
    };
    let mut out = LayerBlock { is_list, variants: Vec::new(), skipped: Vec::new() };
    for (index, inner) in structs.into_iter().enumerate() {
        let mut fields = Vec::new();
        match inner {
            None => out.skipped.push((format!("variant {index}"), "this list entry is not a struct".into())),
            Some(inner) => {
                for (key, value) in &inner.0 {
                    let field = name_of(key.0, labels);
                    if let Some(value) = prc_number(value, labels) {
                        fields.push((field, value));
                        continue;
                    }
                    let reason = match value {
                        prc::ParamKind::Struct(s) => nested_reason(s.0.len()),
                        prc::ParamKind::List(l) => nested_reason(l.0.len()),
                        _ => "a value type Visionary doesn't edit".to_string(),
                    };
                    out.skipped.push((skip_path(is_list, index, &field), reason));
                }
            }
        }
        out.variants.push(fields);
    }
    Ok(Some(out))
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

/// A leaf element of a block: its `hash` attribute, its element name, and the byte range of its
/// text with surrounding whitespace excluded.
type Leaf = (String, String, std::ops::Range<usize>);

/// A block of a prcxml: leaves per variant, nested entries as (field, child count) per variant,
/// and list entries that are not structs.
struct XmlBlock {
    is_list: bool,
    variants: Vec<(Vec<Leaf>, Vec<(String, usize)>)>,
    bad_entries: Vec<usize>,
}

/// The direct child open tags of element `n`, and the index just past its close tag.
fn children(tags: &[Tag], n: usize) -> (Vec<usize>, usize) {
    if tags[n].self_closing {
        return (Vec::new(), n + 1);
    }
    let (mut found, mut depth, mut m) = (Vec::new(), 1usize, n + 1);
    while m < tags.len() && depth > 0 {
        if tags[m].open {
            if depth == 1 {
                found.push(m);
            }
            if !tags[m].self_closing {
                depth += 1;
            }
        } else {
            depth -= 1;
        }
        m += 1;
    }
    (found, m)
}

fn xml_variant(src: &str, tags: &[Tag], n: usize) -> (Vec<Leaf>, Vec<(String, usize)>) {
    let (mut leaves, mut nested) = (Vec::new(), Vec::new());
    for child in children(tags, n).0 {
        let tag = &tags[child];
        let leaf = !tag.self_closing && tags.get(child + 1).is_some_and(|c| !c.open && c.name == tag.name);
        let label = tag.hash.clone().unwrap_or_else(|| tag.name.clone());
        if leaf {
            if let Some(h) = &tag.hash {
                let (from, to) = (tag.end, tags[child + 1].start);
                let raw = &src[from..to];
                let lead = raw.len() - raw.trim_start().len();
                let start = from + lead;
                leaves.push((h.clone(), tag.name.clone(), start..start + raw.trim().len()));
            }
        } else {
            nested.push((label, children(tags, child).0.len()));
        }
    }
    (leaves, nested)
}

/// The struct or list `target` directly under the root; `None` if there is no such element.
fn xml_block(src: &str, target: u64) -> Option<XmlBlock> {
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
    let idx = idx?;
    if tags[idx].name == "struct" {
        return Some(XmlBlock {
            is_list: false,
            variants: vec![xml_variant(src, &tags, idx)],
            bad_entries: Vec::new(),
        });
    }
    let mut block = XmlBlock { is_list: true, variants: Vec::new(), bad_entries: Vec::new() };
    for (index, child) in children(&tags, idx).0.into_iter().enumerate() {
        if tags[child].name == "struct" {
            block.variants.push(xml_variant(src, &tags, child));
        } else {
            block.bad_entries.push(index);
            block.variants.push((Vec::new(), Vec::new()));
        }
    }
    Some(block)
}

fn read_xml_block(path: &Path, target: u64) -> std::result::Result<Option<LayerBlock>, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let Some(block) = xml_block(&src, target) else { return Ok(None) };
    let mut out = LayerBlock { is_list: block.is_list, variants: Vec::new(), skipped: Vec::new() };
    for (index, (leaves, nested)) in block.variants.into_iter().enumerate() {
        if block.bad_entries.contains(&index) {
            out.skipped.push((format!("variant {index}"), "this list entry is not a struct".into()));
        }
        for (field, count) in nested {
            out.skipped.push((skip_path(block.is_list, index, &field), nested_reason(count)));
        }
        out.variants.push(
            leaves
                .into_iter()
                .map(|(field, element, range)| {
                    let text = unescape(&src[range]);
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
        );
    }
    Ok(Some(out))
}

/// Set `block.field` of `variant` (`block` a full `param_…` name; `field` a label or `0x…` hash;
/// `variant` 0 for a plain struct) to `value` in `file`, keeping the field's type. A `.prcxml` has
/// only that element's text replaced, so every other byte (CRLF and padding included) is kept and no
/// binary `.prc` appears beside it; anything else is rewritten through `prc::save`. The block,
/// variant and field must already exist.
pub fn write_param_field(
    file: &Path,
    block: &str,
    field: &str,
    variant: usize,
    value: f64,
    _labels: &HashMap<u64, String>,
) -> Result<()> {
    let (block_h, field_h) = (hash_of(block), hash_of(field));
    if is_xml(file) {
        let src = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        let found = xml_block(&src, block_h).ok_or_else(|| anyhow!("{block} is not in {}", file.display()))?;
        let (leaves, _) = found
            .variants
            .into_iter()
            .nth(variant)
            .ok_or_else(|| anyhow!("{block} has no variant {variant}"))?;
        let (_, element, range) = leaves
            .into_iter()
            .find(|(name, _, _)| hash_of(name) == field_h)
            .ok_or_else(|| anyhow!("{field} is not in {block} variant {variant}"))?;
        let text = match element.as_str() {
            "float" => float_text(field, value)?,
            "sbyte" | "byte" | "short" | "ushort" | "int" | "uint" => {
                whole_number(field, element.as_str(), value)?.to_string()
            }
            other => bail!("{field} is a {other}, not a number"),
        };
        let out = format!("{}{text}{}", &src[..range.start], &src[range.end..]);
        return std::fs::write(file, out).with_context(|| format!("writing {}", file.display()));
    }
    let mut root = prc::open(file).map_err(|e| anyhow!("reading {}: {e}", file.display()))?;
    let value_of_block = root
        .0
        .iter_mut()
        .find_map(|(key, v)| (key.0 == block_h).then_some(v))
        .ok_or_else(|| anyhow!("{block} is not in {}", file.display()))?;
    let inner = match value_of_block {
        prc::ParamKind::Struct(s) if variant == 0 => s,
        prc::ParamKind::Struct(_) => bail!("{block} is a plain struct and has no variant {variant}"),
        prc::ParamKind::List(list) => match list.0.get_mut(variant) {
            Some(prc::ParamKind::Struct(s)) => s,
            Some(_) => bail!("{block} variant {variant} is not a struct"),
            None => bail!("{block} has no variant {variant}"),
        },
        _ => bail!("{block} is not a struct or list"),
    };
    let slot = inner
        .0
        .iter_mut()
        .find_map(|(key, v)| (key.0 == field_h).then_some(v))
        .ok_or_else(|| anyhow!("{field} is not in {block} variant {variant}"))?;
    match slot {
        prc::ParamKind::Float(v) => {
            float_text(field, value)?;
            *v = value as f32
        }
        prc::ParamKind::I8(v) => *v = whole_number(field, "sbyte", value)? as i8,
        prc::ParamKind::U8(v) => *v = whole_number(field, "byte", value)? as u8,
        prc::ParamKind::I16(v) => *v = whole_number(field, "short", value)? as i16,
        prc::ParamKind::U16(v) => *v = whole_number(field, "ushort", value)? as u16,
        prc::ParamKind::I32(v) => *v = whole_number(field, "int", value)? as i32,
        prc::ParamKind::U32(v) => *v = whole_number(field, "uint", value)? as u32,
        _ => bail!("{field} is not a number"),
    }
    prc::save(file, &root).map_err(|e| anyhow!("writing {}: {e}", file.display()))
}

/// Whether `file` holds `block.field` in `variant` (`block` a full `param_…` name; `field` a label
/// or `0x…` hash). A missing file holds nothing; one that cannot be read is an error, so a caller
/// never mistakes it for a layer without the field.
pub fn layer_has_field(file: &Path, block: &str, field: &str, variant: usize) -> Result<bool> {
    if !file.exists() {
        return Ok(false);
    }
    let read = if is_xml(file) {
        read_xml_block(file, hash_of(block))
    } else {
        read_prc_block(file, hash_of(block), &HashMap::new())
    };
    let found = read.map_err(|error| anyhow!("Visionary could not read {}: {error}", file.display()))?;
    let field = hash_of(field);
    Ok(found
        .and_then(|found| found.variants.into_iter().nth(variant))
        .is_some_and(|fields| fields.iter().any(|(name, _)| hash_of(name) == field)))
}

/// A float param's text, refusing a value an `f32` cannot hold.
fn float_text(field: &str, value: f64) -> Result<String> {
    let single = value as f32;
    if !single.is_finite() {
        bail!("{field} holds a decimal number, and {value} is too large or not a number");
    }
    Ok(single.to_string())
}

/// `value` rounded to a whole number, refused when the param's type (`sbyte`, `byte`, `short`,
/// `ushort`, `int`, `uint`, as a prcxml names them) cannot hold it rather than clamped to the
/// nearest value it can.
fn whole_number(field: &str, kind: &str, value: f64) -> Result<i64> {
    let (min, max, words) = match kind {
        "sbyte" => (i8::MIN as i64, i8::MAX as i64, "a small whole number"),
        "byte" => (0, u8::MAX as i64, "a small whole number that can't be negative"),
        "short" => (i16::MIN as i64, i16::MAX as i64, "a whole number"),
        "ushort" => (0, u16::MAX as i64, "a whole number that can't be negative"),
        "int" => (i32::MIN as i64, i32::MAX as i64, "a whole number"),
        "uint" => (0, u32::MAX as i64, "a whole number that can't be negative"),
        other => bail!("{field} is a {other}, not a number"),
    };
    let rounded = value.round();
    if !rounded.is_finite() || rounded < min as f64 || rounded > max as f64 {
        bail!("{field} holds {words} from {min} to {max}, so it can't be set to {value}");
    }
    Ok(rounded as i64)
}

/// One `param_config::update_{int,float}{,_2}(kind, slots, (hash40("block"), hash40("field"), v))`
/// call in a moveset's source: a value the moveset sets in code over its param files.
/// (`lib_paramconfig`: `update_int_2(kind: i32, slots: Vec<i32>, param: (u64, u64, i32))`; the
/// float forms take an `f32`; a common param has `0` as its second hash.)
#[derive(Debug, Clone, PartialEq)]
pub struct ParamOverride {
    /// The kind argument as written, whitespace removed (`*FIGHTER_KIND_SAMUS`, `-(*WEAPON_…)`).
    pub kind: String,
    /// The first hash's text (`param_cshot`, or `weight` for a common param).
    pub block: String,
    /// The second hash's text; `None` for a common param (`0`).
    pub field: Option<String>,
    pub value: f64,
    pub file: PathBuf,
    /// 1-based line of the call.
    pub line: usize,
}

impl ParamOverride {
    /// Whether this sets a fighter's own param (not a weapon's) for `fighter`.
    pub fn is_for_fighter(&self, fighter: &str) -> bool {
        let kind = self.kind.trim_start_matches('*');
        let kind = kind.rsplit("::").next().unwrap_or(kind).trim_start_matches('*');
        kind == format!("FIGHTER_KIND_{}", fighter.to_uppercase())
    }
}

/// The text inside `hash40("x")` / `smash::hash40("x")` / `Hash40::new("x").hash`, or `None` for
/// `0` and anything else.
fn hash_arg(text: &str) -> Option<String> {
    let open = text.find('"')?;
    let close = text.rfind('"')?;
    (close > open && (text.contains("hash40") || text.contains("Hash40")))
        .then(|| text[open + 1..close].to_string())
}

fn number_arg(expr: &syn::Expr) -> Option<f64> {
    match expr {
        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. }) => i.base10_parse::<i64>().ok().map(|v| v as f64),
        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Float(f), .. }) => f.base10_parse::<f64>().ok(),
        syn::Expr::Unary(syn::ExprUnary { op: syn::UnOp::Neg(_), expr, .. }) => number_arg(expr).map(|v| -v),
        syn::Expr::Paren(p) => number_arg(&p.expr),
        syn::Expr::Cast(c) => number_arg(&c.expr),
        _ => None,
    }
}

struct OverrideReader<'a> {
    file: &'a Path,
    text: &'a str,
    line_starts: Vec<usize>,
    out: Vec<ParamOverride>,
}

impl OverrideReader<'_> {
    fn offset(&self, at: proc_macro2::LineColumn) -> usize {
        let start = self.line_starts.get(at.line.saturating_sub(1)).copied().unwrap_or(self.text.len());
        let line = &self.text[start..];
        start + line.char_indices().nth(at.column).map_or(line.len(), |(i, _)| i)
    }

    fn source(&self, node: &impl syn::spanned::Spanned) -> String {
        let span = node.span();
        let (a, b) = (self.offset(span.start()), self.offset(span.end()));
        self.text.get(a..b).unwrap_or_default().to_string()
    }
}

impl<'ast> syn::visit::Visit<'ast> for OverrideReader<'_> {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref() {
            let name = path.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
            let is_config = path.path.segments.iter().any(|s| s.ident == "param_config");
            if is_config && matches!(name.as_str(), "update_int" | "update_int_2" | "update_float" | "update_float_2") {
                if let (Some(kind), Some(syn::Expr::Tuple(tuple))) = (call.args.first(), call.args.last()) {
                    let parts: Vec<&syn::Expr> = tuple.elems.iter().collect();
                    if let [block, field, value] = parts.as_slice() {
                        if let (Some(block), Some(value)) = (hash_arg(&self.source(*block)), number_arg(value)) {
                            let kind: String = self.source(kind).chars().filter(|c| !c.is_whitespace()).collect();
                            self.out.push(ParamOverride {
                                kind,
                                block,
                                field: hash_arg(&self.source(*field)),
                                value,
                                file: self.file.to_path_buf(),
                                line: syn::spanned::Spanned::span(call).start().line,
                            });
                        }
                    }
                }
            }
        }
        syn::visit::visit_expr_call(self, call);
    }
}

/// Every `param_config` override in `sources`. A file that never names `param_config` is not
/// parsed.
pub fn param_config_overrides(sources: &[(PathBuf, String)]) -> Vec<ParamOverride> {
    let mut out = Vec::new();
    for (file, text) in sources {
        if !text.contains("param_config") {
            continue;
        }
        let Ok(ast) = syn::parse_file(text) else {
            continue;
        };
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let mut reader = OverrideReader { file, text, line_starts, out: Vec::new() };
        syn::visit::Visit::visit_file(&mut reader, &ast);
        out.extend(reader.out);
    }
    out
}

/// The overrides for `fighter`'s `param_<name>` block, as [`param_block`] takes them.
pub fn block_overrides(overrides: &[ParamOverride], fighter: &str, block: &str) -> Vec<(String, String, f64)> {
    overrides
        .iter()
        .filter(|o| o.is_for_fighter(fighter) && o.block == block)
        .filter_map(|o| Some((o.block.clone(), o.field.clone()?, o.value)))
        .collect()
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

    fn speed_variant(speed: f32) -> prc::ParamKind {
        prc::ParamKind::Struct(prc::ParamStruct(vec![
            (hash40::hash40("speed"), prc::ParamKind::Float(speed)),
            (hash40::hash40("life"), prc::ParamKind::I32(60)),
        ]))
    }

    fn list_root(speeds: &[f32]) -> prc::ParamStruct {
        prc::ParamStruct(vec![(
            hash40::hash40("param_x"),
            prc::ParamKind::List(prc::ParamList(speeds.iter().map(|s| speed_variant(*s)).collect())),
        )])
    }

    fn speed_of(block: &ParamBlock, variant: usize) -> ParamValue {
        block.variants[variant]["speed"].0.clone()
    }

    #[test]
    fn clone_block_present_latios() {
        let dir = tempfile::tempdir().unwrap();
        let file = prc_file(dir.path(), "vl.prc", "param_lativolt", 1.6);
        let block = param_block(&[file], &[], "lativolt", &labels());
        assert!(block.present);
        assert!(block.variants.is_empty());
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
    fn list_block_reads_every_variant() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("vl.prc");
        prc::save(&file, &list_root(&[2.8, 0.5, 7.0])).unwrap();
        let block = param_block(&[file], &[], "x", &labels());
        assert_eq!(block.variants.len(), 3);
        assert_eq!(speed_of(&block, 0), ParamValue::Float(2.8));
        assert_eq!(speed_of(&block, 1), ParamValue::Float(0.5));
        assert_eq!(speed_of(&block, 2), ParamValue::Float(7.0));
        assert_eq!(block.fields["speed"].0, ParamValue::Float(2.8));
    }

    #[test]
    fn variants_merge_per_index_across_layers() {
        let dir = tempfile::tempdir().unwrap();
        let (low, high) = (dir.path().join("low.prc"), dir.path().join("high.prc"));
        prc::save(&low, &list_root(&[1.0, 2.0, 3.0])).unwrap();
        prc::save(&high, &list_root(&[9.0])).unwrap();
        let block = param_block(&[low, high], &[], "x", &labels());
        assert_eq!(block.variants.len(), 3);
        assert_eq!(speed_of(&block, 0), ParamValue::Float(9.0));
        assert_eq!(speed_of(&block, 2), ParamValue::Float(3.0));
    }

    #[test]
    fn writing_a_variant_changes_only_that_variant() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("vl.prc");
        prc::save(&file, &list_root(&[2.8, 0.5, 7.0])).unwrap();
        write_param_field(&file, "param_x", "speed", 2, 1.25, &labels()).unwrap();
        assert_eq!(prc::open(&file).unwrap(), list_root(&[2.8, 0.5, 1.25]));
        assert!(write_param_field(&file, "param_x", "speed", 3, 1.0, &labels()).is_err());

        let xml = dir.path().join("vl.prcxml");
        let before = "<struct>\n<list size=\"3\" hash=\"param_x\">\n<struct index=\"0\"><float hash=\"speed\">2.8</float></struct>\n<struct index=\"1\"><float hash=\"speed\">0.5</float></struct>\n<struct index=\"2\"><float hash=\"speed\">7</float></struct>\n</list>\n</struct>";
        std::fs::write(&xml, before).unwrap();
        write_param_field(&xml, "param_x", "speed", 2, 1.25, &labels()).unwrap();
        assert_eq!(std::fs::read_to_string(&xml).unwrap(), before.replace(">7<", ">1.25<"));
        assert!(write_param_field(&xml, "param_x", "speed", 3, 1.0, &labels()).is_err());
    }

    #[test]
    fn nested_entries_are_reported_as_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let inner = prc::ParamStruct(vec![
            (hash40::hash40("speed"), prc::ParamKind::Float(1.0)),
            (
                hash40::hash40("group"),
                prc::ParamKind::Struct(prc::ParamStruct(vec![
                    (hash40::hash40("a"), prc::ParamKind::Float(1.0)),
                    (hash40::hash40("b"), prc::ParamKind::Float(2.0)),
                ])),
            ),
        ]);
        let file = dir.path().join("vl.prc");
        prc::save(&file, &prc::ParamStruct(vec![(hash40::hash40("param_x"), prc::ParamKind::Struct(inner))])).unwrap();
        let xml = dir.path().join("vl.prcxml");
        std::fs::write(&xml, "<struct><struct hash=\"param_y\"><float hash=\"speed\">1</float><struct hash=\"group\"><float hash=\"a\">1</float></struct></struct></struct>").unwrap();

        let mut names = labels();
        names.insert(hash40::hash40("group").0, "group".into());
        let block = param_block(&[file], &[], "x", &names);
        assert_eq!(block.fields.len(), 1);
        assert_eq!(
            block.skipped,
            vec![("group".to_string(), "a nested group of 2 values Visionary doesn't edit".to_string())]
        );
        let block = param_block(&[xml], &[], "y", &names);
        assert_eq!(block.skipped[0], ("group".to_string(), "a nested group of 1 values Visionary doesn't edit".to_string()));
    }

    #[test]
    fn unreadable_layer_is_reported_and_the_rest_still_merge() {
        let dir = tempfile::tempdir().unwrap();
        let good = prc_file(dir.path(), "good.prc", "param_x", 2.0);
        let corrupt = dir.path().join("corrupt.prc");
        std::fs::write(&corrupt, b"not a param file").unwrap();
        let bad_xml = dir.path().join("bad.prcxml");
        std::fs::write(&bad_xml, [0xff, 0xfe, 0x00, 0xc3, 0x28]).unwrap();
        let block = param_block(&[corrupt.clone(), bad_xml, good], &[], "x", &labels());
        assert!(block.present);
        assert_eq!(block.fields["speed"].0, ParamValue::Float(2.0));
        assert_eq!(block.skipped.len(), 2);
        assert_eq!(block.skipped[0].0, corrupt.display().to_string());
        assert!(block.skipped[0].1.starts_with("Visionary could not read this param file: "));
    }

    #[test]
    fn prc_write_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let file = prc_file(dir.path(), "vl.prc", "param_x", 1.0);
        write_param_field(&file, "param_x", "speed", 0, 4.5, &labels()).unwrap();
        let block = param_block(&[file], &[], "x", &labels());
        assert_eq!(block.fields["speed"].0, ParamValue::Float(4.5));
    }

    fn mixed_root(speed: f32) -> prc::ParamStruct {
        let hash = hash40::hash40;
        prc::ParamStruct(vec![
            (
                hash("param_x"),
                prc::ParamKind::Struct(prc::ParamStruct(vec![
                    (hash("speed"), prc::ParamKind::Float(speed)),
                    (hash("life"), prc::ParamKind::I32(60)),
                    (hash("tiny"), prc::ParamKind::U8(7)),
                    (hash("flag"), prc::ParamKind::Bool(true)),
                    (hash("kind"), prc::ParamKind::Hash(hash("shape_circle"))),
                    (hash("tag"), prc::ParamKind::Str("hello".into())),
                ])),
            ),
            (
                hash("param_other"),
                prc::ParamKind::Struct(prc::ParamStruct(vec![
                    (hash("speed"), prc::ParamKind::Float(9.0)),
                    (hash("name"), prc::ParamKind::Str("keep".into())),
                ])),
            ),
            (hash("loose"), prc::ParamKind::I16(-3)),
        ])
    }

    #[test]
    fn prc_write_leaves_every_other_value_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("vl.prc");
        prc::save(&file, &mixed_root(1.0)).unwrap();
        write_param_field(&file, "param_x", "speed", 0, 4.5, &labels()).unwrap();
        assert_eq!(prc::open(&file).unwrap(), mixed_root(4.5));
    }

    /// prc-rs reorders or re-lays-out the file on save, so an unedited open+save may not be byte
    /// identical to the original. Known limitation of the binary write; values are still preserved
    /// (see `prc_write_leaves_every_other_value_alone`).
    #[test]
    fn prc_open_save_without_edit_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (dir.path().join("a.prc"), dir.path().join("b.prc"));
        prc::save(&first, &mixed_root(1.0)).unwrap();
        prc::save(&second, &prc::open(&first).unwrap()).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), std::fs::read(&second).unwrap());
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

        write_param_field(&file, "param_lativolt", "speed", 0, 2.5, &labels()).unwrap();
        let after = std::fs::read_to_string(&file).unwrap();
        assert_eq!(after, before.replace("<float hash=\"speed\">1.6<", "<float hash=\"speed\">2.5<"));
        assert!(!dir.path().join("vl.prc").exists());
    }

    #[test]
    fn prcxml_write_keeps_padding_around_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("vl.prcxml");
        let before = "<struct><struct hash=\"param_x\"><float hash=\"speed\"> \r\n 1.6\t </float></struct></struct>";
        std::fs::write(&file, before).unwrap();
        assert_eq!(param_block(&[file.clone()], &[], "x", &labels()).fields["speed"].0, ParamValue::Float(1.6));
        write_param_field(&file, "param_x", "speed", 0, 2.5, &labels()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before.replace("1.6", "2.5"));
    }

    /// R8: an integer param refuses a value its type cannot hold instead of clamping it
    /// (`300 as u8` would silently write 255, `-1` would write 0).
    #[test]
    fn integer_write_refuses_out_of_range_values() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("vl.prc");
        prc::save(&file, &mixed_root(1.0)).unwrap();
        for value in [300.0, -1.0, f64::NAN] {
            let error = write_param_field(&file, "param_x", "tiny", 0, value, &labels()).unwrap_err();
            assert!(error.to_string().contains("can't be set to"), "{error}");
        }
        assert_eq!(prc::open(&file).unwrap(), mixed_root(1.0), "a refused write changes nothing");
        write_param_field(&file, "param_x", "tiny", 0, 255.0, &labels()).unwrap();
        let error = write_param_field(&file, "param_x", "life", 0, 3e9, &labels()).unwrap_err();
        assert!(error.to_string().contains("from -2147483648 to 2147483647"), "{error}");

        let xml = dir.path().join("vl.prcxml");
        let before = "<struct><struct hash=\"param_x\"><byte hash=\"tiny\">7</byte></struct></struct>";
        std::fs::write(&xml, before).unwrap();
        let error = write_param_field(&xml, "param_x", "tiny", 0, 300.0, &labels()).unwrap_err();
        assert!(error.to_string().contains("can't be set to 300"), "{error}");
        assert_eq!(std::fs::read_to_string(&xml).unwrap(), before);
        write_param_field(&xml, "param_x", "tiny", 0, 200.0, &labels()).unwrap();
        assert_eq!(std::fs::read_to_string(&xml).unwrap(), before.replace(">7<", ">200<"));
    }

    /// Whether prc-rs's open+save reproduces a real, game/mod-written `vl.prc` byte for byte.
    #[test]
    #[ignore = "reads a real moveset checkout"]
    fn real_latios_open_save_is_byte_identical() {
        let real = PathBuf::from("C:/Users/notja/Documents/Coding/SSBU Modding/Movesets/Latios-Moveset/romfs/fighter/samus/param/vl.prc");
        let dir = tempfile::tempdir().unwrap();
        let (copy, saved) = (dir.path().join("vl.prc"), dir.path().join("saved.prc"));
        std::fs::copy(&real, &copy).unwrap();
        prc::save(&saved, &prc::open(&copy).unwrap()).unwrap();
        let (before, after) = (std::fs::read(&copy).unwrap(), std::fs::read(&saved).unwrap());
        assert_eq!(before.len(), after.len(), "saved file length differs");
        assert!(before == after, "saved bytes differ from the original");
    }

    #[test]
    #[ignore = "reads a real moveset checkout"]
    fn real_latios_file() {
        let file = PathBuf::from("C:/Users/notja/Documents/Coding/SSBU Modding/Movesets/Latios-Moveset/romfs/fighter/samus/param/vl.prc");
        let block = param_block(&[file], &[], "lativolt", &HashMap::new());
        assert!(block.present);
        assert_eq!(block.variants.len(), 4);
    }

    /// Latios's shape: `update_int_2(*FIGHTER_KIND_SAMUS, slots, (hash40("param_cshot"), hash40("life"), 60))`.
    #[test]
    fn reads_param_config_overrides() {
        let text = "fn f(marked_slots: Vec<i32>) {
    param_config::update_int_2(*FIGHTER_KIND_SAMUS, marked_slots.clone(),(smash::hash40(\"air_lasso_type\"), 0, 0));
    param_config::update_int_2(*FIGHTER_KIND_SAMUS, marked_slots.clone(), (smash::hash40(\"param_cshot\"), smash::hash40(\"life\"), 60));
    param_config::update_float_2(*FIGHTER_KIND_SAMUS, marked_slots.clone(), (smash::hash40(\"param_cshot\"), smash::hash40(\"speed\"), -1.5));
    param_config::update_int_2(-(*WEAPON_KIND_SAMUS_CSHOT), marked_slots.clone(), (smash::hash40(\"param_cshot\"), smash::hash40(\"angle\"), 0));
}
";
        let found = param_config_overrides(&[(PathBuf::from("lib.rs"), text.to_string())]);
        assert_eq!(found.len(), 4, "{found:#?}");
        assert_eq!(found[0].field, None);
        assert_eq!(found[1].block, "param_cshot");
        assert_eq!(found[1].field.as_deref(), Some("life"));
        assert_eq!(found[1].value, 60.0);
        assert_eq!(found[1].line, 3);
        assert_eq!(found[2].value, -1.5);
        assert!(found[1].is_for_fighter("samus"));
        assert!(!found[3].is_for_fighter("samus"), "a weapon kind is not the fighter's");
        assert_eq!(
            block_overrides(&found, "samus", "param_cshot"),
            [("param_cshot".to_string(), "life".to_string(), 60.0), ("param_cshot".to_string(), "speed".to_string(), -1.5)]
        );
    }
}
