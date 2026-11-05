//! Atomic additions and repair-safe removal of independent controller charts.

use std::collections::BTreeMap;

use crate::gimmicks::{
    behavior::{self, BehaviorGraph},
    controller_project,
    controller_protocol::MAX_CHART_ID_BYTES,
    controller_runtime,
};
use crate::mod_project::ControllerChart;

const MAIN_CHART: &str = controller_project::MAIN_CHART;

/// Add an independent chart to a fighter's complete authored controller set.
///
/// effective_main is the caller-expanded legacy chart. Existing authored
/// drafts are never rewritten; the candidate must compile as a whole before
/// charts is changed.
pub fn add(
    fighter: &str,
    effective_main: &BehaviorGraph,
    charts: &mut BTreeMap<String, ControllerChart>,
    stem: &str,
    graph: BehaviorGraph,
) -> Result<String, String> {
    preflight_existing(fighter, effective_main, charts)?;
    behavior::preflight_bounds(&graph)
        .map_err(|reason| format!("fighter {fighter} new controller graph: {reason}"))?;
    ensure_capacity(fighter, effective_main, charts)?;
    validate_stem(stem)?;

    let id = next_id(stem, charts)?;
    let mut candidate = charts.clone();
    candidate.insert(
        id.clone(),
        ControllerChart {
            graph,
            layout: Default::default(),
        },
    );
    controller_project::compile(fighter, effective_main, &candidate)?;

    let inserted = candidate
        .remove(&id)
        .ok_or_else(|| "validated controller candidate disappeared".to_owned())?;
    charts.insert(id.clone(), inserted);
    Ok(id)
}

/// Duplicate one additional chart, including its editor layout, under a stable
/// generated ID. The reserved legacy main chart is not in charts and cannot
/// be used as a source.
pub fn duplicate(
    fighter: &str,
    effective_main: &BehaviorGraph,
    charts: &mut BTreeMap<String, ControllerChart>,
    source_id: &str,
) -> Result<String, String> {
    preflight_existing(fighter, effective_main, charts)?;
    if source_id == MAIN_CHART {
        return Err("reserved main controller cannot be duplicated as an additional chart".into());
    }
    let source = charts
        .get(source_id)
        .ok_or_else(|| "unknown additional controller".to_owned())?;
    ensure_capacity(fighter, effective_main, charts)?;

    // All graph/layout bounds are checked by preflight_existing before these
    // recursive draft clones.
    let id = next_id(source_id, charts)?;
    let copied = source.clone();
    let mut candidate = charts.clone();
    candidate.insert(id.clone(), copied);
    controller_project::compile(fighter, effective_main, &candidate)?;

    let inserted = candidate
        .remove(&id)
        .ok_or_else(|| "validated controller candidate disappeared".to_owned())?;
    charts.insert(id.clone(), inserted);
    Ok(id)
}

/// Remove exactly one additional chart. This deliberately does not compile the
/// remainder: deleting a malformed draft is a valid repair operation.
pub fn remove(
    charts: &mut BTreeMap<String, ControllerChart>,
    id: &str,
) -> Result<ControllerChart, String> {
    if id == MAIN_CHART {
        return Err("reserved main controller cannot be removed".into());
    }
    charts
        .remove(id)
        .ok_or_else(|| "unknown additional controller".to_owned())
}

fn preflight_existing(
    fighter: &str,
    effective_main: &BehaviorGraph,
    charts: &BTreeMap<String, ControllerChart>,
) -> Result<(), String> {
    controller_project::preflight(effective_main, charts)
        .map_err(|reason| format!("fighter {fighter}: {reason}"))?;
    behavior::preflight_bounds(effective_main)
        .map_err(|reason| format!("fighter {fighter} main controller: {reason}"))
}

fn ensure_capacity(
    fighter: &str,
    effective_main: &BehaviorGraph,
    charts: &BTreeMap<String, ControllerChart>,
) -> Result<(), String> {
    let count = charts
        .len()
        .saturating_add(usize::from(!effective_main.is_empty()))
        .saturating_add(1);
    if count > controller_runtime::MAX_CHARTS {
        return Err(format!(
            "fighter {fighter}: adding a controller would exceed {} charts including main",
            controller_runtime::MAX_CHARTS
        ));
    }
    Ok(())
}

fn validate_stem(stem: &str) -> Result<(), String> {
    if stem.len() > MAX_CHART_ID_BYTES {
        return Err(format!(
            "controller ID stem exceeds {MAX_CHART_ID_BYTES} UTF-8 bytes"
        ));
    }
    if stem.trim().is_empty() {
        return Err("controller ID stem must be nonblank".into());
    }
    if stem.as_bytes().contains(&0) {
        return Err("controller ID stem must not contain NUL".into());
    }
    Ok(())
}

fn next_id(stem: &str, charts: &BTreeMap<String, ControllerChart>) -> Result<String, String> {
    for number in 1..=charts.len().saturating_add(1) {
        let suffix = format!("_{number}");
        let prefix_budget = MAX_CHART_ID_BYTES.saturating_sub(suffix.len());
        let mut prefix_len = stem.len().min(prefix_budget);
        while !stem.is_char_boundary(prefix_len) {
            prefix_len -= 1;
        }
        let id = format!("{}{suffix}", &stem[..prefix_len]);
        if id.trim().is_empty()
            || id.len() > MAX_CHART_ID_BYTES
            || id.as_bytes().contains(&0)
            || id == MAIN_CHART
        {
            return Err(format!(
                "generated controller ID must be nonblank, at most {MAX_CHART_ID_BYTES} UTF-8 bytes, contain no NUL, and not be reserved main"
            ));
        }
        if !charts.contains_key(&id) {
            return Ok(id);
        }
    }
    Err("could not generate a unique controller ID".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::templates::Preset;

    fn graph() -> BehaviorGraph {
        Preset::Timer.graph()
    }

    fn chart() -> ControllerChart {
        ControllerChart {
            graph: graph(),
            layout: Default::default(),
        }
    }

    #[test]
    fn add_uses_stable_collision_free_ids() {
        let mut charts = BTreeMap::from([("timer_1".into(), chart()), ("timer_2".into(), chart())]);
        assert_eq!(
            add(
                "mario",
                &BehaviorGraph::default(),
                &mut charts,
                "timer",
                graph()
            )
            .unwrap(),
            "timer_3"
        );
        assert_eq!(
            add(
                "mario",
                &BehaviorGraph::default(),
                &mut charts,
                "timer",
                graph()
            )
            .unwrap(),
            "timer_4"
        );
    }

    #[test]
    fn add_and_duplicate_count_nonempty_main_toward_eight_chart_limit() {
        let mut charts = (0..7)
            .map(|index| (format!("chart{index}"), chart()))
            .collect::<BTreeMap<_, _>>();
        let before = charts.clone();
        let main = graph();
        assert!(add("mario", &main, &mut charts, "extra", graph())
            .unwrap_err()
            .contains("8 charts"));
        assert_eq!(charts, before);
        assert!(duplicate("mario", &main, &mut charts, "chart0")
            .unwrap_err()
            .contains("8 charts"));
        assert_eq!(charts, before);
    }

    #[test]
    fn generated_id_is_utf8_bounded_nonblank_and_nul_free() {
        let stem = "é".repeat(24);
        let mut charts = BTreeMap::new();
        let id = add(
            "mario",
            &BehaviorGraph::default(),
            &mut charts,
            &stem,
            graph(),
        )
        .unwrap();
        assert_eq!(id, format!("{}_1", "é".repeat(23)));
        assert_eq!(id.len(), MAX_CHART_ID_BYTES);
        assert!(!id.trim().is_empty());
        assert!(!id.as_bytes().contains(&0));
        assert_ne!(id, MAIN_CHART);
    }
}
