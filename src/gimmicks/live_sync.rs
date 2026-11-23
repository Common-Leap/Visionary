//! Authoritative, reconnect-aware live preview of saved project gimmicks.
use super::behavior::BehaviorGraph;
use crate::game_link::GameLink;
use crate::mod_project::{ParamMod, ParamValue, RosterMod};
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

    /// Replace the current move recipe compilation failures. An empty map clears
    /// old failures after a successful recompile.
    pub fn set_recipe_errors(&mut self, errors: BTreeMap<String, String>) {
        self.recipe_errors = errors;
    }

    pub fn update(
        &mut self,
        params: &BTreeMap<String, ParamMod>,
        graphs: &BTreeMap<String, BehaviorGraph>,
        roster: &RosterMod,
        link: &GameLink,
    ) {
        self.update_with_controllers(params, graphs, &BTreeMap::new(), roster, link);
    }

    /// `graphs` and `charts` are keyed by character: a fighter's name, or a made character's
    /// key. `roster` says which costumes each one runs on.
    pub fn update_with_controllers(
        &mut self,
        params: &BTreeMap<String, ParamMod>,
        graphs: &BTreeMap<String, BehaviorGraph>,
        charts: &ControllerCharts,
        roster: &RosterMod,
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
        let behavior = self.prepare_controller_snapshot(graphs, charts, roster);
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
            Err(error) => self.set_error("behavior", format!("Gimmicks not sent: {error}")),
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
        roster: &RosterMod,
    ) -> Result<Vec<String>, String> {
        let mut behavior = self.prepare_controller_snapshot(graphs, charts, roster)?;
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
    /// entire outgoing snapshot instead of truncating it. Each character's gimmick is its own
    /// owner on its own costumes; a fighter is disabled only when none of its characters'
    /// gimmicks compiled.
    fn prepare_controller_snapshot(
        &mut self,
        graphs: &BTreeMap<String, BehaviorGraph>,
        charts: &ControllerCharts,
        roster: &RosterMod,
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
                return Err("more than 96 characters have gimmicks".into());
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
            return Err("there are more than 256 gimmicks".into());
        }
        let empty_graph = BehaviorGraph::default();
        let empty_charts = BTreeMap::new();
        let mut records = Vec::new();
        let mut active = BTreeSet::new();
        for key in owners {
            let graph = graphs.get(key).unwrap_or(&empty_graph);
            let fighter = super::controller_project::owner_fighter(key);
            if super::registry::fighter_kind_symbol(fighter).is_none() {
                self.warnings
                    .entry(key.into())
                    .or_default()
                    .push("Not live: the game has no fighter of this name".into());
                continue;
            }
            self.managed.insert(fighter.into());
            // A gimmick that runs on no costume is not sent at all.
            let Some(owner) = crate::character::gimmick_owner(key, &graph.costumes, roster) else {
                continue;
            };
            if let Some(error) = self.recipe_errors.get(key) {
                self.warnings
                    .entry(key.into())
                    .or_default()
                    .push(format!("Not live: {error}"));
                continue;
            }
            let extra = charts.get(key).unwrap_or(&empty_charts);
            match super::controller_project::compile_owner(&owner, graph, extra) {
                Ok(compiled) => {
                    if !compiled.is_empty() {
                        active.insert(fighter.to_owned());
                        records.extend(compiled);
                    }
                }
                Err(error) => self
                    .warnings
                    .entry(key.into())
                    .or_default()
                    .push(format!("Not live: {error}")),
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
            self.set_error(family, "Not sent: restart Visionary to send again".into());
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
        return Err("not live, the game has no fighter of this name".into());
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
                .map_err(|_| "not live, the number is too large")?,
        ),
        _ => return Err("not live, the game reads this kind of value once".into()),
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
        let request = controller_request(
            sync.prepare_controller_snapshot(&graphs, &charts, &RosterMod::default())
                .unwrap(),
        );
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
        let refused = controller_request(
            sync.prepare_controller_snapshot(&graphs, &charts, &RosterMod::default())
                .unwrap(),
        );
        assert_eq!(refused.charts.len(), 1);
        assert_eq!(refused.charts[0].program.fighter, "cloud");
        assert_eq!(refused.disabled_fighters, ["mario"]);
        assert!(sync.warnings.contains_key("mario"));
        let recovered = controller_request(
            sync.prepare_controller_snapshot(&graphs, &valid, &RosterMod::default())
                .unwrap(),
        );
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
            sync.prepare_controller_snapshot(&graphs, &extra_charts(), &RosterMod::default())
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
        sync.prepare_controller_snapshot(&BTreeMap::new(), &extra_charts(), &RosterMod::default())
            .unwrap();
        sync.enabled = false;
        let mut invalid = extra_charts();
        invalid
            .get_mut("mario")
            .unwrap()
            .insert(" ".into(), Default::default());
        let cleared = sync
            .prepare_controller_snapshot(&BTreeMap::new(), &invalid, &RosterMod::default())
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
            .prepare_controller_snapshot(&BTreeMap::new(), &charts, &RosterMod::default())
            .unwrap_err()
            .contains("256 gimmicks"));
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
        assert!(overflow.contains("the number is too large"), "{overflow}");
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

    fn roster_with_vision() -> RosterMod {
        let mut roster = RosterMod::default();
        roster.authored.push(crate::mod_project::AuthoredEntry {
            key: crate::roster::RosterKey::slot("mario", 8),
            donor: "mario".into(),
            slot: 8,
            slots: (9..=15).collect(),
            display_name: "Vision".into(),
            name_id: "vision".into(),
            moveset_scaffolded: false,
            files_root: None,
        });
        roster
    }

    /// Mario's gimmick and Vision's (Mario costumes c08-c15) are sent side by side, each on
    /// costumes the other never claims. Mario's was saved with no costume limit, before made
    /// characters existed: it is limited when sent, and the saved graph is left as it was.
    #[test]
    fn a_made_characters_gimmick_is_sent_beside_its_fighters_on_disjoint_costumes() {
        let mut sync = LiveSync::default();
        let vision = crate::gimmicks::templates::Preset::Charge.graph();
        let graphs = BTreeMap::from([
            ("mario".to_string(), recipe_graph()),
            ("mario#c08".to_string(), vision.clone()),
        ]);
        let before = graphs.clone();
        let request = controller_request(
            sync.prepare_controller_snapshot(&graphs, &BTreeMap::new(), &roster_with_vision())
                .unwrap(),
        );
        assert_eq!(graphs, before);
        let program = |id: &str| {
            &request
                .charts
                .iter()
                .find(|chart| chart.program.id == id)
                .unwrap_or_else(|| panic!("{id} was not sent"))
                .program
        };
        let (mario, vision) = (program("live:mario"), program("live:mario#c08"));
        let theirs: Vec<u8> = (8..=15).collect();
        assert_eq!(
            (mario.fighter.as_str(), vision.fighter.as_str()),
            ("mario", "mario")
        );
        assert!(mario.costumes.is_empty());
        assert_eq!(mario.except_costumes, theirs);
        assert_eq!(vision.costumes, theirs);
        assert!(vision.except_costumes.is_empty());
        // Each carries its own character's graph.
        let charge = &graphs["mario#c08"].vars;
        assert_eq!(vision.vars, *charge);
        assert_ne!(mario.vars, *charge);
        // Two gimmicks of one fighter, installed as two coordinators.
        assert_eq!(request.coordinators().unwrap()["mario"].len(), 2);
        // The exported mod replays exactly the same snapshot.
        let packed = sync
            .pack_messages(
                &BTreeMap::new(),
                &graphs,
                &BTreeMap::new(),
                &roster_with_vision(),
            )
            .unwrap();
        let replayed = super::super::controller_protocol::decode_request(&packed[0]).unwrap();
        assert_eq!(replayed.charts, request.charts);
    }

    /// The paired control: with no made character, Mario's gimmick goes out exactly as before,
    /// with no costume fields at all, so an older plugin reads it unchanged.
    #[test]
    fn a_fighter_without_made_characters_sends_its_gimmick_as_before() {
        let mut sync = LiveSync::default();
        let graphs = BTreeMap::from([("mario".to_string(), recipe_graph())]);
        let body = sync
            .prepare_controller_snapshot(&graphs, &BTreeMap::new(), &RosterMod::default())
            .unwrap();
        let text = body.to_string();
        assert!(!text.contains("costumes"), "{text}");
        assert!(text.contains("\"live:mario\""));
        // And with Vision in the project the exclusion appears.
        let text = sync
            .prepare_controller_snapshot(&graphs, &BTreeMap::new(), &roster_with_vision())
            .unwrap()
            .to_string();
        assert!(
            text.contains("\"except_costumes\":[8,9,10,11,12,13,14,15]"),
            "{text}"
        );
    }

    /// The plugin is a crate `cargo test` never builds, so its use of the costume is pinned
    /// here by reading its source.
    const PLUGIN_CONTROLLERS: &str = include_str!(
        "../../plugins/slight_replica/src/slight/gimmicks/live_behavior/controllers.rs"
    );

    /// The body of `fn name(` in `source`, up to the next top-level item.
    fn plugin_fn(source: &str, name: &str) -> String {
        let normalized = source.replace("\r\n", "\n");
        let start = normalized
            .find(&format!("\nfn {name}("))
            .or_else(|| normalized.find(&format!("\npub(super) fn {name}(")))
            .or_else(|| normalized.find(&format!("\npub(crate) fn {name}(")))
            .unwrap_or_else(|| panic!("plugin has no fn {name}"));
        let rest = &normalized[start + 1..];
        let end = rest.find("\n}\n").map_or(rest.len(), |end| end + 3);
        rest[..end].to_owned()
    }

    /// True when the plugin's dispatch picks the acting fighter's gimmick by its costume, for
    /// live and for compiled gimmicks alike, rather than one gimmick per fighter.
    fn picks_by_costume(source: &str) -> bool {
        let active = plugin_fn(source, "active");
        let object = plugin_fn(source, "active_for_object");
        active
            .matches("coordinator_for_costume(owners, color)")
            .count()
            == 2
            && object.contains("active(fighter, costume_of(boid))")
            && plugin_fn(source, "costume_of").contains("FIGHTER_INSTANCE_WORK_ID_INT_COLOR")
    }

    #[test]
    fn the_plugin_picks_each_fighters_gimmick_by_its_costume() {
        assert!(picks_by_costume(PLUGIN_CONTROLLERS));
        // Per frame this is a few reads: no allocation and no file access on that path.
        let picked = plugin_fn(PLUGIN_CONTROLLERS, "costume_of");
        for banned in ["format!", "to_owned", "Vec::", "std::fs", "File"] {
            assert!(!picked.contains(banned), "costume_of uses {banned}");
        }
    }

    /// The paired positive: the scan rejects the plugin as it was, one gimmick per fighter
    /// filtered by a costume list.
    #[test]
    fn the_costume_scan_rejects_one_gimmick_per_fighter() {
        let old = "\nfn active(fighter: &str) -> Result<(), String> {\n    let runtime = RUNTIME.lock();\n    if let Some(coordinator) = runtime.controllers.get(fighter) {\n        return Ok(());\n    }\n    runtime.compiled_controllers.get(fighter)\n}\n\nfn active_for_object(boid: u32, fighter: &str) {\n    let (coordinator, source) = active(fighter)?;\n    coordinator.runs_for_costume(costume)\n}\n\npub(super) fn costume_of(boid: u32) -> i32 {\n    FIGHTER_INSTANCE_WORK_ID_INT_COLOR\n}\n";
        assert!(!picks_by_costume(old));
        // And a one-line revert of either half is caught on the real source.
        let live_only = PLUGIN_CONTROLLERS.replacen(
            "coordinator_for_costume(owners, color)",
            "owners.first()",
            1,
        );
        assert!(!picks_by_costume(&live_only));
        let colourless =
            PLUGIN_CONTROLLERS.replace("active(fighter, costume_of(boid))", "active(fighter, 0)");
        assert!(!picks_by_costume(&colourless));
    }
}
