const VISIONARY_STATIC_BEHAVIOR_ADAPTER_SLOT: &str = "visionary.gimmicks.generated-slot.v1";

pub fn dispatch_named(boid: u32, name: &str) -> Result<(), String> {
    Err(format!(
        "No compiled behavior owns event {name:?} for object {boid}"
    ))
}

pub fn meter_bindings() -> &'static [super::meter_snapshot::MeterBinding] {
    &[]
}

pub fn register_all() -> Result<(), String> {
    Ok(())
}

pub fn read_controller_value(
    _boid: u32,
    _chart_id: &str,
    _var: &str,
) -> Option<super::live_model::Value> {
    None
}
