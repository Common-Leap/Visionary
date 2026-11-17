//! Closed behavior-adapter runtime for generated gimmick artifacts.
//!
//! This is deliberately an interpreter for a tiny, static data language. An
//! artifact can only read/write its own in-memory [`state`] variables and use
//! the F2.1 predicate/action vocabulary; it has no engine accessor, no work
//! ID, and no callback supplied by a project. Generated modules provide a
//! fighter predicate and plain dispatcher functions, while this module owns
//! validation and evaluation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::native_inputs::{
    NativeInput, NativeKind, NativeObservation, NativeSnapshot, ATTACK_RESULT_FLAGS_FEATURE,
};
use super::native_values;
use super::{dispatcher, state};

/// Value types shared by a generated artifact and the runtime store.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Float(f32),
    Int(i32),
    Flag(bool),
}

/// Static-artifact native source: the saved reference (`NativeInput::reference`)
/// of a source the desktop catalog admitted. Validation and the sampler resolve
/// it against the pinned exact-owner registry before any native read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NativeSource(pub &'static str);

impl NativeSource {
    pub(crate) fn to_input(self) -> NativeInput {
        NativeInput::from_reference(self.0).canonical()
    }
}

impl Value {
    fn kind(self) -> VarKind {
        match self {
            Self::Float(_) => VarKind::Float,
            Self::Int(_) => VarKind::Int,
            Self::Flag(_) => VarKind::Flag,
        }
    }

    fn stored(self) -> state::VarValue {
        match self {
            Self::Float(value) => state::VarValue::Float(value),
            Self::Int(value) => state::VarValue::Int(value),
            Self::Flag(value) => state::VarValue::Flag(value),
        }
    }
}

impl From<state::VarValue> for Value {
    fn from(value: state::VarValue) -> Self {
        match value {
            state::VarValue::Float(value) => Self::Float(value),
            state::VarValue::Int(value) => Self::Int(value),
            state::VarValue::Flag(value) => Self::Flag(value),
        }
    }
}

/// A variable kind. Timers are represented by a non-negative bounded integer.
/// They advance only when the program receives its per-frame `ChargeTick`
/// binding; reaching zero routes the program's existing `Timeout` binding
/// once. This keeps countdown policy inside the closed artifact rather than
/// making an engine timer or a project callback another source of state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    Float,
    Int,
    Flag,
    Timer,
}

impl VarKind {
    fn accepts(self, value: Value) -> bool {
        match self {
            Self::Float => matches!(value, Value::Float(_)),
            Self::Int | Self::Timer => matches!(value, Value::Int(_)),
            Self::Flag => matches!(value, Value::Flag(_)),
        }
    }
}

/// One variable declared by a generated program.
#[derive(Debug, Clone, Copy)]
pub struct VarDef {
    pub name: &'static str,
    pub kind: VarKind,
    pub initial: Value,
}

/// Numeric expression. Event fields are visible only to their exact receiver;
/// there is intentionally no function-call variant.
#[derive(Debug, Clone, Copy)]
pub enum Expr {
    Const(Value),
    PendingDamage,
    Var(&'static str),
    /// Read the remaining frame count from a declared Timer without ticking it.
    TimerRemaining(&'static str),
    EventField {
        event: &'static str,
        field: &'static str,
    },
    NativeFloat(NativeSource),
    NativeInt(NativeSource),
    Add(&'static Expr, &'static Expr),
    Sub(&'static Expr, &'static Expr),
    Mul(&'static Expr, &'static Expr),
}

#[derive(Debug, Clone, Copy)]
pub enum CmpOp {
    Eq,
    Lt,
    Lte,
    Gt,
    Gte,
}

/// Guards read authored values, declared native snapshots, or an exact
/// receiver's event payload; they never invoke engine getters.
#[derive(Debug, Clone, Copy)]
pub enum Predicate {
    Flag(&'static str),
    EventFlag {
        event: &'static str,
        field: &'static str,
    },
    NativeFlag(NativeSource),
    DamageStatusIs(&'static str, &'static str),
    Expired(&'static str),
    Compare {
        op: CmpOp,
        left: &'static Expr,
        right: &'static Expr,
    },
    Not(&'static Predicate),
    And(&'static Predicate, &'static Predicate),
    Or(&'static Predicate, &'static Predicate),
}

/// One statically-authored value evaluated when its named event is sent.
/// Numeric and flag trees remain borrowed static data; only their bounded
/// scalar results are copied into the event FIFO.
#[derive(Debug, Clone, Copy)]
pub enum EventValue {
    Constant(Value),
    Numeric(&'static Expr),
    Flag(&'static Predicate),
}

/// The full closed action set. State actions remain pure. Article actions
/// carry only an allowlisted symbolic name; weapon-status actions carry an
/// explicit nonzero object id plus an allowlisted symbolic name. Both
/// delegate to `operations`, which re-proves every live-object boundary
/// before reaching the engine.
#[derive(Debug, Clone, Copy)]
pub enum Action {
    RequestArticleStatus {
        symbol: &'static str,
        status: &'static str,
    },
    RemoveNewestArticle {
        symbol: &'static str,
    },
    Set {
        var: &'static str,
        value: &'static Expr,
    },
    SetFlag {
        var: &'static str,
        value: &'static Predicate,
    },
    Add {
        var: &'static str,
        value: &'static Expr,
    },
    Raise {
        var: &'static str,
    },
    Clear {
        var: &'static str,
    },
    Reset {
        var: &'static str,
    },
    GenerateArticle {
        symbol: &'static str,
    },
    RemoveArticle {
        symbol: &'static str,
    },
    RequestFighterStatus {
        symbol: &'static str,
    },
    RequestWeaponStatus {
        weapon_boid: u32,
        symbol: &'static str,
    },
    EmitEvent {
        name: &'static str,
    },
    /// Emit a named event with authored scalar fields. The receiver schema
    /// determines canonical order; fields are checked before they enter the
    /// bounded FIFO.
    EmitEventArguments {
        name: &'static str,
        arguments: &'static [EventArgument],
    },
    /// Emit a receiver-schema-ordered event with values evaluated at this
    /// action, using the current staged state and sender context.
    EmitEventValues {
        name: &'static str,
        arguments: &'static [EventValueArgument],
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Transition {
    pub from: i32,
    pub to: i32,
    pub guard: &'static Predicate,
    pub actions: &'static [Action],
}

/// One evaluated dispatcher hook and the transition indices it owns.
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub hook: dispatcher::GimmickHook,
    pub article: Option<&'static str>,
    pub transitions: &'static [usize],
}

#[derive(Debug, Clone, Copy)]
pub struct EventBinding {
    pub name: &'static str,
    pub transitions: &'static [usize],
    pub fields: &'static [EventField],
}

#[derive(Debug, Clone, Copy)]
pub struct EventField {
    pub name: &'static str,
    pub kind: VarKind,
}

#[derive(Debug, Clone, Copy)]
pub struct EventArgument {
    pub field: &'static str,
    pub value: Value,
}

#[derive(Debug, Clone, Copy)]
pub struct EventValueArgument {
    pub field: &'static str,
    pub value: EventValue,
}

/// A generated, compile-time behavior artifact.
pub struct Program {
    /// Stable namespace for this program's internal state. It must be unique
    /// across generated artifacts even when two fighters reuse variable names.
    pub id: &'static str,
    pub fighter: &'static str,
    /// Generated from the pinned `FIGHTER_KIND_*` constant. This must return
    /// false for unknown, non-fighter, wrong-fighter or stale sampled objects.
    /// Production matchers also prove the current engine kind, player entry,
    /// module accessor and observed stock identity through `actor_matches`.
    pub matches_fighter: fn(u32) -> bool,
    pub initial_state: i32,
    pub vars: &'static [VarDef],
    pub native_inputs: &'static [NativeSource],
    pub transitions: &'static [Transition],
    pub bindings: &'static [Binding],
    pub event_bindings: &'static [EventBinding],
}

const MAX_EVENTS_PER_STEP: usize = 64;
const MAX_EVENT_DEPTH: usize = 8;
const MAX_EVENT_FIELDS: usize = 8;
const MAX_EVENT_BINDINGS: usize = super::live_model::MAX_EVENT_BINDINGS;
const MAX_BINDINGS: usize = super::live_model::MAX_BINDINGS;
const MAX_TRANSITIONS: usize = super::live_model::MAX_TRANSITIONS;
const MAX_ACTIONS_PER_TRANSITION: usize = super::live_model::MAX_ACTIONS_PER_TRANSITION;
const MAX_EXPR_DEPTH: usize = super::live_model::MAX_EXPR_DEPTH;
const MAX_EXPR_NODES: usize = super::live_model::MAX_EXPR_NODES;
const MAX_VARIABLES: usize = super::live_model::MAX_VARIABLES;
const MAX_PROGRAM_ID_BYTES: usize = super::live_model::MAX_PROGRAM_ID_BYTES;
const MAX_FIGHTER_BYTES: usize = super::live_model::MAX_FIGHTER_BYTES;
const MAX_NAME_BYTES: usize = super::live_model::MAX_NAME_BYTES;
const MAX_SYMBOL_BYTES: usize = super::live_model::MAX_SYMBOL_BYTES;
const SUPPORTED_NATIVE_FEATURES: &[&str] = &[
    super::native_inputs::HELD_OPPONENT_FEATURE,
    super::native_inputs::MOVEMENT_FEATURE,
    super::native_inputs::ITEMS_FEATURE,
    super::native_inputs::FIGHTER_DAMAGE_FEATURE,
    super::native_inputs::FIGHTER_STOCKS_FEATURE,
    super::native_inputs::GRAB_EVENTS_FEATURE,
    super::native_inputs::ITEM_EVENTS_FEATURE,
    super::native_inputs::COMMAND_INPUT_FEATURE,
    super::native_inputs::HELD_ITEM_KIND_FEATURE,
    super::native_inputs::FIGHTER_STATUS_IS_FEATURE,
    super::native_inputs::FIGHTER_STATUS_CHANGING_FEATURE,
    super::native_inputs::ARTICLE_PRESENCE_FEATURE,
    super::native_inputs::OWNED_ARTICLES_FEATURE,
    super::native_inputs::STATUS_PHASE_FEATURE,
    ATTACK_RESULT_FLAGS_FEATURE,
];

/// Inline, owned storage for one event's scalar payload.
#[derive(Debug, Clone, Copy, PartialEq)]
struct EventPayload {
    len: u8,
    values: [Value; MAX_EVENT_FIELDS],
}

impl EventPayload {
    fn try_from_slice(values: &[Value]) -> Result<Self, String> {
        if values.len() > MAX_EVENT_FIELDS {
            return Err(format!(
                "named event payload has {} fields; maximum is {MAX_EVENT_FIELDS}",
                values.len()
            ));
        }
        for value in values {
            let valid = match value {
                Value::Float(value) => {
                    value.is_finite()
                        && f64::from(*value).abs() <= super::live_model::MAX_ABS_NUMBER
                }
                Value::Int(value) => {
                    i64::from(*value).abs() <= super::live_model::MAX_ABS_NUMBER as i64
                }
                Value::Flag(_) => true,
            };
            if !valid {
                return Err(
                    "named event payload contains a non-finite or out-of-range number".into(),
                );
            }
        }

        let mut payload = Self {
            len: values.len() as u8,
            values: [Value::Flag(false); MAX_EVENT_FIELDS],
        };
        payload.values[..values.len()].copy_from_slice(values);
        Ok(payload)
    }

    fn as_slice(&self) -> &[Value] {
        &self.values[..usize::from(self.len)]
    }
}

fn validate_event_schema(binding: &EventBinding) -> Result<(), String> {
    if binding.name.len() > 48 || binding.name.trim().is_empty() || binding.name.contains('\0') {
        return Err("named event has an invalid name".into());
    }
    if binding.fields.len() > MAX_EVENT_FIELDS {
        return Err(format!(
            "named event schema has {} fields; maximum is {MAX_EVENT_FIELDS}",
            binding.fields.len()
        ));
    }
    let mut names = BTreeSet::new();
    for field in binding.fields {
        if field.name.len() > 48
            || field.name.trim().is_empty()
            || field.name.contains('\0')
            || !names.insert(field.name)
        {
            return Err("named event schema has an invalid or duplicate field name".into());
        }
        if field.kind == VarKind::Timer {
            return Err(format!(
                "named event {:?} field {:?} cannot use Timer",
                binding.name, field.name
            ));
        }
    }
    Ok(())
}

fn validate_event_binding(program: &Program, binding: &EventBinding) -> Result<(), String> {
    validate_event_schema(binding)?;
    if binding.transitions.len() > MAX_TRANSITIONS {
        return Err(format!(
            "{}: named event transition list exceeds {MAX_TRANSITIONS}",
            program.id
        ));
    }
    let mut seen = BTreeSet::new();
    if binding.transitions.is_empty()
        || binding
            .transitions
            .iter()
            .any(|index| *index >= program.transitions.len() || !seen.insert(*index))
    {
        return Err(format!(
            "{}: named event {:?} has invalid or duplicate transitions",
            program.id, binding.name
        ));
    }
    Ok(())
}

fn resolve_event_binding<'a>(program: &'a Program, name: &str) -> Result<&'a EventBinding, String> {
    if program.transitions.len() > MAX_TRANSITIONS {
        return Err("named event transition table exceeds the limit".into());
    }
    if program.event_bindings.len() > MAX_EVENT_BINDINGS {
        return Err(format!(
            "{}: event binding table exceeds {MAX_EVENT_BINDINGS}",
            program.id
        ));
    }
    if name.len() > 48 || name.trim().is_empty() || name.contains('\0') {
        return Err("named event name is invalid".into());
    }
    let mut found = None;
    for binding in program.event_bindings {
        if binding.name == name {
            if found.is_some() {
                return Err(format!(
                    "{}: named event {name:?} has more than one receiver binding",
                    program.id
                ));
            }
            validate_event_binding(program, binding)?;
            found = Some(binding);
        }
    }
    found.ok_or_else(|| format!("{}: unbound named event {name:?}", program.id))
}

fn payload_for_binding(binding: &EventBinding, values: &[Value]) -> Result<EventPayload, String> {
    if values.len() > MAX_EVENT_FIELDS || binding.fields.len() > MAX_EVENT_FIELDS {
        return Err(format!(
            "named event payload or schema exceeds {MAX_EVENT_FIELDS} fields"
        ));
    }
    validate_event_schema(binding)?;
    if values.len() != binding.fields.len() {
        return Err(format!(
            "named event {:?} expects {} payload fields, received {}",
            binding.name,
            binding.fields.len(),
            values.len()
        ));
    }
    for (field, value) in binding.fields.iter().zip(values) {
        if !field.kind.accepts(*value) {
            return Err(format!(
                "named event {:?} field {:?} expects {:?}, received {:?}",
                binding.name,
                field.name,
                field.kind,
                value.kind()
            ));
        }
    }
    EventPayload::try_from_slice(values)
}

fn payload_for_arguments(
    binding: &EventBinding,
    arguments: &[EventArgument],
) -> Result<EventPayload, String> {
    if arguments.len() > MAX_EVENT_FIELDS || binding.fields.len() > MAX_EVENT_FIELDS {
        return Err(format!(
            "named event arguments or schema exceeds {MAX_EVENT_FIELDS} fields"
        ));
    }
    if binding.fields.is_empty() {
        return Err(
            "typed named-event arguments require a nonempty receiver schema; use the legacy send for zero-field receivers".into(),
        );
    }
    validate_event_schema(binding)?;
    if arguments.len() != binding.fields.len() {
        return Err(format!(
            "named event {:?} expects {} arguments, received {}",
            binding.name,
            binding.fields.len(),
            arguments.len()
        ));
    }
    let mut values = [Value::Flag(false); MAX_EVENT_FIELDS];
    for (index, (field, argument)) in binding.fields.iter().zip(arguments).enumerate() {
        if argument.field != field.name {
            return Err(format!(
                "named event argument descriptor at index {index} does not match canonical schema order"
            ));
        }
        if !field.kind.accepts(argument.value) {
            return Err(format!(
                "named event {:?} field {:?} expects {:?}, received {:?}",
                binding.name,
                field.name,
                field.kind,
                argument.value.kind()
            ));
        }
        values[index] = argument.value;
    }
    EventPayload::try_from_slice(&values[..arguments.len()])
}

fn validate_event_value_shape(
    binding: &EventBinding,
    arguments: &[EventValueArgument],
) -> Result<(), String> {
    if arguments.len() > MAX_EVENT_FIELDS || binding.fields.len() > MAX_EVENT_FIELDS {
        return Err(format!(
            "named event values or schema exceeds {MAX_EVENT_FIELDS} fields"
        ));
    }
    if binding.name.len() > MAX_NAME_BYTES
        || binding.name.contains('\0')
        || binding
            .fields
            .iter()
            .any(|field| field.name.len() > MAX_NAME_BYTES || field.name.contains('\0'))
        || arguments
            .iter()
            .any(|argument| argument.field.len() > MAX_NAME_BYTES || argument.field.contains('\0'))
    {
        return Err("named event value labels exceed their structural bounds".into());
    }
    validate_event_schema(binding)?;
    if binding.fields.is_empty() {
        return Err(
            "computed event values require a nonempty receiver schema; use EmitEvent for zero-field receivers"
                .into(),
        );
    }
    if arguments.len() != binding.fields.len() {
        return Err(format!(
            "named event {:?} expects {} values, received {}",
            binding.name,
            binding.fields.len(),
            arguments.len()
        ));
    }
    for (index, (field, argument)) in binding.fields.iter().zip(arguments).enumerate() {
        if argument.field != field.name {
            return Err(format!(
                "named event value descriptor at index {index} does not match canonical schema order"
            ));
        }
    }
    Ok(())
}

fn payload_for_event_values(
    program: &Program,
    values: &BTreeMap<&str, Value>,
    binding: &EventBinding,
    arguments: &[EventValueArgument],
    native_snapshot: &NativeSnapshot,
    event_context: Option<&EventContext<'_>>,
) -> Result<EventPayload, String> {
    validate_event_value_shape(binding, arguments)?;
    let mut payload = [Value::Flag(false); MAX_EVENT_FIELDS];
    for (index, (field, argument)) in binding.fields.iter().zip(arguments).enumerate() {
        let value = match argument.value {
            EventValue::Constant(value) => value,
            EventValue::Numeric(expression) => {
                expr(program, values, expression, native_snapshot, event_context)?
            }
            EventValue::Flag(predicate_tree) => Value::Flag(predicate(
                program,
                values,
                predicate_tree,
                native_snapshot,
                event_context,
            )?),
        };
        if !field.kind.accepts(value) {
            return Err(format!(
                "named event {:?} field {:?} expects {:?}, received {:?}",
                binding.name,
                field.name,
                field.kind,
                value.kind()
            ));
        }
        payload[index] = validate_value(program, value)?;
    }
    EventPayload::try_from_slice(&payload[..arguments.len()])
}

struct QueuedEvent {
    name: &'static str,
    depth: usize,
    payload: EventPayload,
}

/// Borrowed only while evaluating one dequeued receiver. Ordinary hooks and
/// later events never inherit the prior event's values.
#[derive(Clone, Copy)]
struct EventContext<'a> {
    binding: &'a EventBinding,
    payload: &'a EventPayload,
}

#[derive(Default)]
struct EventQueue {
    items: VecDeque<QueuedEvent>,
    count: usize,
}

impl EventQueue {
    fn preflight(&self, parent_depth: Option<usize>) -> Result<usize, String> {
        let depth = parent_depth.map_or(0, |depth| depth.saturating_add(1));
        if depth > MAX_EVENT_DEPTH {
            return Err(format!(
                "named event causal depth exceeds {MAX_EVENT_DEPTH}"
            ));
        }
        if self.count >= MAX_EVENTS_PER_STEP {
            return Err(format!(
                "named event count exceeds {MAX_EVENTS_PER_STEP} per step"
            ));
        }
        Ok(depth)
    }

    fn push_root(&mut self, name: &'static str, payload: EventPayload) -> Result<(), String> {
        let depth = self.preflight(None)?;
        self.push_preflighted(name, depth, payload);
        Ok(())
    }

    fn push_preflighted(&mut self, name: &'static str, depth: usize, payload: EventPayload) {
        self.count += 1;
        self.items.push_back(QueuedEvent {
            name,
            depth,
            payload,
        });
    }
}

/// A generated plain function connected to one program/hook pair. A plain
/// function keeps the dispatcher allocation-free and lets it deduplicate a
/// reconnect safely.
#[derive(Clone, Copy)]
pub struct Registration {
    pub hook: dispatcher::GimmickHook,
    pub handler: dispatcher::Handler,
    /// An exact compile-time status constant for semantic status hooks. The
    /// generated resolver keeps symbolic identity in source; no project may
    /// inject a raw status number at runtime.
    pub status: Option<fn() -> i32>,
    /// Exact transition subset for OnStatusEnter/OnStatusExit; status triggers
    /// retain legacy whole-binding behavior.
    pub transitions: Option<&'static [usize]>,
}

/// Validate and install a static program. This must be called only by code
/// compiled into the plugin; TCP/project input never reaches this boundary.
pub fn register(program: &'static Program, registrations: &[Registration]) -> Result<(), String> {
    register_with_damage(program, registrations, &[])
}

/// Typed handlers never enter the legacy object-only dispatcher. Validation is
/// all-or-none across both registries, including exact hook ownership.
#[derive(Clone, Copy)]
pub struct DamageRegistration {
    pub direction: super::damage_dispatch::Direction,
    pub handler: super::damage_dispatch::Handler,
}

pub fn register_with_damage(
    program: &'static Program,
    registrations: &[Registration],
    damage: &[DamageRegistration],
) -> Result<(), String> {
    if registrations.len().saturating_add(damage.len()) > MAX_BINDINGS {
        return Err("static registration count exceeds the hook limit".into());
    }
    if registrations.iter().any(|registration| {
        matches!(
            registration.hook,
            dispatcher::GimmickHook::DealDamage
                | dispatcher::GimmickHook::TakeDamage
                | dispatcher::GimmickHook::AttackShielded
                | dispatcher::GimmickHook::ShieldHit
        )
    }) {
        return Err("damage roots require typed contact registrations".into());
    }
    // Reuse complete program validation with metadata only. This function is
    // never registered or invoked; every installed damage function takes Event.
    fn metadata_only(_: u32) {}
    let mut metadata = registrations.to_vec();
    metadata.extend(damage.iter().map(|registration| Registration {
        hook: match registration.direction {
            super::damage_dispatch::Direction::Deal => dispatcher::GimmickHook::DealDamage,
            super::damage_dispatch::Direction::Take => dispatcher::GimmickHook::TakeDamage,
            super::damage_dispatch::Direction::AttackShielded => {
                dispatcher::GimmickHook::AttackShielded
            }
            super::damage_dispatch::Direction::ShieldHit => dispatcher::GimmickHook::ShieldHit,
        },
        handler: metadata_only,
        status: None,
        transitions: None,
    }));
    validate(program, &metadata)?;
    for registration in registrations {
        if let Some(status) = registration.status {
            dispatcher::register_status(registration.hook, status(), registration.handler);
        } else {
            dispatcher::register(registration.hook, registration.handler);
        }
    }
    for registration in damage {
        super::damage_dispatch::register(registration.direction, registration.handler);
    }
    Ok(())
}

/// Run every transition bound to `hook` for one object. Wrong-fighter and
/// unknown objects are ignored before any state is initialized.
pub fn execute(
    program: &'static Program,
    hook: dispatcher::GimmickHook,
    boid: u32,
) -> Result<(), String> {
    if matches!(
        hook,
        dispatcher::GimmickHook::DealDamage
            | dispatcher::GimmickHook::TakeDamage
            | dispatcher::GimmickHook::AttackShielded
            | dispatcher::GimmickHook::ShieldHit
    ) {
        return Err("damage hooks require an explicit pending-damage input".into());
    }
    if matches!(
        hook,
        dispatcher::GimmickHook::StatusEnter | dispatcher::GimmickHook::StatusExit
    ) {
        return Err(format!(
            "{}: {hook:?} requires an exact registration",
            program.id
        ));
    }
    execute_selected(program, hook, boid, None)
}

/// Run the transition subset associated with one exact status registration.
pub fn execute_status(
    program: &'static Program,
    hook: dispatcher::GimmickHook,
    boid: u32,
    transitions: &'static [usize],
) -> Result<(), String> {
    if !matches!(
        hook,
        dispatcher::GimmickHook::StatusEnter | dispatcher::GimmickHook::StatusExit
    ) || transitions.is_empty()
    {
        return Err(format!(
            "{}: invalid exact status transition selection",
            program.id
        ));
    }
    let Some(binding) = program.bindings.iter().find(|binding| binding.hook == hook) else {
        return Err(format!("{}: no {hook:?} binding", program.id));
    };
    let mut seen = BTreeSet::new();
    if transitions.iter().any(|index| {
        *index >= program.transitions.len()
            || !binding.transitions.contains(index)
            || !seen.insert(*index)
    }) {
        return Err(format!(
            "{}: invalid or duplicate {hook:?} transition selection",
            program.id
        ));
    }
    execute_selected(program, hook, boid, Some(transitions))
}

/// Dispatch one declared named event for this exact fighter program. The
/// receiver enters the same bounded FIFO evaluation used by hook-produced
/// events, without fabricating a hook or advancing timers. Wrong-fighter
/// objects are errors so a generated router cannot silently hide a bad route.
pub fn execute_named(program: &'static Program, boid: u32, name: &str) -> Result<(), String> {
    execute_named_with_payload(program, boid, name, &[])
}

/// Dispatch one declared event with an owned, schema-checked scalar payload.
/// Validation happens before identity reservation, native sampling, or state
/// staging so malformed roots cannot consume a dispatch ticket or mutate state.
pub fn execute_named_with_payload(
    program: &'static Program,
    boid: u32,
    name: &str,
    values: &[Value],
) -> Result<(), String> {
    if values.len() > MAX_EVENT_FIELDS {
        return Err(format!(
            "named event payload exceeds {MAX_EVENT_FIELDS} fields"
        ));
    }
    let binding = resolve_event_binding(program, name)?;
    let payload = payload_for_binding(binding, values)?;
    if !(program.matches_fighter)(boid) {
        return Err(format!(
            "{}: battle object {boid} does not match this fighter program",
            program.id
        ));
    }
    execute_program(
        program,
        None,
        boid,
        None,
        Some((binding, payload)),
        None,
        None,
    )
}

fn execute_selected(
    program: &'static Program,
    hook: dispatcher::GimmickHook,
    boid: u32,
    transition_filter: Option<&'static [usize]>,
) -> Result<(), String> {
    execute_program(
        program,
        Some(hook),
        boid,
        transition_filter,
        None,
        None,
        None,
    )
}

/// Offline interpreter fixture. Production entry points require a checked
/// contact and retain its actor/lifetime guard through execution.
#[cfg(test)]
pub fn execute_damage_input(
    program: &'static Program,
    boid: u32,
    input: super::native_inputs::DamageInput,
) -> Result<(), String> {
    let hook = match input.direction() {
        super::native_inputs::DamageDirection::Deal => dispatcher::GimmickHook::DealDamage,
        super::native_inputs::DamageDirection::Take => dispatcher::GimmickHook::TakeDamage,
        super::native_inputs::DamageDirection::AttackShielded => {
            dispatcher::GimmickHook::AttackShielded
        }
        super::native_inputs::DamageDirection::ShieldHit => dispatcher::GimmickHook::ShieldHit,
        super::native_inputs::DamageDirection::Incoming
        | super::native_inputs::DamageDirection::Reflect
        | super::native_inputs::DamageDirection::Absorb => {
            return Err("receiver contacts require the controller adapter".into())
        }
    };
    execute_program(program, Some(hook), boid, None, None, Some(input), None)
}

/// Execute a root supplied by the sealed native contact dispatcher. The exact
/// contact remains attached to sampling, actions and the authored-state commit.
pub fn execute_damage_event(
    program: &'static Program,
    event: super::damage_dispatch::Event,
) -> Result<(), String> {
    let input = super::damage_context::input(event)?;
    let hook = match input.direction() {
        super::native_inputs::DamageDirection::Deal => dispatcher::GimmickHook::DealDamage,
        super::native_inputs::DamageDirection::Take => dispatcher::GimmickHook::TakeDamage,
        super::native_inputs::DamageDirection::AttackShielded => {
            dispatcher::GimmickHook::AttackShielded
        }
        super::native_inputs::DamageDirection::ShieldHit => dispatcher::GimmickHook::ShieldHit,
        super::native_inputs::DamageDirection::Incoming
        | super::native_inputs::DamageDirection::Reflect
        | super::native_inputs::DamageDirection::Absorb => {
            return Err("receiver contacts require the controller adapter".into())
        }
    };
    execute_program(
        program,
        Some(hook),
        event.receiver().identity.object_id,
        None,
        None,
        Some(input),
        Some(event),
    )
}

fn execute_program(
    program: &'static Program,
    hook: Option<dispatcher::GimmickHook>,
    boid: u32,
    transition_filter: Option<&'static [usize]>,
    event_root: Option<(&'static EventBinding, EventPayload)>,
    damage: Option<super::native_inputs::DamageInput>,
    contact: Option<super::damage_dispatch::Event>,
) -> Result<(), String> {
    let named_event = event_root.map(|(binding, _)| binding.name);
    if super::live_behavior::suppress_compiled(boid, program.fighter) {
        return match named_event {
            Some(name) => Err(format!(
                "{} for actor {boid}: named event {name:?} was cancelled because a live override became active",
                program.id
            )),
            None => Ok(()),
        };
    }
    if !(program.matches_fighter)(boid) {
        return match named_event {
            Some(name) => Err(format!(
                "{} for actor {boid}: named event {name:?} was cancelled because the fighter identity changed",
                program.id
            )),
            None => Ok(()),
        };
    }
    let Some(identity) = super::live_behavior::Identity::from_live_fighter(boid, program.fighter)
    else {
        return match named_event {
            Some(name) => Err(format!(
                "{} for actor {boid}: named event {name:?} was cancelled because the live identity is no longer exact",
                program.id
            )),
            None => Ok(()),
        };
    };
    super::damage_context::require_current(contact, boid, identity)?;
    let Some(dispatch_ticket) =
        super::live_behavior::begin_compiled_dispatch(boid, program.fighter, identity)
    else {
        return match named_event {
            Some(name) => Err(format!(
                "{} for actor {boid}: named event {name:?} was cancelled before dispatch reservation",
                program.id
            )),
            None => Ok(()),
        };
    };
    let native_snapshot = (if contact.is_some() {
        native_values::sample_static_inputs_guarded(
            boid,
            program.fighter,
            identity,
            program.native_inputs,
            || super::damage_context::require_current(contact, boid, identity),
        )
    } else {
        native_values::sample_static_inputs(boid, program.fighter, identity, program.native_inputs)
    })
    .map_err(|error| {
        format!(
            "{} for actor {boid}: native input sample failed: {error}",
            program.id
        )
    })?;
    let native_snapshot = match damage {
        Some(input) => std::borrow::Cow::Owned(native_snapshot.with_damage(input)),
        None => native_snapshot.without_damage(),
    };
    if !super::damage_context::is_current(contact, boid, identity)
        || !super::live_behavior::compiled_dispatch_ticket_is_current(identity, &dispatch_ticket)
    {
        return Err(format!(
            "{} for actor {boid}: static dispatch became stale during native sampling",
            program.id
        ));
    }
    let state_transaction = state::begin(boid, identity.state_identity());
    // Reject an unbound hook before deriving any timer state. Generated
    // artifacts declare every callable hook, so this is a fail-closed guard
    // against a direct or malformed ChargeTick invocation advancing a timer.
    if let Some(hook) = hook {
        if !program.bindings.iter().any(|binding| binding.hook == hook) {
            return Err(format!("{}: no binding for {hook:?}", program.id));
        }
    }
    let mut values = values_for(program, &state_transaction)?;
    let state_key = key(program, "state_id");
    let mut current_state = match state_transaction.get(&state_key) {
        Some(state::VarValue::Int(value)) => value,
        Some(_) => {
            return Err(format!(
                "{}: internal state_id has the wrong type",
                program.id
            ))
        }
        None => program.initial_state,
    };
    let mut changed = false;
    let mut queued_events = EventQueue::default();
    if let Some((binding, payload)) = event_root {
        queued_events.push_root(binding.name, payload)?;
    }
    let mut engine_actions = Vec::new();
    let (timers_changed, expired) = if hook == Some(dispatcher::GimmickHook::ChargeTick) {
        decrement_timers(program, &mut values)?
    } else {
        (false, BTreeSet::new())
    };
    changed |= timers_changed;
    changed |= if let Some(transitions) = transition_filter {
        execute_transitions(
            program,
            transitions,
            boid,
            &mut current_state,
            &mut values,
            None,
            None,
            &mut queued_events,
            &mut engine_actions,
            0,
            &native_snapshot,
        )?
    } else if let Some(hook) = hook {
        execute_binding(
            program,
            hook,
            boid,
            &mut current_state,
            &mut values,
            None,
            None,
            &mut queued_events,
            &mut engine_actions,
            0,
            &native_snapshot,
        )?
    } else {
        false
    };

    // A timer only produces a synthetic timeout on the positive-to-zero edge.
    // A zero initial value is a deliberately inactive timer, and a timeout
    // action cannot retrigger forever on later frame ticks. If this tick
    // refreshed the timer, it won the race and no timeout is delivered.
    if hook == Some(dispatcher::GimmickHook::ChargeTick)
        && expired
            .iter()
            .any(|name| matches!(values.get(name), Some(Value::Int(value)) if *value <= 0))
        && program
            .bindings
            .iter()
            .any(|binding| binding.hook == dispatcher::GimmickHook::Timeout)
    {
        changed |= execute_binding(
            program,
            dispatcher::GimmickHook::Timeout,
            boid,
            &mut current_state,
            &mut values,
            Some(&expired),
            None,
            &mut queued_events,
            &mut engine_actions,
            0,
            &native_snapshot,
        )?;
    }
    let event_snapshot = native_snapshot.without_damage();
    while let Some(event) = queued_events.items.pop_front() {
        let binding = resolve_event_binding(program, event.name)?;
        // Revalidate the exact receiver schema when the event is dequeued;
        // queued payloads are owned copies and never borrow from their sender.
        payload_for_binding(binding, event.payload.as_slice())?;
        let event_context = EventContext {
            binding,
            payload: &event.payload,
        };
        changed |= execute_transitions(
            program,
            binding.transitions,
            boid,
            &mut current_state,
            &mut values,
            None,
            Some(&event_context),
            &mut queued_events,
            &mut engine_actions,
            event.depth,
            &event_snapshot,
        )?;
    }
    for action in engine_actions {
        if !super::damage_context::is_current(contact, boid, identity)
            || !super::live_behavior::compiled_dispatch_is_current(
                identity,
                &dispatch_ticket,
                &state_transaction,
            )
        {
            return Err(format!(
                "{} for actor {boid}: static dispatch was cancelled before a native action",
                program.id
            ));
        }
        let action_result = apply_engine_action(program, boid, identity, action);
        if !super::damage_context::is_current(contact, boid, identity)
            || !super::live_behavior::compiled_dispatch_is_current(
                identity,
                &dispatch_ticket,
                &state_transaction,
            )
        {
            return Err(format!(
                "{} for actor {boid}: static dispatch was cancelled during a native action",
                program.id
            ));
        }
        action_result.map_err(|error| {
            format!(
                "{} for actor {boid}: native action failed: {error}",
                program.id
            )
        })?;
    }
    if !super::damage_context::is_current(contact, boid, identity)
        || !super::live_behavior::compiled_dispatch_is_current(
            identity,
            &dispatch_ticket,
            &state_transaction,
        )
    {
        return Err(format!(
            "{} for actor {boid}: static dispatch was cancelled before state commit",
            program.id
        ));
    }
    let mut writes = BTreeMap::new();
    if changed {
        // Commit only after the entire binding evaluated. A malformed artifact
        // therefore cannot leave a half-applied transition in the store.
        writes.insert(state_key, state::VarValue::Int(current_state));
        for variable in program.vars {
            let value = values.get(variable.name).copied().ok_or_else(|| {
                format!(
                    "{}: missing internal variable {:?}",
                    program.id, variable.name
                )
            })?;
            writes.insert(key(program, variable.name), value.stored());
        }
    }
    if !super::live_behavior::commit_compiled_dispatch(
        identity,
        &dispatch_ticket,
        state_transaction,
        &writes,
        contact,
    ) {
        return Err(format!(
            "{} for actor {boid}: static dispatch was cancelled before state commit",
            program.id
        ));
    }
    Ok(())
}

/// Advance every active timer by one frame without allowing it to underflow.
/// Returns whether any stored value changed and the names which crossed from
/// positive to zero on this tick. The caller decides whether a still-expired
/// timer should route `Timeout` after normal frame work has had a chance to
/// refresh it.
fn decrement_timers(
    program: &Program,
    values: &mut BTreeMap<&'static str, Value>,
) -> Result<(bool, BTreeSet<&'static str>), String> {
    let mut changed = false;
    let mut expired = BTreeSet::new();
    for variable in program.vars {
        if variable.kind != VarKind::Timer {
            continue;
        }
        let value = values.get(variable.name).copied().ok_or_else(|| {
            format!(
                "{}: missing internal timer variable {:?}",
                program.id, variable.name
            )
        })?;
        let Value::Int(remaining) = value else {
            return Err(format!(
                "{}: internal timer {:?} has the wrong type",
                program.id, variable.name
            ));
        };
        if remaining > 0 {
            let next = remaining - 1;
            values.insert(variable.name, Value::Int(next));
            changed = true;
            if next == 0 {
                expired.insert(variable.name);
            }
        }
    }
    Ok((changed, expired))
}

/// Evaluate one binding against staged values. Keeping this separate from
/// [`execute`] lets a frame tick deliver its normal transition before safely
/// routing one internally-derived timeout, without writing half a result.
#[allow(clippy::too_many_arguments)] // Explicit borrowed staging state; no global evaluator context.
fn execute_binding(
    program: &Program,
    hook: dispatcher::GimmickHook,
    boid: u32,
    current_state: &mut i32,
    values: &mut BTreeMap<&'static str, Value>,
    timeout_edge: Option<&BTreeSet<&'static str>>,
    event_context: Option<&EventContext<'_>>,
    queued_events: &mut EventQueue,
    engine_actions: &mut Vec<PreparedEngineAction>,
    event_depth: usize,
    native_snapshot: &NativeSnapshot,
) -> Result<bool, String> {
    let binding = program
        .bindings
        .iter()
        .find(|binding| binding.hook == hook)
        .ok_or_else(|| format!("{}: no binding for {hook:?}", program.id))?;
    if binding.article.is_some_and(|symbol| {
        native_snapshot
            .article_event
            .as_ref()
            .map(|(spawned, kind)| (*spawned, kind.as_str()))
            != Some((hook == dispatcher::GimmickHook::ArticleSpawn, symbol))
    }) {
        return Ok(false);
    }
    execute_transitions(
        program,
        binding.transitions,
        boid,
        current_state,
        values,
        timeout_edge,
        event_context,
        queued_events,
        engine_actions,
        event_depth,
        native_snapshot,
    )
}

#[allow(clippy::too_many_arguments)] // Shares the binding's staged values, effects and bounded event queue.
fn execute_transitions(
    program: &Program,
    transitions: &[usize],
    boid: u32,
    current_state: &mut i32,
    values: &mut BTreeMap<&'static str, Value>,
    timeout_edge: Option<&BTreeSet<&'static str>>,
    event_context: Option<&EventContext<'_>>,
    queued_events: &mut EventQueue,
    engine_actions: &mut Vec<PreparedEngineAction>,
    event_depth: usize,
    native_snapshot: &NativeSnapshot,
) -> Result<bool, String> {
    let mut changed = false;
    for index in transitions {
        let transition = program.transitions.get(*index).ok_or_else(|| {
            format!(
                "{}: transition binding references transition {index} outside the artifact",
                program.id
            )
        })?;
        // An internally-derived timeout is only allowed to evaluate a
        // transition that explicitly names a timer which crossed to zero on
        // this frame. A separate timer at zero is inert; it cannot borrow
        // another timer's timeout event. Explicit external Timeout firings
        // pass `None` and retain their original behavior.
        let targets_timeout_edge = timeout_edge
            .is_none_or(|expired| predicate_mentions_expired_timer(transition.guard, expired));
        if targets_timeout_edge
            && *current_state == transition.from
            && predicate(
                program,
                values,
                transition.guard,
                native_snapshot,
                event_context,
            )?
        {
            for action in transition.actions {
                match action {
                    Action::EmitEvent { name } => {
                        let depth = queued_events.preflight(Some(event_depth))?;
                        let binding = resolve_event_binding(program, name)?;
                        if !binding.fields.is_empty() {
                            return Err(format!(
                                "{}: name-only event {name:?} cannot target a fieldful receiver",
                                program.id
                            ));
                        }
                        let payload = payload_for_binding(binding, &[])?;
                        queued_events.push_preflighted(name, depth, payload);
                    }
                    Action::EmitEventArguments { name, arguments } => {
                        let depth = queued_events.preflight(Some(event_depth))?;
                        if arguments.len() > MAX_EVENT_FIELDS {
                            return Err(format!(
                                "named event arguments exceed {MAX_EVENT_FIELDS} fields"
                            ));
                        }
                        let binding = resolve_event_binding(program, name)?;
                        let payload = payload_for_arguments(binding, arguments)?;
                        queued_events.push_preflighted(name, depth, payload);
                    }
                    Action::EmitEventValues { name, arguments } => {
                        let depth = queued_events.preflight(Some(event_depth))?;
                        let binding = resolve_event_binding(program, name)?;
                        let payload = payload_for_event_values(
                            program,
                            values,
                            binding,
                            arguments,
                            native_snapshot,
                            event_context,
                        )?;
                        queued_events.push_preflighted(name, depth, payload);
                    }
                    Action::GenerateArticle { .. }
                    | Action::RemoveArticle { .. }
                    | Action::RemoveNewestArticle { .. }
                    | Action::RequestArticleStatus { .. }
                    | Action::RequestFighterStatus { .. }
                    | Action::RequestWeaponStatus { .. } => {
                        engine_actions.push(PreparedEngineAction::Legacy(*action))
                    }
                    _ => apply(
                        program,
                        values,
                        boid,
                        *action,
                        native_snapshot,
                        event_context,
                    )?,
                }
            }
            *current_state = transition.to;
            changed = true;
        }
    }
    Ok(changed)
}

/// Whether a guard explicitly depends on any timer that crossed to zero this
/// frame. This is a structural gate in addition to [`predicate`]'s normal
/// value evaluation: it prevents `Expired("other_timer")` from becoming true
/// merely because a different timer supplied the synthetic timeout event.
fn predicate_mentions_expired_timer(guard: &Predicate, expired: &BTreeSet<&'static str>) -> bool {
    match guard {
        Predicate::Expired(name) => expired.contains(name),
        Predicate::Flag(_)
        | Predicate::EventFlag { .. }
        | Predicate::NativeFlag(_)
        | Predicate::DamageStatusIs(_, _)
        | Predicate::Compare { .. } => false,
        Predicate::Not(inner) => predicate_mentions_expired_timer(inner, expired),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            predicate_mentions_expired_timer(left, expired)
                || predicate_mentions_expired_timer(right, expired)
        }
    }
}

/// Read one generated variable for diagnostics and meter integration. This is
/// read-only and returns the declared initial when the object has not taken a
/// transition yet.
pub fn read(program: &Program, boid: u32, name: &str) -> Result<Value, String> {
    let variable = find_var(program, name)?;
    match state::get(boid, &key(program, name)) {
        Some(value) => Ok(Value::from(value)),
        None => Ok(variable.initial),
    }
}

/// Read static ACMD state only for the current fighter object generation.
pub fn read_for_actor(program: &Program, boid: u32, name: &str) -> Option<Value> {
    let identity = super::live_behavior::Identity::from_live_fighter(boid, program.fighter)?;
    let variable = find_var(program, name).ok()?;
    Some(
        state::get_for_identity(boid, identity.state_identity(), &key(program, name))
            .map(Value::from)
            .unwrap_or(variable.initial),
    )
}

fn values_for(
    program: &Program,
    state: &state::Transaction,
) -> Result<BTreeMap<&'static str, Value>, String> {
    let mut values = BTreeMap::new();
    for variable in program.vars {
        let value = state
            .get(&key(program, variable.name))
            .map(Value::from)
            .unwrap_or(variable.initial);
        values.insert(variable.name, validate_value(program, value)?);
    }
    Ok(values)
}

fn key(program: &Program, name: &str) -> String {
    format!("gimmick/{}/{}", program.id, name)
}

fn find_var<'a>(program: &'a Program, name: &str) -> Result<&'a VarDef, String> {
    program
        .vars
        .iter()
        .find(|variable| variable.name == name)
        .ok_or_else(|| format!("{}: unknown variable {name:?}", program.id))
}

fn event_field_value(
    program: &Program,
    event_context: Option<&EventContext<'_>>,
    event: &str,
    field_name: &str,
    expected: impl FnOnce(VarKind) -> bool,
) -> Result<Value, String> {
    if event.len() > 48 || field_name.len() > 48 {
        return Err(format!(
            "{}: event field reference exceeds the name limit",
            program.id
        ));
    }
    let context = event_context.ok_or_else(|| {
        format!(
            "{}: event field reads require a named-event receiver context",
            program.id
        )
    })?;
    if context.binding.name != event {
        return Err(format!(
            "{}: event field reference does not match the active receiver",
            program.id
        ));
    }
    let (index, field) = context
        .binding
        .fields
        .iter()
        .enumerate()
        .find(|(_, field)| field.name == field_name)
        .ok_or_else(|| format!("{}: unknown field in active named event", program.id))?;
    if !expected(field.kind) {
        return Err(format!(
            "{}: event field has the wrong scalar kind for this read",
            program.id
        ));
    }
    let value = context
        .payload
        .as_slice()
        .get(index)
        .copied()
        .ok_or_else(|| {
            format!(
                "{}: active event payload is missing a declared field",
                program.id
            )
        })?;
    if !field.kind.accepts(value) {
        return Err(format!(
            "{}: active event payload field has the wrong scalar kind",
            program.id
        ));
    }
    validate_value(program, value)
}

fn expr(
    program: &Program,
    values: &BTreeMap<&str, Value>,
    expr: &Expr,
    native_snapshot: &NativeSnapshot,
    event_context: Option<&EventContext<'_>>,
) -> Result<Value, String> {
    let value = match expr {
        Expr::PendingDamage => native_snapshot.pending_damage().map(Value::Float),
        Expr::Const(value) => Ok(*value),
        Expr::Var(name) => values
            .get(name)
            .copied()
            .ok_or_else(|| format!("{}: expression reads unknown variable {name:?}", program.id)),
        Expr::TimerRemaining(name) => timer_remaining(program, values, name),
        Expr::EventField { event, field } => {
            event_field_value(program, event_context, event, field, |kind| {
                matches!(kind, VarKind::Int | VarKind::Float)
            })
        }
        Expr::NativeFloat(source) if source.to_input().kind() == Some(NativeKind::Float) => {
            validate_native_source_bounds(*source).map_err(|error| error.to_string())?;
            native_snapshot
                .read(&source.to_input())
                .map(Value::Float)
                .map_err(|error| format!("{}: {error}", program.id))
        }
        Expr::NativeInt(source) if source.to_input().kind() == Some(NativeKind::Int) => {
            validate_native_source_bounds(*source).map_err(|error| error.to_string())?;
            native_snapshot
                .read_int(&source.to_input())
                .map(Value::Int)
                .map_err(|error| format!("{}: {error}", program.id))
        }
        Expr::NativeFloat(_) => Err(format!(
            "{}: native float expression uses a non-float source",
            program.id
        )),
        Expr::NativeInt(_) => Err(format!(
            "{}: native integer expression uses a non-integer source",
            program.id
        )),
        Expr::Add(left, right) => numeric(
            program,
            values,
            (left, right),
            native_snapshot,
            event_context,
            |a, b| a + b,
            |a, b| a.checked_add(b),
        ),
        Expr::Sub(left, right) => numeric(
            program,
            values,
            (left, right),
            native_snapshot,
            event_context,
            |a, b| a - b,
            |a, b| a.checked_sub(b),
        ),
        Expr::Mul(left, right) => numeric(
            program,
            values,
            (left, right),
            native_snapshot,
            event_context,
            |a, b| a * b,
            |a, b| a.checked_mul(b),
        ),
    }?;
    validate_value(program, value)
}

fn timer_remaining(
    program: &Program,
    values: &BTreeMap<&str, Value>,
    name: &'static str,
) -> Result<Value, String> {
    let variable = find_var(program, name)?;
    if variable.kind != VarKind::Timer {
        return Err(format!(
            "{}: timer remaining requires a Timer variable, got {:?}",
            program.id, variable.kind
        ));
    }
    match values.get(name).copied() {
        Some(Value::Int(remaining))
            if (0..=super::live_model::MAX_TIMER_FRAMES).contains(&remaining) =>
        {
            Ok(Value::Int(remaining))
        }
        Some(Value::Int(_)) => Err(format!(
            "{}: timer {name:?} is outside the bounded countdown range",
            program.id
        )),
        Some(_) => Err(format!(
            "{}: timer {name:?} is not stored as an integer",
            program.id
        )),
        None => Err(format!(
            "{}: expression reads unknown timer {name:?}",
            program.id
        )),
    }
}

fn validate_value(program: &Program, value: Value) -> Result<Value, String> {
    match value {
        Value::Float(value)
            if value.is_finite() && f64::from(value).abs() <= super::live_model::MAX_ABS_NUMBER =>
        {
            Ok(Value::Float(value))
        }
        Value::Float(_) => Err(format!(
            "{}: float value is non-finite or outside the live range",
            program.id
        )),
        Value::Int(value) if i64::from(value).abs() <= super::live_model::MAX_ABS_NUMBER as i64 => {
            Ok(Value::Int(value))
        }
        Value::Int(_) => Err(format!(
            "{}: integer value is outside the live range",
            program.id
        )),
        Value::Flag(value) => Ok(Value::Flag(value)),
    }
}

fn numeric(
    program: &Program,
    values: &BTreeMap<&str, Value>,
    operands: (&Expr, &Expr),
    native_snapshot: &NativeSnapshot,
    event_context: Option<&EventContext<'_>>,
    float: impl FnOnce(f32, f32) -> f32,
    int: impl FnOnce(i32, i32) -> Option<i32>,
) -> Result<Value, String> {
    match (
        expr(program, values, operands.0, native_snapshot, event_context)?,
        expr(program, values, operands.1, native_snapshot, event_context)?,
    ) {
        (Value::Float(left), Value::Float(right)) => Ok(Value::Float(float(left, right))),
        (Value::Int(left), Value::Int(right)) => int(left, right)
            .map(Value::Int)
            .ok_or_else(|| format!("{}: integer arithmetic overflow", program.id)),
        (left, right) => Err(format!(
            "{}: numeric operands disagree ({:?} and {:?})",
            program.id,
            left.kind(),
            right.kind()
        )),
    }
}

fn predicate(
    program: &Program,
    values: &BTreeMap<&str, Value>,
    guard: &Predicate,
    native_snapshot: &NativeSnapshot,
    event_context: Option<&EventContext<'_>>,
) -> Result<bool, String> {
    match guard {
        Predicate::Flag(name) => match values.get(name).copied() {
            Some(Value::Flag(value)) => Ok(value),
            Some(value) => Err(format!(
                "{}: {name:?} is {:?}, not a flag",
                program.id,
                value.kind()
            )),
            None => Err(format!(
                "{}: predicate reads unknown variable {name:?}",
                program.id
            )),
        },
        Predicate::EventFlag { event, field } => {
            match event_field_value(program, event_context, event, field, |kind| {
                kind == VarKind::Flag
            })? {
                Value::Flag(value) => Ok(value),
                _ => Err(format!(
                    "{}: event flag field has the wrong payload kind",
                    program.id
                )),
            }
        }
        Predicate::DamageStatusIs(fighter, symbol) => native_snapshot
            .damage_status_is(&NativeInput::FighterStatusIs {
                fighter: (*fighter).into(),
                symbol: (*symbol).into(),
            })
            .map_err(|error| format!("{}: {error}", program.id)),
        Predicate::NativeFlag(source) if source.to_input().kind() == Some(NativeKind::Flag) => {
            validate_native_source_bounds(*source).map_err(|error| error.to_string())?;
            native_snapshot
                .read_flag(&source.to_input())
                .map_err(|error| format!("{}: {error}", program.id))
        }
        Predicate::NativeFlag(_) => Err(format!(
            "{}: native flag predicate uses a non-flag source",
            program.id
        )),
        Predicate::Expired(name) => match values.get(name).copied() {
            Some(Value::Int(value)) if find_var(program, name)?.kind == VarKind::Timer => {
                Ok(value <= 0)
            }
            Some(value) => Err(format!(
                "{}: {name:?} cannot expire as {:?}",
                program.id,
                value.kind()
            )),
            None => Err(format!(
                "{}: predicate reads unknown variable {name:?}",
                program.id
            )),
        },
        Predicate::Compare { op, left, right } => {
            match (
                expr(program, values, left, native_snapshot, event_context)?,
                expr(program, values, right, native_snapshot, event_context)?,
            ) {
                (Value::Float(left), Value::Float(right)) => Ok(match op {
                    CmpOp::Eq => left == right,
                    CmpOp::Lt => left < right,
                    CmpOp::Lte => left <= right,
                    CmpOp::Gt => left > right,
                    CmpOp::Gte => left >= right,
                }),
                (Value::Int(left), Value::Int(right)) => Ok(match op {
                    CmpOp::Eq => left == right,
                    CmpOp::Lt => left < right,
                    CmpOp::Lte => left <= right,
                    CmpOp::Gt => left > right,
                    CmpOp::Gte => left >= right,
                }),
                (left, right) => Err(format!(
                    "{}: comparison values disagree ({:?} and {:?})",
                    program.id,
                    left.kind(),
                    right.kind()
                )),
            }
        }
        Predicate::Not(inner) => Ok(!predicate(
            program,
            values,
            inner,
            native_snapshot,
            event_context,
        )?),
        Predicate::And(left, right) => {
            Ok(
                predicate(program, values, left, native_snapshot, event_context)?
                    && predicate(program, values, right, native_snapshot, event_context)?,
            )
        }
        Predicate::Or(left, right) => {
            Ok(
                predicate(program, values, left, native_snapshot, event_context)?
                    || predicate(program, values, right, native_snapshot, event_context)?,
            )
        }
    }
}

fn apply(
    program: &Program,
    values: &mut BTreeMap<&'static str, Value>,
    boid: u32,
    action: Action,
    native_snapshot: &NativeSnapshot,
    event_context: Option<&EventContext<'_>>,
) -> Result<(), String> {
    match action {
        Action::Set { var, value } => {
            let target = find_var(program, var)?;
            let value = validate_value(
                program,
                expr(program, values, value, native_snapshot, event_context)?,
            )?;
            if !target.kind.accepts(value) {
                return Err(format!("{}: set {var:?} has wrong value type", program.id));
            }
            if target.kind == VarKind::Timer && matches!(value, Value::Int(value) if value < 0) {
                return Err(format!(
                    "{}: timer {var:?} cannot start below zero",
                    program.id
                ));
            }
            values.insert(var, value);
        }
        Action::SetFlag { var, value } => {
            let target = find_var(program, var)?;
            if target.kind != VarKind::Flag {
                return Err(format!(
                    "{}: computed flag target {var:?} is not a flag",
                    program.id
                ));
            }
            let next = predicate(program, values, value, native_snapshot, event_context)?;
            values.insert(var, Value::Flag(next));
        }
        Action::Add { var, value } => {
            let target = find_var(program, var)?;
            if !matches!(target.kind, VarKind::Float | VarKind::Int) {
                return Err(format!("{}: add {var:?} is not numeric", program.id));
            }
            let left = values
                .get(var)
                .copied()
                .ok_or_else(|| format!("{}: missing {var:?}", program.id))?;
            let right = expr(program, values, value, native_snapshot, event_context)?;
            let result = match (left, right) {
                (Value::Float(left), Value::Float(right)) => Value::Float(left + right),
                (Value::Int(left), Value::Int(right)) => Value::Int(
                    left.checked_add(right)
                        .ok_or_else(|| format!("{}: integer add overflow", program.id))?,
                ),
                _ => return Err(format!("{}: add {var:?} has wrong value type", program.id)),
            };
            let result = validate_value(program, result)?;
            values.insert(var, result);
        }
        Action::Raise { var } | Action::Clear { var } => {
            let target = find_var(program, var)?;
            if target.kind != VarKind::Flag {
                return Err(format!(
                    "{}: flag action targets non-flag {var:?}",
                    program.id
                ));
            }
            values.insert(var, Value::Flag(matches!(action, Action::Raise { .. })));
        }
        Action::Reset { var } => {
            let target = find_var(program, var)?;
            values.insert(var, validate_value(program, target.initial)?);
        }
        Action::RemoveNewestArticle { symbol } => {
            super::operations::remove_newest_article_from_behavior(program.fighter, boid, symbol)?;
        }
        Action::RequestArticleStatus { symbol, status } => {
            super::operations::request_article_status_from_behavior(
                program.fighter,
                boid,
                symbol,
                status,
            )?;
        }
        Action::GenerateArticle { symbol } => {
            super::operations::generate_article_from_behavior(program.fighter, boid, symbol)?;
        }
        Action::RemoveArticle { symbol } => {
            super::operations::remove_article_from_behavior(program.fighter, boid, symbol)?;
        }
        Action::RequestFighterStatus { symbol } => {
            super::operations::request_fighter_status_from_behavior(program.fighter, boid, symbol)?;
        }
        Action::RequestWeaponStatus {
            weapon_boid,
            symbol,
        } => {
            super::operations::request_weapon_status_from_behavior(
                program.fighter,
                boid,
                weapon_boid,
                symbol,
            )?;
        }
        Action::EmitEvent { .. }
        | Action::EmitEventArguments { .. }
        | Action::EmitEventValues { .. } => {}
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum PreparedEngineAction {
    Legacy(Action),
}

fn apply_engine_action(
    program: &Program,
    boid: u32,
    _identity: super::live_behavior::Identity,
    action: PreparedEngineAction,
) -> Result<(), String> {
    let PreparedEngineAction::Legacy(action) = action;
    match action {
        Action::RemoveNewestArticle { symbol } => {
            super::operations::remove_newest_article_from_behavior(program.fighter, boid, symbol)
        }
        Action::RequestArticleStatus { symbol, status } => {
            super::operations::request_article_status_from_behavior(
                program.fighter,
                boid,
                symbol,
                status,
            )
        }
        Action::GenerateArticle { symbol } => {
            super::operations::generate_article_from_behavior(program.fighter, boid, symbol)
        }
        Action::RemoveArticle { symbol } => {
            super::operations::remove_article_from_behavior(program.fighter, boid, symbol)
        }
        Action::RequestFighterStatus { symbol } => {
            super::operations::request_fighter_status_from_behavior(program.fighter, boid, symbol)
        }
        Action::RequestWeaponStatus {
            weapon_boid,
            symbol,
        } => super::operations::request_weapon_status_from_behavior(
            program.fighter,
            boid,
            weapon_boid,
            symbol,
        ),
        _ => Ok(()),
    }
}

enum TreeVisit<'a> {
    Expr(&'a Expr, usize),
    Predicate(&'a Predicate, usize),
    ExitExpr(usize),
    ExitPredicate(usize),
}

fn validate_native_source_bounds(source: NativeSource) -> Result<(), &'static str> {
    if source.0.len() > NativeInput::MAX_REFERENCE_BYTES || !source.to_input().is_supported() {
        return Err("native source reference is invalid");
    }
    Ok(())
}

/// Bound every recursive evaluator/source/reference walk before any of them
/// starts. Iterative traversal detects cycles in malformed static references
/// without relying on the recursive collectors to reach a limit first.
fn preflight_evaluation_trees(program: &Program) -> Result<(), String> {
    if program.transitions.len() > MAX_TRANSITIONS
        || program.event_bindings.len() > MAX_EVENT_BINDINGS
    {
        return Err(format!(
            "{}: event preflight tables exceed their structural bounds",
            program.id
        ));
    }
    // Bound event descriptors before any tree traversal. Constant-only dynamic
    // arguments consume no expression nodes, so the shared tree budget alone
    // cannot cap their count.
    for transition in program.transitions {
        if transition.actions.len() > MAX_ACTIONS_PER_TRANSITION {
            return Err(format!(
                "{}: transition action count exceeds {MAX_ACTIONS_PER_TRANSITION}",
                program.id
            ));
        }
        for action in transition.actions {
            match action {
                Action::EmitEvent { name } => {
                    if name.len() > MAX_NAME_BYTES || name.contains('\0') {
                        return Err(format!(
                            "{}: named event label exceeds its bound",
                            program.id
                        ));
                    }
                }
                Action::EmitEventArguments { name, arguments } => {
                    if name.len() > MAX_NAME_BYTES || name.contains('\0') {
                        return Err(format!(
                            "{}: named event label exceeds its bound",
                            program.id
                        ));
                    }
                    if arguments.len() > MAX_EVENT_FIELDS {
                        return Err(format!(
                            "{}: named event arguments exceed {MAX_EVENT_FIELDS} fields",
                            program.id
                        ));
                    }
                    if arguments.iter().any(|argument| {
                        argument.field.len() > MAX_NAME_BYTES || argument.field.contains('\0')
                    }) {
                        return Err(format!(
                            "{}: named event field label exceeds its bound",
                            program.id
                        ));
                    }
                }
                Action::EmitEventValues { name, arguments } => {
                    if name.len() > MAX_NAME_BYTES || name.contains('\0') {
                        return Err(format!(
                            "{}: named event label exceeds its bound",
                            program.id
                        ));
                    }
                    if arguments.len() > MAX_EVENT_FIELDS {
                        return Err(format!(
                            "{}: named event arguments exceed {MAX_EVENT_FIELDS} fields",
                            program.id
                        ));
                    }
                    if arguments.iter().any(|argument| {
                        argument.field.len() > MAX_NAME_BYTES || argument.field.contains('\0')
                    }) {
                        return Err(format!(
                            "{}: named event field label exceeds its bound",
                            program.id
                        ));
                    }
                }
                Action::Set { .. }
                | Action::SetFlag { .. }
                | Action::Add { .. }
                | Action::Raise { .. }
                | Action::Clear { .. }
                | Action::Reset { .. }
                | Action::GenerateArticle { .. }
                | Action::RemoveArticle { .. }
                | Action::RemoveNewestArticle { .. }
                | Action::RequestArticleStatus { .. }
                | Action::RequestFighterStatus { .. }
                | Action::RequestWeaponStatus { .. } => {}
            }
        }
    }
    // Receiver names and field labels can be interpolated by schema errors;
    // check their byte bounds before any later validation formats them.
    for binding in program.event_bindings {
        if binding.name.len() > MAX_NAME_BYTES
            || binding.name.contains('\0')
            || binding.fields.len() > MAX_EVENT_FIELDS
        {
            return Err(format!(
                "{}: named event schema exceeds its structural bounds",
                program.id
            ));
        }
        if binding
            .fields
            .iter()
            .any(|field| field.name.len() > MAX_NAME_BYTES || field.name.contains('\0'))
        {
            return Err(format!(
                "{}: named event field label exceeds its bound",
                program.id
            ));
        }
    }

    let mut remaining = MAX_EXPR_NODES;
    for transition in program.transitions {
        preflight_tree(
            program,
            TreeVisit::Predicate(transition.guard, 1),
            &mut remaining,
        )?;
        for action in transition.actions {
            if let Action::Set { value, .. } | Action::Add { value, .. } = action {
                preflight_tree(program, TreeVisit::Expr(value, 1), &mut remaining)?;
            }
            if let Action::SetFlag { value, .. } = action {
                preflight_tree(program, TreeVisit::Predicate(value, 1), &mut remaining)?;
            }
            if let Action::EmitEventValues { arguments, .. } = action {
                for argument in *arguments {
                    match argument.value {
                        EventValue::Numeric(expression) => {
                            preflight_tree(
                                program,
                                TreeVisit::Expr(expression, 1),
                                &mut remaining,
                            )?;
                        }
                        EventValue::Flag(predicate) => {
                            preflight_tree(
                                program,
                                TreeVisit::Predicate(predicate, 1),
                                &mut remaining,
                            )?;
                        }
                        EventValue::Constant(_) => {}
                    }
                }
            }
        }
    }
    Ok(())
}

fn preflight_tree(
    program: &Program,
    root: TreeVisit<'_>,
    remaining: &mut usize,
) -> Result<(), String> {
    let mut stack = vec![root];
    let mut active_expressions = BTreeSet::new();
    let mut active_predicates = BTreeSet::new();
    while let Some(visit) = stack.pop() {
        match visit {
            TreeVisit::ExitExpr(address) => {
                active_expressions.remove(&address);
            }
            TreeVisit::ExitPredicate(address) => {
                active_predicates.remove(&address);
            }
            TreeVisit::Expr(expression, depth) => {
                if depth > MAX_EXPR_DEPTH || *remaining == 0 {
                    return Err(format!(
                        "{}: expression depth or node budget exceeded",
                        program.id
                    ));
                }
                let address = expression as *const Expr as usize;
                if !active_expressions.insert(address) {
                    return Err(format!(
                        "{}: cyclic static expression reference",
                        program.id
                    ));
                }
                *remaining -= 1;
                stack.push(TreeVisit::ExitExpr(address));
                match expression {
                    Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
                        stack.push(TreeVisit::Expr(right, depth + 1));
                        stack.push(TreeVisit::Expr(left, depth + 1));
                    }
                    Expr::NativeFloat(source) | Expr::NativeInt(source) => {
                        validate_native_source_bounds(*source)
                            .map_err(|error| format!("{}: {error}", program.id))?;
                    }
                    Expr::PendingDamage
                    | Expr::Const(_)
                    | Expr::Var(_)
                    | Expr::TimerRemaining(_)
                    | Expr::EventField { .. } => {}
                }
            }
            TreeVisit::Predicate(predicate, depth) => {
                if depth > MAX_EXPR_DEPTH || *remaining == 0 {
                    return Err(format!(
                        "{}: predicate depth or node budget exceeded",
                        program.id
                    ));
                }
                let address = predicate as *const Predicate as usize;
                if !active_predicates.insert(address) {
                    return Err(format!("{}: cyclic static predicate reference", program.id));
                }
                *remaining -= 1;
                stack.push(TreeVisit::ExitPredicate(address));
                match predicate {
                    Predicate::Compare { left, right, .. } => {
                        stack.push(TreeVisit::Expr(right, depth + 1));
                        stack.push(TreeVisit::Expr(left, depth + 1));
                    }
                    Predicate::Not(inner) => {
                        stack.push(TreeVisit::Predicate(inner, depth + 1));
                    }
                    Predicate::And(left, right) | Predicate::Or(left, right) => {
                        stack.push(TreeVisit::Predicate(right, depth + 1));
                        stack.push(TreeVisit::Predicate(left, depth + 1));
                    }
                    Predicate::NativeFlag(source) => {
                        validate_native_source_bounds(*source)
                            .map_err(|error| format!("{}: {error}", program.id))?;
                    }
                    Predicate::DamageStatusIs(fighter, symbol) => {
                        if !(NativeInput::FighterStatusIs {
                            fighter: (*fighter).into(),
                            symbol: (*symbol).into(),
                        })
                        .is_supported()
                        {
                            return Err(format!(
                                "{}: native source reference is invalid",
                                program.id
                            ));
                        }
                    }
                    Predicate::Flag(_) | Predicate::EventFlag { .. } | Predicate::Expired(_) => {}
                }
            }
        }
    }
    Ok(())
}

fn event_field_kind(
    owner: Option<&EventBinding>,
    event: &str,
    field_name: &str,
) -> Result<VarKind, &'static str> {
    if event.len() > 48 || field_name.len() > 48 {
        return Err("event field reference exceeds the name limit");
    }
    let binding = owner.ok_or("event field read has no named-event receiver")?;
    if binding.name != event {
        return Err("event field reference does not name its owning receiver");
    }
    binding
        .fields
        .iter()
        .find(|field| field.name == field_name)
        .map(|field| field.kind)
        .ok_or("event field reference is not declared")
}

/// Check types structurally rather than evaluating guards with placeholder
/// values. Every branch is visited, including branches hidden by And/Or
/// short-circuiting at runtime.
fn validate_program_types(program: &Program) -> Result<(), String> {
    let mut errors = Vec::new();
    for (index, transition) in program.transitions.iter().enumerate() {
        let owner = program
            .event_bindings
            .iter()
            .find(|binding| binding.transitions.contains(&index));
        let owner = ReadScope {
            event: owner,
            damage: pending_damage_allowed(program, index),
        };
        validate_predicate_types(program, transition.guard, owner, &mut errors);
        for action in transition.actions {
            match action {
                Action::Set { var, value } => {
                    let value_kind = validate_expr_types(program, value, owner, &mut errors);
                    match program.vars.iter().find(|candidate| candidate.name == *var) {
                        Some(target)
                            if value_kind == Some(target.kind)
                                || (target.kind == VarKind::Timer
                                    && value_kind == Some(VarKind::Int)) => {}
                        Some(_) => errors.push("set expression type does not match target"),
                        None => errors.push("set action has an unknown target variable"),
                    }
                }
                Action::SetFlag { var, value } => {
                    validate_predicate_types(program, value, owner, &mut errors);
                    if !program
                        .vars
                        .iter()
                        .any(|candidate| candidate.name == *var && candidate.kind == VarKind::Flag)
                    {
                        errors.push("computed flag assignment requires a declared flag target");
                    }
                }
                Action::Add { var, value } => {
                    let value_kind = validate_expr_types(program, value, owner, &mut errors);
                    match program.vars.iter().find(|candidate| candidate.name == *var) {
                        Some(target)
                            if matches!(target.kind, VarKind::Float | VarKind::Int)
                                && value_kind == Some(target.kind) => {}
                        Some(_) => errors.push("add requires matching numeric types"),
                        None => errors.push("add action has an unknown target variable"),
                    }
                }
                Action::Raise { var } | Action::Clear { var } => {
                    if !program
                        .vars
                        .iter()
                        .any(|candidate| candidate.name == *var && candidate.kind == VarKind::Flag)
                    {
                        errors.push("flag action requires a declared flag target");
                    }
                }
                Action::Reset { var } => {
                    if !program.vars.iter().any(|candidate| candidate.name == *var) {
                        errors.push("reset action has an unknown target variable");
                    }
                }
                Action::EmitEventValues { name, arguments } => {
                    let receiver = program
                        .event_bindings
                        .iter()
                        .find(|binding| binding.name == *name);
                    let Some(receiver) = receiver else {
                        errors.push("dynamic event targets an unknown receiver");
                        continue;
                    };
                    if validate_event_value_shape(receiver, arguments).is_err() {
                        errors.push("dynamic event values do not match canonical receiver schema");
                        continue;
                    }
                    for (field, argument) in receiver.fields.iter().zip(*arguments) {
                        match argument.value {
                            EventValue::Constant(value) => {
                                if validate_value(program, value).is_err()
                                    || !field.kind.accepts(value)
                                {
                                    errors.push(
                                        "dynamic event constant has the wrong exact field kind",
                                    );
                                }
                            }
                            EventValue::Numeric(expression) => {
                                let kind =
                                    validate_expr_types(program, expression, owner, &mut errors);
                                if !matches!(field.kind, VarKind::Float | VarKind::Int)
                                    || kind != Some(field.kind)
                                {
                                    errors.push("dynamic numeric event value has the wrong exact field kind");
                                }
                            }
                            EventValue::Flag(predicate_tree) => {
                                validate_predicate_types(
                                    program,
                                    predicate_tree,
                                    owner,
                                    &mut errors,
                                );
                                if field.kind != VarKind::Flag {
                                    errors.push("dynamic flag event value requires a flag field");
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    errors
        .first()
        .map(|error| Err(format!("{}: {error}", program.id)))
        .unwrap_or(Ok(()))
}

#[derive(Clone, Copy, Default)]
struct ReadScope<'a> {
    event: Option<&'a EventBinding>,
    damage: bool,
}

fn pending_damage_allowed(program: &Program, index: usize) -> bool {
    let owners: Vec<_> = program
        .bindings
        .iter()
        .filter(|binding| binding.transitions.contains(&index))
        .map(|binding| binding.hook)
        .collect();
    !owners.is_empty()
        && owners.iter().all(|hook| {
            matches!(
                hook,
                dispatcher::GimmickHook::DealDamage
                    | dispatcher::GimmickHook::TakeDamage
                    | dispatcher::GimmickHook::AttackShielded
                    | dispatcher::GimmickHook::ShieldHit
            )
        })
        && !program
            .event_bindings
            .iter()
            .any(|binding| binding.transitions.contains(&index))
}

fn validate_expr_types(
    program: &Program,
    expression: &Expr,
    owner: ReadScope<'_>,
    errors: &mut Vec<&'static str>,
) -> Option<VarKind> {
    match expression {
        Expr::PendingDamage => {
            if !owner.damage {
                errors.push(
                    "pending damage requires an exclusively damage/shield contact root transition",
                );
            }
            Some(VarKind::Float)
        }
        Expr::Const(value) => {
            if validate_value(program, *value).is_err() {
                errors.push("constant is non-finite or outside the bounded range");
            }
            Some(value.kind())
        }
        Expr::Var(name) => {
            if name.len() > 48 {
                errors.push("expression variable reference exceeds the name limit");
                None
            } else {
                program
                    .vars
                    .iter()
                    .find(|candidate| candidate.name == *name)
                    .map(|variable| variable.kind)
                    .or_else(|| {
                        errors.push("expression references an unknown variable");
                        None
                    })
            }
        }
        Expr::TimerRemaining(name) => {
            if name.is_empty() || name.len() > 48 || name.contains('\0') {
                errors.push("timer variable reference is empty or exceeds the name limit");
                None
            } else {
                match program
                    .vars
                    .iter()
                    .find(|variable| variable.name == *name)
                    .map(|variable| variable.kind)
                {
                    Some(VarKind::Timer) => Some(VarKind::Int),
                    Some(_) => {
                        errors.push("timer remaining requires a declared Timer variable");
                        None
                    }
                    None => {
                        errors.push("timer remaining references an unknown variable");
                        None
                    }
                }
            }
        }
        Expr::EventField { event, field } => match event_field_kind(owner.event, event, field) {
            Ok(kind) if matches!(kind, VarKind::Int | VarKind::Float) => Some(kind),
            Ok(_) => {
                errors.push("numeric event expression requires an integer or float field");
                None
            }
            Err(error) => {
                errors.push(error);
                None
            }
        },
        Expr::NativeFloat(source) => {
            if validate_native_source_bounds(*source).is_err() {
                errors.push("native source reference is invalid");
            } else if source.to_input().kind() != Some(NativeKind::Float) {
                errors.push("native float expression has the wrong source kind");
            }
            Some(VarKind::Float)
        }
        Expr::NativeInt(source) => {
            if validate_native_source_bounds(*source).is_err() {
                errors.push("native source reference is invalid");
            } else if source.to_input().kind() != Some(NativeKind::Int) {
                errors.push("native integer expression has the wrong source kind");
            }
            Some(VarKind::Int)
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            let left = validate_expr_types(program, left, owner, errors);
            let right = validate_expr_types(program, right, owner, errors);
            match (left, right) {
                (Some(VarKind::Int), Some(VarKind::Int)) => Some(VarKind::Int),
                (Some(VarKind::Float), Some(VarKind::Float)) => Some(VarKind::Float),
                (Some(_), Some(_)) => {
                    errors.push("numeric operands have incompatible types");
                    None
                }
                _ => None,
            }
        }
    }
}

fn validate_predicate_types(
    program: &Program,
    predicate: &Predicate,
    owner: ReadScope<'_>,
    errors: &mut Vec<&'static str>,
) {
    match predicate {
        Predicate::Flag(name) => {
            if name.len() > 48
                || !program
                    .vars
                    .iter()
                    .any(|variable| variable.name == *name && variable.kind == VarKind::Flag)
            {
                errors.push("flag predicate requires a declared flag variable");
            }
        }
        Predicate::EventFlag { event, field } => {
            match event_field_kind(owner.event, event, field) {
                Ok(VarKind::Flag) => {}
                Ok(_) => errors.push("event flag predicate requires a flag field"),
                Err(error) => errors.push(error),
            }
        }
        Predicate::NativeFlag(source) => {
            if validate_native_source_bounds(*source).is_err() {
                errors.push("native source reference is invalid");
            } else if source.to_input().kind() != Some(NativeKind::Flag) {
                errors.push("native flag predicate has the wrong source kind");
            }
        }
        Predicate::DamageStatusIs(fighter, symbol) => {
            if !(NativeInput::FighterStatusIs {
                fighter: (*fighter).into(),
                symbol: (*symbol).into(),
            })
            .valid_for(program.fighter)
            {
                errors.push(
                    "contact status requires an exact registered receiver fighter-status selector",
                );
            }
            if !owner.damage {
                errors.push(
                    "contact status requires an exclusively damage/shield contact root transition",
                );
            }
        }
        Predicate::Expired(name) => {
            if name.len() > 48
                || !program
                    .vars
                    .iter()
                    .any(|variable| variable.name == *name && variable.kind == VarKind::Timer)
            {
                errors.push("expired predicate requires a declared timer variable");
            }
        }
        Predicate::Compare { left, right, .. } => {
            let left = validate_expr_types(program, left, owner, errors);
            let right = validate_expr_types(program, right, owner, errors);
            if left != right || matches!(left, Some(VarKind::Flag | VarKind::Timer)) {
                errors.push("comparison values must have matching numeric types");
            }
        }
        Predicate::Not(inner) => validate_predicate_types(program, inner, owner, errors),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            validate_predicate_types(program, left, owner, errors);
            validate_predicate_types(program, right, owner, errors);
        }
    }
}

fn validate(program: &Program, registrations: &[Registration]) -> Result<(), String> {
    if program.id.is_empty()
        || program.id.len() > MAX_PROGRAM_ID_BYTES
        || program.id.contains('\0')
        || program.fighter.is_empty()
        || program.fighter.len() > MAX_FIGHTER_BYTES
        || program.fighter.contains('\0')
    {
        return Err("behavior artifact has an invalid id or fighter label".to_string());
    }
    if program.transitions.is_empty()
        || (program.bindings.is_empty() && program.event_bindings.is_empty())
    {
        return Err(format!(
            "{}: artifact must declare transitions and hook or named event bindings",
            program.id
        ));
    }
    if program.transitions.len() > MAX_TRANSITIONS {
        return Err(format!(
            "{}: too many transitions; maximum is {MAX_TRANSITIONS}",
            program.id
        ));
    }
    if program.event_bindings.len() > MAX_EVENT_BINDINGS {
        return Err(format!("{}: too many named event bindings", program.id));
    }
    if program.bindings.len() > MAX_BINDINGS || registrations.len() > MAX_BINDINGS {
        return Err(format!(
            "{}: hook binding or registration count exceeds {MAX_BINDINGS}",
            program.id
        ));
    }
    if program.native_inputs.len() > native_values::MAX_WORK_SAMPLES {
        return Err(format!(
            "{}: too many native input declarations",
            program.id
        ));
    }
    if program.vars.len() > MAX_VARIABLES {
        return Err(format!(
            "{}: too many variables; maximum is {MAX_VARIABLES}",
            program.id
        ));
    }
    for binding in program.bindings {
        if let Some(symbol) = binding.article {
            if !matches!(
                binding.hook,
                dispatcher::GimmickHook::ArticleSpawn | dispatcher::GimmickHook::ArticleRemoved
            ) {
                return Err("article filter needs a lifecycle hook".into());
            }
            super::operations::validate_behavior_article(program.fighter, symbol)?;
        }
        if binding.transitions.len() > MAX_TRANSITIONS {
            return Err(format!(
                "{}: hook transition list exceeds {MAX_TRANSITIONS}",
                program.id
            ));
        }
    }
    for binding in program.event_bindings {
        if binding.transitions.len() > MAX_TRANSITIONS {
            return Err(format!(
                "{}: named event transition list exceeds {MAX_TRANSITIONS}",
                program.id
            ));
        }
    }
    for registration in registrations {
        if registration
            .transitions
            .is_some_and(|transitions| transitions.len() > MAX_TRANSITIONS)
        {
            return Err(format!(
                "{}: registration transition list exceeds {MAX_TRANSITIONS}",
                program.id
            ));
        }
    }
    for variable in program.vars {
        if variable.name.len() > MAX_NAME_BYTES || variable.name.contains('\0') {
            return Err(format!(
                "{}: variable name is invalid or exceeds the supported label bound",
                program.id
            ));
        }
    }
    for source in program.native_inputs {
        validate_native_source_bounds(*source)
            .map_err(|error| format!("{}: {error}", program.id))?;
    }
    preflight_evaluation_trees(program)?;
    let mut declared_native = BTreeSet::new();
    for source in program.native_inputs {
        let input = source.to_input();
        if input
            .required_feature()
            .is_some_and(|feature| !SUPPORTED_NATIVE_FEATURES.contains(&feature))
        {
            return Err(format!(
                "{}: native input requires an unsupported feature",
                program.id
            ));
        }
        if !input.valid_for(program.fighter) || !declared_native.insert(input) {
            return Err(format!(
                "{}: native input declaration is foreign or duplicated",
                program.id
            ));
        }
    }
    let mut used_native = BTreeSet::new();
    for transition in program.transitions {
        collect_predicate_sources(transition.guard, &mut used_native)
            .map_err(|error| format!("{}: {error}", program.id))?;
        for action in transition.actions {
            if let Action::Set { value, .. } | Action::Add { value, .. } = action {
                collect_expr_sources(value, &mut used_native)
                    .map_err(|error| format!("{}: {error}", program.id))?;
            }
            if let Action::SetFlag { value, .. } = action {
                collect_predicate_sources(value, &mut used_native)
                    .map_err(|error| format!("{}: {error}", program.id))?;
            }
            if let Action::EmitEventValues { arguments, .. } = action {
                for argument in *arguments {
                    match argument.value {
                        EventValue::Numeric(expression) => {
                            collect_expr_sources(expression, &mut used_native)
                                .map_err(|error| format!("{}: {error}", program.id))?;
                        }
                        EventValue::Flag(predicate_tree) => {
                            collect_predicate_sources(predicate_tree, &mut used_native)
                                .map_err(|error| format!("{}: {error}", program.id))?;
                        }
                        EventValue::Constant(_) => {}
                    }
                }
            }
        }
    }
    if declared_native != used_native {
        return Err(format!(
            "{}: native input declarations do not exactly match expression references",
            program.id
        ));
    }
    let mut names = BTreeSet::new();
    for variable in program.vars {
        if variable.name.is_empty()
            || !names.insert(variable.name)
            || !variable.kind.accepts(variable.initial)
            || validate_value(program, variable.initial).is_err()
            || matches!(variable, VarDef { kind: VarKind::Timer, initial: Value::Int(value), .. } if *value < 0)
        {
            return Err(format!(
                "{}: invalid variable {:?}",
                program.id, variable.name
            ));
        }
    }
    let mut bound = BTreeSet::new();
    for binding in program.bindings {
        if binding.transitions.is_empty() {
            return Err(format!(
                "{}: {:?} has no transitions",
                program.id, binding.hook
            ));
        }
        for index in binding.transitions {
            if *index >= program.transitions.len() || !bound.insert(*index) {
                return Err(format!(
                    "{}: invalid or duplicate transition {index}",
                    program.id
                ));
            }
        }
    }
    let mut event_names = BTreeSet::new();
    let mut event_transition_owners = BTreeMap::new();
    for binding in program.event_bindings {
        if binding.name.is_empty() || binding.name.len() > 48 || binding.name.contains('\0') {
            return Err(format!("{}: invalid named event binding name", program.id));
        }
        validate_event_binding(program, binding)?;
        if !event_names.insert(binding.name) || binding.transitions.is_empty() {
            return Err(format!(
                "{}: duplicate or empty named event binding",
                program.id
            ));
        }
        for index in binding.transitions {
            if *index >= program.transitions.len() || !bound.insert(*index) {
                return Err(format!(
                    "{}: invalid or duplicate event transition {index}",
                    program.id
                ));
            }
            event_transition_owners.insert(*index, binding);
        }
    }
    if bound.len() != program.transitions.len() {
        return Err(format!(
            "{}: one or more transitions have no binding",
            program.id
        ));
    }
    // Validate every receiver-field leaf structurally, without relying on
    // predicate short-circuiting or a synthetic payload value.
    for (index, transition) in program.transitions.iter().enumerate() {
        let event_owner = event_transition_owners.get(&index).copied();
        validate_predicate_event_fields(program, transition.guard, event_owner)?;
        for action in transition.actions {
            match action {
                Action::Set { value, .. } | Action::Add { value, .. } => {
                    validate_expr_event_fields(program, value, event_owner)?;
                }
                Action::EmitEventValues { arguments, .. } => {
                    for argument in *arguments {
                        match argument.value {
                            EventValue::Numeric(expression) => {
                                validate_expr_event_fields(program, expression, event_owner)?;
                            }
                            EventValue::Flag(predicate_tree) => {
                                validate_predicate_event_fields(
                                    program,
                                    predicate_tree,
                                    event_owner,
                                )?;
                            }
                            EventValue::Constant(_) => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
    validate_program_types(program)?;
    let registered: BTreeSet<_> = registrations
        .iter()
        .map(|registration| registration.hook)
        .collect();
    let expected: BTreeSet<_> = program
        .bindings
        .iter()
        .map(|binding| binding.hook)
        .collect();
    if registered != expected || expected.len() != program.bindings.len() {
        return Err(format!(
            "{}: generated registration hooks must match bindings exactly once",
            program.id
        ));
    }
    for binding in program.bindings {
        let count = registrations
            .iter()
            .filter(|registration| registration.hook == binding.hook)
            .count();
        let valid_count = if matches!(
            binding.hook,
            dispatcher::GimmickHook::StatusEnter | dispatcher::GimmickHook::StatusExit
        ) {
            count > 0
        } else {
            count == 1
        };
        if !valid_count {
            return Err(format!(
                "{}: generated registration count does not match {:?} binding",
                program.id, binding.hook
            ));
        }
    }
    let mut selected_status_transitions = BTreeSet::new();
    for registration in registrations {
        match (
            registration.hook,
            registration.status,
            registration.transitions,
        ) {
            (
                dispatcher::GimmickHook::StatusEnter | dispatcher::GimmickHook::StatusExit,
                Some(_),
                Some(transitions),
            ) if !transitions.is_empty() => {
                let binding = program
                    .bindings
                    .iter()
                    .find(|binding| binding.hook == registration.hook)
                    .expect("registration hook checked above");
                if transitions.iter().any(|index| {
                    *index >= program.transitions.len() || !binding.transitions.contains(index)
                }) {
                    return Err(format!(
                        "{}: {:?} transition selection is outside its binding",
                        program.id, registration.hook,
                    ));
                }
                if transitions
                    .iter()
                    .any(|index| !selected_status_transitions.insert(*index))
                {
                    return Err(format!(
                        "{}: {:?} transition is selected more than once",
                        program.id, registration.hook
                    ));
                }
            }
            (dispatcher::GimmickHook::StatusEnter | dispatcher::GimmickHook::StatusExit, _, _) => {
                return Err(format!(
                    "{}: {:?} requires an exact status resolver and transition selection",
                    program.id, registration.hook
                ));
            }
            (_, _, Some(_)) => {
                return Err(format!(
                    "{}: only StatusEnter/StatusExit can carry transition selection",
                    program.id
                ));
            }
            _ => {}
        }
        match (registration.hook, registration.status) {
            (
                dispatcher::GimmickHook::CloudLimitGroundedNeutralSpend
                | dispatcher::GimmickHook::CloudLimitIdleExpiry,
                None,
            ) if program.fighter != "cloud" => {
                return Err(format!(
                    "{}: Cloud Limit resource observation belongs only to cloud",
                    program.id
                ));
            }
            (
                dispatcher::GimmickHook::StatusEnter | dispatcher::GimmickHook::StatusExit,
                Some(_),
            ) => {}
            (dispatcher::GimmickHook::Spend, Some(_)) => {}
            (dispatcher::GimmickHook::Spend, None) => {
                return Err(format!(
                    "{}: Spend requires one exact status resolver",
                    program.id
                ));
            }
            (_, Some(_)) => {
                return Err(format!(
                    "{}: {:?} cannot carry a status resolver",
                    program.id, registration.hook
                ));
            }
            (_, None) => {}
        }
    }
    for binding in program.bindings.iter().filter(|binding| {
        matches!(
            binding.hook,
            dispatcher::GimmickHook::StatusEnter | dispatcher::GimmickHook::StatusExit
        )
    }) {
        if binding
            .transitions
            .iter()
            .any(|index| !selected_status_transitions.contains(index))
        {
            return Err(format!(
                "{}: {:?} transition has no exact status registration",
                program.id, binding.hook
            ));
        }
    }
    // Evaluate every expression/action against declared defaults to prove the
    // data vocabulary and types before a game object can reach it.
    let defaults: BTreeMap<_, _> = program
        .vars
        .iter()
        .map(|variable| (variable.name, variable.initial))
        .collect();
    let validation_snapshot = if program.native_inputs.is_empty() {
        NativeSnapshot::empty()
    } else {
        let observations = program
            .native_inputs
            .iter()
            .map(|source| match source.to_input().kind() {
                Some(NativeKind::Float) => NativeObservation::Float(0.0),
                Some(NativeKind::Int) => NativeObservation::Int(0),
                Some(NativeKind::Flag) => NativeObservation::Flag(false),
                None => unreachable!("native source validated above"),
            })
            .collect::<Vec<_>>();
        NativeSnapshot::from_observations(
            &program
                .native_inputs
                .iter()
                .copied()
                .map(|source| source.to_input())
                .collect::<Vec<_>>(),
            &observations,
        )?
    };
    for (index, transition) in program.transitions.iter().enumerate() {
        let validation_snapshot = if pending_damage_allowed(program, index) {
            let input = super::native_inputs::DamageInput::new(
                super::native_inputs::DamageDirection::Deal,
                1.0,
            )?;
            let input = if let Some(kind) = super::registry::FIGHTER_KINDS
                .iter()
                .find(|entry| entry.fighter == program.fighter)
                .and_then(|entry| i32::try_from(entry.value).ok())
            {
                input.with_receiver_status(kind, Some(0))?
            } else {
                input
            };
            std::borrow::Cow::Owned(validation_snapshot.with_damage(input))
        } else {
            validation_snapshot.without_damage()
        };
        let event_owner = event_transition_owners.get(&index).copied();
        let validation_payload = event_owner
            .map(validation_payload_for_binding)
            .transpose()?;
        let event_context = match (event_owner, validation_payload.as_ref()) {
            (Some(binding), Some(payload)) => Some(EventContext { binding, payload }),
            _ => None,
        };
        predicate(
            program,
            &defaults,
            transition.guard,
            &validation_snapshot,
            event_context.as_ref(),
        )?;
        let mut values = defaults.clone();
        for action in transition.actions {
            validate_action(
                program,
                &mut values,
                *action,
                &validation_snapshot,
                event_context.as_ref(),
            )?;
            match action {
                Action::EmitEvent { name } => {
                    let binding = resolve_event_binding(program, name)?;
                    if !binding.fields.is_empty() {
                        return Err(format!(
                            "{}: name-only event {name:?} cannot target a fieldful receiver",
                            program.id
                        ));
                    }
                }
                Action::EmitEventArguments { name, arguments } => {
                    if arguments.len() > MAX_EVENT_FIELDS {
                        return Err(format!(
                            "named event arguments exceed {MAX_EVENT_FIELDS} fields"
                        ));
                    }
                    let binding = resolve_event_binding(program, name)?;
                    payload_for_arguments(binding, arguments)?;
                }
                Action::EmitEventValues { name, arguments } => {
                    let binding = resolve_event_binding(program, name)?;
                    payload_for_event_values(
                        program,
                        &values,
                        binding,
                        arguments,
                        &validation_snapshot,
                        event_context.as_ref(),
                    )?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

// Validate each source's kind while walking the complete tree. Evaluating a
// synthetic guard is insufficient: short-circuiting can hide an invalid leaf.
fn collect_expr_sources(expr: &Expr, sources: &mut BTreeSet<NativeInput>) -> Result<(), String> {
    match expr {
        Expr::NativeFloat(source) if source.to_input().kind() != Some(NativeKind::Float) => {
            return Err(format!(
                "native Float input {source:?} has the wrong source kind"
            ));
        }
        Expr::NativeInt(source) if source.to_input().kind() != Some(NativeKind::Int) => {
            return Err(format!(
                "native Integer input {source:?} has the wrong source kind"
            ));
        }
        Expr::NativeFloat(source) | Expr::NativeInt(source) => {
            sources.insert(source.to_input());
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            collect_expr_sources(left, sources)?;
            collect_expr_sources(right, sources)?;
        }
        Expr::PendingDamage
        | Expr::Const(_)
        | Expr::Var(_)
        | Expr::TimerRemaining(_)
        | Expr::EventField { .. } => {}
    }
    Ok(())
}

fn collect_predicate_sources(
    predicate: &Predicate,
    sources: &mut BTreeSet<NativeInput>,
) -> Result<(), String> {
    match predicate {
        Predicate::Compare { left, right, .. } => {
            collect_expr_sources(left, sources)?;
            collect_expr_sources(right, sources)?;
        }
        Predicate::NativeFlag(source) => {
            if source.to_input().kind() != Some(NativeKind::Flag) {
                return Err(format!(
                    "native Flag input {source:?} has the wrong source kind"
                ));
            }
            sources.insert(source.to_input());
        }
        Predicate::Not(inner) => collect_predicate_sources(inner, sources)?,
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            collect_predicate_sources(left, sources)?;
            collect_predicate_sources(right, sources)?;
        }
        Predicate::Flag(_)
        | Predicate::EventFlag { .. }
        | Predicate::Expired(_)
        | Predicate::DamageStatusIs(_, _) => {}
    }
    Ok(())
}

fn validate_event_field_reference(
    program: &Program,
    owner: Option<&EventBinding>,
    event: &str,
    field_name: &str,
    expected: impl FnOnce(VarKind) -> bool,
) -> Result<(), String> {
    if event.len() > 48 || field_name.len() > 48 {
        return Err(format!(
            "{}: event field reference exceeds the name limit",
            program.id
        ));
    }
    let binding = owner.ok_or_else(|| {
        format!(
            "{}: event field reads are only valid in named-event receiver transitions",
            program.id
        )
    })?;
    if binding.name != event {
        return Err(format!(
            "{}: event field reference does not name its owning receiver",
            program.id
        ));
    }
    let field = binding
        .fields
        .iter()
        .find(|field| field.name == field_name)
        .ok_or_else(|| format!("{}: event field reference is not declared", program.id))?;
    if !expected(field.kind) {
        return Err(format!(
            "{}: event field reference has the wrong scalar kind",
            program.id
        ));
    }
    Ok(())
}

fn validate_expr_event_fields(
    program: &Program,
    expression: &Expr,
    owner: Option<&EventBinding>,
) -> Result<(), String> {
    match expression {
        Expr::EventField { event, field } => {
            validate_event_field_reference(program, owner, event, field, |kind| {
                matches!(kind, VarKind::Int | VarKind::Float)
            })
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            validate_expr_event_fields(program, left, owner)?;
            validate_expr_event_fields(program, right, owner)
        }
        Expr::PendingDamage
        | Expr::Const(_)
        | Expr::Var(_)
        | Expr::TimerRemaining(_)
        | Expr::NativeFloat(_)
        | Expr::NativeInt(_) => Ok(()),
    }
}

fn validate_predicate_event_fields(
    program: &Program,
    predicate: &Predicate,
    owner: Option<&EventBinding>,
) -> Result<(), String> {
    match predicate {
        Predicate::EventFlag { event, field } => {
            validate_event_field_reference(program, owner, event, field, |kind| {
                kind == VarKind::Flag
            })
        }
        Predicate::Compare { left, right, .. } => {
            validate_expr_event_fields(program, left, owner)?;
            validate_expr_event_fields(program, right, owner)
        }
        Predicate::Not(inner) => validate_predicate_event_fields(program, inner, owner),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            // Intentionally visit both branches even if runtime evaluation
            // short-circuits the left side.
            validate_predicate_event_fields(program, left, owner)?;
            validate_predicate_event_fields(program, right, owner)
        }
        Predicate::Flag(_)
        | Predicate::NativeFlag(_)
        | Predicate::Expired(_)
        | Predicate::DamageStatusIs(_, _) => Ok(()),
    }
}

fn validation_payload_for_binding(binding: &EventBinding) -> Result<EventPayload, String> {
    if binding.fields.len() > MAX_EVENT_FIELDS {
        return Err(format!(
            "named event schema exceeds {MAX_EVENT_FIELDS} fields"
        ));
    }
    validate_event_schema(binding)?;
    let mut values = [Value::Flag(false); MAX_EVENT_FIELDS];
    for (index, field) in binding.fields.iter().enumerate() {
        values[index] = match field.kind {
            VarKind::Float => Value::Float(0.0),
            VarKind::Int => Value::Int(0),
            VarKind::Flag => Value::Flag(false),
            VarKind::Timer => return Err("Timer event fields are unsupported".into()),
        };
    }
    EventPayload::try_from_slice(&values[..binding.fields.len()])
}

/// Validate an action at registration without executing its engine operation.
/// A generated module is static code, but registration still treats its
/// symbolic names and weapon target as untrusted artifact data and rejects a
/// mismatched or unallowlisted engine action before any dispatcher handler is
/// armed.
fn validate_action(
    program: &Program,
    values: &mut BTreeMap<&'static str, Value>,
    action: Action,
    snapshot: &NativeSnapshot,
    event_context: Option<&EventContext<'_>>,
) -> Result<(), String> {
    match action {
        Action::GenerateArticle { symbol }
        | Action::RemoveArticle { symbol }
        | Action::RemoveNewestArticle { symbol } => {
            super::operations::validate_behavior_article(program.fighter, symbol)
        }
        Action::RequestArticleStatus { symbol, status } => {
            super::operations::validate_behavior_article_status(program.fighter, symbol, status)
        }
        Action::RequestFighterStatus { symbol } => {
            super::operations::validate_behavior_fighter_status(program.fighter, symbol)
        }
        Action::RequestWeaponStatus {
            weapon_boid,
            symbol,
        } => {
            super::operations::validate_behavior_weapon_status(program.fighter, weapon_boid, symbol)
        }
        Action::EmitEventArguments { .. } | Action::EmitEventValues { .. } => Ok(()),
        _ => apply(program, values, 0, action, snapshot, event_context),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slight::gimmicks::TEST_LOCK;
    use std::sync::atomic::{AtomicBool, Ordering};

    static MATCH: AtomicBool = AtomicBool::new(true);
    fn matches(_: u32) -> bool {
        MATCH.load(Ordering::SeqCst)
    }

    static ONE: Expr = Expr::Const(Value::Int(1));
    static TRUE: Predicate = Predicate::Flag("armed");
    static ACTIONS: [Action; 2] = [
        Action::Add {
            var: "count",
            value: &ONE,
        },
        Action::Clear { var: "armed" },
    ];
    static TRANSITIONS: [Transition; 1] = [Transition {
        from: 0,
        to: 1,
        guard: &TRUE,
        actions: &ACTIONS,
    }];
    static BINDINGS: [Binding; 1] = [Binding {
        hook: dispatcher::GimmickHook::ChargeTick,
        article: None,
        transitions: &[0],
    }];
    static VARS: [VarDef; 2] = [
        VarDef {
            name: "count",
            kind: VarKind::Int,
            initial: Value::Int(0),
        },
        VarDef {
            name: "armed",
            kind: VarKind::Flag,
            initial: Value::Flag(true),
        },
    ];
    static PROGRAM: Program = Program {
        id: "test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &[],
        transitions: &TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &[],
    };
    static EMIT_FIRST: [Action; 1] = [Action::EmitEvent { name: "first" }];
    static RECEIVE_FIRST: [Action; 2] = [
        Action::Add {
            var: "count",
            value: &ONE,
        },
        Action::EmitEvent { name: "second" },
    ];
    static RECEIVE_SECOND: [Action; 1] = [Action::Add {
        var: "count",
        value: &ONE,
    }];
    static EVENT_TRANSITIONS: [Transition; 3] = [
        Transition {
            from: 0,
            to: 0,
            guard: &TRUE,
            actions: &EMIT_FIRST,
        },
        Transition {
            from: 0,
            to: 0,
            guard: &TRUE,
            actions: &RECEIVE_FIRST,
        },
        Transition {
            from: 0,
            to: 1,
            guard: &TRUE,
            actions: &RECEIVE_SECOND,
        },
    ];
    static FIRST_EVENT_TRANSITIONS: [usize; 1] = [1];
    static SECOND_EVENT_TRANSITIONS: [usize; 1] = [2];
    static EVENT_BINDINGS: [EventBinding; 2] = [
        EventBinding {
            name: "first",
            transitions: &FIRST_EVENT_TRANSITIONS,
            fields: &[],
        },
        EventBinding {
            name: "second",
            transitions: &SECOND_EVENT_TRANSITIONS,
            fields: &[],
        },
    ];
    static EVENT_PROGRAM: Program = Program {
        id: "named_event_test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &[],
        transitions: &EVENT_TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &EVENT_BINDINGS,
    };
    static EVENT_ALWAYS: Predicate = Predicate::Compare {
        op: CmpOp::Eq,
        left: &Expr::Const(Value::Int(1)),
        right: &Expr::Const(Value::Int(1)),
    };
    static EVENT_ADD_ONE: Expr = Expr::Const(Value::Int(1));
    static EVENT_ADD_HUNDRED: Expr = Expr::Const(Value::Int(100));
    static EVENT_EXTERNAL_ROOT_ACTIONS: [Action; 2] = [
        Action::Add {
            var: "count",
            value: &EVENT_ADD_ONE,
        },
        Action::EmitEvent { name: "nested" },
    ];
    static EVENT_NESTED_ACTIONS: [Action; 1] = [Action::Add {
        var: "count",
        value: &EVENT_ADD_ONE,
    }];
    static EVENT_HOOK_ACTIONS: [Action; 2] = [
        Action::Add {
            var: "count",
            value: &EVENT_ADD_HUNDRED,
        },
        Action::EmitEvent { name: "root" },
    ];
    static EVENT_EXTERNAL_ONLY_ACTIONS: [Action; 1] = [Action::Add {
        var: "count",
        value: &EVENT_ADD_HUNDRED,
    }];
    static EVENT_EXTERNAL_TRANSITIONS: [Transition; 4] = [
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &EVENT_EXTERNAL_ROOT_ACTIONS,
        },
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &EVENT_NESTED_ACTIONS,
        },
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &EVENT_HOOK_ACTIONS,
        },
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &EVENT_EXTERNAL_ONLY_ACTIONS,
        },
    ];
    static EVENT_TIMER_VARS: [VarDef; 3] = [
        VarDef {
            name: "count",
            kind: VarKind::Int,
            initial: Value::Int(0),
        },
        VarDef {
            name: "armed",
            kind: VarKind::Flag,
            initial: Value::Flag(true),
        },
        VarDef {
            name: "timer",
            kind: VarKind::Timer,
            initial: Value::Int(2),
        },
    ];
    static EVENT_TIMER_BINDINGS: [Binding; 1] = [Binding {
        hook: dispatcher::GimmickHook::ChargeTick,
        article: None,
        transitions: &[2],
    }];
    static EVENT_EXTERNAL_BINDINGS: [EventBinding; 3] = [
        EventBinding {
            name: "root",
            transitions: &[0],
            fields: &[],
        },
        EventBinding {
            name: "nested",
            transitions: &[1],
            fields: &[],
        },
        EventBinding {
            name: "acmd_only",
            transitions: &[3],
            fields: &[],
        },
    ];
    static EVENT_TIMER_PROGRAM: Program = Program {
        id: "named_event_no_timer_tick_test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &EVENT_TIMER_VARS,
        native_inputs: &[],
        transitions: &EVENT_EXTERNAL_TRANSITIONS,
        bindings: &EVENT_TIMER_BINDINGS,
        event_bindings: &EVENT_EXTERNAL_BINDINGS,
    };
    fn event_timer_handler(_: u32) {}
    static EVENT_TIMER_REGISTRATIONS: [Registration; 1] = [Registration {
        hook: dispatcher::GimmickHook::ChargeTick,
        handler: event_timer_handler,
        status: None,
        transitions: None,
    }];

    static TYPED_FIELDS: [EventField; 3] = [
        EventField {
            name: "ready",
            kind: VarKind::Flag,
        },
        EventField {
            name: "count",
            kind: VarKind::Int,
        },
        EventField {
            name: "ratio",
            kind: VarKind::Float,
        },
    ];
    static TYPED_ARGUMENTS: [EventArgument; 3] = [
        EventArgument {
            field: "ready",
            value: Value::Flag(true),
        },
        EventArgument {
            field: "count",
            value: Value::Int(4),
        },
        EventArgument {
            field: "ratio",
            value: Value::Float(0.5),
        },
    ];
    static TYPED_BAD_ORDER_ARGUMENTS: [EventArgument; 3] = [
        EventArgument {
            field: "count",
            value: Value::Int(4),
        },
        EventArgument {
            field: "ready",
            value: Value::Flag(true),
        },
        EventArgument {
            field: "ratio",
            value: Value::Float(0.5),
        },
    ];
    static TYPED_ROOT_ACTIONS: [Action; 2] = [
        Action::Add {
            var: "count",
            value: &EVENT_ADD_ONE,
        },
        Action::EmitEventArguments {
            name: "typed",
            arguments: &TYPED_ARGUMENTS,
        },
    ];
    static TYPED_BAD_ROOT_ACTIONS: [Action; 2] = [
        Action::Add {
            var: "count",
            value: &EVENT_ADD_ONE,
        },
        Action::EmitEventArguments {
            name: "typed",
            arguments: &TYPED_BAD_ORDER_ARGUMENTS,
        },
    ];
    static TYPED_LEGACY_ROOT_ACTIONS: [Action; 1] = [Action::EmitEvent { name: "typed" }];
    static TYPED_RECEIVE_ACTIONS: [Action; 1] = [Action::Add {
        var: "count",
        value: &EVENT_ADD_HUNDRED,
    }];
    static TYPED_TRANSITIONS: [Transition; 3] = [
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &[],
        },
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &TYPED_ROOT_ACTIONS,
        },
        Transition {
            from: 0,
            to: 1,
            guard: &EVENT_ALWAYS,
            actions: &TYPED_RECEIVE_ACTIONS,
        },
    ];
    static TYPED_BAD_TRANSITIONS: [Transition; 3] = [
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &[],
        },
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &TYPED_BAD_ROOT_ACTIONS,
        },
        Transition {
            from: 0,
            to: 1,
            guard: &EVENT_ALWAYS,
            actions: &TYPED_RECEIVE_ACTIONS,
        },
    ];
    static TYPED_LEGACY_TRANSITIONS: [Transition; 3] = [
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &[],
        },
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &TYPED_LEGACY_ROOT_ACTIONS,
        },
        Transition {
            from: 0,
            to: 1,
            guard: &EVENT_ALWAYS,
            actions: &TYPED_RECEIVE_ACTIONS,
        },
    ];
    static TYPED_EVENT_BINDINGS: [EventBinding; 2] = [
        EventBinding {
            name: "root",
            transitions: &[1],
            fields: &[],
        },
        EventBinding {
            name: "typed",
            transitions: &[2],
            fields: &TYPED_FIELDS,
        },
    ];
    static TYPED_EVENT_PROGRAM: Program = Program {
        id: "typed_event_test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &[],
        transitions: &TYPED_TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &TYPED_EVENT_BINDINGS,
    };
    static TYPED_BAD_EVENT_PROGRAM: Program = Program {
        id: "typed_event_bad_order_test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &[],
        transitions: &TYPED_BAD_TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &TYPED_EVENT_BINDINGS,
    };
    static TYPED_LEGACY_EVENT_PROGRAM: Program = Program {
        id: "typed_event_legacy_send_test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &[],
        transitions: &TYPED_LEGACY_TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &TYPED_EVENT_BINDINGS,
    };

    static BAD_EVENT_ACTIONS: [Action; 1] = [Action::Set {
        var: "count",
        value: &Expr::Const(Value::Flag(true)),
    }];
    static BAD_EVENT_TRANSITIONS: [Transition; 1] = [Transition {
        from: 0,
        to: 1,
        guard: &EVENT_ALWAYS,
        actions: &BAD_EVENT_ACTIONS,
    }];
    static BAD_EVENT_BINDINGS: [EventBinding; 1] = [EventBinding {
        name: "bad",
        transitions: &[0],
        fields: &[],
    }];
    static BAD_EVENT_PROGRAM: Program = Program {
        id: "bad_named_event_test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &[],
        transitions: &BAD_EVENT_TRANSITIONS,
        bindings: &[],
        event_bindings: &BAD_EVENT_BINDINGS,
    };

    static LOOP_EVENT_ACTIONS: [Action; 2] = [
        Action::Add {
            var: "count",
            value: &EVENT_ADD_ONE,
        },
        Action::EmitEvent { name: "loop" },
    ];
    static LOOP_EVENT_TRANSITIONS: [Transition; 2] = [
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &LOOP_EVENT_ACTIONS,
        },
        Transition {
            from: 0,
            to: 0,
            guard: &EVENT_ALWAYS,
            actions: &LOOP_EVENT_ACTIONS,
        },
    ];
    static LOOP_EVENT_BINDINGS: [EventBinding; 1] = [EventBinding {
        name: "loop",
        transitions: &[0, 1],
        fields: &[],
    }];
    static LOOP_EVENT_PROGRAM: Program = Program {
        id: "named_event_budget_test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &[],
        transitions: &LOOP_EVENT_TRANSITIONS,
        bindings: &[],
        event_bindings: &LOOP_EVENT_BINDINGS,
    };
    fn handler(boid: u32) {
        let _ = execute(&PROGRAM, dispatcher::GimmickHook::ChargeTick, boid);
    }
    static REGS: [Registration; 1] = [Registration {
        hook: dispatcher::GimmickHook::ChargeTick,
        handler,
        status: None,
        transitions: None,
    }];

    static LUCARIO_AURA: NativeSource =
        NativeSource("work_float:lucario:FIGHTER_LUCARIO_INSTANCE_WORK_ID_FLOAT_CURR_AURAPOWER");
    static PEACH_SPORES: NativeSource =
        NativeSource("work_int:peach:FIGHTER_PEACH_INSTANCE_WORK_ID_INT_KINOPIOSPORE_SHOOT_NUM");
    static PEACH_RAISE: NativeSource =
        NativeSource("work_flag:peach:FIGHTER_PEACH_INSTANCE_WORK_ID_FLAG_SPECIAL_N_RAISE");
    static GENERIC_ZERO_FLOAT: Expr = Expr::Const(Value::Float(0.0));
    static GENERIC_AURA_EXPR: Expr = Expr::NativeFloat(LUCARIO_AURA);
    static GENERIC_AURA_GUARD: Predicate = Predicate::Compare {
        op: CmpOp::Gte,
        left: &GENERIC_AURA_EXPR,
        right: &GENERIC_ZERO_FLOAT,
    };
    static GENERIC_AURA_ACTIONS: [Action; 1] = [Action::Set {
        var: "aura",
        value: &GENERIC_AURA_EXPR,
    }];
    static GENERIC_AURA_TRANSITIONS: [Transition; 1] = [Transition {
        from: 0,
        to: 1,
        guard: &GENERIC_AURA_GUARD,
        actions: &GENERIC_AURA_ACTIONS,
    }];
    static GENERIC_AURA_INPUTS: [NativeSource; 1] = [LUCARIO_AURA];
    static GENERIC_AURA_VARS: [VarDef; 1] = [VarDef {
        name: "aura",
        kind: VarKind::Float,
        initial: Value::Float(0.0),
    }];
    static GENERIC_AURA_PROGRAM: Program = Program {
        id: "generic_aura_test",
        fighter: "lucario",
        matches_fighter: matches,
        initial_state: 0,
        vars: &GENERIC_AURA_VARS,
        native_inputs: &GENERIC_AURA_INPUTS,
        transitions: &GENERIC_AURA_TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &[],
    };
    static GENERIC_ZERO_INT: Expr = Expr::Const(Value::Int(0));
    static GENERIC_SPORES_EXPR: Expr = Expr::NativeInt(PEACH_SPORES);
    static GENERIC_PEACH_GUARD: Predicate = Predicate::And(
        &Predicate::NativeFlag(PEACH_RAISE),
        &Predicate::Compare {
            op: CmpOp::Gte,
            left: &GENERIC_SPORES_EXPR,
            right: &GENERIC_ZERO_INT,
        },
    );
    static GENERIC_PEACH_ACTIONS: [Action; 1] = [Action::Set {
        var: "spores",
        value: &GENERIC_SPORES_EXPR,
    }];
    static GENERIC_PEACH_TRANSITIONS: [Transition; 1] = [Transition {
        from: 0,
        to: 1,
        guard: &GENERIC_PEACH_GUARD,
        actions: &GENERIC_PEACH_ACTIONS,
    }];
    static GENERIC_PEACH_INPUTS: [NativeSource; 2] = [PEACH_SPORES, PEACH_RAISE];
    static GENERIC_PEACH_VARS: [VarDef; 1] = [VarDef {
        name: "spores",
        kind: VarKind::Int,
        initial: Value::Int(0),
    }];
    static GENERIC_PEACH_PROGRAM: Program = Program {
        id: "generic_peach_test",
        fighter: "peach",
        matches_fighter: matches,
        initial_state: 0,
        vars: &GENERIC_PEACH_VARS,
        native_inputs: &GENERIC_PEACH_INPUTS,
        transitions: &GENERIC_PEACH_TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &[],
    };
    static FOREIGN_NATIVE_PROGRAM: Program = Program {
        id: "foreign_native_test",
        fighter: "peach",
        matches_fighter: matches,
        initial_state: 0,
        vars: &GENERIC_AURA_VARS,
        native_inputs: &GENERIC_AURA_INPUTS,
        transitions: &GENERIC_AURA_TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &[],
    };
    static UNDECLARED_NATIVE_PROGRAM: Program = Program {
        id: "undeclared_native_test",
        fighter: "lucario",
        matches_fighter: matches,
        initial_state: 0,
        vars: &GENERIC_AURA_VARS,
        native_inputs: &[],
        transitions: &GENERIC_AURA_TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &[],
    };
    static CLOUD_WORK_ALIAS: NativeSource =
        NativeSource("work_float:cloud:FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE");
    static DUPLICATE_ALIAS_INPUTS: [NativeSource; 2] =
        [NativeSource("cloud_limit_gauge"), CLOUD_WORK_ALIAS];
    static DUPLICATE_ALIAS_PROGRAM: Program = Program {
        id: "duplicate_alias_native_test",
        fighter: "cloud",
        matches_fighter: matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &DUPLICATE_ALIAS_INPUTS,
        transitions: &TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &[],
    };
    static ARTICLE_ACTIONS: [Action; 1] = [Action::GenerateArticle {
        symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
    }];
    static ARTICLE_TRANSITIONS: [Transition; 1] = [Transition {
        from: 0,
        to: 1,
        guard: &TRUE,
        actions: &ARTICLE_ACTIONS,
    }];
    static ARTICLE_PROGRAM: Program = Program {
        id: "article_action_test",
        fighter: "cloud",
        matches_fighter: matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &[],
        transitions: &ARTICLE_TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &[],
    };
    static WEAPON_STATUS_ACTIONS: [Action; 1] = [Action::RequestWeaponStatus {
        weapon_boid: 0x2000_0001,
        symbol: "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR",
    }];
    static WEAPON_STATUS_TRANSITIONS: [Transition; 1] = [Transition {
        from: 0,
        to: 1,
        guard: &TRUE,
        actions: &WEAPON_STATUS_ACTIONS,
    }];
    static WEAPON_STATUS_PROGRAM: Program = Program {
        id: "weapon_status_action_test",
        fighter: "cloud",
        matches_fighter: matches,
        initial_state: 0,
        vars: &VARS,
        native_inputs: &[],
        transitions: &WEAPON_STATUS_TRANSITIONS,
        bindings: &BINDINGS,
        event_bindings: &[],
    };

    static TIMER_IDLE: Predicate = Predicate::Flag("never");
    static TIMER_EXPIRED: Predicate = Predicate::Expired("duration");
    static TIMER_TIMEOUT_ACTIONS: [Action; 1] = [Action::Raise { var: "fired" }];
    static TIMER_TRANSITIONS: [Transition; 2] = [
        // This inert frame transition exists only to model a generated
        // ChargeTick binding; the interpreter owns decrementing the timer.
        Transition {
            from: 0,
            to: 0,
            guard: &TIMER_IDLE,
            actions: &[],
        },
        Transition {
            from: 0,
            to: 1,
            guard: &TIMER_EXPIRED,
            actions: &TIMER_TIMEOUT_ACTIONS,
        },
    ];
    static TIMER_BINDINGS: [Binding; 2] = [
        Binding {
            hook: dispatcher::GimmickHook::ChargeTick,
            article: None,
            transitions: &[0],
        },
        Binding {
            hook: dispatcher::GimmickHook::Timeout,
            article: None,
            transitions: &[1],
        },
    ];
    static TIMER_VARS: [VarDef; 3] = [
        VarDef {
            name: "duration",
            kind: VarKind::Timer,
            initial: Value::Int(2),
        },
        VarDef {
            name: "never",
            kind: VarKind::Flag,
            initial: Value::Flag(false),
        },
        VarDef {
            name: "fired",
            kind: VarKind::Flag,
            initial: Value::Flag(false),
        },
    ];
    static TIMER_PROGRAM: Program = Program {
        id: "timer_test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &TIMER_VARS,
        native_inputs: &[],
        transitions: &TIMER_TRANSITIONS,
        bindings: &TIMER_BINDINGS,
        event_bindings: &[],
    };
    static REMAINING_READ: Expr = Expr::TimerRemaining("duration");
    static REMAINING_ACTIONS: [Action; 1] = [Action::Set {
        var: "observed",
        value: &REMAINING_READ,
    }];
    static REMAINING_GUARD_VALUE: Expr = Expr::Const(Value::Int(1));
    static REMAINING_GUARD: Predicate = Predicate::Compare {
        op: CmpOp::Eq,
        left: &REMAINING_GUARD_VALUE,
        right: &REMAINING_GUARD_VALUE,
    };
    static REMAINING_TRANSITIONS: [Transition; 1] = [Transition {
        from: 0,
        to: 0,
        guard: &REMAINING_GUARD,
        actions: &REMAINING_ACTIONS,
    }];
    static REMAINING_BINDINGS: [Binding; 1] = [Binding {
        hook: dispatcher::GimmickHook::ChargeTick,
        article: None,
        transitions: &[0],
    }];
    static REMAINING_VARS: [VarDef; 2] = [
        VarDef {
            name: "duration",
            kind: VarKind::Timer,
            initial: Value::Int(2),
        },
        VarDef {
            name: "observed",
            kind: VarKind::Int,
            initial: Value::Int(0),
        },
    ];
    static REMAINING_PROGRAM: Program = Program {
        id: "timer_remaining_test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &REMAINING_VARS,
        native_inputs: &[],
        transitions: &REMAINING_TRANSITIONS,
        bindings: &REMAINING_BINDINGS,
        event_bindings: &[],
    };
    static TIMER_A_EXPIRED: Predicate = Predicate::Expired("timer_a");
    static TIMER_B_EXPIRED: Predicate = Predicate::Expired("timer_b");
    static TIMER_A_ACTIONS: [Action; 1] = [Action::Raise { var: "a_fired" }];
    static TIMER_B_ACTIONS: [Action; 1] = [Action::Raise { var: "b_fired" }];
    static MULTI_TIMER_TRANSITIONS: [Transition; 3] = [
        Transition {
            from: 0,
            to: 0,
            guard: &TIMER_IDLE,
            actions: &[],
        },
        Transition {
            from: 0,
            to: 1,
            guard: &TIMER_B_EXPIRED,
            actions: &TIMER_B_ACTIONS,
        },
        Transition {
            from: 0,
            to: 2,
            guard: &TIMER_A_EXPIRED,
            actions: &TIMER_A_ACTIONS,
        },
    ];
    static MULTI_TIMER_BINDINGS: [Binding; 2] = [
        Binding {
            hook: dispatcher::GimmickHook::ChargeTick,
            article: None,
            transitions: &[0],
        },
        Binding {
            hook: dispatcher::GimmickHook::Timeout,
            article: None,
            transitions: &[1, 2],
        },
    ];
    static MULTI_TIMER_VARS: [VarDef; 5] = [
        VarDef {
            name: "never",
            kind: VarKind::Flag,
            initial: Value::Flag(false),
        },
        VarDef {
            name: "timer_a",
            kind: VarKind::Timer,
            initial: Value::Int(1),
        },
        VarDef {
            name: "timer_b",
            kind: VarKind::Timer,
            initial: Value::Int(0),
        },
        VarDef {
            name: "a_fired",
            kind: VarKind::Flag,
            initial: Value::Flag(false),
        },
        VarDef {
            name: "b_fired",
            kind: VarKind::Flag,
            initial: Value::Flag(false),
        },
    ];
    static MULTI_TIMER_PROGRAM: Program = Program {
        id: "multi_timer_test",
        fighter: "test",
        matches_fighter: matches,
        initial_state: 0,
        vars: &MULTI_TIMER_VARS,
        native_inputs: &[],
        transitions: &MULTI_TIMER_TRANSITIONS,
        bindings: &MULTI_TIMER_BINDINGS,
        event_bindings: &[],
    };

    fn isolated(boid: u32) {
        // Live controller tests share this process and can leave an override
        // that suppresses static dispatch, even though tests hold TEST_LOCK.
        super::super::live_behavior::clear_overrides();
        super::super::live_behavior::clear_compiled_baseline();
        state::reset_all();
        dispatcher::clear();
        crate::slight::gimmicks::on_reset_fighter(boid);
        MATCH.store(true, Ordering::SeqCst);
    }

    #[test]
    fn registered_closed_artifact_executes_and_is_reconnect_safe() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let boid = 0xF0AA_0001;
        isolated(boid);
        register(&PROGRAM, &REGS).unwrap();
        register(&PROGRAM, &REGS).unwrap();
        assert_eq!(
            dispatcher::handler_count(dispatcher::GimmickHook::ChargeTick),
            1
        );
        dispatcher::fire(dispatcher::GimmickHook::ChargeTick, boid);
        assert_eq!(read(&PROGRAM, boid, "count").unwrap(), Value::Int(1));
        assert_eq!(read(&PROGRAM, boid, "armed").unwrap(), Value::Flag(false));
        assert_eq!(
            state::get(boid, "gimmick/test/state_id"),
            Some(state::VarValue::Int(1))
        );
        isolated(boid);
    }

    #[test]
    fn static_named_events_drain_fifo_within_one_dispatch_step() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let boid = 0xF0AA_0081;
        isolated(boid);
        validate(&EVENT_PROGRAM, &REGS).unwrap();
        execute(&EVENT_PROGRAM, dispatcher::GimmickHook::ChargeTick, boid).unwrap();
        assert_eq!(read(&EVENT_PROGRAM, boid, "count").unwrap(), Value::Int(2));
        assert_eq!(
            state::get(boid, "gimmick/named_event_test/state_id"),
            Some(state::VarValue::Int(1))
        );
        isolated(boid);
    }

    #[test]
    fn receiver_field_reads_use_each_nested_event_context() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        static FIRST_FIELDS: [EventField; 1] = [EventField {
            name: "amount",
            kind: VarKind::Float,
        }];
        static SECOND_FIELDS: [EventField; 2] = [
            EventField {
                name: "active",
                kind: VarKind::Flag,
            },
            EventField {
                name: "amount",
                kind: VarKind::Int,
            },
        ];
        static FIRST_AMOUNT: Expr = Expr::EventField {
            event: "first",
            field: "amount",
        };
        static SECOND_AMOUNT: Expr = Expr::EventField {
            event: "second",
            field: "amount",
        };
        static SECOND_ACTIVE: Predicate = Predicate::EventFlag {
            event: "second",
            field: "active",
        };
        static ALWAYS: Predicate = Predicate::Compare {
            op: CmpOp::Eq,
            left: &Expr::Const(Value::Int(1)),
            right: &Expr::Const(Value::Int(1)),
        };
        static SECOND_ARGS: [EventArgument; 2] = [
            EventArgument {
                field: "active",
                value: Value::Flag(true),
            },
            EventArgument {
                field: "amount",
                value: Value::Int(9),
            },
        ];
        static FIRST_ACTIONS: [Action; 2] = [
            Action::Set {
                var: "first_value",
                value: &FIRST_AMOUNT,
            },
            Action::EmitEventArguments {
                name: "second",
                arguments: &SECOND_ARGS,
            },
        ];
        static SECOND_ACTIONS: [Action; 1] = [Action::Add {
            var: "second_value",
            value: &SECOND_AMOUNT,
        }];
        static TRANSITIONS: [Transition; 3] = [
            Transition {
                from: 0,
                to: 0,
                guard: &ALWAYS,
                actions: &[],
            },
            Transition {
                from: 0,
                to: 0,
                guard: &ALWAYS,
                actions: &FIRST_ACTIONS,
            },
            Transition {
                from: 0,
                to: 0,
                guard: &SECOND_ACTIVE,
                actions: &SECOND_ACTIONS,
            },
        ];
        static EVENT_BINDINGS: [EventBinding; 2] = [
            EventBinding {
                name: "first",
                transitions: &[1],
                fields: &FIRST_FIELDS,
            },
            EventBinding {
                name: "second",
                transitions: &[2],
                fields: &SECOND_FIELDS,
            },
        ];
        static VARS: [VarDef; 2] = [
            VarDef {
                name: "first_value",
                kind: VarKind::Float,
                initial: Value::Float(0.0),
            },
            VarDef {
                name: "second_value",
                kind: VarKind::Int,
                initial: Value::Int(0),
            },
        ];
        static BINDINGS: [Binding; 1] = [Binding {
            hook: dispatcher::GimmickHook::ChargeTick,
            article: None,
            transitions: &[0],
        }];
        static PROGRAM: Program = Program {
            id: "event_field_context_test",
            fighter: "test",
            matches_fighter: matches,
            initial_state: 0,
            vars: &VARS,
            native_inputs: &[],
            transitions: &TRANSITIONS,
            bindings: &BINDINGS,
            event_bindings: &EVENT_BINDINGS,
        };

        let boid = 0xF0AA_00D1;
        isolated(boid);
        validate(&PROGRAM, &REGS).unwrap();
        execute_named_with_payload(&PROGRAM, boid, "first", &[Value::Float(1.25)]).unwrap();
        assert_eq!(
            read(&PROGRAM, boid, "first_value").unwrap(),
            Value::Float(1.25)
        );
        assert_eq!(read(&PROGRAM, boid, "second_value").unwrap(), Value::Int(9));
        isolated(boid);
    }

    #[test]
    fn external_named_event_uses_fifo_without_hook_or_timer_and_fails_atomically() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let boid = 0xF0AA_0082;
        isolated(boid);
        MATCH.store(true, Ordering::SeqCst);
        register(&EVENT_TIMER_PROGRAM, &EVENT_TIMER_REGISTRATIONS).unwrap();

        // This receiver has no internal EmitEvent producer: the trusted ACMD
        // bridge is its producer and still routes through execute_named.
        execute_named(&EVENT_TIMER_PROGRAM, boid, "acmd_only").unwrap();
        assert_eq!(
            read(&EVENT_TIMER_PROGRAM, boid, "count").unwrap(),
            Value::Int(100)
        );
        assert_eq!(
            read(&EVENT_TIMER_PROGRAM, boid, "timer").unwrap(),
            Value::Int(2)
        );

        execute_named(&EVENT_TIMER_PROGRAM, boid, "root").unwrap();
        assert_eq!(
            read(&EVENT_TIMER_PROGRAM, boid, "count").unwrap(),
            Value::Int(102)
        );
        assert_eq!(
            read(&EVENT_TIMER_PROGRAM, boid, "timer").unwrap(),
            Value::Int(2)
        );
        execute(
            &EVENT_TIMER_PROGRAM,
            dispatcher::GimmickHook::ChargeTick,
            boid,
        )
        .unwrap();
        assert_eq!(
            read(&EVENT_TIMER_PROGRAM, boid, "timer").unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            read(&EVENT_TIMER_PROGRAM, boid, "count").unwrap(),
            Value::Int(204)
        );
        assert_eq!(
            state::get(boid, "gimmick/named_event_no_timer_tick_test/state_id"),
            Some(state::VarValue::Int(0))
        );

        let before_missing = state::get(boid, "gimmick/named_event_no_timer_tick_test/state_id");
        assert!(execute_named(&EVENT_TIMER_PROGRAM, boid, "missing")
            .unwrap_err()
            .contains("unbound named event"));
        assert_eq!(
            state::get(boid, "gimmick/named_event_no_timer_tick_test/state_id"),
            before_missing
        );

        MATCH.store(false, Ordering::SeqCst);
        assert!(execute_named(&EVENT_TIMER_PROGRAM, boid, "root")
            .unwrap_err()
            .contains("does not match this fighter program"));
        MATCH.store(true, Ordering::SeqCst);
        isolated(boid);

        let bad_boid = boid + 1;
        isolated(bad_boid);
        assert!(execute_named(&BAD_EVENT_PROGRAM, bad_boid, "bad")
            .unwrap_err()
            .contains("wrong value type"));
        assert_eq!(state::boid_count(), 0);
        assert_eq!(
            read(&BAD_EVENT_PROGRAM, bad_boid, "count").unwrap(),
            Value::Int(0)
        );
        assert_eq!(
            state::get(bad_boid, "gimmick/bad_named_event_test/state_id"),
            None
        );
        isolated(bad_boid);

        let budget_boid = boid + 2;
        isolated(budget_boid);
        let error = execute_named(&LOOP_EVENT_PROGRAM, budget_boid, "loop").unwrap_err();
        assert!(error.contains("event count exceeds"), "{error}");
        assert_eq!(state::boid_count(), 0);
        assert_eq!(
            read(&LOOP_EVENT_PROGRAM, budget_boid, "count").unwrap(),
            Value::Int(0)
        );
        assert_eq!(
            state::get(budget_boid, "gimmick/named_event_budget_test/state_id"),
            None
        );
        isolated(budget_boid);
    }

    #[test]
    fn wrong_fighter_does_not_allocate_or_mutate_state() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let boid = 0xF0AA_0002;
        isolated(boid);
        MATCH.store(false, Ordering::SeqCst);
        execute(&PROGRAM, dispatcher::GimmickHook::ChargeTick, boid).unwrap();
        assert_eq!(state::boid_count(), 0);
        isolated(boid);
    }

    #[test]
    fn malformed_static_artifact_fails_before_registration() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let boid = 0xF0AA_0003;
        isolated(boid);
        static BAD_BINDINGS: [Binding; 1] = [Binding {
            hook: dispatcher::GimmickHook::Ko,
            article: None,
            transitions: &[],
        }];
        let bad = Program {
            id: PROGRAM.id,
            fighter: PROGRAM.fighter,
            matches_fighter: PROGRAM.matches_fighter,
            initial_state: PROGRAM.initial_state,
            vars: PROGRAM.vars,
            native_inputs: PROGRAM.native_inputs,
            transitions: PROGRAM.transitions,
            bindings: &BAD_BINDINGS,
            event_bindings: &[],
        };
        assert!(register(Box::leak(Box::new(bad)), &REGS).is_err());
        assert_eq!(
            dispatcher::handler_count(dispatcher::GimmickHook::ChargeTick),
            0
        );
        isolated(boid);
    }

    #[test]
    fn symbolic_article_actions_are_validated_before_handlers_arm() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let boid = 0xF0AA_0008;
        isolated(boid);
        register(&ARTICLE_PROGRAM, &REGS).unwrap();
        assert_eq!(
            dispatcher::handler_count(dispatcher::GimmickHook::ChargeTick),
            1
        );

        static WRONG_CATEGORY_ACTIONS: [Action; 1] = [Action::RemoveArticle {
            symbol: "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR",
        }];
        static WRONG_CATEGORY_TRANSITIONS: [Transition; 1] = [Transition {
            from: 0,
            to: 1,
            guard: &TRUE,
            actions: &WRONG_CATEGORY_ACTIONS,
        }];
        let bad = Program {
            id: "bad_article_action_test",
            fighter: "cloud",
            matches_fighter: matches,
            initial_state: 0,
            vars: &VARS,
            native_inputs: &[],
            transitions: &WRONG_CATEGORY_TRANSITIONS,
            bindings: &BINDINGS,
            event_bindings: &[],
        };
        assert!(register(Box::leak(Box::new(bad)), &REGS)
            .unwrap_err()
            .contains("not allowlisted"));
        isolated(boid);
    }

    #[test]
    fn native_article_rejection_does_not_commit_earlier_staged_variable_writes() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let boid = 0xF0AA_0009;
        isolated(boid);
        static FAILING_ACTIONS: [Action; 2] = [
            Action::Add {
                var: "count",
                value: &ONE,
            },
            Action::GenerateArticle {
                symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
            },
        ];
        static FAILING_TRANSITIONS: [Transition; 1] = [Transition {
            from: 0,
            to: 1,
            guard: &TRUE,
            actions: &FAILING_ACTIONS,
        }];
        let program = Program {
            id: "failed_article_action_test",
            fighter: "cloud",
            matches_fighter: matches,
            initial_state: 0,
            vars: &VARS,
            native_inputs: &[],
            transitions: &FAILING_TRANSITIONS,
            bindings: &BINDINGS,
            event_bindings: &[],
        };
        let program = Box::leak(Box::new(program));
        let error = execute(program, dispatcher::GimmickHook::ChargeTick, boid).unwrap_err();
        // The host engine mock proves an exact Cloud identity and rejects the
        // native article operation; authored writes must remain staged.
        assert!(error.contains("native action failed"), "{error}");
        assert!(
            error.contains("injected native article generation rejection"),
            "{error}"
        );
        assert_eq!(state::boid_count(), 0);
        assert_eq!(read(program, boid, "count").unwrap(), Value::Int(0));
        assert_eq!(
            state::get(boid, "gimmick/failed_article_action_test/state_id"),
            None
        );
        isolated(boid);
    }

    #[test]
    fn frame_ticks_decrement_timers_once_and_route_timeout_once() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let boid = 0xF0AA_0004;
        isolated(boid);

        execute(&TIMER_PROGRAM, dispatcher::GimmickHook::ChargeTick, boid).unwrap();
        assert_eq!(
            read(&TIMER_PROGRAM, boid, "duration").unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            read(&TIMER_PROGRAM, boid, "fired").unwrap(),
            Value::Flag(false)
        );

        execute(&TIMER_PROGRAM, dispatcher::GimmickHook::ChargeTick, boid).unwrap();
        assert_eq!(
            read(&TIMER_PROGRAM, boid, "duration").unwrap(),
            Value::Int(0)
        );
        assert_eq!(
            read(&TIMER_PROGRAM, boid, "fired").unwrap(),
            Value::Flag(true)
        );
        assert_eq!(
            state::get(boid, "gimmick/timer_test/state_id"),
            Some(state::VarValue::Int(1))
        );

        // Zero is a floor, not an underflowing counter, and expiry is an
        // edge: this later frame cannot run the timeout transition again.
        execute(&TIMER_PROGRAM, dispatcher::GimmickHook::ChargeTick, boid).unwrap();
        assert_eq!(
            read(&TIMER_PROGRAM, boid, "duration").unwrap(),
            Value::Int(0)
        );
        assert_eq!(
            state::get(boid, "gimmick/timer_test/state_id"),
            Some(state::VarValue::Int(1))
        );

        // Object lifecycle owns the countdown: a reset cannot leak the old
        // stock's elapsed timer into a new one.
        state::reset_boid(boid);
        assert_eq!(
            read(&TIMER_PROGRAM, boid, "duration").unwrap(),
            Value::Int(2)
        );
        assert_eq!(state::get(boid, "gimmick/timer_test/state_id"), None);
        isolated(boid);
    }

    #[test]
    fn timer_remaining_reads_staged_countdown_without_mutating_or_ticking() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let boid = 0xF0AA_0014;
        isolated(boid);

        let values = BTreeMap::from([("duration", Value::Int(2)), ("observed", Value::Int(0))]);
        let before = values.clone();
        assert_eq!(
            expr(
                &REMAINING_PROGRAM,
                &values,
                &REMAINING_READ,
                &NativeSnapshot::empty(),
                None,
            )
            .unwrap(),
            Value::Int(2)
        );
        assert_eq!(values, before, "a read must not tick or write the timer");
        let out_of_range = BTreeMap::from([(
            "duration",
            Value::Int(super::super::live_model::MAX_TIMER_FRAMES + 1),
        )]);
        assert!(expr(
            &REMAINING_PROGRAM,
            &out_of_range,
            &REMAINING_READ,
            &NativeSnapshot::empty(),
            None,
        )
        .is_err());

        let mut errors = Vec::new();
        assert_eq!(
            validate_expr_types(
                &REMAINING_PROGRAM,
                &REMAINING_READ,
                ReadScope::default(),
                &mut errors
            ),
            Some(VarKind::Int)
        );
        assert!(errors.is_empty());
        static WRONG_KIND: Expr = Expr::TimerRemaining("observed");
        static MISSING: Expr = Expr::TimerRemaining("missing");
        static RAW_TIMER: Expr = Expr::Var("duration");
        for expression in [&WRONG_KIND, &MISSING] {
            let mut errors = Vec::new();
            assert_eq!(
                validate_expr_types(
                    &REMAINING_PROGRAM,
                    expression,
                    ReadScope::default(),
                    &mut errors
                ),
                None
            );
            assert!(!errors.is_empty());
        }
        let mut errors = Vec::new();
        validate_predicate_types(
            &REMAINING_PROGRAM,
            &Predicate::Compare {
                op: CmpOp::Eq,
                left: &RAW_TIMER,
                right: &REMAINING_GUARD_VALUE,
            },
            ReadScope::default(),
            &mut errors,
        );
        assert!(errors.iter().any(|error| error.contains("numeric types")));

        execute(
            &REMAINING_PROGRAM,
            dispatcher::GimmickHook::ChargeTick,
            boid,
        )
        .unwrap();
        assert_eq!(
            read(&REMAINING_PROGRAM, boid, "observed").unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            read(&REMAINING_PROGRAM, boid, "duration").unwrap(),
            Value::Int(1)
        );
        execute(
            &REMAINING_PROGRAM,
            dispatcher::GimmickHook::ChargeTick,
            boid,
        )
        .unwrap();
        assert_eq!(
            read(&REMAINING_PROGRAM, boid, "observed").unwrap(),
            Value::Int(0)
        );
        assert_eq!(
            read(&REMAINING_PROGRAM, boid, "duration").unwrap(),
            Value::Int(0)
        );
        isolated(boid);
    }

    #[test]
    fn one_timer_crossing_cannot_trigger_another_timer_timeout() {
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let boid = 0xF0AA_0006;
        isolated(boid);
        execute(
            &MULTI_TIMER_PROGRAM,
            dispatcher::GimmickHook::ChargeTick,
            boid,
        )
        .unwrap();
        assert_eq!(
            read(&MULTI_TIMER_PROGRAM, boid, "timer_a").unwrap(),
            Value::Int(0)
        );
        assert_eq!(
            read(&MULTI_TIMER_PROGRAM, boid, "timer_b").unwrap(),
            Value::Int(0)
        );
        assert_eq!(
            read(&MULTI_TIMER_PROGRAM, boid, "a_fired").unwrap(),
            Value::Flag(true)
        );
        assert_eq!(
            read(&MULTI_TIMER_PROGRAM, boid, "b_fired").unwrap(),
            Value::Flag(false)
        );
        assert_eq!(
            state::get(boid, "gimmick/multi_timer_test/state_id"),
            Some(state::VarValue::Int(2))
        );
        isolated(boid);
    }
}
