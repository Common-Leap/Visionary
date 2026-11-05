//! Versioned, bounded, portable assets for one additional controller chart.
//!
//! An asset preserves the graph and editor layout exactly. Its donor fighter is
//! validation provenance; native identifiers are never rewritten on import.
//! Import returns a complete candidate chart map and never mutates its inputs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::gimmicks::{
    behavior::{self, BehaviorGraph},
    controller_project,
    controller_protocol::MAX_CHART_ID_BYTES,
    field_projection::FieldRef,
    registry,
};
use crate::mod_project::ControllerChart;

const ASSET_KIND: &str = "visionary_controller_chart";
const ASSET_VERSION: u32 = 2;
pub const MAX_ASSET_BYTES: usize = 1024 * 1024;
pub const MAX_LABEL_BYTES: usize = 128;
const MAX_LAYOUT_POSITIONS: usize = 8_192;
const MAX_LAYOUT_KEY_BYTES: usize = 4_096;
const MAX_LAYOUT_ABS: f32 = 100_000.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetDocument {
    kind: String,
    version: u32,
    donor_fighter: String,
    source_chart_id: String,
    label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    required_features: Vec<String>,
    chart: ControllerChart,
}

/// One validated additional controller chart and its pinned source identity.
#[derive(Debug, Clone, PartialEq)]
pub struct ControllerAsset {
    donor_fighter: String,
    source_chart_id: String,
    label: String,
    required_features: Vec<String>,
    chart: ControllerChart,
}

impl ControllerAsset {
    /// Capture an additional chart after validating its exact donor ownership.
    pub fn capture(
        donor_fighter: &str,
        source_chart_id: &str,
        label: &str,
        chart: &ControllerChart,
    ) -> Result<Self, String> {
        if registry::fighter_kind_symbol(donor_fighter).is_none() {
            return Err(format!(
                "donor fighter {donor_fighter:?} has no exact pinned fighter-kind mapping"
            ));
        }
        validate_chart_id(source_chart_id, "source controller ID")?;
        if label.trim().is_empty() || label.len() > MAX_LABEL_BYTES || label.as_bytes().contains(&0)
        {
            return Err(format!(
                "controller label must be nonblank, at most {MAX_LABEL_BYTES} UTF-8 bytes, and contain no NUL"
            ));
        }
        // Reject unsafe recursive/table sizes before cloning the source chart.
        validate_chart_bounds(chart)?;
        let asset = Self {
            donor_fighter: donor_fighter.to_owned(),
            source_chart_id: source_chart_id.to_owned(),
            label: label.to_owned(),
            required_features: required_features(&chart.graph),
            chart: chart.clone(),
        };
        asset.validate()?;
        Ok(asset)
    }

    /// Decode a strict, bounded asset. Unknown fields are rejected in nested IR.
    pub fn decode(json: &str) -> Result<Self, String> {
        if json.len() > MAX_ASSET_BYTES {
            return Err(format!(
                "controller asset exceeds the {MAX_ASSET_BYTES}-byte limit"
            ));
        }
        let mut raw: Value = serde_json::from_str(json)
            .map_err(|error| format!("invalid controller asset JSON: {error}"))?;
        preflight_raw(&raw)?;
        // Deserialize the original stream after raw-tree bounds so serde's struct
        // visitors can reject duplicate known fields instead of accepting the last.
        let document: AssetDocument = serde_json::from_str(json)
            .map_err(|error| format!("invalid controller asset fields: {error}"))?;
        if document.kind != ASSET_KIND {
            return Err(
                "unsupported controller asset kind; expected visionary_controller_chart".into(),
            );
        }
        if document.version != ASSET_VERSION
            && !(document.version == 1
                && document
                    .chart
                    .graph
                    .states
                    .iter()
                    .all(|s| s.status.is_none()))
        {
            return Err(format!(
                "unsupported controller asset version {}; expected {ASSET_VERSION}",
                document.version
            ));
        }

        // The behavior IR intentionally tolerates unknown serde fields for project
        // compatibility. Compare against its canonical typed form so an asset cannot
        // silently discard unrecognized data. Keep f32 JSON encodings intact.
        let canonical_text = serde_json::to_string(&document)
            .map_err(|error| format!("could not canonicalize controller asset: {error}"))?;
        let canonical: Value = serde_json::from_str(&canonical_text)
            .map_err(|error| format!("could not check controller asset fields: {error}"))?;
        normalize_optional_empty_fields(&mut raw);
        if canonical != raw {
            return Err("controller asset contains unknown or non-canonical nested fields".into());
        }
        let asset = Self {
            donor_fighter: document.donor_fighter,
            source_chart_id: document.source_chart_id,
            label: document.label,
            required_features: document.required_features,
            chart: document.chart,
        };
        asset.validate()?;
        Ok(asset)
    }

    /// Deterministic, pretty JSON encoding for a validated asset.
    pub fn encode(&self) -> Result<String, String> {
        self.validate()?;
        let json = serde_json::to_string_pretty(&AssetDocument {
            kind: ASSET_KIND.to_owned(),
            version: ASSET_VERSION,
            donor_fighter: self.donor_fighter.clone(),
            source_chart_id: self.source_chart_id.clone(),
            label: self.label.clone(),
            required_features: self.required_features.clone(),
            chart: self.chart.clone(),
        })
        .map_err(|error| format!("could not encode controller asset: {error}"))?;
        if json.len() > MAX_ASSET_BYTES {
            return Err(format!(
                "encoded controller asset exceeds {MAX_ASSET_BYTES} bytes"
            ));
        }
        Ok(json)
    }

    pub fn donor_fighter(&self) -> &str {
        &self.donor_fighter
    }

    pub fn source_chart_id(&self) -> &str {
        &self.source_chart_id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn chart(&self) -> &ControllerChart {
        &self.chart
    }

    /// Return the imported chart under an explicit target ID after compiling the
    /// complete target main chart and all existing plus imported charts.
    pub fn candidate(
        &self,
        target_fighter: &str,
        target_main: &BehaviorGraph,
        existing: &BTreeMap<String, ControllerChart>,
        target_chart_id: &str,
    ) -> Result<BTreeMap<String, ControllerChart>, String> {
        self.validate()?;
        validate_chart_id(target_chart_id, "target controller ID")?;
        if existing.contains_key(target_chart_id) {
            return Err(format!(
                "target controller ID {target_chart_id:?} already exists"
            ));
        }
        // Bounds are checked before recursively cloning the user-owned graph map.
        controller_project::preflight(target_main, existing)
            .map_err(|reason| format!("target fighter {target_fighter}: {reason}"))?;
        behavior::preflight_bounds(target_main).map_err(|reason| {
            format!("target fighter {target_fighter} main controller: {reason}")
        })?;

        let mut candidate = existing.clone();
        candidate.insert(target_chart_id.to_owned(), self.chart.clone());
        controller_project::compile(target_fighter, target_main, &candidate)
            .map_err(|reason| format!("import into fighter {target_fighter}: {reason}"))?;
        Ok(candidate)
    }

    fn validate(&self) -> Result<(), String> {
        if registry::fighter_kind_symbol(&self.donor_fighter).is_none() {
            return Err(format!(
                "donor fighter {:?} has no exact pinned fighter-kind mapping",
                self.donor_fighter
            ));
        }
        validate_chart_id(&self.source_chart_id, "source controller ID")?;
        if self.label.trim().is_empty()
            || self.label.len() > MAX_LABEL_BYTES
            || self.label.as_bytes().contains(&0)
        {
            return Err(format!(
                "controller label must be nonblank, at most {MAX_LABEL_BYTES} UTF-8 bytes, and contain no NUL"
            ));
        }
        validate_chart_bounds(&self.chart)?;
        let expected_features = required_features(&self.chart.graph);
        if self.required_features != expected_features {
            return Err(
                "controller asset required_features do not match graph requirements".into(),
            );
        }
        for source in behavior::collect_native_inputs(&self.chart.graph) {
            if source.fighter() != Some(self.donor_fighter.as_str()) {
                return Err(format!(
                    "controller asset native input {:?} does not belong to donor fighter {:?}",
                    source, self.donor_fighter
                ));
            }
        }
        controller_project::compile(
            &self.donor_fighter,
            &BehaviorGraph::default(),
            &BTreeMap::from([(self.source_chart_id.clone(), self.chart.clone())]),
        )
        .map_err(|reason| format!("donor fighter {}: {reason}", self.donor_fighter))?;
        Ok(())
    }
}

fn required_features(graph: &BehaviorGraph) -> Vec<String> {
    behavior::required_native_features(graph)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn validate_chart_id(id: &str, description: &str) -> Result<(), String> {
    if id == controller_project::MAIN_CHART
        || id.trim().is_empty()
        || id.len() > MAX_CHART_ID_BYTES
        || id.as_bytes().contains(&0)
    {
        return Err(format!(
            "{description} must be nonblank, not reserved main, at most {MAX_CHART_ID_BYTES} UTF-8 bytes, and contain no NUL"
        ));
    }
    Ok(())
}

fn validate_chart_bounds(chart: &ControllerChart) -> Result<(), String> {
    if chart.graph.is_empty() {
        return Err("controller chart graph must not be empty".into());
    }
    behavior::preflight_bounds(&chart.graph)
        .map_err(|reason| format!("controller graph: {reason}"))?;
    for (transition, definition) in chart.graph.transitions.iter().enumerate() {
        for (action, definition) in definition.actions.iter().enumerate() {
            if matches!(definition, behavior::Action::RequestWeaponStatus { .. }) {
                return Err(format!("controller transition {transition} action {action}: explicit live weapon object IDs are not portable; remove this weapon-status action before sharing the controller"));
            }
        }
    }
    if chart.layout.positions.len() > MAX_LAYOUT_POSITIONS {
        return Err(format!(
            "controller layout exceeds {MAX_LAYOUT_POSITIONS} positions"
        ));
    }
    for (key, point) in &chart.layout.positions {
        if key.len() > MAX_LAYOUT_KEY_BYTES {
            return Err(format!(
                "controller layout key exceeds {MAX_LAYOUT_KEY_BYTES} bytes"
            ));
        }
        FieldRef::from_key(key).map_err(|reason| format!("controller layout: {reason}"))?;
        if point
            .iter()
            .any(|value| !value.is_finite() || value.abs() > MAX_LAYOUT_ABS)
        {
            return Err(format!(
                "controller layout coordinate for {key:?} is out of bounds"
            ));
        }
    }
    Ok(())
}

/// Bound raw JSON collection sizes before constructing the typed recursive graph.
fn preflight_raw(value: &Value) -> Result<(), String> {
    let document = value
        .as_object()
        .ok_or_else(|| "controller asset root must be an object".to_owned())?;
    let chart = document
        .get("chart")
        .and_then(Value::as_object)
        .ok_or_else(|| "controller asset chart must be an object".to_owned())?;
    let graph = chart
        .get("graph")
        .and_then(Value::as_object)
        .ok_or_else(|| "controller asset graph must be an object".to_owned())?;
    if array_len(graph, "states")? > crate::gimmicks::live_model::MAX_STATES
        || array_len(graph, "vars")? > crate::gimmicks::live_model::MAX_VARIABLES
        || array_len(graph, "transitions")? > crate::gimmicks::live_model::MAX_TRANSITIONS
    {
        return Err("controller asset graph exceeds a bounded table limit".into());
    }
    let mut aggregate_actions = 0usize;
    if let Some(transitions) = graph.get("transitions") {
        for transition in transitions.as_array().into_iter().flatten() {
            let actions = transition
                .as_object()
                .and_then(|object| object.get("actions"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if actions > crate::gimmicks::live_model::MAX_ACTIONS_PER_TRANSITION {
                return Err("controller asset transition exceeds its action limit".into());
            }
            aggregate_actions = aggregate_actions.saturating_add(actions);
        }
    }
    if aggregate_actions
        > crate::gimmicks::live_model::MAX_ACTIONS_PER_TRANSITION
            .saturating_mul(crate::gimmicks::live_model::MAX_TRANSITIONS)
    {
        return Err("controller asset exceeds its aggregate action limit".into());
    }
    if let Some(layout) = chart.get("layout") {
        let layout = layout
            .as_object()
            .ok_or_else(|| "controller asset layout must be an object".to_owned())?;
        if layout
            .get("positions")
            .and_then(Value::as_object)
            .map_or(0, serde_json::Map::len)
            > MAX_LAYOUT_POSITIONS
        {
            return Err(format!(
                "controller layout exceeds {MAX_LAYOUT_POSITIONS} positions"
            ));
        }
    }
    Ok(())
}

fn array_len(value: &serde_json::Map<String, Value>, name: &str) -> Result<usize, String> {
    value
        .get(name)
        .map(|field| {
            field
                .as_array()
                .map(Vec::len)
                .ok_or_else(|| format!("controller graph {name} must be an array"))
        })
        .unwrap_or(Ok(0))
}

fn normalize_optional_empty_fields(raw: &mut Value) {
    if raw
        .get("required_features")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        raw.as_object_mut()
            .expect("asset object checked before normalization")
            .remove("required_features");
    }
    let Some(chart) = raw.get_mut("chart").and_then(Value::as_object_mut) else {
        return;
    };
    if let Some(graph) = chart.get_mut("graph").and_then(Value::as_object_mut) {
        if graph
            .get("emission_plan")
            .and_then(Value::as_object)
            .is_some_and(|plan| {
                plan.keys().all(|key| {
                    matches!(
                        key.as_str(),
                        "bindings" | "event_bindings" | "status_triggers" | "resource_triggers"
                    )
                }) && plan
                    .values()
                    .all(|value| value.as_array().is_some_and(Vec::is_empty))
            })
        {
            graph.remove("emission_plan");
        }
    }
    if let Some(layout) = chart.get("layout").and_then(Value::as_object) {
        if (layout.is_empty() || layout.keys().all(|key| key == "positions"))
            && layout
                .get("positions")
                .and_then(Value::as_object)
                .is_none_or(serde_json::Map::is_empty)
        {
            chart.remove("layout");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::{field_canvas::FieldLayout, templates::Preset};

    fn chart() -> ControllerChart {
        ControllerChart {
            graph: Preset::Timer.graph(),
            layout: FieldLayout::default(),
        }
    }

    fn event_chart() -> ControllerChart {
        let mut graph = Preset::Charge.graph();
        graph.transitions[0]
            .actions
            .push(behavior::Action::EmitEventArguments {
                name: "ready".into(),
                arguments: vec![
                    crate::gimmicks::event_payload::Argument {
                        field: "count".into(),
                        value: behavior::ConstValue::Int(4),
                    },
                    crate::gimmicks::event_payload::Argument {
                        field: "score".into(),
                        value: behavior::ConstValue::Float(7.5),
                    },
                    crate::gimmicks::event_payload::Argument {
                        field: "active".into(),
                        value: behavior::ConstValue::Flag(true),
                    },
                ],
            });
        graph.transitions.push(behavior::Transition {
            from: "charging".into(),
            to: "charging".into(),
            guard: behavior::Predicate::Flag("ready".into()),
            on_frame: true,
            actions: Vec::new(),
        });
        graph
            .emission_plan
            .event_bindings
            .push(crate::gimmicks::codegen::EventBinding {
                name: "ready".into(),
                transitions: vec![3],
                fields: vec![
                    crate::gimmicks::event_payload::FieldDef {
                        name: "score".into(),
                        kind: behavior::VarType::Float,
                    },
                    crate::gimmicks::event_payload::FieldDef {
                        name: "active".into(),
                        kind: behavior::VarType::Flag,
                    },
                    crate::gimmicks::event_payload::FieldDef {
                        name: "count".into(),
                        kind: behavior::VarType::Int,
                    },
                ],
            });
        ControllerChart {
            graph,
            layout: FieldLayout::default(),
        }
    }

    #[test]
    fn capture_roundtrip_and_cross_fighter_import_preserve_every_authored_field() {
        for mut source in [chart(), event_chart()] {
            source
                .layout
                .positions
                .insert("state/waiting".into(), [12.25, -3.5]);
            let before = source.clone();
            let asset =
                ControllerAsset::capture("cloud", "charge", "Charge loop", &source).unwrap();
            let decoded = ControllerAsset::decode(&asset.encode().unwrap()).unwrap();
            assert_eq!(decoded, asset);
            assert_eq!(decoded.chart, before);
            assert_eq!(decoded.donor_fighter(), "cloud");
            assert_eq!(decoded.source_chart_id(), "charge");
            let existing = BTreeMap::from([("existing".into(), chart())]);
            let existing_before = existing.clone();
            let imported = decoded
                .candidate("mario", &BehaviorGraph::default(), &existing, "copy")
                .unwrap();
            assert_eq!(imported["copy"], before);
            assert_eq!(imported["existing"], existing_before["existing"]);
            assert_eq!(existing, existing_before);
            assert_eq!(source, before);
        }
    }
}
