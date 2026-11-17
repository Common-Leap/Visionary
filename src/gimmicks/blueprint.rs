//! Portable snapshots of a fighter's personal parameters, behavior, and meters.
//!
//! A blueprint stores authored values and typed graph data only. Preparing it
//! for a target validates every component and returns a fresh sparse patch;
//! callers remain responsible for merging that patch into the target fighter.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::mod_project::{ParamMod, ParamValue};

use super::behavior::BehaviorGraph;
use super::meter::{self, MeterBinding};
use super::params::{self, PersonalEdit};
use super::paths::NestedPath;

const BLUEPRINT_KIND: &str = "visionary.gimmick-blueprint";
const SCHEMA_VERSION: u32 = 1;
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_NAME_CHARS: usize = 128;

/// A complete reusable setup, scoped to one source fighter's authored data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GimmickBlueprint {
    pub kind: String,
    pub schema_version: u32,
    pub name: String,
    pub source_fighter: String,
    pub personal: BTreeMap<String, ParamValue>,
    pub behavior: BehaviorGraph,
    pub meters: Vec<MeterBinding>,
}

/// A validated, detached set of edits ready to merge into a target fighter.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedGimmicks {
    pub personal: ParamMod,
    pub behavior: BehaviorGraph,
    pub meters: Vec<MeterBinding>,
}

impl GimmickBlueprint {
    /// Snapshot only this fighter's personal `vl.prc` sparse edits.
    pub fn from_fighter(
        name: &str,
        fighter: &str,
        params: &ParamMod,
        behavior: BehaviorGraph,
        meters: Vec<MeterBinding>,
    ) -> Result<Self, String> {
        let blueprint = Self {
            kind: BLUEPRINT_KIND.to_owned(),
            schema_version: SCHEMA_VERSION,
            name: name.to_owned(),
            source_fighter: fighter.to_owned(),
            personal: params::personal_edits(params, fighter).clone(),
            behavior,
            meters,
        };
        blueprint.validate()?;
        Ok(blueprint)
    }

    /// Decode a bounded document, rejecting unknown nested graph and meter keys.
    pub fn decode(json: &str) -> Result<Self, String> {
        if json.len() > MAX_DOCUMENT_BYTES {
            return Err(format!(
                "gimmick blueprint exceeds {MAX_DOCUMENT_BYTES} bytes"
            ));
        }
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|error| format!("invalid gimmick blueprint JSON: {error}"))?;
        reject_unknown_nested_fields(&value)?;

        // BehaviorGraph intentionally remains permissive for old project files.
        // Reuse the stricter template importer before deserializing this nested
        // graph, so misspelled graph fields cannot be silently discarded.
        let root = value
            .as_object()
            .ok_or_else(|| "gimmick blueprint must be a JSON object".to_string())?;
        let graph = root
            .get("behavior")
            .ok_or_else(|| "gimmick blueprint is missing behavior".to_string())?;
        let strict_graph = serde_json::json!({
            "schema_version": 1,
            "name": "Blueprint behavior check",
            "description": "blueprint behavior validation",
            "source_fighter": root.get("source_fighter").cloned().unwrap_or(serde_json::Value::Null),
            "graph": graph.clone(),
        });
        let graph_json = serde_json::to_string(&strict_graph)
            .map_err(|error| format!("could not validate blueprint behavior: {error}"))?;
        let graph_model: BehaviorGraph = serde_json::from_value(graph.clone())
            .map_err(|error| format!("blueprint behavior: {error}"))?;
        if !graph_model.is_empty() {
            super::templates::BehaviorTemplate::decode(&graph_json)
                .map_err(|error| format!("blueprint behavior: {error}"))?;
        }

        let blueprint: Self = serde_json::from_value(value)
            .map_err(|error| format!("invalid gimmick blueprint document: {error}"))?;
        blueprint.validate()?;
        Ok(blueprint)
    }

    /// Encode a validated blueprint as stable, readable JSON.
    pub fn encode(&self) -> Result<String, String> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("could not encode gimmick blueprint: {error}"))?;
        if json.len() > MAX_DOCUMENT_BYTES {
            return Err(format!(
                "gimmick blueprint exceeds {MAX_DOCUMENT_BYTES} bytes"
            ));
        }
        Ok(json)
    }

    /// Prepare this setup for one registered target fighter without changing
    /// the blueprint or the supplied base tree.
    pub fn prepare(
        &self,
        target: &str,
        base: Option<&prc::ParamStruct>,
    ) -> Result<PreparedGimmicks, String> {
        self.validate()?;
        if super::registry::fighter_kind_symbol(target).is_none() {
            return Err(format!("unknown target fighter {target:?}"));
        }

        let behavior = if self.behavior.is_empty() {
            self.behavior.clone()
        } else {
            super::templates::copy_for_fighter(&self.behavior, &self.source_fighter, target)
                .map_err(|error| format!("behavior for target {target}: {error}"))?
        };

        let mut meters = self.meters.clone();
        for binding in &mut meters {
            binding.fighter = target.to_owned();
            let errors = meter::validate(binding);
            if !errors.is_empty() {
                return Err(format!(
                    "meter {:?} for target {target}: {}",
                    meter_symbol(&binding.value),
                    errors.join("; ")
                ));
            }
        }

        let mut personal = ParamMod::default();
        if !self.personal.is_empty() && base.is_none() {
            return Err(format!(
                "target {target}: a base personal parameter tree is required for blueprint edits"
            ));
        }
        if let Some(base) = base {
            for (spelling, value) in &self.personal {
                let path = NestedPath::parse(spelling).map_err(|error| {
                    format!("target {target}: personal path {spelling:?}: {error}")
                })?;
                if !params::path_is_resolved(&path) {
                    return Err(format!(
                        "target {target}: personal path {spelling:?} is unresolved"
                    ));
                }
                validate_param_value(spelling, value)?;
                let base_value = params::read_leaf(base, target, &path)
                    .map_err(|error| format!("target {target}: {error}"))?;
                if !same_param_variant(value, &base_value) {
                    return Err(format!(
                        "target {target}: personal path {spelling:?} expects {}, but the target base contains {}",
                        params::value_kind_name(&base_value),
                        params::value_kind_name(value),
                    ));
                }
                let edit = PersonalEdit::from_path(path, *value);
                params::record_personal_edit(
                    &mut personal,
                    target,
                    &edit.path,
                    edit.value,
                    Some(base_value),
                )
                .map_err(|error| format!("target {target}: personal path {spelling:?}: {error}"))?;
            }
        }

        Ok(PreparedGimmicks {
            personal,
            behavior,
            meters,
        })
    }

    fn validate(&self) -> Result<(), String> {
        if self.kind != BLUEPRINT_KIND {
            return Err(format!(
                "unsupported gimmick blueprint kind {:?}",
                self.kind
            ));
        }
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported gimmick blueprint schema version {}",
                self.schema_version
            ));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > MAX_NAME_CHARS {
            return Err(format!(
                "blueprint name must be non-empty and at most {MAX_NAME_CHARS} characters"
            ));
        }
        if super::registry::fighter_kind_symbol(&self.source_fighter).is_none() {
            return Err(format!("unknown source fighter {:?}", self.source_fighter));
        }
        if !self.behavior.is_empty() {
            super::templates::BehaviorTemplate::from_graph(
                &self.name,
                Some(&self.source_fighter),
                self.behavior.clone(),
            )
            .map_err(|error| format!("source behavior: {error}"))?;
        }

        for (path_text, value) in &self.personal {
            let path = NestedPath::parse(path_text)
                .map_err(|error| format!("source personal path {path_text:?}: {error}"))?;
            if !params::path_is_resolved(&path) {
                return Err(format!("source personal path {path_text:?} is unresolved"));
            }
            validate_param_value(path_text, value)?;
        }
        for (index, binding) in self.meters.iter().enumerate() {
            if binding.fighter != self.source_fighter {
                return Err(format!(
                    "meter {index} belongs to {:?}, not source fighter {:?}",
                    binding.fighter, self.source_fighter
                ));
            }
            let errors = meter::validate(binding);
            if !errors.is_empty() {
                return Err(format!("meter {index}: {}", errors.join("; ")));
            }
        }
        Ok(())
    }
}

fn validate_param_value(path: &str, value: &ParamValue) -> Result<(), String> {
    if let ParamValue::Float(value) = value {
        if !value.is_finite() {
            return Err(format!("personal path {path:?} has a non-finite float"));
        }
    }
    Ok(())
}

fn same_param_variant(left: &ParamValue, right: &ParamValue) -> bool {
    matches!(
        (left, right),
        (ParamValue::Bool(_), ParamValue::Bool(_))
            | (ParamValue::I8(_), ParamValue::I8(_))
            | (ParamValue::U8(_), ParamValue::U8(_))
            | (ParamValue::I16(_), ParamValue::I16(_))
            | (ParamValue::U16(_), ParamValue::U16(_))
            | (ParamValue::I32(_), ParamValue::I32(_))
            | (ParamValue::U32(_), ParamValue::U32(_))
            | (ParamValue::Float(_), ParamValue::Float(_))
            | (ParamValue::Hash(_), ParamValue::Hash(_))
    )
}

fn meter_symbol(value: &super::meter::MeterValue) -> &str {
    use super::meter::MeterValue;
    match value {
        MeterValue::Float { symbol, .. }
        | MeterValue::Integer { symbol, .. }
        | MeterValue::Flag { symbol }
        | MeterValue::Pips { symbol, .. }
        | MeterValue::Cycle { symbol, .. } => symbol,
    }
}

fn reject_unknown_nested_fields(value: &serde_json::Value) -> Result<(), String> {
    let root = value
        .as_object()
        .ok_or_else(|| "gimmick blueprint must be a JSON object".to_string())?;
    if let Some(personal) = root.get("personal").and_then(serde_json::Value::as_object) {
        for (path, value) in personal {
            let parsed = NestedPath::parse(path)
                .map_err(|error| format!("personal path {path:?}: {error}"))?;
            if !params::path_is_resolved(&parsed) {
                return Err(format!("personal path {path:?} is unresolved"));
            }
            let Some(object) = value.as_object() else {
                return Err(format!("personal path {path:?} has an invalid typed value"));
            };
            if object.len() != 1 {
                return Err(format!("personal path {path:?} must have one typed value"));
            }
            let (variant, value) = object.iter().next().expect("one typed value");
            if ![
                "Bool", "I8", "U8", "I16", "U16", "I32", "U32", "Float", "Hash",
            ]
            .contains(&variant.as_str())
            {
                return Err(format!(
                    "personal path {path:?} has unknown value kind {variant:?}"
                ));
            }
            if variant == "Float" && !value.as_f64().is_some_and(f64::is_finite) {
                return Err(format!("personal path {path:?} has a non-finite float"));
            }
        }
    }
    if let Some(behavior) = root.get("behavior").and_then(serde_json::Value::as_object) {
        reject_keys(
            behavior,
            &["initial", "states", "vars", "transitions", "emission_plan"],
            "behavior",
        )?;
        if let Some(plan) = behavior
            .get("emission_plan")
            .and_then(serde_json::Value::as_object)
        {
            reject_keys(
                plan,
                &["bindings", "status_triggers", "resource_triggers"],
                "behavior.emission_plan",
            )?;
        }
    }
    if let Some(meters) = root.get("meters").and_then(serde_json::Value::as_array) {
        for (index, meter) in meters.iter().enumerate() {
            let Some(object) = meter.as_object() else {
                continue;
            };
            reject_keys(
                object,
                &["fighter", "enabled", "style", "value"],
                &format!("meters[{index}]"),
            )?;
            if let Some(value) = object.get("value").and_then(serde_json::Value::as_object) {
                let kind = value
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let fields: &[&str] = match kind {
                    "float" | "integer" => &["kind", "symbol", "min", "max"],
                    "flag" => &["kind", "symbol"],
                    "pips" => &["kind", "symbol", "count"],
                    "cycle" => &["kind", "symbol", "labels"],
                    _ => &[],
                };
                reject_keys(value, fields, &format!("meters[{index}].value"))?;
            }
        }
    }
    Ok(())
}

fn reject_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    path: &str,
) -> Result<(), String> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("unknown field {key:?} at {path}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::templates::Preset;
    use prc::hash40::Hash40;
    use prc::{ParamKind, ParamStruct};

    const PERSONAL_KEY: &str = "fighter/cloud/param/vl.prc";
    const PERSONAL_PATH: &str = "walk_speed_mul";

    fn cloud_meter() -> MeterBinding {
        MeterBinding {
            fighter: "cloud".into(),
            enabled: true,
            style: super::super::meter::MeterStyle::Bar,
            value: super::super::meter::MeterValue::Float {
                symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                min: 0.0,
                max: 100.0,
            },
        }
    }

    fn base_tree(kind: ParamKind, name: &str) -> ParamStruct {
        ParamStruct(vec![(Hash40(hash40::hash40(name).0), kind)])
    }

    fn source_params() -> ParamMod {
        ParamMod {
            files: BTreeMap::from([
                (
                    PERSONAL_KEY.into(),
                    BTreeMap::from([(PERSONAL_PATH.into(), ParamValue::Float(1.25))]),
                ),
                (
                    "param/fighter_param.prc".into(),
                    BTreeMap::from([("weight".into(), ParamValue::Float(100.0))]),
                ),
            ]),
        }
    }

    fn blueprint() -> GimmickBlueprint {
        GimmickBlueprint::from_fighter(
            "Cloud setup",
            "cloud",
            &source_params(),
            Preset::Charge.graph(),
            vec![cloud_meter()],
        )
        .unwrap()
    }

    #[test]
    fn snapshot_reads_only_owner_personal_edits_and_never_mutates_source() {
        let source = source_params();
        let snapshot = GimmickBlueprint::from_fighter(
            "Cloud setup",
            "cloud",
            &source,
            Preset::Charge.graph(),
            vec![cloud_meter()],
        )
        .unwrap();
        assert_eq!(
            snapshot.personal,
            BTreeMap::from([(PERSONAL_PATH.into(), ParamValue::Float(1.25))])
        );
        assert_eq!(source, source_params());
    }

    #[test]
    fn portable_behavior_prepares_for_another_fighter_without_source_maps() {
        let mut setup = blueprint();
        setup.name = "Renamed setup".into();
        setup.personal.clear();
        setup.meters.clear();
        let prepared = setup.prepare("mario", None).unwrap();
        assert!(prepared.personal.is_empty());
        assert_eq!(prepared.behavior, setup.behavior);
        assert!(prepared.meters.is_empty());
    }

    #[test]
    fn native_cloud_behavior_and_meter_symbols_reject_cross_fighter_prepare() {
        let mut native = Preset::KoTally.graph();
        let cloud_article = super::super::registry::entries_for(
            "cloud",
            super::super::registry::IdentifierKind::Article,
        )
        .next()
        .expect("Cloud has a registered article")
        .symbol;
        native.transitions[0]
            .actions
            .push(crate::gimmicks::behavior::Action::GenerateArticle {
                symbol: cloud_article.into(),
            });
        let native_setup =
            GimmickBlueprint::from_fighter("Donor", "cloud", &ParamMod::default(), native, vec![])
                .unwrap();
        assert!(native_setup
            .prepare("mario", None)
            .unwrap_err()
            .contains("allowlisted"));
        assert!(blueprint()
            .prepare("mario", None)
            .unwrap_err()
            .contains("LIMIT_GAUGE"));
    }

    #[test]
    fn personal_edits_require_matching_target_leaf_types_and_clear_base_equal_values() {
        let mut setup = blueprint();
        setup.meters.clear();
        let matching = base_tree(ParamKind::Float(2.0), PERSONAL_PATH);
        let prepared = setup.prepare("mario", Some(&matching)).unwrap();
        assert_eq!(
            params::personal_edits(&prepared.personal, "mario")[PERSONAL_PATH],
            ParamValue::Float(1.25)
        );
        assert!(params::personal_edits(&prepared.personal, "cloud").is_empty());
        assert_eq!(
            params::read_leaf(
                &matching,
                "mario",
                &NestedPath::parse(PERSONAL_PATH).unwrap()
            )
            .unwrap(),
            ParamValue::Float(2.0)
        );

        let equal = base_tree(ParamKind::Float(1.25), PERSONAL_PATH);
        assert!(setup
            .prepare("mario", Some(&equal))
            .unwrap()
            .personal
            .is_empty());
        assert!(setup
            .prepare("mario", None)
            .unwrap_err()
            .contains("base personal"));
        assert!(setup
            .prepare("mario", Some(&ParamStruct(vec![])))
            .unwrap_err()
            .contains(PERSONAL_PATH));
        assert!(setup
            .prepare("mario", Some(&base_tree(ParamKind::I32(2), PERSONAL_PATH)))
            .unwrap_err()
            .contains("target base contains"));
    }

    #[test]
    fn nonfinite_personal_values_fail_before_prepare() {
        let mut source = ParamMod::default();
        source.files.insert(
            PERSONAL_KEY.into(),
            BTreeMap::from([(PERSONAL_PATH.into(), ParamValue::Float(f32::NAN))]),
        );
        assert!(GimmickBlueprint::from_fighter(
            "Bad",
            "cloud",
            &source,
            Preset::Charge.graph(),
            vec![]
        )
        .unwrap_err()
        .contains("non-finite"));
    }

    #[test]
    fn strict_json_checks_version_kind_nested_fields_and_document_bounds() {
        let setup = blueprint();
        let json = setup.encode().unwrap();
        assert_eq!(GimmickBlueprint::decode(&json).unwrap(), setup);
        let unknown = json.replace(
            "\"schema_version\": 1",
            "\"schema_version\": 1, \"extra\": true",
        );
        assert!(GimmickBlueprint::decode(&unknown)
            .unwrap_err()
            .contains("unknown field"));
        let bad_meter = json.replace(
            "\"kind\": \"float\"",
            "\"kind\": \"float\", \"surprise\": true",
        );
        assert!(GimmickBlueprint::decode(&bad_meter)
            .unwrap_err()
            .contains("unknown field"));
        let bad_personal = json.replace("\"Float\": 1.25", "\"Float\": 1.25, \"surprise\": true");
        assert!(GimmickBlueprint::decode(&bad_personal)
            .unwrap_err()
            .contains("one typed value"));
        let bad_graph = json.replace(
            "\"initial\": \"charging\"",
            "\"initial\": \"charging\", \"extra\": true",
        );
        assert!(GimmickBlueprint::decode(&bad_graph)
            .unwrap_err()
            .contains("unknown field"));
        let mut bad_version = setup.clone();
        bad_version.schema_version = 3;
        assert!(bad_version.encode().unwrap_err().contains("schema version"));
        let bad_version_json = json.replace("\"schema_version\": 1", "\"schema_version\": 3");
        assert!(GimmickBlueprint::decode(&bad_version_json)
            .unwrap_err()
            .contains("schema version"));
        let bad_kind = json.replace("visionary.gimmick-blueprint", "someone.elses.blueprint");
        assert!(GimmickBlueprint::decode(&bad_kind)
            .unwrap_err()
            .contains("kind"));
        let bad_source = json.replace(
            "\"source_fighter\": \"cloud\"",
            "\"source_fighter\": \"unknown_fighter\"",
        );
        assert!(GimmickBlueprint::decode(&bad_source)
            .unwrap_err()
            .contains("source fighter"));
        assert!(setup
            .prepare("unknown_fighter", None)
            .unwrap_err()
            .contains("target fighter"));
        let long_name = json.replace(
            "\"name\": \"Cloud setup\"",
            &format!("\"name\": \"{}\"", "x".repeat(MAX_NAME_CHARS + 1)),
        );
        assert!(GimmickBlueprint::decode(&long_name)
            .unwrap_err()
            .contains("blueprint name"));
        assert!(
            GimmickBlueprint::decode(&format!("{} ", " ".repeat(MAX_DOCUMENT_BYTES)))
                .unwrap_err()
                .contains("exceeds")
        );
    }

    #[test]
    fn blank_behavior_blueprint_round_trips_and_prepares_an_empty_setup() {
        let setup = GimmickBlueprint::from_fighter(
            "Empty setup",
            "cloud",
            &ParamMod::default(),
            BehaviorGraph::default(),
            vec![],
        )
        .unwrap();
        let decoded = GimmickBlueprint::decode(&setup.encode().unwrap()).unwrap();
        assert_eq!(decoded, setup);
        let prepared = decoded.prepare("mario", None).unwrap();
        assert!(prepared.personal.is_empty());
        assert!(prepared.behavior.is_empty());
        assert!(prepared.meters.is_empty());
    }
}
