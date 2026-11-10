//! Static Skyline behavior and meter-adapter rendering.
//!
//! This module is deliberately an artifact renderer, not an exporter. It
//! turns already-saved graphs and meter bindings into the `slight::gimmicks::generated` module
//! shape, but never searches for a plugin, changes a project, or copies a
//! module into an exported mod. A caller must select the destination and call
//! [`write_to`] explicitly after reviewing the returned source.
//!
//! Every rendered fighter is gated by the behavior validator, the emission
//! plan validator, and the closed emitted-source verifier. Fighter matching
//! imports a named `FIGHTER_KIND_*` constant from the source-pinned registry;
//! no numeric kind or spelling derived by the renderer reaches the artifact.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::behavior::{Action, BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, VarType};
use super::codegen;
use super::hooks::HookName;
use super::{meter, registry, verifier};

pub(crate) const STATIC_CONTROLLER_BUNDLE_CONTRACT: &str =
    "visionary.gimmicks.static-controller-bundle.v1";

mod controller_adapter;

const GENERATED_SLOT_MARKER: &str = "visionary.gimmicks.generated-slot.v1";
const GENERATED_RELATIVE_PATH: &str = "src/slight/gimmicks/generated.rs";
const GENERATED_ACMD_RELATIVE_PATH: &str = "src/slight/gimmicks/acmd_generated.rs";

/// Result of replacing a verified plugin's one managed adapter slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginExport {
    pub destination: PathBuf,
    /// False means the destination already contained this exact artifact and
    /// no filesystem mutation was made.
    pub changed: bool,
}

/// The two and only two Visionary-managed generated source slots in a
/// selected compatible plugin. Keeping the paths together prevents a caller
/// from treating the second ACMD slot as arbitrary write permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDestinations {
    pub behavior: PathBuf,
    pub acmd: PathBuf,
}

/// Result of replacing the behavior and ACMD generated slots as one logical
/// operation. Either changed bit is useful to package progress reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinedPluginExport {
    pub behavior_changed: bool,
    pub acmd_changed: bool,
}

impl CombinedPluginExport {
    pub fn changed(&self) -> bool {
        self.behavior_changed || self.acmd_changed
    }
}

/// A rendered, parse-checked source module. The source is not written until a
/// caller explicitly passes a destination to [`write_to`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedAdapter {
    pub source: String,
    pub fighters: Vec<String>,
}

/// One fail-closed rendering diagnostic. `subject` always starts with the
/// authored fighter name and then identifies the graph node or adapter gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError {
    pub subject: String,
    pub reason: String,
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.subject, self.reason)
    }
}

/// Render a complete `generated.rs`-shaped adapter module.
///
/// The map's lexical order is the artifact order, so repeated calls with the
/// same graphs are byte-identical. Empty behavior graphs are omitted; callers
/// can therefore render every project fighter without creating inert runtime
/// registrations. Every non-empty graph must have an exact symbolic fighter
/// kind in the pinned registry and pass all codegen gates.
pub fn render(
    graphs: &BTreeMap<String, BehaviorGraph>,
) -> Result<RenderedAdapter, Vec<AdapterError>> {
    render_with_meters(graphs, &BTreeMap::new())
}

/// Render behavior plus enabled, validated symbolic meter snapshot bindings.
pub fn render_with_meters(
    graphs: &BTreeMap<String, BehaviorGraph>,
    meters: &BTreeMap<String, Vec<meter::MeterBinding>>,
) -> Result<RenderedAdapter, Vec<AdapterError>> {
    render_with_controllers_internal(graphs, &BTreeMap::new(), meters)
}

/// Render static behavior, meters, and complete independent controller charts.
/// Owners with additional charts are installed as one bounded V5 bundle and
/// deliberately omit their legacy V4 program registration.
pub(crate) fn render_with_controllers(
    graphs: &BTreeMap<String, BehaviorGraph>,
    controllers: &BTreeMap<String, BTreeMap<String, crate::mod_project::ControllerChart>>,
    meters: &BTreeMap<String, Vec<meter::MeterBinding>>,
) -> Result<RenderedAdapter, Vec<AdapterError>> {
    render_with_controllers_internal(graphs, controllers, meters)
}

fn render_with_controllers_internal(
    graphs: &BTreeMap<String, BehaviorGraph>,
    controllers: &BTreeMap<String, BTreeMap<String, crate::mod_project::ControllerChart>>,
    meters: &BTreeMap<String, Vec<meter::MeterBinding>>,
) -> Result<RenderedAdapter, Vec<AdapterError>> {
    // Compile and validate the entire charted-owner candidate before the
    // existing renderer starts constructing any legacy artifacts.
    let compiled_bundle = controller_adapter::compile_bundle(graphs, controllers)?;
    let mut errors = Vec::new();
    let mut programs = Vec::new();
    let mut names = Names::default();
    for (fighter, graph) in graphs {
        if super::behavior::requires_controller_runtime(graph)
            || controllers
                .get(fighter)
                .is_some_and(|charts| !charts.is_empty())
        {
            continue;
        }
        if graph.is_empty() {
            continue;
        }
        match ProgramRenderer::new(fighter, graph, &mut names) {
            Ok(renderer) => match renderer.render() {
                Ok(program) => programs.push(program),
                Err(mut found) => errors.append(&mut found),
            },
            Err(mut found) => errors.append(&mut found),
        }
    }
    let mut rendered_meters = Vec::new();
    for (fighter, bindings) in meters {
        let mut enabled_sources = BTreeSet::new();
        for (index, binding) in bindings.iter().enumerate() {
            if !binding.enabled {
                continue;
            }
            if binding.fighter != *fighter {
                errors.push(AdapterError {
                    subject: format!("{fighter} / meter {index}"),
                    reason: format!(
                        "binding fighter {:?} does not match its project fighter",
                        binding.fighter
                    ),
                });
                continue;
            }
            let symbol = meter_symbol(&binding.value);
            if !enabled_sources.insert(symbol) {
                errors.push(AdapterError {
                    subject: format!("{fighter} / meter {index}"),
                    reason: format!(
                        "source {symbol:?} is already used by another enabled meter for this fighter"
                    ),
                });
                continue;
            }
            let validation = meter::validate(binding);
            if validation.is_empty() {
                rendered_meters.push(RenderedMeter {
                    id: format!("meter.{fighter}.{index}"),
                    fighter: fighter.clone(),
                    symbol: symbol.to_string(),
                    kind: meter_kind(&binding.value),
                    presentation: meter_presentation(binding),
                });
            } else {
                errors.extend(validation.into_iter().map(|reason| AdapterError {
                    subject: format!("{fighter} / meter {index}"),
                    reason,
                }));
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut source = String::from(
        "//! @generated by Visionary's static behavior adapter renderer; do not edit.\n\
         const VISIONARY_STATIC_BEHAVIOR_ADAPTER_SLOT: &str = \"visionary.gimmicks.generated-slot.v1\";\n\n",
    );
    let symbols: BTreeSet<&str> = programs
        .iter()
        .flat_map(|program| {
            std::iter::once(program.kind_symbol.as_str())
                .chain(program.status_symbols.iter().map(String::as_str))
        })
        .chain(
            compiled_bundle
                .iter()
                .flat_map(|bundle| bundle.owners.iter().map(|owner| owner.kind_symbol.as_str())),
        )
        .collect();
    if !symbols.is_empty() {
        source.push_str("use smash::lib::lua_const::{");
        for (index, symbol) in symbols.iter().enumerate() {
            if index > 0 {
                source.push_str(", ");
            }
            source.push_str(symbol);
        }
        source.push_str("};\n\n");
    }
    source.push_str(
        "use super::behavior::{\n    self, Action, Binding, CmpOp, EventBinding, Expr, NativeSource, Predicate, Program, Registration, Transition, Value,\n    VarDef, VarKind,\n};\nuse super::dispatcher::GimmickHook;\n\n",
    );
    for program in &programs {
        source.push_str(&program.source);
    }
    if let Some(bundle) = &compiled_bundle {
        source.push_str(&format!(
            "pub const VISIONARY_STATIC_CONTROLLER_BUNDLE_CONTRACT: &str = {:?};\nconst VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = {:?};\n\n",
            STATIC_CONTROLLER_BUNDLE_CONTRACT, bundle.encoded
        ));
    }
    source.push_str("/// Deliver an ACMD event only to its exact fighter-owned program.\npub fn dispatch_named(boid: u32, name: &str) -> Result<(), String> {\n");
    if let Some(bundle) = &compiled_bundle {
        for owner in &bundle.owners {
            source.push_str(&controller_adapter::dispatch_arm(owner));
        }
    }
    for program in &programs {
        source.push_str(&format!(
            "    if ({}.matches_fighter)(boid) {{\n        if let Some(result) = super::live_behavior::dispatch_named(boid, {}.fighter, name) {{ return result; }}\n        return behavior::execute_named(&{}, boid, name);\n    }}\n",
            program.program_name, program.program_name, program.program_name
        ));
    }
    source.push_str("    Err(format!(\"no compiled fighter program owns ACMD event {name:?} for object {boid}\"))\n}\n\n");
    source.push_str("/// Read the exact actor's legacy main controller.\npub fn read_controller_value(boid: u32, chart_id: &str, var: &str) -> Option<super::live_model::Value> {\n    if chart_id != \"main\" { return None; }\n");
    for program in &programs {
        source.push_str(&format!(
            "    if ({}.matches_fighter)(boid) {{\n        return behavior::read_for_actor(&{}, boid, var).map(|value| match value {{\n            Value::Int(value) => super::live_model::Value::Int(value),\n            Value::Float(value) => super::live_model::Value::Float(value),\n            Value::Flag(value) => super::live_model::Value::Flag(value),\n        }});\n    }}\n",
            program.program_name, program.program_name
        ));
    }
    source.push_str("    None\n}\n\n");
    source.push_str("fn report(result: Result<(), String>) {\n    if let Err(error) = result {\n        crate::slight::diag::note(format!(\"generated gimmick behavior rejected: {error}\"));\n    }\n}\n\n");
    source.push_str(&format!(
        "static METER_BINDINGS: [super::meter_snapshot::MeterBinding; {}] = [\n",
        rendered_meters.len()
    ));
    for binding in &rendered_meters {
        source.push_str(&format!(
            "    super::meter_snapshot::MeterBinding {{ id: {:?}, fighter: {:?}, source: super::meter_snapshot::MeterSource::Work {{ symbol: {:?}, kind: super::registry::IdentifierKind::{} }}, presentation: {} }},\n",
            binding.id, binding.fighter, binding.symbol, binding.kind, binding.presentation
        ));
    }
    source.push_str(
        "];\n\n/// Enabled, validated symbolic meter sources for the snapshot pass.\npub fn meter_bindings() -> &'static [super::meter_snapshot::MeterBinding] { &METER_BINDINGS }\n\n",
    );
    if let Some(bundle) = &compiled_bundle {
        source.push_str(&bundle.status_definitions);
    }
    source.push_str("/// Register every compiled static behavior artifact.\npub fn register_all() -> Result<(), String> {\n");
    if compiled_bundle.is_some() {
        source.push_str(
            "    super::live_behavior::install_compiled_controllers(VISIONARY_STATIC_CONTROLLER_BUNDLE)?;\n",
        );
    }
    if let Some(bundle) = &compiled_bundle {
        source.push_str(&bundle.status_install);
    }
    for program in &programs {
        source.push_str(&program.registration_call());
    }
    source.push_str("    Ok(())\n}\n");

    if let Err(error) = syn::parse_file(&source) {
        return Err(vec![AdapterError {
            subject: "adapter / file".to_string(),
            reason: format!("renderer produced unparsable Rust: {error}"),
        }]);
    }
    let fighters = programs
        .iter()
        .map(|program| program.fighter.clone())
        .chain(
            compiled_bundle
                .iter()
                .flat_map(|bundle| bundle.owners.iter().map(|owner| owner.fighter.clone())),
        )
        .chain(
            rendered_meters
                .iter()
                .map(|binding| binding.fighter.clone()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(RenderedAdapter { source, fighters })
}

struct RenderedMeter {
    id: String,
    fighter: String,
    symbol: String,
    kind: &'static str,
    presentation: String,
}

fn meter_presentation(binding: &meter::MeterBinding) -> String {
    match (&binding.style, &binding.value) {
        (meter::MeterStyle::Bar, meter::MeterValue::Float { min, max, .. }) => format!(
            "super::meter_snapshot::MeterPresentation::FloatBar {{ min: {min:?}, max: {max:?} }}"
        ),
        (meter::MeterStyle::Bar, meter::MeterValue::Integer { min, max, .. }) => {
            format!("super::meter_snapshot::MeterPresentation::IntBar {{ min: {min}, max: {max} }}")
        }
        (meter::MeterStyle::Icon, meter::MeterValue::Flag { .. }) => {
            "super::meter_snapshot::MeterPresentation::FlagIcon".to_string()
        }
        (meter::MeterStyle::Icon, meter::MeterValue::Cycle { labels, .. }) => format!(
            "super::meter_snapshot::MeterPresentation::CycleIcon {{ labels: &[{}] }}",
            labels
                .iter()
                .map(|label| literal_string(label))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (meter::MeterStyle::Pips, meter::MeterValue::Pips { count, .. }) => {
            format!("super::meter_snapshot::MeterPresentation::Pips {{ count: {count} }}")
        }
        (meter::MeterStyle::StockArrows, meter::MeterValue::Integer { min, max, .. }) => format!(
            "super::meter_snapshot::MeterPresentation::StockArrows {{ min: {min}, max: {max} }}"
        ),
        _ => unreachable!("meter::validate accepted an incompatible style/value pair"),
    }
}

fn meter_symbol(value: &meter::MeterValue) -> &str {
    match value {
        meter::MeterValue::Float { symbol, .. }
        | meter::MeterValue::Integer { symbol, .. }
        | meter::MeterValue::Flag { symbol }
        | meter::MeterValue::Pips { symbol, .. }
        | meter::MeterValue::Cycle { symbol, .. } => symbol,
    }
}

fn meter_kind(value: &meter::MeterValue) -> &'static str {
    match value {
        meter::MeterValue::Float { .. } => "WorkFloat",
        meter::MeterValue::Integer { .. }
        | meter::MeterValue::Pips { .. }
        | meter::MeterValue::Cycle { .. } => "WorkInt",
        meter::MeterValue::Flag { .. } => "WorkFlag",
    }
}

/// Replace one explicit regular `.rs` file with a parse-checked artifact.
///
/// This function intentionally has no plugin discovery or export-root logic.
/// The parent must already exist, the final component must be a normal `.rs`
/// filename, and an existing symlink is rejected so the caller cannot replace
/// an indirect target by accident.
pub fn write_to(path: &Path, artifact: &RenderedAdapter) -> Result<(), AdapterError> {
    write_source_to(path, &artifact.source)
}

/// Replace one already-verified managed generated source slot. This is kept
/// private to the combined exporter so callers cannot turn an arbitrary path
/// plus ACMD text into a general source-writing surface.
fn write_source_to(path: &Path, source: &str) -> Result<(), AdapterError> {
    let parent = path.parent().ok_or_else(|| AdapterError {
        subject: "adapter / destination".to_string(),
        reason: "destination has no parent directory".to_string(),
    })?;
    if !parent.is_dir() {
        return Err(AdapterError {
            subject: "adapter / destination".to_string(),
            reason: format!("parent {} is not an existing directory", parent.display()),
        });
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(AdapterError {
            subject: "adapter / destination".to_string(),
            reason: "destination must end in a UTF-8 filename".to_string(),
        });
    };
    if name.is_empty() || !name.ends_with(".rs") {
        return Err(AdapterError {
            subject: "adapter / destination".to_string(),
            reason: "destination must be a Rust source file ending in .rs".to_string(),
        });
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(AdapterError {
                subject: "adapter / destination".to_string(),
                reason: "refusing to replace a symlinked destination".to_string(),
            });
        }
        if !metadata.file_type().is_file() {
            return Err(AdapterError {
                subject: "adapter / destination".to_string(),
                reason: "destination exists but is not a regular file".to_string(),
            });
        }
    }
    syn::parse_file(source).map_err(|error| AdapterError {
        subject: "adapter / file".to_string(),
        reason: format!("refusing to write unparsable source: {error}"),
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| AdapterError {
        subject: "adapter / destination".to_string(),
        reason: format!(
            "could not create a temporary file beside {}: {error}",
            path.display()
        ),
    })?;
    temporary
        .write_all(source.as_bytes())
        .and_then(|()| temporary.as_file_mut().sync_all())
        .map_err(|error| AdapterError {
            subject: "adapter / destination".to_string(),
            reason: format!("could not stage {}: {error}", path.display()),
        })?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| AdapterError {
            subject: "adapter / destination".to_string(),
            reason: format!("could not replace {}: {}", path.display(), error.error),
        })
}

/// Atomically replace the one managed generated-adapter slot in an explicitly
/// selected Skyline plugin root.
pub fn export_to_plugin(
    root: &Path,
    artifact: &RenderedAdapter,
) -> Result<PluginExport, AdapterError> {
    if artifact_uses_controller_bundle(&artifact.source)? {
        return Err(contract_error(
            "multi-chart controller artifacts require the selected combined-plugin export path",
        ));
    }
    let destination = verify_plugin_destination(root)?;
    let existing = std::fs::read_to_string(&destination).map_err(|error| AdapterError {
        subject: "adapter / plugin destination".to_string(),
        reason: format!("could not read {}: {error}", destination.display()),
    })?;
    if existing == artifact.source {
        return Ok(PluginExport {
            destination,
            changed: false,
        });
    }
    write_to(&destination, artifact)?;
    Ok(PluginExport {
        destination,
        changed: true,
    })
}

/// Atomically update the two fixed generated slots of an explicitly selected
/// compatible plugin. If the second replacement cannot be made, the first is
/// restored before this returns. Later package/build failures are restored by
/// [`super::package`], which holds the pre-write contents for both slots.
pub fn export_combined_to_plugin(
    root: &Path,
    behavior: &RenderedAdapter,
    acmd: &super::acmd_adapter::RenderedAcmdAdapter,
) -> Result<CombinedPluginExport, AdapterError> {
    let destinations = verify_plugin_for_combined_artifacts(root, behavior, acmd)?;
    let prior_behavior = read_contract_file(&destinations.behavior)?;
    let prior_acmd = read_contract_file(&destinations.acmd)?;
    let behavior_changed = prior_behavior != behavior.source;
    let acmd_changed = prior_acmd != acmd.source;

    if behavior_changed {
        write_source_to(&destinations.behavior, &behavior.source)?;
    }
    if acmd_changed {
        if let Err(error) = write_source_to(&destinations.acmd, &acmd.source) {
            if behavior_changed {
                let restore = write_source_to(&destinations.behavior, &prior_behavior);
                return match restore {
                    Ok(()) => Err(error),
                    Err(restore_error) => Err(AdapterError {
                        subject: "adapter / combined plugin destination".to_string(),
                        reason: format!(
                            "{error}; additionally could not restore the behavior adapter: {restore_error}"
                        ),
                    }),
                };
            }
            return Err(error);
        }
    }
    Ok(CombinedPluginExport {
        behavior_changed,
        acmd_changed,
    })
}

/// Verify the fixed, opt-in plugin layout and return its only writable slot.
///
/// Kept public for callers that want to present a preflight result without
/// rendering or mutating a project. It never writes to the selected root.
pub fn verify_plugin_destination(root: &Path) -> Result<PathBuf, AdapterError> {
    Ok(verify_plugin_destinations(root)?.behavior)
}

/// Verify the fixed, opt-in plugin layout and return both managed generated
/// source slots. This is the combined-package preflight: it performs no
pub fn verify_plugin_destinations(root: &Path) -> Result<PluginDestinations, AdapterError> {
    require_regular_directory(root, "plugin root")?;
    let cargo = checked_file(root, Path::new("Cargo.toml"))?;
    verify_plugin_manifest(&read_contract_file(&cargo)?)?;
    let behavior = checked_file(root, Path::new(GENERATED_RELATIVE_PATH))?;
    if !read_contract_file(&behavior)?.contains(GENERATED_SLOT_MARKER) {
        return Err(contract_error(
            "selected behavior file is not a managed slot",
        ));
    }
    let acmd = checked_file(root, Path::new(GENERATED_ACMD_RELATIVE_PATH))?;
    if !read_contract_file(&acmd)?.contains(super::acmd_adapter::GENERATED_ACMD_SLOT_MARKER) {
        return Err(contract_error("selected ACMD file is not a managed slot"));
    }
    Ok(PluginDestinations { behavior, acmd })
}

pub(crate) fn verify_plugin_for_artifact(
    root: &Path,
    behavior: &RenderedAdapter,
) -> Result<PluginDestinations, AdapterError> {
    syn::parse_file(&behavior.source)
        .map_err(|error| contract_error(&format!("invalid behavior artifact: {error}")))?;
    verify_plugin_destinations(root)
}

pub(crate) fn verify_plugin_for_combined_artifacts(
    root: &Path,
    behavior: &RenderedAdapter,
    acmd: &super::acmd_adapter::RenderedAcmdAdapter,
) -> Result<PluginDestinations, AdapterError> {
    syn::parse_file(&acmd.source)
        .map_err(|error| contract_error(&format!("invalid ACMD artifact: {error}")))?;
    verify_plugin_for_artifact(root, behavior)
}

fn artifact_uses_controller_bundle(source: &str) -> Result<bool, AdapterError> {
    let file = syn::parse_file(source)
        .map_err(|error| contract_error(&format!("invalid behavior artifact: {error}")))?;
    Ok(file.items.iter().any(|item| {
        matches!(item,
        syn::Item::Const(item) if item.ident == "VISIONARY_STATIC_CONTROLLER_BUNDLE")
    }))
}

fn contract_error(reason: &str) -> AdapterError {
    AdapterError {
        subject: "adapter / plugin destination".to_string(),
        reason: reason.to_string(),
    }
}

/// Parse the selected manifest structurally. In particular, a comment that
/// merely mentions the package name or `smash_script` revision must not grant
/// write access to either managed source slot.
fn verify_plugin_manifest(cargo_text: &str) -> Result<(), AdapterError> {
    let manifest: toml::Value = cargo_text
        .parse()
        .map_err(|error| contract_error(&format!("Cargo.toml is not valid TOML: {error}")))?;
    let package_name = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str);
    if package_name != Some("slight_replica") {
        return Err(contract_error(
            "Cargo.toml does not identify the reviewed slight_replica plugin",
        ));
    }
    Ok(())
}

fn read_contract_file(path: &Path) -> Result<String, AdapterError> {
    std::fs::read_to_string(path).map_err(|error| AdapterError {
        subject: "adapter / plugin destination".to_string(),
        reason: format!("could not read {}: {error}", path.display()),
    })
}

/// Resolve a fixed relative path one component at a time, refusing a symlink
/// in either the parent chain or final component. The fixed paths above never
/// contain `..`, but validating that here keeps this boundary robust if the
/// contract gains another file later.
fn checked_file(root: &Path, relative: &Path) -> Result<PathBuf, AdapterError> {
    let mut current = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let std::path::Component::Normal(part) = component else {
            return Err(contract_error(
                "plugin contract path is not a normal relative path",
            ));
        };
        current.push(part);
        if components.peek().is_some() {
            require_regular_directory(&current, "plugin contract directory")?;
        } else {
            require_regular_file(&current, "plugin contract file")?;
        }
    }
    Ok(current)
}

fn require_regular_directory(path: &Path, label: &str) -> Result<(), AdapterError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| AdapterError {
        subject: "adapter / plugin destination".to_string(),
        reason: format!("{label} {} is unavailable: {error}", path.display()),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(contract_error(&format!(
            "{label} {} must be a non-symlink directory",
            path.display()
        )));
    }
    Ok(())
}

fn require_regular_file(path: &Path, label: &str) -> Result<(), AdapterError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| AdapterError {
        subject: "adapter / plugin destination".to_string(),
        reason: format!("{label} {} is unavailable: {error}", path.display()),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(contract_error(&format!(
            "{label} {} must be a non-symlink regular file",
            path.display()
        )));
    }
    Ok(())
}

struct RenderedProgram {
    fighter: String,
    kind_symbol: String,
    status_symbols: Vec<String>,
    program_name: String,
    registrations_name: String,
    damage_registrations_name: Option<String>,
    source: String,
}

impl RenderedProgram {
    fn registration_call(&self) -> String {
        match &self.damage_registrations_name {
            Some(damage) => format!(
                "    behavior::register_with_damage(&{}, &{}, &{damage})?;\n",
                self.program_name, self.registrations_name
            ),
            None => format!(
                "    behavior::register(&{}, &{})?;\n",
                self.program_name, self.registrations_name
            ),
        }
    }
}

#[derive(Default)]
struct Names {
    taken: BTreeSet<String>,
}

impl Names {
    fn unique(&mut self, raw: &str) -> String {
        let base = format!("gimmick_{}", tag(raw));
        if self.taken.insert(base.clone()) {
            return base;
        }
        let mut suffix = 2;
        loop {
            let candidate = format!("{base}_{suffix}");
            if self.taken.insert(candidate.clone()) {
                return candidate;
            }
            suffix += 1;
        }
    }
}

struct ProgramRenderer<'a> {
    fighter: &'a str,
    graph: &'a BehaviorGraph,
    prefix: String,
    kind_symbol: &'static str,
    next_expr: usize,
    next_predicate: usize,
    next_event_arguments: usize,
    next_event_values: usize,
    statics: String,
}

impl<'a> ProgramRenderer<'a> {
    fn new(
        fighter: &'a str,
        graph: &'a BehaviorGraph,
        names: &mut Names,
    ) -> Result<Self, Vec<AdapterError>> {
        let mut errors = gate(fighter, graph);
        let kind_symbol = registry::fighter_kind_symbol(fighter);
        if kind_symbol.is_none() {
            errors.push(AdapterError {
                subject: format!("{fighter} / fighter kind"),
                reason: "no exact FIGHTER_KIND_* symbol exists in the pinned registry".to_string(),
            });
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(Self {
            fighter,
            graph,
            prefix: names.unique(fighter).to_ascii_uppercase(),
            kind_symbol: kind_symbol.expect("checked above"),
            next_expr: 0,
            next_predicate: 0,
            next_event_arguments: 0,
            next_event_values: 0,
            statics: String::new(),
        })
    }

    fn render(mut self) -> Result<RenderedProgram, Vec<AdapterError>> {
        let program_name = format!("{}_PROGRAM", self.prefix);
        let registrations_name = format!("{}_REGISTRATIONS", self.prefix);
        let bindings_name = format!("{}_BINDINGS", self.prefix);
        let event_bindings_name = format!("{}_EVENT_BINDINGS", self.prefix);
        let vars_name = format!("{}_VARS", self.prefix);
        let native_inputs_name = format!("{}_NATIVE_INPUTS", self.prefix);
        let transitions_name = format!("{}_TRANSITIONS", self.prefix);
        let native_inputs: Vec<_> = super::behavior::collect_native_inputs(self.graph)
            .into_iter()
            .map(|source| source.canonical())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let unsupported_sources: Vec<_> = native_inputs
            .iter()
            .filter(|source| !source.is_supported() || !source.valid_for(self.fighter))
            .collect();
        if !unsupported_sources.is_empty() {
            return Err(unsupported_sources
                .iter()
                .map(|source| AdapterError {
                    subject: format!("{} / native input", self.fighter),
                    reason: format!("{} is not supported for this fighter", source.symbol()),
                })
                .collect());
        }
        let matcher = format!("is_{}", self.prefix.to_ascii_lowercase());
        let mut out = format!(
            "fn {matcher}(boid: u32) -> bool {{\n    crate::slight::agents::lookup(boid).is_some_and(|record| record.category == 0 && record.kind == *{kind})\n        && super::live_behavior::actor_matches(boid, {fighter})\n}}\n\n",
            kind = self.kind_symbol,
            fighter = literal_string(self.fighter),
        );

        let mut guards = Vec::new();
        let mut actions = Vec::new();
        for (binding_index, binding) in self.graph.emission_plan.event_bindings.iter().enumerate() {
            if binding.fields.is_empty() {
                continue;
            }
            let name = format!("{}_EVENT_FIELDS_{binding_index}", self.prefix);
            let fields = binding
                .fields
                .iter()
                .map(|field| {
                    format!(
                        "behavior::EventField {{ name: {}, kind: behavior::VarKind::{} }}",
                        literal_string(&field.name),
                        kind(field.kind)
                    )
                })
                .collect::<Vec<_>>();
            self.statics.push_str(&format!(
                "static {name}: [behavior::EventField; {}] = [{}];\n",
                fields.len(),
                fields.join(", ")
            ));
        }
        for (index, transition) in self.graph.transitions.iter().enumerate() {
            guards.push(self.predicate(&transition.guard));
            let name = format!("{}_ACTIONS_{index}", self.prefix);
            let values: Vec<String> = transition
                .actions
                .iter()
                .map(|action| self.action(index, action))
                .collect();
            self.statics.push_str(&format!(
                "static {name}: [Action; {}] = [{}];\n",
                values.len(),
                values.join(", ")
            ));
            actions.push(name);
        }
        out.push_str(&self.statics);
        out.push_str(&format!(
            "static {native_inputs_name}: [NativeSource; {}] = [",
            native_inputs.len()
        ));
        for source in &native_inputs {
            out.push_str(&format!("{},", native_source(source)));
        }
        out.push_str("];\n");
        out.push_str(&format!(
            "static {vars_name}: [VarDef; {}] = [",
            self.graph.vars.len()
        ));
        for variable in &self.graph.vars {
            out.push_str(&format!(
                "VarDef {{ name: {}, kind: VarKind::{}, initial: {} }},",
                literal_string(&variable.name),
                kind(variable.kind),
                value(variable.initial),
            ));
        }
        out.push_str("];\n");
        out.push_str(&format!(
            "static {transitions_name}: [Transition; {}] = [",
            self.graph.transitions.len()
        ));
        for (index, transition) in self.graph.transitions.iter().enumerate() {
            out.push_str(&format!(
                "Transition {{ from: {}, to: {}, guard: &{}, actions: &{} }},",
                state_id(self.graph, &transition.from),
                state_id(self.graph, &transition.to),
                guards[index],
                actions[index],
            ));
        }
        out.push_str("];\n");
        out.push_str(&format!(
            "static {bindings_name}: [Binding; {}] = [",
            self.graph.emission_plan.bindings.len()
        ));
        for binding in &self.graph.emission_plan.bindings {
            out.push_str(&format!(
                "Binding {{ hook: GimmickHook::{}, article: {}, transitions: &[{}] }},",
                runtime_hook(&self.graph.emission_plan, binding.hook),
                binding
                    .article
                    .as_ref()
                    .map(|symbol| format!("Some({})", literal_string(symbol)))
                    .unwrap_or_else(|| "None".into()),
                binding
                    .transitions
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push_str("];\n");
        out.push_str(&format!(
            "static {event_bindings_name}: [EventBinding; {}] = [",
            self.graph.emission_plan.event_bindings.len()
        ));
        for (binding_index, binding) in self.graph.emission_plan.event_bindings.iter().enumerate() {
            let fields = if binding.fields.is_empty() {
                "&[]".to_string()
            } else {
                format!("&{}_EVENT_FIELDS_{binding_index}", self.prefix)
            };
            out.push_str(&format!(
                "EventBinding {{ name: {}, transitions: &[{}], fields: {fields} }},",
                literal_string(&binding.name),
                binding
                    .transitions
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push_str("];\n");
        out.push_str(&format!(
            "static {program_name}: Program = Program {{ id: {}, fighter: {}, matches_fighter: {matcher}, initial_state: {}, vars: &{vars_name}, native_inputs: &{native_inputs_name}, transitions: &{transitions_name}, bindings: &{bindings_name}, event_bindings: &{event_bindings_name} }};\n\n",
            literal_string(&format!("visionary/{}", self.fighter)),
            literal_string(self.fighter),
            state_id(self.graph, &self.graph.initial),
        ));

        let mut registrations = Vec::new();
        let mut damage_registrations = Vec::new();
        for (index, binding) in self.graph.emission_plan.bindings.iter().enumerate() {
            if matches!(
                binding.hook,
                HookName::OnStatusEnter | HookName::OnStatusExit
            ) {
                let mut groups: Vec<(u32, String, Vec<usize>)> = Vec::new();
                for trigger in self
                    .graph
                    .emission_plan
                    .status_triggers
                    .iter()
                    .filter(|trigger| trigger.hook == binding.hook)
                {
                    let Some(entry) = registry::resolve(
                        self.fighter,
                        &trigger.symbol,
                        registry::IdentifierKind::FighterStatus,
                    )
                    .ok() else {
                        continue;
                    };
                    let group = if let Some(group) =
                        groups.iter_mut().find(|group| group.0 == entry.value)
                    {
                        group
                    } else {
                        groups.push((entry.value, trigger.symbol.clone(), Vec::new()));
                        groups.last_mut().expect("just pushed status group")
                    };
                    for index in &trigger.transitions {
                        if !group.2.contains(index) {
                            group.2.push(*index);
                        }
                    }
                }
                for (_, _, transitions) in &mut groups {
                    let selected: BTreeSet<_> = transitions.iter().copied().collect();
                    *transitions = binding
                        .transitions
                        .iter()
                        .filter(|index| selected.contains(index))
                        .copied()
                        .collect();
                }
                for (status_index, (_, symbol, transitions)) in groups.into_iter().enumerate() {
                    let status_scope = if binding.hook == HookName::OnStatusEnter {
                        "status"
                    } else {
                        "status_exit"
                    };
                    let suffix = format!(
                        "{}_{}_{}",
                        self.prefix.to_ascii_lowercase(),
                        status_scope,
                        status_index
                    );
                    let transition_list = format!(
                        "{}_{}_{}_TRANSITIONS",
                        self.prefix.to_ascii_uppercase(),
                        status_scope.to_ascii_uppercase(),
                        status_index
                    );
                    let handler = format!("{suffix}_handler");
                    let resolver = format!("{suffix}_resolver");
                    out.push_str(&format!(
                        "static {transition_list}: [usize; {}] = [{}];\n",
                        transitions.len(),
                        transitions
                            .iter()
                            .map(usize::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    out.push_str(&format!(
                        "fn {handler}(boid: u32) {{ report(behavior::execute_status(&{program_name}, GimmickHook::{}, boid, &{transition_list})); }}\n",
                        runtime_hook(&self.graph.emission_plan, binding.hook)
                    ));
                    out.push_str(&format!("fn {resolver}() -> i32 {{ *{symbol} }}\n"));
                    registrations.push(format!(
                        "Registration {{ hook: GimmickHook::{}, handler: {handler}, status: Some({resolver}), transitions: Some(&{transition_list}) }}",
                        runtime_hook(&self.graph.emission_plan, binding.hook)
                    ));
                }
                let _ = index;
                continue;
            }
            let handler = format!("{}_on_{}", self.prefix.to_ascii_lowercase(), index);
            if matches!(
                binding.hook,
                HookName::OnDealDamage
                    | HookName::OnTakeDamage
                    | HookName::OnAttackShielded
                    | HookName::OnShieldHit
            ) {
                out.push_str(&format!(
                    "fn {handler}(event: super::damage_dispatch::Event) {{ report(behavior::execute_damage_event(&{program_name}, event)); }}\n"
                ));
                let direction = match binding.hook {
                    HookName::OnDealDamage => "Deal",
                    HookName::OnTakeDamage => "Take",
                    HookName::OnAttackShielded => "AttackShielded",
                    HookName::OnShieldHit => "ShieldHit",
                    _ => unreachable!(),
                };
                damage_registrations.push(format!("behavior::DamageRegistration {{ direction: super::damage_dispatch::Direction::{direction}, handler: {handler} }}"));
                continue;
            }
            out.push_str(&format!(
                "fn {handler}(boid: u32) {{ report(behavior::execute(&{program_name}, GimmickHook::{}, boid)); }}\n",
                runtime_hook(&self.graph.emission_plan, binding.hook)
            ));
            let status = self
                .graph
                .emission_plan
                .status_triggers
                .iter()
                .find(|trigger| trigger.hook == binding.hook)
                .map(|trigger| {
                    let resolver = format!("{}_status_{}", self.prefix.to_ascii_lowercase(), index);
                    out.push_str(&format!(
                        "fn {resolver}() -> i32 {{ *{} }}\n",
                        trigger.symbol
                    ));
                    resolver
                });
            registrations.push(format!(
                "Registration {{ hook: GimmickHook::{}, handler: {handler}, status: {}, transitions: None }}",
                runtime_hook(&self.graph.emission_plan, binding.hook),
                status
                    .map(|resolver| format!("Some({resolver})"))
                    .unwrap_or_else(|| "None".to_string())
            ));
        }
        out.push_str(&format!(
            "static {registrations_name}: [Registration; {}] = [{}];\n\n",
            registrations.len(),
            registrations.join(", ")
        ));
        let damage_registrations_name = if damage_registrations.is_empty() {
            None
        } else {
            let name = format!("{}_DAMAGE_REGISTRATIONS", self.prefix);
            out.push_str(&format!(
                "static {name}: [behavior::DamageRegistration; {}] = [{}];\n\n",
                damage_registrations.len(),
                damage_registrations.join(", ")
            ));
            Some(name)
        };
        Ok(RenderedProgram {
            fighter: self.fighter.to_string(),
            kind_symbol: self.kind_symbol.to_string(),
            status_symbols: self
                .graph
                .emission_plan
                .status_triggers
                .iter()
                .map(|trigger| trigger.symbol.clone())
                .collect(),
            program_name,
            registrations_name,
            damage_registrations_name,
            source: out,
        })
    }

    fn expr(&mut self, expr: &Expr) -> String {
        let text = match expr {
            Expr::StatusFrame
            | Expr::MotionFrame
            | Expr::StickX { .. }
            | Expr::StickY
            | Expr::RandomInt { .. }
            | Expr::RandomFloat { .. } => unreachable!("shared controller bundle"),
            Expr::PendingDamage => "Expr::PendingDamage".to_owned(),
            Expr::Const(constant) => format!("Expr::Const({})", value(*constant)),
            Expr::Var(name) => format!("Expr::Var({})", literal_string(name)),
            Expr::TimerRemaining(name) => {
                format!("Expr::TimerRemaining({})", literal_string(name))
            }
            Expr::EventField { event, field } => format!(
                "Expr::EventField {{ event: {}, field: {} }}",
                literal_string(event),
                literal_string(field)
            ),
            Expr::NativeFloat(source) => format!("Expr::NativeFloat({})", native_source(source)),
            Expr::NativeInt(source) => format!("Expr::NativeInt({})", native_source(source)),
            Expr::Add(left, right) => {
                format!("Expr::Add(&{}, &{})", self.expr(left), self.expr(right))
            }
            Expr::Sub(left, right) => {
                format!("Expr::Sub(&{}, &{})", self.expr(left), self.expr(right))
            }
            Expr::Mul(left, right) => {
                format!("Expr::Mul(&{}, &{})", self.expr(left), self.expr(right))
            }
        };
        let name = format!("{}_EXPR_{}", self.prefix, self.next_expr);
        self.next_expr += 1;
        self.statics
            .push_str(&format!("static {name}: Expr = {text};\n"));
        name
    }

    fn predicate(&mut self, predicate: &Predicate) -> String {
        let text = match predicate {
            Predicate::DamageIsProjectile
            | Predicate::InStatus { .. }
            | Predicate::InMotion { .. }
            | Predicate::SituationChanged
            | Predicate::MotionEnded
            | Predicate::CancelEnabled
            | Predicate::TermEnabled { .. }
            | Predicate::Situation { .. }
            | Predicate::Button { .. } => unreachable!("shared controller bundle"),
            Predicate::Flag(name) => format!("Predicate::Flag({})", literal_string(name)),
            Predicate::EventFlag { event, field } => format!(
                "Predicate::EventFlag {{ event: {}, field: {} }}",
                literal_string(event),
                literal_string(field)
            ),
            Predicate::DamageStatusIs(super::native_inputs::NativeInput::FighterStatusIs {
                fighter,
                symbol,
            }) => format!(
                "Predicate::DamageStatusIs({},{})",
                literal_string(fighter),
                literal_string(symbol)
            ),
            Predicate::DamageStatusIs(_) => {
                unreachable!("contact status selector validated before rendering")
            }
            Predicate::NativeFlag(source) => {
                format!("Predicate::NativeFlag({})", native_source(source))
            }
            Predicate::Expired(name) => format!("Predicate::Expired({})", literal_string(name)),
            Predicate::Compare { op, left, right } => format!(
                "Predicate::Compare {{ op: CmpOp::{}, left: &{}, right: &{} }}",
                cmp(*op),
                self.expr(left),
                self.expr(right)
            ),
            Predicate::Not(inner) => format!("Predicate::Not(&{})", self.predicate(inner)),
            Predicate::And(left, right) => format!(
                "Predicate::And(&{}, &{})",
                self.predicate(left),
                self.predicate(right)
            ),
            Predicate::Or(left, right) => format!(
                "Predicate::Or(&{}, &{})",
                self.predicate(left),
                self.predicate(right)
            ),
        };
        let name = format!("{}_PREDICATE_{}", self.prefix, self.next_predicate);
        self.next_predicate += 1;
        self.statics
            .push_str(&format!("static {name}: Predicate = {text};\n"));
        name
    }

    fn action(&mut self, transition_index: usize, action: &Action) -> String {
        match action {
            Action::Status(_) => unreachable!("shared controller bundle"),
            Action::AddHeldOpponentDamage { .. }
            | Action::AddDamage { .. }
            | Action::CombatPolicy { .. }
            | Action::ThrowHeldOpponent { .. }
            | Action::ReleaseHeldOpponent
            | Action::CancelIncomingHit
            | Action::Movement(_)
            | Action::ArticleMovement { .. }
            | Action::Item(_)
            | Action::MeshVisibility { .. }
            | Action::SetMotionRate { .. }
            | Action::ChooseHeroMenuSpell { .. }
            | Action::AddSpeed { .. }
            | Action::Object { .. }
            | Action::Resource(_)
            | Action::SetNative { .. }
            | Action::AddNative { .. }
            | Action::SetNativeFlag { .. } => {
                unreachable!("shared controller bundle")
            }
            Action::Set {
                var,
                value: expression,
            } => format!(
                "Action::Set {{ var: {}, value: &{} }}",
                literal_string(var),
                self.expr(expression)
            ),
            Action::SetFlag { var, value } => format!(
                "Action::SetFlag {{ var: {}, value: &{} }}",
                literal_string(var),
                self.predicate(value)
            ),
            Action::Add {
                var,
                value: expression,
            } => format!(
                "Action::Add {{ var: {}, value: &{} }}",
                literal_string(var),
                self.expr(expression)
            ),
            Action::Raise { var } => format!("Action::Raise {{ var: {} }}", literal_string(var)),
            Action::Clear { var } => format!("Action::Clear {{ var: {} }}", literal_string(var)),
            Action::Reset { var } => format!("Action::Reset {{ var: {} }}", literal_string(var)),
            Action::GenerateArticle { symbol } => format!(
                "Action::GenerateArticle {{ symbol: {} }}",
                literal_string(symbol)
            ),
            Action::RemoveNewestArticle { symbol } => format!(
                "Action::RemoveNewestArticle {{ symbol: {} }}",
                literal_string(symbol)
            ),
            Action::RequestArticleStatus { symbol, status } => format!(
                "Action::RequestArticleStatus {{ symbol: {}, status: {} }}",
                literal_string(symbol),
                literal_string(status)
            ),
            Action::RemoveArticle { symbol } => format!(
                "Action::RemoveArticle {{ symbol: {} }}",
                literal_string(symbol)
            ),
            Action::RequestFighterStatus { symbol } => format!(
                "Action::RequestFighterStatus {{ symbol: {} }}",
                literal_string(symbol)
            ),
            Action::RequestWeaponStatus { target, symbol } => format!(
                "Action::RequestWeaponStatus {{ weapon_boid: 0x{:X}, symbol: {} }}",
                target.battle_object_id,
                literal_string(symbol)
            ),
            Action::EmitEvent { name } => {
                format!("Action::EmitEvent {{ name: {} }}", literal_string(name))
            }
            Action::EmitEventArguments { name, arguments } => {
                let binding = self
                    .graph
                    .emission_plan
                    .event_bindings
                    .iter()
                    .find(|binding| binding.name == *name)
                    .expect("adapter gate validates every typed event target");
                let values = super::event_payload::canonicalize_arguments(
                    self.fighter,
                    name,
                    &binding.fields,
                    arguments,
                )
                .expect("adapter gate validates typed event arguments");
                let arguments_name = format!(
                    "{}_EVENT_ARGUMENTS_{}",
                    self.prefix, self.next_event_arguments
                );
                self.next_event_arguments += 1;
                let descriptors = binding
                    .fields
                    .iter()
                    .zip(values)
                    .map(|(field, constant)| {
                        format!(
                            "behavior::EventArgument {{ field: {}, value: {} }}",
                            literal_string(&field.name),
                            value(constant)
                        )
                    })
                    .collect::<Vec<_>>();
                self.statics.push_str(&format!(
                    "static {arguments_name}: [behavior::EventArgument; {}] = [{}];\n",
                    descriptors.len(),
                    descriptors.join(", ")
                ));
                format!(
                    "Action::EmitEventArguments {{ name: {}, arguments: &{} }}",
                    literal_string(name),
                    arguments_name
                )
            }
            Action::EmitEventValues { name, arguments } => {
                let binding = self
                    .graph
                    .emission_plan
                    .event_bindings
                    .iter()
                    .find(|binding| binding.name == *name)
                    .expect("adapter gate validates every dynamic event target");
                let ordered = super::event_payload::canonicalize_value_arguments(
                    self.fighter,
                    name,
                    &binding.fields,
                    arguments,
                    |value| super::behavior::event_value_kind(self.graph, transition_index, value),
                )
                .expect("adapter gate validates dynamic event arguments");
                let arguments_name =
                    format!("{}_EVENT_VALUES_{}", self.prefix, self.next_event_values);
                self.next_event_values += 1;
                let descriptors = ordered
                    .into_iter()
                    .map(|argument| {
                        let value = match &argument.value {
                            super::event_payload::EventValue::Constant(constant) => {
                                format!("behavior::EventValue::Constant({})", value(*constant))
                            }
                            super::event_payload::EventValue::Numeric(expression) => {
                                format!("behavior::EventValue::Numeric(&{})", self.expr(expression))
                            }
                            super::event_payload::EventValue::Flag(predicate) => format!(
                                "behavior::EventValue::Flag(&{})",
                                self.predicate(predicate)
                            ),
                        };
                        format!(
                            "behavior::EventValueArgument {{ field: {}, value: {} }}",
                            literal_string(&argument.field),
                            value
                        )
                    })
                    .collect::<Vec<_>>();
                self.statics.push_str(&format!(
                    "static {arguments_name}: [behavior::EventValueArgument; {}] = [{}];\n",
                    descriptors.len(),
                    descriptors.join(", ")
                ));
                format!(
                    "Action::EmitEventValues {{ name: {}, arguments: &{} }}",
                    literal_string(name),
                    arguments_name
                )
            }
        }
    }
}

fn gate(fighter: &str, graph: &BehaviorGraph) -> Vec<AdapterError> {
    let mut errors: Vec<AdapterError> = super::behavior::validate_for_fighter(fighter, graph)
        .into_iter()
        .map(|error| AdapterError {
            subject: format!("{fighter} / {}", error.item),
            reason: error.reason,
        })
        .collect();
    errors.extend(
        codegen::check_plan(graph, &graph.emission_plan)
            .into_iter()
            .map(|error| AdapterError {
                subject: format!("{fighter} / {}", error.item),
                reason: error.reason,
            }),
    );
    errors.extend(
        codegen::check_status_triggers(fighter, &graph.emission_plan)
            .into_iter()
            .map(|error| AdapterError {
                subject: format!("{fighter} / {}", error.item),
                reason: error.reason,
            }),
    );
    let report = verifier::verify_behavior(fighter, graph, &graph.emission_plan);
    errors.extend(report.blockers().map(|finding| AdapterError {
        subject: finding.subject.clone(),
        reason: finding.message.clone(),
    }));
    errors
}

fn tag(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push_str("fighter");
    }
    if out.as_bytes()[0].is_ascii_digit() {
        out.insert(0, '_');
    }
    out
}

/// Static artifacts carry the saved reference; the plugin parses it with the
/// same `NativeInput::from_reference` the editor uses.
fn native_source(source: &super::native_inputs::NativeInput) -> String {
    format!(
        "NativeSource({})",
        literal_string(&source.canonical().reference())
    )
}

fn literal_string(value: &str) -> String {
    format!("{value:?}")
}
fn value(value: ConstValue) -> String {
    match value {
        ConstValue::Float(v) => format!("Value::Float({v:?})"),
        ConstValue::Int(v) => format!("Value::Int({v})"),
        ConstValue::Flag(v) => format!("Value::Flag({v})"),
    }
}
fn kind(kind: VarType) -> &'static str {
    match kind {
        VarType::Float => "Float",
        VarType::Int => "Int",
        VarType::Flag => "Flag",
        VarType::Timer => "Timer",
    }
}
fn cmp(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "Eq",
        CmpOp::Lt => "Lt",
        CmpOp::Lte => "Lte",
        CmpOp::Gt => "Gt",
        CmpOp::Gte => "Gte",
    }
}
fn state_id(graph: &BehaviorGraph, state: &str) -> usize {
    graph
        .states
        .iter()
        .position(|candidate| candidate.name == state)
        .unwrap_or(0)
}
fn hook(hook: HookName) -> &'static str {
    match hook {
        HookName::OnDealDamage => "DealDamage",
        HookName::OnTakeDamage => "TakeDamage",
        HookName::OnAttackShielded => "AttackShielded",
        HookName::OnShieldHit => "ShieldHit",
        HookName::OnGrab => "Grab",
        HookName::OnItemPickup => "ItemPickup",
        HookName::OnItemThrown => "ItemThrown",
        HookName::OnThrow => "Throw",
        HookName::OnGrabbed => "Grabbed",
        HookName::OnChargeTick => "ChargeTick",
        HookName::OnSpend => "Spend",
        HookName::OnStatusEnter => "StatusEnter",
        HookName::OnStatusExit => "StatusExit",
        HookName::OnTimeout => "Timeout",
        HookName::OnSwitch => "Switch",
        HookName::OnStockStart => "StockStart",
        HookName::OnKo => "Ko",
        HookName::OnArticleSpawn => "ArticleSpawn",
        HookName::OnArticleRemoved => "ArticleRemoved",
        _ => unreachable!("the plan gate rejects unavailable hooks"),
    }
}

fn runtime_hook(plan: &codegen::EmissionPlan, name: HookName) -> &'static str {
    plan.resource_triggers
        .iter()
        .find(|trigger| trigger.hook == name)
        .map_or_else(|| hook(name), |trigger| trigger.source.runtime_hook())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::behavior::{StateDef, Transition, VarDef};
    use crate::gimmicks::codegen::{EmissionPlan, EventBinding, HookBinding};
    use crate::gimmicks::native_inputs::NativeInput;

    fn cloud() -> BehaviorGraph {
        BehaviorGraph {
            costumes: Vec::new(),
            custom_statuses: Default::default(),
            initial: "idle".into(),
            states: vec![
                StateDef {
                    status: None,
                    name: "idle".into(),
                },
                StateDef {
                    status: None,
                    name: "full".into(),
                },
            ],
            vars: vec![VarDef {
                keep_across_form_switch: false,
                name: "gauge".into(),
                kind: VarType::Float,
                initial: ConstValue::Float(0.0),
            }],
            transitions: vec![Transition {
                from: "idle".into(),
                to: "full".into(),
                guard: Predicate::Compare {
                    op: CmpOp::Gte,
                    left: Expr::Var("gauge".into()),
                    right: Expr::Const(ConstValue::Float(100.0)),
                },
                on_frame: true,
                actions: vec![],
            }],
            emission_plan: EmissionPlan {
                bindings: vec![HookBinding {
                    hook: HookName::OnChargeTick,
                    article: None,
                    transitions: vec![0],
                }],
                event_bindings: Vec::new(),
                resource_triggers: Vec::new(),
                status_triggers: Vec::new(),
            },
        }
    }

    #[test]
    fn adapter_mirrors_computed_flag_assignment_and_predicate_tree() {
        let mut graph = cloud();
        graph.vars.push(VarDef {
            keep_across_form_switch: false,
            name: "powered".into(),
            kind: VarType::Flag,
            initial: ConstValue::Flag(false),
        });
        graph.transitions[0].actions.push(Action::SetFlag {
            var: "powered".into(),
            value: Predicate::Compare {
                op: CmpOp::Gte,
                left: Expr::Var("gauge".into()),
                right: Expr::Const(ConstValue::Float(100.0)),
            },
        });
        let rendered = render(&BTreeMap::from([("cloud".into(), graph)])).unwrap();
        assert!(rendered
            .source
            .contains("Action::SetFlag { var: \"powered\""));
        assert!(rendered
            .source
            .contains("Predicate::Compare { op: CmpOp::Gte"));
    }

    fn cloud_with_typed_ready_event() -> BehaviorGraph {
        let mut graph = cloud();
        graph.transitions.push(Transition {
            from: "idle".into(),
            to: "idle".into(),
            guard: Predicate::And(
                Box::new(Predicate::EventFlag {
                    event: "ready".into(),
                    field: "active".into(),
                }),
                Box::new(Predicate::Compare {
                    op: CmpOp::Eq,
                    left: Expr::EventField {
                        event: "ready".into(),
                        field: "count".into(),
                    },
                    right: Expr::Const(ConstValue::Int(4)),
                }),
            ),
            on_frame: true,
            actions: Vec::new(),
        });
        graph.transitions[0]
            .actions
            .push(Action::EmitEventArguments {
                name: "ready".into(),
                // Authored in reverse order to prove rendering follows the receiver.
                arguments: vec![
                    super::super::event_payload::Argument {
                        field: "active".into(),
                        value: ConstValue::Flag(true),
                    },
                    super::super::event_payload::Argument {
                        field: "count".into(),
                        value: ConstValue::Int(4),
                    },
                ],
            });
        graph.emission_plan.event_bindings.push(EventBinding {
            name: "ready".into(),
            transitions: vec![1],
            fields: vec![
                super::super::event_payload::FieldDef {
                    name: "count".into(),
                    kind: VarType::Int,
                },
                super::super::event_payload::FieldDef {
                    name: "active".into(),
                    kind: VarType::Flag,
                },
            ],
        });
        graph
    }

    #[test]
    fn adapter_renders_schema_and_canonical_typed_event_constants() {
        let source = render(&BTreeMap::from([(
            "cloud".to_string(),
            cloud_with_typed_ready_event(),
        )]))
        .unwrap()
        .source;
        assert!(source.contains(
            "static GIMMICK_CLOUD_EVENT_FIELDS_0: [behavior::EventField; 2] = [behavior::EventField { name: \"count\", kind: behavior::VarKind::Int }, behavior::EventField { name: \"active\", kind: behavior::VarKind::Flag }];"
        ));
        assert!(source.contains(
            "static GIMMICK_CLOUD_EVENT_ARGUMENTS_0: [behavior::EventArgument; 2] = [behavior::EventArgument { field: \"count\", value: Value::Int(4) }, behavior::EventArgument { field: \"active\", value: Value::Flag(true) }];"
        ));
        assert!(source.contains(
            "Action::EmitEventArguments { name: \"ready\", arguments: &GIMMICK_CLOUD_EVENT_ARGUMENTS_0 }"
        ));
        assert!(source.contains(
            "EventBinding { name: \"ready\", transitions: &[1], fields: &GIMMICK_CLOUD_EVENT_FIELDS_0 }"
        ));
        assert!(source.contains("Expr::EventField { event: \"ready\", field: \"count\" }"));
        assert!(source.contains("Predicate::EventFlag { event: \"ready\", field: \"active\" }"));
    }

    #[test]
    fn registered_generic_work_float_integer_and_flag_render_as_static_sources() {
        let aura = NativeInput::from_registry(
            "lucario",
            "FIGHTER_LUCARIO_INSTANCE_WORK_ID_FLOAT_CURR_AURAPOWER",
            super::super::native_inputs::NativeKind::Float,
        )
        .unwrap();
        let spores = NativeInput::from_registry(
            "peach",
            "FIGHTER_PEACH_INSTANCE_WORK_ID_INT_KINOPIOSPORE_SHOOT_NUM",
            super::super::native_inputs::NativeKind::Int,
        )
        .unwrap();
        let raise = NativeInput::from_registry(
            "peach",
            "FIGHTER_PEACH_INSTANCE_WORK_ID_FLAG_SPECIAL_N_RAISE",
            super::super::native_inputs::NativeKind::Flag,
        )
        .unwrap();

        let mut lucario = cloud();
        lucario.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Gte,
            left: Expr::NativeFloat(aura.clone()),
            right: Expr::Const(ConstValue::Float(0.0)),
        };
        lucario.transitions[0].actions.push(Action::Set {
            var: "gauge".into(),
            value: Expr::NativeFloat(aura),
        });

        let mut peach = cloud();
        peach.vars.push(VarDef {
            keep_across_form_switch: false,
            name: "spores".into(),
            kind: VarType::Int,
            initial: ConstValue::Int(0),
        });
        peach.transitions[0].guard = Predicate::And(
            Box::new(Predicate::NativeFlag(raise)),
            Box::new(Predicate::Compare {
                op: CmpOp::Gte,
                left: Expr::NativeInt(spores.clone()),
                right: Expr::Const(ConstValue::Int(0)),
            }),
        );
        peach.transitions[0].actions.push(Action::Set {
            var: "spores".into(),
            value: Expr::NativeInt(spores),
        });

        let artifact = render(&BTreeMap::from([
            ("lucario".into(), lucario),
            ("peach".into(), peach),
        ]))
        .unwrap();
        for expected in [
            "NativeSource(\"work_float:lucario:FIGHTER_LUCARIO_INSTANCE_WORK_ID_FLOAT_CURR_AURAPOWER\")",
            "NativeSource(\"work_int:peach:FIGHTER_PEACH_INSTANCE_WORK_ID_INT_KINOPIOSPORE_SHOOT_NUM\")",
            "NativeSource(\"work_flag:peach:FIGHTER_PEACH_INSTANCE_WORK_ID_FLAG_SPECIAL_N_RAISE\")",
            "native_inputs: &GIMMICK_LUCARIO_NATIVE_INPUTS",
            "native_inputs: &GIMMICK_PEACH_NATIVE_INPUTS",
        ] {
            assert!(artifact.source.contains(expected), "missing {expected}");
        }
        assert!(syn::parse_file(&artifact.source).is_ok());
    }

    fn cloud_meter(value: meter::MeterValue, style: meter::MeterStyle) -> meter::MeterBinding {
        meter::MeterBinding {
            fighter: "cloud".into(),
            enabled: true,
            style,
            value,
        }
    }

    #[test]
    fn enabled_meters_render_symbolically_with_stable_ids_and_kinds() {
        let values = vec![
            cloud_meter(
                meter::MeterValue::Float {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                    min: 0.0,
                    max: 100.0,
                },
                meter::MeterStyle::Bar,
            ),
            cloud_meter(
                meter::MeterValue::Integer {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_BREAK_CLEAR_FRAME".into(),
                    min: 0,
                    max: 900,
                },
                meter::MeterStyle::StockArrows,
            ),
            cloud_meter(
                meter::MeterValue::Flag {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK".into(),
                },
                meter::MeterStyle::Icon,
            ),
            cloud_meter(
                meter::MeterValue::Pips {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_GAUGE_REMOVE_FRAME".into(),
                    count: 3,
                },
                meter::MeterStyle::Pips,
            ),
            cloud_meter(
                meter::MeterValue::Cycle {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_GAUGE_EFFECT".into(),
                    labels: vec!["A".into(), "B".into()],
                },
                meter::MeterStyle::Icon,
            ),
        ];
        let meters = BTreeMap::from([("cloud".into(), values)]);
        let artifact = render_with_meters(&BTreeMap::new(), &meters).unwrap();
        assert_eq!(artifact.fighters, ["cloud"]);
        for (index, kind) in ["WorkFloat", "WorkInt", "WorkFlag", "WorkInt", "WorkInt"]
            .into_iter()
            .enumerate()
        {
            assert!(artifact.source.contains(&format!("meter.cloud.{index}")));
            assert!(artifact.source.contains(&format!("IdentifierKind::{kind}")));
        }
        assert!(artifact
            .source
            .contains("FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE"));
        for presentation in [
            "MeterPresentation::FloatBar { min: 0.0, max: 100.0 }",
            "MeterPresentation::StockArrows { min: 0, max: 900 }",
            "MeterPresentation::FlagIcon",
            "MeterPresentation::Pips { count: 3 }",
            "MeterPresentation::CycleIcon { labels: &[\"A\", \"B\"] }",
        ] {
            assert!(
                artifact.source.contains(presentation),
                "missing {presentation}"
            );
        }
        assert!(!artifact.source.contains("0x4C"));
        assert_eq!(
            artifact,
            render_with_meters(&BTreeMap::new(), &meters).unwrap()
        );
        syn::parse_file(&artifact.source).unwrap();
    }

    #[test]
    fn symbolic_article_actions_are_typed_to_the_fighter_and_render_without_ids() {
        let mut graph = cloud();
        graph.transitions[0].actions = vec![Action::GenerateArticle {
            symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
        }];
        let artifact = render(&BTreeMap::from([("cloud".into(), graph.clone())])).unwrap();
        assert_eq!(
            artifact,
            render(&BTreeMap::from([("cloud".into(), graph)])).unwrap()
        );
        assert!(artifact.source.contains(
            "Action::GenerateArticle { symbol: \"FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE\" }"
        ));
        assert!(!artifact
            .source
            .contains("Action::GenerateArticle { symbol: 0"));

        let mut wrong_kind = cloud();
        wrong_kind.transitions[0].actions = vec![Action::RemoveArticle {
            symbol: "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR".into(),
        }];
        let errors = render(&BTreeMap::from([("cloud".into(), wrong_kind)])).unwrap_err();
        assert!(errors.iter().any(|error| {
            error.subject == "cloud / transition 0 / action 0"
                && error.reason.contains("allowlisted Article")
        }));

        let mut wrong_fighter = cloud();
        wrong_fighter.transitions[0].actions = vec![Action::GenerateArticle {
            symbol: "FIGHTER_BRAVE_GENERATE_ARTICLE_FIREBALL".into(),
        }];
        let errors = render(&BTreeMap::from([("cloud".into(), wrong_fighter)])).unwrap_err();
        assert!(errors.iter().any(|error| {
            error.subject == "cloud / transition 0 / action 0"
                && error.reason.contains("WrongFighter")
        }));
    }

    #[test]
    fn weapon_status_actions_render_the_explicit_target_and_reject_unsafe_symbols() {
        let mut graph = cloud();
        graph.transitions[0].actions = vec![Action::RequestWeaponStatus {
            target: crate::gimmicks::live_model::WeaponTarget {
                battle_object_id: 0x2000_0001,
            },
            symbol: "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR".into(),
        }];
        let artifact = render(&BTreeMap::from([("cloud".into(), graph.clone())])).unwrap();
        assert_eq!(
            artifact,
            render(&BTreeMap::from([("cloud".into(), graph)])).unwrap()
        );
        assert!(artifact.source.contains(
            "Action::RequestWeaponStatus { weapon_boid: 0x20000001, symbol: \"WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR\" }"
        ));

        let mut zero_target = cloud();
        zero_target.transitions[0].actions = vec![Action::RequestWeaponStatus {
            target: crate::gimmicks::live_model::WeaponTarget {
                battle_object_id: 0,
            },
            symbol: "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR".into(),
        }];
        let errors = render(&BTreeMap::from([("cloud".into(), zero_target)])).unwrap_err();
        assert!(
            errors.iter().any(|error| {
                error.subject.contains("transition 0")
                    && error.subject.ends_with("action 0")
                    && error
                        .reason
                        .contains("nonzero explicit weapon battle-object id")
            }),
            "{errors:?}"
        );

        let mut wrong_kind = cloud();
        wrong_kind.transitions[0].actions = vec![Action::RequestWeaponStatus {
            target: crate::gimmicks::live_model::WeaponTarget {
                battle_object_id: 0x2000_0001,
            },
            symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
        }];
        let errors = render(&BTreeMap::from([("cloud".into(), wrong_kind)])).unwrap_err();
        assert!(errors.iter().any(|error| {
            error.subject == "cloud / transition 0 / action 0"
                && error.reason.contains("allowlisted WeaponStatus")
        }));
    }

    #[test]
    fn a_valid_artifact_replaces_an_existing_regular_file() {
        let artifact = render(&BTreeMap::from([("cloud".into(), cloud())])).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("generated.rs");
        std::fs::write(&destination, "stale source").unwrap();
        write_to(&destination, &artifact).unwrap();
        assert_eq!(
            std::fs::read_to_string(destination).unwrap(),
            artifact.source
        );
    }

    fn compatible_plugin_slot(dir: &tempfile::TempDir) -> PathBuf {
        let root = dir.path().join("selected_plugin");
        let gimmicks = root.join("src/slight/gimmicks");
        std::fs::create_dir_all(&gimmicks).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"slight_replica\"\n",
        )
        .unwrap();
        std::fs::write(root.join(GENERATED_RELATIVE_PATH), GENERATED_SLOT_MARKER).unwrap();
        std::fs::write(
            root.join(GENERATED_ACMD_RELATIVE_PATH),
            super::super::acmd_adapter::GENERATED_ACMD_SLOT_MARKER,
        )
        .unwrap();
        root
    }

    #[test]
    fn explicit_compatible_plugin_export_changes_only_the_managed_slot() {
        let artifact = render(&BTreeMap::from([("cloud".into(), cloud())])).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = compatible_plugin_slot(&dir);
        let unrelated = root.join("src/unrelated.rs");
        std::fs::write(&unrelated, "keep me").unwrap();

        let exported = export_to_plugin(&root, &artifact).unwrap();
        assert!(exported.changed);
        assert_eq!(
            exported.destination,
            root.join(GENERATED_RELATIVE_PATH),
            "the plugin root chooses one fixed generated slot, not an arbitrary output path"
        );
        assert_eq!(
            std::fs::read_to_string(exported.destination).unwrap(),
            artifact.source
        );
        assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "keep me");

        let unchanged = export_to_plugin(&root, &artifact).unwrap();
        assert!(
            !unchanged.changed,
            "identical artifacts do not rewrite the slot"
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_export_refuses_a_symlinked_contract_file() {
        use std::os::unix::fs::symlink;

        let artifact = render(&BTreeMap::from([("cloud".into(), cloud())])).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = compatible_plugin_slot(&dir);
        let cargo = root.join("Cargo.toml");
        let replacement = root.join("Cargo.real.toml");
        std::fs::rename(&cargo, &replacement).unwrap();
        symlink(&replacement, &cargo).unwrap();

        let error = export_to_plugin(&root, &artifact).unwrap_err();
        assert!(error.reason.contains("non-symlink regular file"));
    }
}
