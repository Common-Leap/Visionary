//! Bounded V5 transport protocol for complete multi-chart controller bundles.
//!
//! The request version is independent of `LiveProgram`'s model version. A
//! bundle replaces the complete live behavior family only after every chart,
//! owner, permission, and per-fighter aggregate has passed validation.
//! Envelope and chart-record fields are closed. Nested program enum fields
//! retain the existing model codec policy; no new native action is admitted.

#![allow(dead_code)] // Data-only codec; active transport installation is separate.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use serde::{Deserialize, Serialize};

use super::controller_runtime::{
    ControllerCoordinator, MAX_AGGREGATE_AUTHORED_ACTIONS, MAX_AGGREGATE_EVENT_BINDINGS,
    MAX_AGGREGATE_TRANSITIONS, MAX_AGGREGATE_VARIABLES, MAX_CHARTS as MAX_CHARTS_PER_FIGHTER,
    MAX_UNION_NATIVE_INPUTS,
};
use super::live_model as model;
use super::native_inputs::NativeInput;
use super::registry;

pub const REQUEST_VERSION: u8 = 6;
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
pub const MAX_FIGHTER_OWNERS: usize = 96;
pub const MAX_CHARTS: usize = 256;
pub const MAX_DISABLED_FIGHTERS: usize = 96;
pub const MAX_CHART_ID_BYTES: usize = 48;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerBundleRequest {
    pub version: u8,
    pub revision: u64,
    #[serde(default)]
    pub disabled_fighters: Vec<String>,
    pub charts: Vec<ControllerProgram>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerProgram {
    pub chart_id: String,
    pub program: model::LiveProgram,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerBundleEnvelope {
    #[serde(rename = "gimmick_behavior")]
    pub gimmick_behavior: ControllerBundleRequest,
}

#[derive(Default)]
struct FighterBudget {
    charts: usize,
    variables: usize,
    transitions: usize,
    event_bindings: usize,
    authored_actions: usize,
    native_inputs: BTreeSet<NativeInput>,
}

#[derive(Serialize)]
struct EnvelopeRef<'a> {
    gimmick_behavior: RequestRef<'a>,
}

#[derive(Serialize)]
struct RequestRef<'a> {
    version: u8,
    revision: u64,
    disabled_fighters: Vec<&'a str>,
    charts: Vec<ControllerProgramRef<'a>>,
}

#[derive(Serialize)]
struct ControllerProgramRef<'a> {
    chart_id: &'a str,
    program: &'a model::LiveProgram,
}

struct BoundedWriter {
    bytes: Vec<u8>,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(next_len) = self.bytes.len().checked_add(bytes.len()) else {
            return Err(io::Error::other(
                "encoded controller bundle size overflowed",
            ));
        };
        if next_len > MAX_REQUEST_BYTES {
            return Err(io::Error::other(format!(
                "encoded controller bundle exceeds {MAX_REQUEST_BYTES} bytes"
            )));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn decode_request(raw: &str) -> Result<ControllerBundleRequest, String> {
    if raw.len() > MAX_REQUEST_BYTES {
        return Err(format!(
            "controller bundle request exceeds {MAX_REQUEST_BYTES} bytes"
        ));
    }
    let envelope: ControllerBundleEnvelope = serde_json::from_str(raw)
        .map_err(|error| format!("malformed controller bundle request: {error}"))?;
    validate_request(&envelope.gimmick_behavior)?;
    Ok(envelope.gimmick_behavior)
}

/// Encode charts and disabled owners in stable order without mutating or cloning the
/// caller's programs. Serialization stops as soon as the byte cap is reached.
pub fn encode_request(request: &ControllerBundleRequest) -> Result<String, String> {
    validate_request(request)?;
    let mut ordered: Vec<&ControllerProgram> = request.charts.iter().collect();
    ordered.sort_by(|left, right| {
        (&left.program.fighter, &left.chart_id).cmp(&(&right.program.fighter, &right.chart_id))
    });
    let mut disabled_fighters: Vec<&str> = request
        .disabled_fighters
        .iter()
        .map(String::as_str)
        .collect();
    disabled_fighters.sort_unstable();
    let envelope = EnvelopeRef {
        gimmick_behavior: RequestRef {
            version: request.version,
            revision: request.revision,
            disabled_fighters,
            charts: ordered
                .into_iter()
                .map(|chart| ControllerProgramRef {
                    chart_id: &chart.chart_id,
                    program: &chart.program,
                })
                .collect(),
        },
    };
    let mut writer = BoundedWriter {
        bytes: Vec::with_capacity(4096),
    };
    serde_json::to_writer(&mut writer, &envelope)
        .map_err(|error| format!("cannot encode controller bundle request: {error}"))?;
    if writer.bytes.len() > MAX_REQUEST_BYTES {
        return Err(format!(
            "encoded controller bundle exceeds {MAX_REQUEST_BYTES} bytes"
        ));
    }
    String::from_utf8(writer.bytes)
        .map_err(|error| format!("encoded controller bundle is not UTF-8: {error}"))
}

/// Validate the complete request without cloning programs or constructing
/// runtime coordinators. All aggregate limits are checked per fighter here.
pub fn validate_request(request: &ControllerBundleRequest) -> Result<(), String> {
    if request.version != REQUEST_VERSION
        && !(request.version == 5
            && request.charts.iter().all(|c| {
                c.program.status_bindings.is_empty()
                    && !c
                        .program
                        .required_native_features()
                        .contains(super::status_model::CANCEL_FEATURE)
            }))
    {
        return Err(format!(
            "unsupported controller bundle request version {}; expected {REQUEST_VERSION}",
            request.version
        ));
    }
    if request.charts.len() > MAX_CHARTS {
        return Err(format!("bundle contains more than {MAX_CHARTS} charts"));
    }
    if request.disabled_fighters.len() > MAX_DISABLED_FIGHTERS {
        return Err(format!(
            "bundle contains more than {MAX_DISABLED_FIGHTERS} disabled fighters"
        ));
    }

    super::status_model::manifest(request.charts.iter().map(|c| c.program.clone()))?;
    let mut owners = BTreeSet::new();
    let mut chart_keys = BTreeSet::new();
    let mut program_ids = BTreeSet::new();
    let mut disabled = BTreeSet::new();
    let mut budgets = BTreeMap::<&str, FighterBudget>::new();

    for (index, record) in request.charts.iter().enumerate() {
        let program = &record.program;
        let context = format!(
            "charts[{index}] fighter {:?} chart {:?}",
            program.fighter, record.chart_id
        );
        if record.chart_id.trim().is_empty()
            || record.chart_id.len() > MAX_CHART_ID_BYTES
            || record.chart_id.as_bytes().contains(&0)
        {
            return Err(format!(
                "{context}: chart ID must be nonblank, at most {MAX_CHART_ID_BYTES} UTF-8 bytes, and contain no NUL"
            ));
        }
        if !chart_keys.insert((program.fighter.as_str(), record.chart_id.as_str())) {
            return Err(format!("{context}: duplicate fighter/chart ID pair"));
        }
        if !program_ids.insert(program.id.as_str()) {
            return Err(format!(
                "{context}: duplicate global program ID {:?}",
                program.id
            ));
        }
        if registry::fighter_kind_symbol(&program.fighter).is_none() {
            return Err(format!(
                "{context}: fighter is not registered in the pinned fighter catalog"
            ));
        }
        owners.insert(program.fighter.as_str());
        if owners.len() > MAX_FIGHTER_OWNERS {
            return Err(format!(
                "bundle has more than {MAX_FIGHTER_OWNERS} fighter owners"
            ));
        }
        let budget = budgets.entry(program.fighter.as_str()).or_default();
        budget.charts = budget
            .charts
            .checked_add(1)
            .ok_or_else(|| format!("{context}: chart count overflowed"))?;
        if budget.charts > MAX_CHARTS_PER_FIGHTER {
            return Err(format!(
                "{context}: fighter exceeds the {MAX_CHARTS_PER_FIGHTER}-chart limit"
            ));
        }

        program.validate().map_err(|errors| {
            format!(
                "{context}: {}",
                errors
                    .into_iter()
                    .map(|error| format!("{}: {}", error.item, error.reason))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })?;
        validate_symbol_permissions(index, program)?;

        budget.variables =
            checked_add(budget.variables, program.vars.len(), &context, "variables")?;
        budget.transitions = checked_add(
            budget.transitions,
            program.transitions.len(),
            &context,
            "transitions",
        )?;
        budget.event_bindings = checked_add(
            budget.event_bindings,
            program.event_bindings.len(),
            &context,
            "event bindings",
        )?;
        budget.authored_actions = checked_add(
            budget.authored_actions,
            program
                .transitions
                .iter()
                .map(|transition| transition.actions.len())
                .sum(),
            &context,
            "authored actions",
        )?;
        budget
            .native_inputs
            .extend(program.native_inputs.iter().cloned());
        ensure_budget(
            &context,
            "variables",
            budget.variables,
            MAX_AGGREGATE_VARIABLES,
        )?;
        ensure_budget(
            &context,
            "transitions",
            budget.transitions,
            MAX_AGGREGATE_TRANSITIONS,
        )?;
        ensure_budget(
            &context,
            "event bindings",
            budget.event_bindings,
            MAX_AGGREGATE_EVENT_BINDINGS,
        )?;
        ensure_budget(
            &context,
            "authored actions",
            budget.authored_actions,
            MAX_AGGREGATE_AUTHORED_ACTIONS,
        )?;
        ensure_budget(
            &context,
            "union native inputs",
            budget.native_inputs.len(),
            MAX_UNION_NATIVE_INPUTS,
        )?;
    }

    for (index, fighter) in request.disabled_fighters.iter().enumerate() {
        if !disabled.insert(fighter.as_str()) {
            return Err(format!(
                "disabled_fighters[{index}] duplicates fighter {fighter:?}"
            ));
        }
        if owners.contains(fighter.as_str()) {
            return Err(format!(
                "disabled_fighters[{index}] overlaps charts for fighter {fighter:?}"
            ));
        }
        if registry::fighter_kind_symbol(fighter).is_none() {
            return Err(format!(
                "disabled_fighters[{index}] names unregistered fighter {fighter:?}"
            ));
        }
        owners.insert(fighter.as_str());
        if owners.len() > MAX_FIGHTER_OWNERS {
            return Err(format!(
                "bundle has more than {MAX_FIGHTER_OWNERS} fighter owners"
            ));
        }
    }
    Ok(())
}

fn validate_symbol_permissions(index: usize, program: &model::LiveProgram) -> Result<(), String> {
    for trigger in &program.status_triggers {
        registry::resolve(
            &program.fighter,
            &trigger.symbol,
            registry::IdentifierKind::FighterStatus,
        )
        .map_err(|error| {
            format!(
                "charts[{index}] status trigger {} for {:?}: {error}",
                trigger.hook.wire_name(),
                program.fighter
            )
        })?;
    }
    for (transition_index, transition) in program.transitions.iter().enumerate() {
        for (action_index, action) in transition.actions.iter().enumerate() {
            let context =
                format!("charts[{index}] transition {transition_index} action {action_index}");
            match action {
                model::Action::GenerateArticle { symbol }
                | model::Action::RemoveArticle { symbol }
                | model::Action::RemoveNewestArticle { symbol } => {
                    registry::resolve(&program.fighter, symbol, registry::IdentifierKind::Article)
                        .map_err(|error| format!("{context} article: {error}"))?;
                }
                model::Action::RequestArticleStatus { symbol, status } => {
                    registry::resolve(&program.fighter, symbol, registry::IdentifierKind::Article)
                        .map_err(|error| error.to_string())?;
                    let entry = registry::resolve(
                        &program.fighter,
                        status,
                        registry::IdentifierKind::WeaponStatus,
                    )
                    .map_err(|error| error.to_string())?;
                    if registry::weapon_kind(entry.fighter, entry.group).is_none() {
                        return Err("article status has no weapon-kind guard".into());
                    }
                }
                model::Action::RequestFighterStatus { symbol } => {
                    registry::resolve(
                        &program.fighter,
                        symbol,
                        registry::IdentifierKind::FighterStatus,
                    )
                    .map_err(|error| format!("fighter status action: {error}"))?;
                }
                model::Action::RequestWeaponStatus { target, symbol } => {
                    if target.battle_object_id == 0 {
                        return Err(format!("{context} weapon-status target is zero"));
                    }
                    let entry = registry::resolve(
                        &program.fighter,
                        symbol,
                        registry::IdentifierKind::WeaponStatus,
                    )
                    .map_err(|error| format!("{context} weapon status: {error}"))?;
                    if registry::weapon_kind(entry.fighter, entry.group).is_none() {
                        return Err(format!(
                            "{context} weapon status has no concrete weapon-kind guard"
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn checked_add(total: usize, next: usize, context: &str, label: &str) -> Result<usize, String> {
    total
        .checked_add(next)
        .ok_or_else(|| format!("{context}: aggregate {label} count overflowed"))
}

fn ensure_budget(context: &str, label: &str, count: usize, limit: usize) -> Result<(), String> {
    if count > limit {
        Err(format!(
            "{context}: aggregate {label} count {count} exceeds limit {limit}"
        ))
    } else {
        Ok(())
    }
}

impl ControllerBundleRequest {
    /// Construct all per-fighter coordinators only after validating the entire
    /// bundle. The caller still owns atomic live installation and state swap.
    pub fn coordinators(&self) -> Result<BTreeMap<String, ControllerCoordinator>, String> {
        validate_request(self)?;
        let mut programs = BTreeMap::<String, BTreeMap<String, model::LiveProgram>>::new();
        for record in &self.charts {
            programs
                .entry(record.program.fighter.clone())
                .or_default()
                .insert(record.chart_id.clone(), record.program.clone());
        }
        programs
            .into_iter()
            .map(|(fighter, charts)| {
                ControllerCoordinator::new(charts)
                    .map(|coordinator| (fighter, coordinator))
                    .map_err(|error| format!("fighter coordinator validation failed: {error}"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::live_model::{
        Action, CmpOp, Expr, HookBinding, HookName, Predicate, Transition, Value, VarDef, VarKind,
    };
    use super::*;

    fn valid_program(id: &str, fighter: &str) -> model::LiveProgram {
        model::LiveProgram {
            custom_statuses: Default::default(),
            status_bindings: Vec::new(),
            version: model::MODEL_VERSION,
            id: id.into(),
            fighter: fighter.into(),
            initial: "idle".into(),
            states: vec!["idle".into()],
            vars: vec![VarDef {
                keep_across_form_switch: false,
                name: "count".into(),
                kind: VarKind::Int,
                initial: Value::Int(0),
            }],
            native_inputs: vec![],
            transitions: vec![Transition {
                from: "idle".into(),
                to: "idle".into(),
                guard: Predicate::Compare {
                    op: CmpOp::Eq,
                    left: Expr::Const(Value::Int(1)),
                    right: Expr::Const(Value::Int(1)),
                },
                on_frame: true,
                actions: vec![Action::Add {
                    var: "count".into(),
                    value: Expr::Const(Value::Int(1)),
                }],
            }],
            bindings: vec![HookBinding {
                hook: HookName::OnChargeTick,
                article: None,
                transitions: vec![0],
            }],
            event_bindings: vec![],
            status_triggers: vec![],
            resource_triggers: vec![],
        }
    }

    #[test]
    fn previous_bundle_and_model_decode_without_status_bindings() {
        let mut program = valid_program("legacy:mario", "mario");
        program.version = 4;
        let mut request = request(vec![record("main", program)]);
        request.version = 5;
        let encoded = encode_request(&request).unwrap();
        assert_eq!(decode_request(&encoded).unwrap(), request);
    }

    fn record(chart_id: &str, program: model::LiveProgram) -> ControllerProgram {
        assert!(program.validate().is_ok());
        ControllerProgram {
            chart_id: chart_id.into(),
            program,
        }
    }

    fn request(charts: Vec<ControllerProgram>) -> ControllerBundleRequest {
        ControllerBundleRequest {
            version: REQUEST_VERSION,
            revision: 7,
            disabled_fighters: vec![],
            charts,
        }
    }

    fn request_json(request: &ControllerBundleRequest) -> String {
        serde_json::to_string(&ControllerBundleEnvelope {
            gimmick_behavior: request.clone(),
        })
        .unwrap()
    }

    #[test]
    fn two_charts_for_one_fighter_roundtrip_and_build_one_coordinator() {
        let original = request(vec![
            record("main", valid_program("cloud-main", "cloud")),
            record("meter", valid_program("cloud-meter", "cloud")),
        ]);
        let encoded = encode_request(&original).unwrap();
        let decoded = decode_request(&encoded).unwrap();
        assert_eq!(decoded, original);
        assert_eq!(decoded.coordinators().unwrap().len(), 1);
        assert_eq!(
            decoded.coordinators().unwrap()["cloud"]
                .chart_ids()
                .collect::<Vec<_>>(),
            ["main", "meter"]
        );
    }

    #[test]
    fn old_v4_request_is_not_coerced_to_bundle_v5() {
        let raw = r#"{"gimmick_behavior":{"version":4,"revision":1,"disabled_fighters":[],"programs":[]}}"#;
        assert!(decode_request(raw).is_err());
    }

    #[test]
    fn request_and_model_versions_are_checked_separately() {
        let mut wrong_request = request(vec![]);
        wrong_request.version = 4;
        assert!(encode_request(&wrong_request)
            .unwrap_err()
            .contains("request version"));
        let mut wrong_model = valid_program("cloud-main", "cloud");
        wrong_model.version = model::MODEL_VERSION.wrapping_add(1);
        let malformed = ControllerBundleRequest {
            version: REQUEST_VERSION,
            revision: 0,
            disabled_fighters: vec![],
            charts: vec![ControllerProgram {
                chart_id: "main".into(),
                program: wrong_model,
            }],
        };
        assert!(validate_request(&malformed)
            .unwrap_err()
            .contains("version"));
    }
    #[test]
    fn malformed_fields_and_expressions_fail_before_version_validation() {
        let raw = request_json(&request(vec![record(
            "main",
            valid_program("cloud-main", "cloud"),
        )]));
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["gimmick_behavior"]["extra"] = serde_json::json!(true);
        assert!(decode_request(&value.to_string())
            .unwrap_err()
            .contains("unknown field"));
        value["gimmick_behavior"]
            .as_object_mut()
            .unwrap()
            .remove("extra");
        value["gimmick_behavior"]["version"] = serde_json::json!(REQUEST_VERSION + 1);
        value["gimmick_behavior"]["charts"][0]["program"]["transitions"][0]["guard"] = serde_json::json!({
            "Compare": {"op": "eq", "left": {"NotAnExpr": "timer"}, "right": {"Const": {"Int": 0}}}
        });
        let error = decode_request(&value.to_string()).unwrap_err();
        assert!(
            error.contains("malformed") && error.contains("NotAnExpr"),
            "{error}"
        );
        assert!(!error.contains("unsupported"), "{error}");
    }
}
