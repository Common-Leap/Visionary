//! Stable spelling for nested personal-prc paths.
//!
//! A personal file like `fighter/cloud/param/vl.prc` holds nested groups
//! (`param_special_lw`) and indexed lists, so a flat field name is not enough
//! to name a tunable. The spelling is dotted fields with an optional `[N]`
//! list index on any segment:
//!
//! ```text
//! param_special_lw[0].limit_gauge_add
//! ```
//!
//! A field is a param label (`limit_gauge_add`) or a raw `0x…` hash for a
//! field whose label is not yet known. Raw hashes keep an unknown field
//! addressable, but verification (slice F4.3) treats them as unresolved: an
//! evidence manifest must cite a real label, never a bare hash.
//!
//! Slice F0.1 builds the project-model integration on this spelling; this
//! module only parses, formats, and validates it.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One `field[index]` step of a nested path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Param label, or a raw `0x…` hash when the label is unknown.
    pub field: String,
    /// List position when the field holds a list, `None` for a plain field.
    pub index: Option<usize>,
}

/// A typed nested path: struct fields with per-segment list indices.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NestedPath(pub Vec<Segment>);

impl NestedPath {
    /// Parse the dotted spelling. Empty paths and empty segments are rejected,
    /// as are unclosed brackets and non-numeric indices.
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        if raw.is_empty() {
            anyhow::bail!("nested path is empty");
        }
        raw.split('.')
            .enumerate()
            .map(|(position, segment)| parse_segment(segment, position, raw))
            .collect::<anyhow::Result<Vec<_>>>()
            .map(NestedPath)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for NestedPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for segment in &self.0 {
            if !first {
                formatter.write_str(".")?;
            }
            first = false;
            formatter.write_str(&segment.field)?;
            if let Some(index) = segment.index {
                write!(formatter, "[{index}]")?;
            }
        }
        Ok(())
    }
}

/// The stable serialized spelling is the dotted string itself
/// (`param_special_lw[0].limit_gauge_add`), decided by slice F0.1 so project
/// files, manifests, and UI text all name one tunable the same way.
/// Deserialization re-parses, so an invalid spelling fails at load rather than
/// travelling as a string that breaks later. An empty path has no serialized
/// form: it formats as `""`, which parsing rejects.
impl Serialize for NestedPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for NestedPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        NestedPath::parse(&raw).map_err(serde::de::Error::custom)
    }
}

fn parse_segment(segment: &str, position: usize, raw: &str) -> anyhow::Result<Segment> {
    let where_in = || format!("segment {} of {raw:?}", position + 1);
    if segment.is_empty() {
        anyhow::bail!("{} is empty", where_in());
    }
    match segment.find('[') {
        None => {
            if segment.contains(']') {
                anyhow::bail!("{} has ']' without '['", where_in());
            }
            check_field(segment, &where_in)?;
            Ok(Segment {
                field: segment.to_string(),
                index: None,
            })
        }
        Some(bracket) => {
            let field = &segment[..bracket];
            check_field(field, &where_in)?;
            let rest = &segment[bracket..];
            if !rest.ends_with(']') || rest.contains('.') {
                anyhow::bail!("{} has a malformed '[N]' suffix", where_in());
            }
            let digits = &rest[1..rest.len() - 1];
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                anyhow::bail!("{} has a non-numeric list index", where_in());
            }
            let index: usize = digits
                .parse()
                .map_err(|_| anyhow::anyhow!("{} has an out-of-range list index", where_in()))?;
            Ok(Segment {
                field: field.to_string(),
                index: Some(index),
            })
        }
    }
}

fn check_field(field: &str, where_in: &dyn Fn() -> String) -> anyhow::Result<()> {
    if field.is_empty() {
        anyhow::bail!("{} names no field", where_in());
    }
    if field.bytes().any(|byte| byte == b']') {
        anyhow::bail!("{} has ']' without '['", where_in());
    }
    Ok(())
}

/// True when the field is a raw `0x…` hash rather than a resolved label.
pub fn is_raw_hash(field: &str) -> bool {
    field.len() > 2
        && field.starts_with("0x")
        && field[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Resolve a manifest-authored field to the hash40 the prc file stores.
///
/// Labels hash by name; raw `0x…` hashes parse directly so unknown fields stay
/// addressable. Returns `None` for a malformed raw hash.
pub fn field_hash(field: &str) -> Option<u64> {
    if is_raw_hash(field) {
        u64::from_str_radix(&field[2..], 16).ok()
    } else {
        Some(hash40::hash40(field).0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_cloud_path_round_trips() {
        let path = NestedPath::parse("param_special_lw[0].limit_gauge_add").unwrap();
        assert_eq!(path.0.len(), 2);
        assert_eq!(path.0[0].field, "param_special_lw");
        assert_eq!(path.0[0].index, Some(0));
        assert_eq!(path.0[1].field, "limit_gauge_add");
        assert_eq!(path.0[1].index, None);
        assert_eq!(path.to_string(), "param_special_lw[0].limit_gauge_add");
    }

    #[test]
    fn malformed_paths_name_their_segment() {
        for raw in [
            "",
            "param_special_lw..limit_gauge_add",
            ".limit_gauge_add",
            "param_special_lw[0.limit_gauge_add",
            "param_special_lw[x].limit_gauge_add",
            "param_special_lw[].limit_gauge_add",
            "param_special_lw]0[.limit_gauge_add",
        ] {
            let error = NestedPath::parse(raw).unwrap_err();
            assert!(!error.to_string().is_empty(), "{raw:?} needs a reason");
        }
    }

    #[test]
    fn labels_hash_and_raw_hashes_parse() {
        assert_eq!(
            field_hash("limit_gauge_add"),
            Some(hash40::hash40("limit_gauge_add").0)
        );
        assert_eq!(field_hash("0x1234abcd"), Some(0x1234abcd));
        assert!(is_raw_hash("0x1234abcd"));
        assert!(!is_raw_hash("limit_gauge_add"));
    }

    #[test]
    fn the_serialized_spelling_is_the_dotted_string() {
        let path = NestedPath::parse("param_special_lw[0].limit_gauge_add").unwrap();
        let json = serde_json::to_string(&path).unwrap();
        assert_eq!(json, "\"param_special_lw[0].limit_gauge_add\"");
        let back: NestedPath = serde_json::from_str(&json).unwrap();
        assert_eq!(back, path);
    }

    #[test]
    fn an_invalid_spelling_fails_at_load() {
        let error = serde_json::from_str::<NestedPath>("\"param_special_lw[x].limit_gauge_add\"")
            .unwrap_err();
        assert!(!error.to_string().is_empty(), "load needs a reason");
    }
}
