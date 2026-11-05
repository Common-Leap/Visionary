//! Authoritative, reconnect-aware live preview of saved project gimmicks.
use super::behavior::BehaviorGraph;
use crate::game_link::GameLink;
use crate::mod_project::{ParamMod, ParamValue};
use std::collections::{BTreeMap, BTreeSet};

type ControllerCharts = BTreeMap<String, BTreeMap<String, crate::mod_project::ControllerChart>>;

pub struct LiveSync {
    pub enabled: bool,
    pub warnings: BTreeMap<String, Vec<String>>,
    epoch: Option<u64>,
    revision: u64,
    sent: BTreeMap<String, String>,
    managed: BTreeSet<String>,
    recipe_errors: BTreeMap<String, String>,
    errors: BTreeMap<String, String>,
    pub error: String,
}

impl Default for LiveSync {
    fn default() -> Self {
        Self {
            enabled: true,
            warnings: BTreeMap::new(),
            epoch: None,
            revision: 0,
            sent: BTreeMap::new(),
            managed: BTreeSet::new(),
            recipe_errors: BTreeMap::new(),
            errors: BTreeMap::new(),
            error: String::new(),
        }
    }
}

impl LiveSync {
    pub fn error_for(&self, family: &str) -> Option<&str> {
        self.errors.get(family).map(String::as_str)
    }
    pub fn retry(&mut self) {
        self.sent.clear();
    }

    /// Replace the current move recipe compilation failures. An empty map clears
    /// old failures after a successful recompile.
    pub fn set_recipe_errors(&mut self, errors: BTreeMap<String, String>) {
        self.recipe_errors = errors;
    }

    pub fn update(
        &mut self,
        params: &BTreeMap<String, ParamMod>,
        graphs: &BTreeMap<String, BehaviorGraph>,
        link: &GameLink,
    ) {
        self.update_with_controllers(params, graphs, &BTreeMap::new(), link);
    }

    pub fn update_with_controllers(
        &mut self,
        params: &BTreeMap<String, ParamMod>,
        graphs: &BTreeMap<String, BehaviorGraph>,
        charts: &ControllerCharts,
        link: &GameLink,
    ) {
        let Some(snapshot) = link.gimmick_controller_snapshot() else {
            return;
        };
        self.revision = self
            .revision
            .max(snapshot.behavior_revision)
            .max(snapshot.parameter_revision);
        let epoch = link.connection_epoch();
        if self.epoch != epoch {
            self.epoch = epoch;
            self.sent.clear();
        }
        let behavior = self.prepare_controller_snapshot(graphs, charts);
        let mut edits = Vec::new();
        if self.enabled {
            for (fighter, personal) in params {
                for (path, value) in super::params::personal_edits(personal, fighter) {
                    match parameter(fighter, path, *value) {
                        Ok(edit) => edits.push(edit),
                        Err(reason) => self
                            .warnings
                            .entry(fighter.clone())
                            .or_default()
                            .push(format!("{path}: {reason}")),
                    }
                }
            }
        }
        match behavior {
            Ok(body) => self.send("behavior", body, link),
            Err(error) => self.set_error(
                "behavior",
                format!("Behavior snapshot not sent: {error}; active preview unchanged"),
            ),
        }
        self.send(
            "parameters",
            serde_json::json!({"version":1,"revision":0,"enabled":self.enabled,"edits":edits}),
            link,
        );
    }

    /// The messages an exported mod replays at boot: the same ones a connected editor sends.
    pub fn pack_messages(
        &mut self,
        params: &BTreeMap<String, ParamMod>,
        graphs: &BTreeMap<String, BehaviorGraph>,
        charts: &ControllerCharts,
    ) -> Result<Vec<String>, String> {
        let mut behavior = self.prepare_controller_snapshot(graphs, charts)?;
        behavior["revision"] = 1.into();
        let mut edits = Vec::new();
        for (fighter, personal) in params {
            for (path, value) in super::params::personal_edits(personal, fighter) {
                if let Ok(edit) = parameter(fighter, path, *value) {
                    edits.push(edit);
                }
            }
        }
        Ok(vec![
            serde_json::json!({ "gimmick_behavior": behavior }).to_string(),
            serde_json::json!({ "gimmick_parameters": {"version":1,"revision":1,"enabled":true,"edits":edits} }).to_string(),
        ])
    }

    /// Compile each complete owner or disable it, never send main while silently
    /// omitting that owner's additional charts. Global overflow refuses the
    /// entire outgoing snapshot instead of truncating it.
    fn prepare_controller_snapshot(
        &mut self,
        graphs: &BTreeMap<String, BehaviorGraph>,
        charts: &ControllerCharts,
    ) -> Result<serde_json::Value, String> {
        if !self.enabled {
            return serde_json::to_value(super::controller_protocol::ControllerBundleRequest {
                version: super::controller_protocol::REQUEST_VERSION,
                revision: 0,
                disabled_fighters: Vec::new(),
                charts: Vec::new(),
            })
            .map_err(|error| error.to_string());
        }
        self.warnings.clear();
        let mut owners = BTreeSet::new();
        for fighter in graphs
            .keys()
            .chain(
                charts
                    .iter()
                    .filter(|(_, charts)| !charts.is_empty())
                    .map(|(fighter, _)| fighter),
            )
            .chain(self.recipe_errors.keys())
        {
            owners.insert(fighter.as_str());
            if owners.len() > super::controller_protocol::MAX_FIGHTER_OWNERS {
                return Err("Live controller snapshot exceeds 96 fighter owners".into());
            }
        }
        let count = owners.iter().fold(0usize, |count, fighter| {
            count
                .saturating_add(charts.get(*fighter).map_or(0, BTreeMap::len))
                .saturating_add(usize::from(
                    graphs.get(*fighter).is_some_and(|graph| !graph.is_empty()),
                ))
        });
        if count > super::controller_protocol::MAX_CHARTS {
            return Err("Live controller snapshot exceeds 256 charts".into());
        }
        let empty_graph = BehaviorGraph::default();
        let empty_charts = BTreeMap::new();
        let mut records = Vec::new();
        let mut active = BTreeSet::new();
        for fighter in owners {
            if super::registry::fighter_kind_symbol(fighter).is_none() {
                self.warnings
                    .entry(fighter.into())
                    .or_default()
                    .push("Controller preview refused: no exact engine fighter".into());
                continue;
            }
            self.managed.insert(fighter.into());
            if let Some(error) = self.recipe_errors.get(fighter) {
                self.warnings
                    .entry(fighter.into())
                    .or_default()
                    .push(format!("Controller preview refused: {error}"));
                continue;
            }
            let graph = graphs.get(fighter).unwrap_or(&empty_graph);
            let extra = charts.get(fighter).unwrap_or(&empty_charts);
            match super::controller_project::compile(fighter, graph, extra) {
                Ok(compiled) => {
                    if !compiled.is_empty() {
                        active.insert(fighter.to_owned());
                        records.extend(compiled);
                    }
                }
                Err(error) => self
                    .warnings
                    .entry(fighter.into())
                    .or_default()
                    .push(format!("Complete controller owner refused: {error}")),
            }
        }
        let disabled: Vec<_> = self.managed.difference(&active).cloned().collect();
        let request = super::controller_protocol::ControllerBundleRequest {
            version: super::controller_protocol::REQUEST_VERSION,
            revision: 0,
            disabled_fighters: disabled,
            charts: records,
        };
        super::controller_protocol::encode_request(&request)?;
        serde_json::to_value(request).map_err(|error| error.to_string())
    }

    fn set_error(&mut self, family: &str, message: String) {
        self.errors.insert(family.into(), message);
        self.error = self
            .errors
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
    }

    fn send(&mut self, family: &str, mut body: serde_json::Value, link: &GameLink) {
        let signature = body.to_string();
        if self.sent.get(family) == Some(&signature) {
            self.errors.remove(family);
            self.error = self
                .errors
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join(" · ");
            return;
        }
        let Some(revision) = self.revision.checked_add(1) else {
            self.set_error(family, format!("{family}: live revision space exhausted"));
            return;
        };
        self.revision = revision;
        body["revision"] = self.revision.into();
        let validated = match family {
            "behavior" => serde_json::from_value::<
                super::controller_protocol::ControllerBundleRequest,
            >(body.clone())
            .map_err(|error| error.to_string())
            .and_then(|request| super::controller_protocol::encode_request(&request).map(|_| ())),
            "parameters" => serde_json::from_value::<super::live_params::Request>(body.clone())
                .map_err(|error| error.to_string())
                .and_then(|request| request.validate()),
            _ => Err("Unknown live snapshot family".into()),
        };
        if let Err(error) = validated {
            self.errors
                .insert(family.into(), format!("{family}: {error}"));
            self.error = self
                .errors
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join(" · ");
            return;
        }
        match link.send_gimmick_live(family, body) {
            Ok(()) => {
                self.sent.insert(family.into(), signature);
                self.errors.remove(family);
            }
            Err(error) => {
                self.errors.insert(family.into(), error);
            }
        }
        self.error = self
            .errors
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
    }
}

pub fn parameter(
    fighter: &str,
    path: &str,
    value: ParamValue,
) -> Result<super::live_params::LiveParameter, String> {
    use super::live_params::{LiveParamValue, LiveParameter};
    if super::registry::fighter_kind_symbol(fighter).is_none() {
        return Err("No exact engine fighter; this value requires export".into());
    }
    let value = match value {
        ParamValue::Float(value) if value.is_finite() => LiveParamValue::Float(value),
        ParamValue::I8(value) => LiveParamValue::Int(value.into()),
        ParamValue::U8(value) => LiveParamValue::Int(value.into()),
        ParamValue::I16(value) => LiveParamValue::Int(value.into()),
        ParamValue::U16(value) => LiveParamValue::Int(value.into()),
        ParamValue::I32(value) => LiveParamValue::Int(value),
        ParamValue::U32(value) => LiveParamValue::Int(
            value
                .try_into()
                .map_err(|_| "Unsigned value exceeds the live getter range")?,
        ),
        _ => return Err("This scalar type requires export".into()),
    };
    let edit = LiveParameter {
        fighter: fighter.into(),
        path: path.into(),
        value,
    };
    // The same pure request validation is repeated in the game.
    let request = super::live_params::Request {
        version: 1,
        revision: 1,
        enabled: true,
        edits: vec![edit.clone()],
    };
    request.validate().map_err(|error| error.to_string())?;
    Ok(edit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::live_params::classify_engine_path;

    fn recipe_graph() -> BehaviorGraph {
        crate::gimmicks::templates::Preset::Timer.graph()
    }

    fn extra_charts() -> ControllerCharts {
        BTreeMap::from([(
            "mario".into(),
            BTreeMap::from([
                (
                    "first".into(),
                    crate::mod_project::ControllerChart {
                        graph: recipe_graph(),
                        layout: Default::default(),
                    },
                ),
                (
                    "second".into(),
                    crate::mod_project::ControllerChart {
                        graph: recipe_graph(),
                        layout: Default::default(),
                    },
                ),
            ]),
        )])
    }

    fn controller_request(
        body: serde_json::Value,
    ) -> super::super::controller_protocol::ControllerBundleRequest {
        let request = serde_json::from_value(body).unwrap();
        super::super::controller_protocol::encode_request(&request).unwrap();
        request
    }

    #[test]
    fn complete_chart_sync_includes_independent_owners_without_mutating_authored_data() {
        let mut sync = LiveSync::default();
        let graphs = BTreeMap::from([
            ("mario".into(), recipe_graph()),
            ("cloud".into(), recipe_graph()),
        ]);
        let charts = extra_charts();
        let before = serde_json::to_value((&graphs, &charts)).unwrap();
        let request =
            controller_request(sync.prepare_controller_snapshot(&graphs, &charts).unwrap());
        assert_eq!(request.charts.len(), 4);
        assert!(request.disabled_fighters.is_empty());
        assert_eq!(
            request
                .charts
                .iter()
                .filter(|chart| chart.program.fighter == "mario")
                .count(),
            3
        );
        assert_eq!(
            request
                .charts
                .iter()
                .filter(|chart| chart.program.fighter == "cloud")
                .count(),
            1
        );
        assert_eq!(serde_json::to_value((&graphs, &charts)).unwrap(), before);
        assert_eq!(
            request
                .charts
                .iter()
                .filter(|chart| chart.chart_id == "main")
                .map(|chart| chart.program.id.as_str())
                .collect::<Vec<_>>(),
            ["live:cloud", "live:mario"]
        );
    }

    #[test]
    fn invalid_additional_chart_refuses_whole_owner_and_can_recover() {
        let mut sync = LiveSync::default();
        let graphs = BTreeMap::from([
            ("mario".into(), recipe_graph()),
            ("cloud".into(), recipe_graph()),
        ]);
        let mut charts = extra_charts();
        let valid = charts.clone();
        charts
            .get_mut("mario")
            .unwrap()
            .get_mut("first")
            .unwrap()
            .graph
            .vars
            .clear();
        let refused =
            controller_request(sync.prepare_controller_snapshot(&graphs, &charts).unwrap());
        assert_eq!(refused.charts.len(), 1);
        assert_eq!(refused.charts[0].program.fighter, "cloud");
        assert_eq!(refused.disabled_fighters, ["mario"]);
        assert!(sync.warnings.contains_key("mario"));
        let recovered =
            controller_request(sync.prepare_controller_snapshot(&graphs, &valid).unwrap());
        assert_eq!(recovered.charts.len(), 4);
        assert!(recovered.disabled_fighters.is_empty());
        assert!(sync.warnings.is_empty());
    }

    #[test]
    fn recipe_failure_never_sends_partial_owner_charts() {
        let mut sync = LiveSync::default();
        sync.set_recipe_errors(BTreeMap::from([(
            "mario".into(),
            "invalid move binding".into(),
        )]));
        let graphs = BTreeMap::from([
            ("mario".into(), recipe_graph()),
            ("cloud".into(), recipe_graph()),
        ]);
        let request = controller_request(
            sync.prepare_controller_snapshot(&graphs, &extra_charts())
                .unwrap(),
        );
        assert_eq!(request.charts.len(), 1);
        assert_eq!(request.charts[0].program.fighter, "cloud");
        assert_eq!(request.disabled_fighters, ["mario"]);
        assert!(sync.warnings["mario"][0].contains("invalid move binding"));
    }

    #[test]
    fn disabling_preview_clears_all_overrides_without_compiling_invalid_charts() {
        let mut sync = LiveSync::default();
        sync.prepare_controller_snapshot(&BTreeMap::new(), &extra_charts())
            .unwrap();
        sync.enabled = false;
        let mut invalid = extra_charts();
        invalid
            .get_mut("mario")
            .unwrap()
            .insert(" ".into(), Default::default());
        let cleared = sync
            .prepare_controller_snapshot(&BTreeMap::new(), &invalid)
            .unwrap();
        assert_eq!(request_parts(&cleared), (Vec::new(), Vec::new()));
    }

    #[test]
    fn chart_limit_is_checked_before_lowering_and_never_truncated() {
        let mut charts = ControllerCharts::new();
        for owner in 0..33 {
            charts.insert(
                format!("unknown{owner}"),
                (0..8)
                    .map(|chart| {
                        (
                            format!("c{chart}"),
                            crate::mod_project::ControllerChart::default(),
                        )
                    })
                    .collect(),
            );
        }
        assert!(LiveSync::default()
            .prepare_controller_snapshot(&BTreeMap::new(), &charts)
            .unwrap_err()
            .contains("256 charts"));
    }

    fn request_parts(body: &serde_json::Value) -> (Vec<String>, Vec<String>) {
        let request: super::super::controller_protocol::ControllerBundleRequest =
            serde_json::from_value(body.clone()).unwrap();
        (
            request
                .charts
                .into_iter()
                .map(|chart| chart.program.fighter)
                .collect(),
            request.disabled_fighters,
        )
    }

    #[test]
    fn parameter_conversion_accepts_only_supported_path_shapes() {
        assert!(parameter("cloud", "limit_max", ParamValue::Float(1.0)).is_ok());
        assert!(parameter(
            "cloud",
            "param_special_lw[0].limit_gauge_add",
            ParamValue::Float(1.0)
        )
        .is_ok());
        assert!(parameter(
            "cloud",
            "param_special_lw.limit_gauge_add",
            ParamValue::Float(1.0)
        )
        .is_ok());

        let nonzero = parameter(
            "cloud",
            "param_special_lw[1].limit_gauge_add",
            ParamValue::Float(1.0),
        )
        .unwrap_err();
        assert!(nonzero.contains("row [0]"), "{nonzero}");
        let deep = parameter(
            "cloud",
            "param_special_lw[0].nested.limit_gauge_add",
            ParamValue::Float(1.0),
        )
        .unwrap_err();
        assert!(deep.contains("depth"), "{deep}");

        assert!(classify_engine_path("limit_max").is_ok());
        let root = classify_engine_path("limit_max").unwrap();
        assert_eq!(root.parent, "limit_max");
        assert!(root.child.is_empty());
        let struct_root = classify_engine_path("param_special_lw.limit_gauge_add").unwrap();
        assert_eq!(struct_root.parent, "param_special_lw");
        assert_eq!(struct_root.child, "limit_gauge_add");
    }

    #[test]
    fn parameter_conversion_refuses_non_scalar_types_and_unsigned_overflow() {
        assert!(parameter("cloud", "limit_max", ParamValue::Bool(true)).is_err());
        assert!(parameter("cloud", "limit_max", ParamValue::Hash(0x1234)).is_err());
        let overflow =
            parameter("cloud", "limit_max", ParamValue::U32(i32::MAX as u32 + 1)).unwrap_err();
        assert!(overflow.contains("Unsigned value exceeds"), "{overflow}");
        assert!(parameter("cloud", "limit_max", ParamValue::Float(f32::INFINITY)).is_err());
        assert!(parameter("cloud", "limit_max", ParamValue::Float(1_000_001.0)).is_err());
        assert!(parameter("cloud", "limit_max", ParamValue::I32(1_000_001)).is_err());
    }

    #[test]
    fn fighter_names_are_checked_against_the_exact_kind_registry() {
        assert_eq!(
            super::super::registry::fighter_kind_symbol("cloud"),
            Some("FIGHTER_KIND_CLOUD")
        );
        assert_eq!(super::super::registry::fighter_kind_symbol("Cloud"), None);
        assert_eq!(super::super::registry::fighter_kind_symbol("pokemon"), None);
        assert_eq!(super::super::registry::fighter_kind_symbol("shield"), None);
    }
}
