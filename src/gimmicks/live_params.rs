//! Shared protocol and validation for in-match personal-parameter overrides.
//!
//! This module deliberately depends only on serde. It can be included by the
//! host editor and by the Skyline plugin without importing either side's
//! project model. Values are a complete replacement snapshot: a request's
//! `enabled` flag and edits replace the previous snapshot together.

use std::collections::HashMap;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 1;
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_EDITS: usize = 256;
pub const MAX_FIGHTER_NAME_BYTES: usize = 32;
pub const MAX_PATH_BYTES: usize = 256;
pub const MAX_ABS_FLOAT: f32 = 1_000_000.0;
pub const MAX_ABS_INT: i32 = 1_000_000;

/// One parameter value supported by the first live-override slice.
///
/// The desktop conversion only emits signed values that fit the engine getter.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum LiveParamValue {
    Float(f32),
    Int(i32),
}

/// One fighter-relative leaf in a full live-parameter replacement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveParameter {
    pub fighter: String,
    pub path: String,
    pub value: LiveParamValue,
}

/// Version-guarded getter path selected by the runtime plugin.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParameterGetterBackend {
    NativeParamData,
    WorkModuleExport,
}

/// Read-path counters reported by the game since the current snapshot was
/// accepted. Counter relationships are intentionally not part of validation:
/// concurrent getter calls can race an accepted-snapshot reset.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterHealth {
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub getter_backend: Option<ParameterGetterBackend>,
    pub float_wrapper_calls: u64,
    pub int_wrapper_calls: u64,
    pub identity_matches: u64,
    pub identity_misses: u64,
    pub identity_lock_busy: u64,
    pub overlay_lock_busy: u64,
    pub overlay_disabled: u64,
    pub parameter_key_matches: u64,
    pub parameter_key_misses: u64,
    pub type_mismatches: u64,
    pub overrides_returned: u64,
}

/// Per-refresh pointer ownership accumulator. A key is usable only when every
/// observation agrees on one owner; ambiguity never resolves by insertion
/// order. Rebuild this value each refresh to discard stale lifecycle mappings.
#[allow(dead_code)] // Runtime plugin consumer is compiled separately from the desktop crate.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UniqueAddressMap {
    owners: HashMap<usize, Option<usize>>,
}

#[allow(dead_code)] // Runtime plugin consumer is compiled separately from the desktop crate.
impl UniqueAddressMap {
    pub fn insert(&mut self, address: usize, owner: usize) {
        self.owners
            .entry(address)
            .and_modify(|current| {
                if current.is_some_and(|existing| existing != owner) {
                    *current = None;
                }
            })
            .or_insert(Some(owner));
    }

    pub fn get(&self, address: usize) -> Option<usize> {
        self.owners.get(&address).copied().flatten()
    }

    pub fn is_ambiguous(&self, address: usize) -> bool {
        self.owners.get(&address).is_some_and(Option::is_none)
    }

    pub fn clear(&mut self) {
        self.owners.clear();
    }
}

/// Complete replacement for the live parameter override table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u8,
    pub revision: u64,
    pub enabled: bool,
    pub edits: Vec<LiveParameter>,
}

impl Request {
    pub fn validate(&self) -> Result<(), String> {
        validate_request(self)
    }
}

/// Named engine lookup keys for a supported personal-PRC spelling.
///
/// WorkModule getters take `(parent, child)` hash40 values. `parent` and
/// `child` are kept as names here; only the plugin turns them into hashes.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineParamPath {
    pub parent: String,
    pub child: String,
}

/// Validate the bounded request and all its edits before the plugin publishes
/// any part of the replacement.
pub fn validate_request(request: &Request) -> Result<(), String> {
    if request.version != VERSION {
        return Err(format!(
            "unsupported live-parameter version {}; expected {VERSION}",
            request.version
        ));
    }
    if request.edits.len() > MAX_EDITS {
        return Err(format!(
            "live-parameter request has {} edits; maximum is {MAX_EDITS}",
            request.edits.len()
        ));
    }

    let mut resolved_keys = HashSet::with_capacity(request.edits.len());
    for (index, edit) in request.edits.iter().enumerate() {
        validate_fighter_name(&edit.fighter).map_err(|error| format!("edit {index}: {error}"))?;
        if edit.path.len() > MAX_PATH_BYTES {
            return Err(format!("edit {index}: path exceeds {MAX_PATH_BYTES} bytes"));
        }
        let engine_path =
            classify_engine_path(&edit.path).map_err(|error| format!("edit {index}: {error}"))?;
        if !resolved_keys.insert((edit.fighter.clone(), engine_path.parent, engine_path.child)) {
            return Err(format!(
                "edit {index}: duplicate engine parameter lookup for {}:{}",
                edit.fighter, edit.path
            ));
        }
        match edit.value {
            LiveParamValue::Float(value) => {
                if !value.is_finite() || value.abs() > MAX_ABS_FLOAT {
                    return Err(format!(
                        "edit {index}: float must be finite and within ±{MAX_ABS_FLOAT}"
                    ));
                }
            }
            LiveParamValue::Int(value) => {
                if value.unsigned_abs() > MAX_ABS_INT as u32 {
                    return Err(format!(
                        "edit {index}: integer must stay within ±{MAX_ABS_INT}"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Check one fighter name without relying on any host or game registry.
pub fn validate_fighter_name(fighter: &str) -> Result<(), String> {
    if fighter.is_empty() || fighter.len() > MAX_FIGHTER_NAME_BYTES {
        return Err(format!(
            "fighter name must contain 1..={MAX_FIGHTER_NAME_BYTES} bytes"
        ));
    }
    if !fighter
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(format!(
            "fighter name {fighter:?} must use lowercase ASCII letters, digits, or underscore"
        ));
    }
    Ok(())
}

/// Map the deliberately small supported path grammar to the exact engine
/// `(parent, child)` names.
///
/// Accepted paths are a scalar directly in `vl.prc` (`leaf`), a row-zero
/// parameter group (`group[0].leaf`), or a struct group (`group.leaf`). A root
/// scalar maps to `(hash40(leaf), 0)`, matching `WorkModule::get_param_*`'s
/// root-parameter convention. Lists other than row zero and deeper paths do
/// not have a proven one-call WorkModule mapping.
pub fn classify_engine_path(path: &str) -> Result<EngineParamPath, String> {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return Err("path must be non-empty and within the path length limit".into());
    }
    if path.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err("path cannot contain whitespace".into());
    }

    let segments: Vec<&str> = path.split('.').collect();
    if segments.len() == 1 {
        let child = parse_named_segment(segments[0], "scalar leaf")?;
        return Ok(EngineParamPath {
            parent: child,
            child: String::new(),
        });
    }
    if segments.len() != 2 {
        return Err(
            "unsupported path depth; only a root scalar or one group and leaf are supported".into(),
        );
    }

    let (parent, index) = parse_group_segment(segments[0])?;
    if let Some(index) = index {
        if index != 0 {
            return Err(format!(
                "unsupported list index [{index}]; live parameters support row [0] only"
            ));
        }
    }
    let child = parse_named_segment(segments[1], "leaf")?;
    Ok(EngineParamPath { parent, child })
}

fn parse_group_segment(segment: &str) -> Result<(String, Option<usize>), String> {
    let Some(open) = segment.find('[') else {
        if segment.contains(']') {
            return Err("unsupported malformed group index".into());
        }
        return Ok((validate_label(segment, "group")?, None));
    };
    if !segment.ends_with(']') || segment[open + 1..segment.len() - 1].contains(['[', ']']) {
        return Err("unsupported malformed group index".into());
    }
    let name = validate_label(&segment[..open], "group")?;
    let digits = &segment[open + 1..segment.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("unsupported malformed group index".into());
    }
    let index = digits
        .parse::<usize>()
        .map_err(|_| "unsupported out-of-range group index".to_string())?;
    Ok((name, Some(index)))
}

fn parse_named_segment(segment: &str, role: &str) -> Result<String, String> {
    if segment.contains(['[', ']']) {
        return Err(format!("unsupported index on {role}"));
    }
    validate_label(segment, role)
}

fn validate_label(label: &str, role: &str) -> Result<String, String> {
    if label.is_empty() {
        return Err(format!("{role} name is empty"));
    }
    if label.starts_with("0x")
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(format!(
            "unsupported {role} name {label:?}; live engine paths require named ASCII labels"
        ));
    }
    Ok(label.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_root_scalar_and_supported_group_shapes() {
        assert_eq!(
            classify_engine_path("limit_max").unwrap(),
            EngineParamPath {
                parent: "limit_max".into(),
                child: String::new(),
            }
        );
        assert_eq!(
            classify_engine_path("param_special_lw[0].limit_gauge_add").unwrap(),
            EngineParamPath {
                parent: "param_special_lw".into(),
                child: "limit_gauge_add".into(),
            }
        );
        assert_eq!(
            classify_engine_path("param_special_lw.limit_gauge_add").unwrap(),
            EngineParamPath {
                parent: "param_special_lw".into(),
                child: "limit_gauge_add".into(),
            }
        );
    }

    #[test]
    fn refuses_paths_without_a_proven_single_getter_mapping() {
        for (path, expected) in [
            ("param_special_lw[1].limit_gauge_add", "index [1]"),
            ("param_special_lw[0].nested.leaf", "depth"),
            ("param_special_lw[0]", "index on scalar leaf"),
            ("param_special_lw[0].limit_gauge_add[0]", "index on leaf"),
            ("0x1234.limit", "named ASCII labels"),
        ] {
            let error = classify_engine_path(path).unwrap_err();
            assert!(error.contains(expected), "{path}: {error}");
        }
    }

    #[test]
    fn request_validation_checks_count_values_and_fighter_names() {
        let mut request = Request {
            version: VERSION,
            revision: 5,
            enabled: true,
            edits: vec![LiveParameter {
                fighter: "cloud".into(),
                path: "param_special_lw[0].limit_gauge_add".into(),
                value: LiveParamValue::Float(1.25),
            }],
        };
        assert!(validate_request(&request).is_ok());

        request.edits[0].value = LiveParamValue::Float(f32::NAN);
        assert!(validate_request(&request).unwrap_err().contains("finite"));
        request.edits[0].value = LiveParamValue::Int(i32::MAX);
        assert!(validate_request(&request).unwrap_err().contains("within"));
        request.edits[0].fighter = "Cloud".into();
        assert!(validate_request(&request)
            .unwrap_err()
            .contains("lowercase"));
    }

    #[test]
    fn request_validation_rejects_alias_paths_with_the_same_engine_key() {
        let request = Request {
            version: VERSION,
            revision: 8,
            enabled: true,
            edits: vec![
                LiveParameter {
                    fighter: "cloud".into(),
                    path: "param_special_lw[0].limit_gauge_add".into(),
                    value: LiveParamValue::Float(1.0),
                },
                LiveParameter {
                    fighter: "cloud".into(),
                    path: "param_special_lw.limit_gauge_add".into(),
                    value: LiveParamValue::Float(2.0),
                },
            ],
        };

        let error = request.validate().unwrap_err();
        assert!(error.contains("duplicate engine parameter lookup"));
    }

    #[test]
    fn parameter_health_uses_camel_case_json_and_round_trips() {
        let health = ParameterHealth {
            revision: 12,
            getter_backend: Some(ParameterGetterBackend::NativeParamData),
            float_wrapper_calls: 34,
            int_wrapper_calls: 56,
            identity_matches: 78,
            identity_misses: 90,
            identity_lock_busy: 2,
            overlay_lock_busy: 3,
            overlay_disabled: 4,
            parameter_key_matches: 5,
            parameter_key_misses: 6,
            type_mismatches: 7,
            overrides_returned: 8,
        };
        let serialized = serde_json::to_value(&health).unwrap();
        let object = serialized.as_object().unwrap();
        let expected_fields = [
            "revision",
            "getterBackend",
            "floatWrapperCalls",
            "intWrapperCalls",
            "identityMatches",
            "identityMisses",
            "identityLockBusy",
            "overlayLockBusy",
            "overlayDisabled",
            "parameterKeyMatches",
            "parameterKeyMisses",
            "typeMismatches",
            "overridesReturned",
        ];
        assert_eq!(object.len(), expected_fields.len());
        for field in expected_fields {
            assert!(object.contains_key(field), "missing JSON field {field}");
        }
        assert!(!object.contains_key("float_wrapper_calls"));
        assert_eq!(
            serde_json::from_value::<ParameterHealth>(serialized).unwrap(),
            health
        );
    }

    #[test]
    fn parameter_health_rejects_unknown_fields_and_malformed_counters() {
        let baseline = serde_json::to_value(ParameterHealth {
            revision: 1,
            ..ParameterHealth::default()
        })
        .unwrap();

        let mut unknown = baseline.clone();
        unknown["unexpectedCounter"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ParameterHealth>(unknown).is_err());
        let mut unknown_backend = baseline.clone();
        unknown_backend["getterBackend"] = serde_json::json!("guessedOffset");
        assert!(serde_json::from_value::<ParameterHealth>(unknown_backend).is_err());
        let mut old_health = baseline.clone();
        old_health.as_object_mut().unwrap().remove("getterBackend");
        assert_eq!(
            serde_json::from_value::<ParameterHealth>(old_health).unwrap(),
            ParameterHealth {
                revision: 1,
                ..ParameterHealth::default()
            }
        );

        let mut incomplete = baseline.clone();
        incomplete
            .as_object_mut()
            .unwrap()
            .remove("floatWrapperCalls");
        assert!(serde_json::from_value::<ParameterHealth>(incomplete).is_err());

        for (field, malformed) in [
            ("revision", serde_json::json!(-1)),
            ("revision", serde_json::json!(1.5)),
            ("identityMatches", serde_json::json!(-1)),
            ("overridesReturned", serde_json::json!("many")),
        ] {
            let mut value = baseline.clone();
            value[field] = malformed;
            assert!(
                serde_json::from_value::<ParameterHealth>(value).is_err(),
                "accepted malformed {field}"
            );
        }
    }

    #[test]
    fn param_data_owner_map_rejects_ambiguity_and_rebuilds_for_lifecycle() {
        let mut map = UniqueAddressMap::default();
        map.insert(0x1000, 0x2000);
        assert_eq!(map.get(0x1000), Some(0x2000));
        map.insert(0x1000, 0x2000);
        assert_eq!(map.get(0x1000), Some(0x2000));
        map.insert(0x1000, 0x3000);
        assert_eq!(map.get(0x1000), None);
        assert!(map.is_ambiguous(0x1000));
        map.insert(0x1000, 0x2000);
        assert_eq!(
            map.get(0x1000),
            None,
            "ambiguity must not be order-dependent"
        );
        let mut reverse = UniqueAddressMap::default();
        reverse.insert(0x1000, 0x3000);
        reverse.insert(0x1000, 0x2000);
        reverse.insert(0x1000, 0x3000);
        assert_eq!(reverse.get(0x1000), None);

        // A new per-refresh map drops stale pointer ownership and permits a
        // legitimate allocator address reuse only after lifecycle reproof.
        let mut refreshed = UniqueAddressMap::default();
        refreshed.insert(0x1000, 0x4000);
        assert_eq!(refreshed.get(0x1000), Some(0x4000));
        refreshed.clear();
        assert_eq!(refreshed.get(0x1000), None);
    }
}
