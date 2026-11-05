//! Exact-owner source bridge; native permissions remain owned by the active chart.

pub(crate) fn dispatch(boid: u32, chart_id: &str, name: &str) -> Result<(), String> {
    validate_target(chart_id, name)?;
    let record = crate::slight::agents::lookup(boid)
        .filter(|record| record.category == 0)
        .ok_or_else(|| "ACMD chart event has no registered fighter".to_owned())?;
    let fighter = super::registry::fighter_name(record.kind)
        .ok_or_else(|| "ACMD chart event fighter has no exact pinned identity".to_owned())?;
    if !super::live_behavior::actor_matches(boid, fighter) {
        return Err("ACMD chart event fighter identity is stale".into());
    }
    super::live_behavior::dispatch_controller_named(boid, fighter, chart_id, name)
}

pub(crate) fn dispatch_values(
    boid: u32,
    chart_id: &str,
    name: &str,
    arguments: &[(&str, super::live_model::Value)],
) -> Result<(), String> {
    validate_target(chart_id, name)?;
    let record = crate::slight::agents::lookup(boid)
        .filter(|record| record.category == 0)
        .ok_or_else(|| "ACMD chart event has no registered fighter".to_owned())?;
    let fighter = super::registry::fighter_name(record.kind)
        .ok_or_else(|| "ACMD chart event fighter has no exact pinned identity".to_owned())?;
    if !super::live_behavior::actor_matches(boid, fighter) {
        return Err("ACMD chart event fighter identity is stale".into());
    }
    super::live_behavior::dispatch_controller_named_values(boid, fighter, chart_id, name, arguments)
}

fn validate_target(chart_id: &str, name: &str) -> Result<(), String> {
    if chart_id == "main"
        || chart_id.trim().is_empty()
        || chart_id.len() > super::controller_protocol::MAX_CHART_ID_BYTES
        || chart_id.contains('\0')
    {
        return Err(
            "ACMD chart event requires a bounded, nonblank additional-chart identity".into(),
        );
    }
    if name.trim().is_empty()
        || name.len() > super::live_model::MAX_NAME_BYTES
        || name.contains('\0')
    {
        return Err("ACMD chart event requires a bounded, nonblank event name".into());
    }
    Ok(())
}
