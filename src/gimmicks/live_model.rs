// Bounded, owned behavior language shared by the desktop and game plugin.
// This module intentionally depends only on serde and the standard library.
// The wire model contains data, never Rust source, callbacks, or raw native
// identifiers. The evaluator is also used by the controller preview so timer
// and transition ordering have one implementation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use super::native_inputs::{
    ButtonEdge, CombatOp, InputButton, ItemAction, NativeInput, NativeKind, NativeSnapshot,
};

pub const MODEL_VERSION: u8 = 5;
/// Holds portable controllers for the complete pinned roster while bounding allocation.
pub const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;
pub const MAX_PROGRAM_ID_BYTES: usize = 64;
pub const MAX_FIGHTER_BYTES: usize = 48;
pub const MAX_NAME_BYTES: usize = 48;
pub const MAX_SYMBOL_BYTES: usize = 160;
pub const MAX_STATES: usize = 64;
pub const MAX_VARIABLES: usize = 64;
pub const MAX_NATIVE_INPUTS: usize = 64;
pub const MAX_TRANSITIONS: usize = 128;
pub const MAX_BINDINGS: usize = 8;
pub const MAX_EVENT_BINDINGS: usize = 32;
pub const MAX_EVENT_FIELDS: usize = 8;
pub const MAX_EVENTS_PER_STEP: usize = 64;
pub const MAX_EVENT_DEPTH: usize = 8;
pub const MAX_ACTIONS_PER_TRANSITION: usize = 24;
pub const MAX_TIMER_FRAMES: i32 = 1_000_000;
pub const MAX_ABS_NUMBER: f64 = super::native_inputs::MAX_ABS_NATIVE_VALUE;
pub const MAX_EXPR_DEPTH: usize = 16;
pub const MAX_EXPR_NODES: usize = 256;

/// The runtime hook subset supported by the game plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::enum_variant_names)] // These names match the shared behavior-hook wire contract.
pub enum HookName {
    OnDealDamage,
    OnTakeDamage,
    OnAttackShielded,
    OnShieldHit,
    OnIncomingHit,
    OnReflect,
    OnAbsorb,
    OnGrab,
    OnThrow,
    OnGrabbed,
    OnItemPickup,
    OnItemThrown,
    OnChargeTick,
    OnTimeout,
    OnSpend,
    OnStatusEnter,
    OnStatusExit,
    OnSwitch,
    OnStockStart,
    #[serde(rename = "onKO")]
    OnKo,
    OnArticleSpawn,
    OnArticleRemoved,
}

impl HookName {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::OnDealDamage => "onDealDamage",
            Self::OnTakeDamage => "onTakeDamage",
            Self::OnAttackShielded => "onAttackShielded",
            Self::OnShieldHit => "onShieldHit",
            Self::OnIncomingHit => "onIncomingHit",
            Self::OnReflect => "onReflect",
            Self::OnAbsorb => "onAbsorb",
            Self::OnGrab => "onGrab",
            Self::OnThrow => "onThrow",
            Self::OnGrabbed => "onGrabbed",
            Self::OnItemPickup => "onItemPickup",
            Self::OnItemThrown => "onItemThrown",
            Self::OnChargeTick => "onChargeTick",
            Self::OnTimeout => "onTimeout",
            Self::OnSpend => "onSpend",
            Self::OnStatusEnter => "onStatusEnter",
            Self::OnStatusExit => "onStatusExit",
            Self::OnSwitch => "onSwitch",
            Self::OnStockStart => "onStockStart",
            Self::OnKo => "onKO",
            Self::OnArticleSpawn => "onArticleSpawn",
            Self::OnArticleRemoved => "onArticleRemoved",
        }
    }
}

/// The variable kinds. Timers are their own kind rather than an int
/// convention: a countdown has start/expiry semantics an int does not.
/// Saved projects spell kinds `Float`; the live wire historically sent `float`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VarKind {
    #[serde(alias = "float")]
    Float,
    #[serde(alias = "int")]
    Int,
    #[serde(alias = "flag")]
    Flag,
    #[serde(alias = "timer")]
    Timer,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Float(f32),
    Int(i32),
    Flag(bool),
}

pub type TypedValue = Value;

impl Value {
    pub fn kind(self) -> VarKind {
        match self {
            Self::Float(_) => VarKind::Float,
            Self::Int(_) => VarKind::Int,
            Self::Flag(_) => VarKind::Flag,
        }
    }

    fn finite_and_bounded(self) -> bool {
        match self {
            Self::Float(value) => value.is_finite() && f64::from(value).abs() <= MAX_ABS_NUMBER,
            Self::Int(value) => i64::from(value).abs() <= MAX_ABS_NUMBER as i64,
            Self::Flag(_) => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VarDef {
    /// Carried to the same controller on the next form (Pyra/Mythra, Pokémon Trainer).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_across_form_switch: bool,
    pub name: String,
    pub kind: VarKind,
    pub initial: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Expr {
    StatusFrame,
    MotionFrame,
    StickX {
        #[serde(default)]
        relative_to_facing: bool,
    },
    StickY,
    RandomInt {
        min: i32,
        max: i32,
    },
    RandomFloat {
        min: f32,
        max: f32,
    },

    Const(Value),
    PendingDamage,
    Var(String),
    TimerRemaining(String),
    EventField {
        event: String,
        field: String,
    },
    NativeFloat(NativeInput),
    /// Legacy `{"ArticleCount":{"symbol":..}}` loads as an owned article count.
    #[serde(
        alias = "ArticleCount",
        deserialize_with = "super::native_inputs::int_source"
    )]
    NativeInt(NativeInput),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
}

/// Comparison operators for numeric guards; saved as `Eq`, accepted as `eq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CmpOp {
    #[serde(alias = "eq")]
    Eq,
    #[serde(alias = "lt")]
    Lt,
    #[serde(alias = "lte")]
    Lte,
    #[serde(alias = "gt")]
    Gt,
    #[serde(alias = "gte")]
    Gte,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Predicate {
    SituationChanged,
    MotionEnded,
    CancelEnabled,
    TermEnabled {
        symbol: String,
    },
    InStatus {
        symbol: String,
    },
    /// The current animation is this motion, e.g. `throw_f` (Hash40 name).
    InMotion {
        motion: String,
    },
    Situation {
        kind: super::native_inputs::SituationKind,
    },
    Button {
        button: InputButton,
        edge: ButtonEdge,
    },
    Flag(String),
    EventFlag {
        event: String,
        field: String,
    },
    NativeFlag(NativeInput),
    DamageStatusIs(NativeInput),
    /// Whether the captured attack came from an owned weapon/article. Damage roots only.
    DamageIsProjectile,
    Expired(String),
    Compare {
        op: CmpOp,
        left: Expr,
        right: Expr,
    },
    Not(Box<Predicate>),
    And(Box<Predicate>, Box<Predicate>),
    Or(Box<Predicate>, Box<Predicate>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeaponTarget {
    pub battle_object_id: u32,
}

/// What fires when a transition is taken. `Set` writes an expression of the
/// variable's own kind (an int expression also starts a timer); `Reset`
/// returns a variable to its declared initial. Saved projects and the live
/// wire share this type; legacy saved action names load through the shim below.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, remote = "Self")]
pub enum Action {
    /// Add to or set a native fighter resource (`resource_commands::RESOURCES`).
    Resource(super::resource_commands::Command<Expr>),
    /// Highlight one slot (0..=3) of Hero's open Command Selection menu.
    ChooseHeroMenuSpell {
        slot: u8,
    },
    Status(super::status_commands::Command<Expr>),
    /// Add 0..=999 percent to the exact held opponent at the owning Main boundary.
    AddHeldOpponentDamage {
        amount: Expr,
    },
    /// Select a native throw from grounded catch wait or a chart-owned status.
    ThrowHeldOpponent {
        direction: super::native_inputs::ThrowDirection,
    },
    /// Cut the current capture link, after re-proving both fighter identities.
    ReleaseHeldOpponent,
    /// Signed percent delta (-999..=999); healing is clamped to current percent.
    AddDamage {
        amount: Expr,
    },
    /// Change how this fighter takes or deals hits until the scope ends.
    CombatPolicy {
        scope: super::native_inputs::PolicyScope,
        op: super::native_inputs::CombatOp<Expr>,
    },
    /// Veto this contact synchronously; valid only in an onIncomingHit root.
    CancelIncomingHit,
    Movement(super::native_inputs::MovementAction),
    /// Hold, throw, drop or remove the main-hand item.
    Item(ItemAction),
    /// `ModelModule::set_mesh_visibility`. Motions can carry visibility tracks
    /// for the mesh, so a mesh meant to stay hidden needs this on every frame.
    MeshVisibility {
        mesh: String,
        visible: bool,
    },
    SetMotionRate {
        rate: Expr,
    },
    AddSpeed {
        x: Expr,
        y: Expr,
    },
    RequestArticleStatus {
        symbol: String,
        status: String,
    },
    RemoveNewestArticle {
        symbol: String,
    },
    /// Set speed of, or move, every live projectile of one owned article kind.
    ArticleMovement {
        symbol: String,
        op: super::native_inputs::MovementAction,
    },
    /// Set the speed of, or remove, the newest live object of one owned article kind.
    Object {
        symbol: String,
        op: super::native_inputs::ObjectOp<Expr>,
    },
    SetNative {
        input: NativeInput,
        value: Expr,
    },
    AddNative {
        input: NativeInput,
        value: Expr,
    },
    SetNativeFlag {
        input: NativeInput,
        value: Predicate,
    },

    Set {
        var: String,
        value: Expr,
    },
    SetFlag {
        var: String,
        value: Predicate,
    },
    Add {
        var: String,
        value: Expr,
    },
    Raise {
        var: String,
    },
    Clear {
        var: String,
    },
    Reset {
        var: String,
    },
    GenerateArticle {
        symbol: String,
    },
    RemoveArticle {
        symbol: String,
    },
    RequestFighterStatus {
        symbol: String,
    },
    RequestWeaponStatus {
        target: WeaponTarget,
        symbol: String,
    },
    EmitEvent {
        name: String,
    },
    EmitEventArguments {
        name: String,
        arguments: Vec<EventArgument>,
    },
    EmitEventValues {
        name: String,
        arguments: Vec<ValueArgument>,
    },
}

impl Serialize for Action {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Action::serialize(self, serializer)
    }
}

/// Saved projects written before `CombatPolicy` load as its status-scoped form.
impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde_json::{json, Value};
        let mut value = Value::deserialize(deserializer)?;
        // Unit variants were saved both as "Name" and as {"Name": null}.
        let (name, body) = match &value {
            Value::String(name) => (name.as_str(), None),
            Value::Object(map) if map.len() == 1 => map
                .iter()
                .next()
                .map(|(name, body)| (name.as_str(), Some(body)))
                .unwrap(),
            _ => ("", None),
        };
        let field = |key: &str| body.and_then(|body| body.get(key)).cloned();
        let legacy = match name {
            "SetSuperArmor" => Some(json!("SuperArmor")),
            "ClearArmor" => Some(json!({"Clear": "Armor"})),
            "SetDamageMultiplier" => field("multiplier").map(|v| json!({"DamageTaken": v})),
            "SetReactionMultiplier" => field("multiplier").map(|v| json!({"KnockbackTaken": v})),
            "SetArmor" => field("threshold").map(|v| json!({"Armor": v})),
            _ => None,
        };
        // Cloud's Limit actions became a row of the resource table; amounts became expressions.
        let resource = match name {
            "AdjustCloudLimit" => field("amount")
                .map(|amount| json!({"op": "add", "resource": "cloudLimit", "amount": amount})),
            "ClearCloudLimit" => {
                Some(json!({"op": "set", "resource": "cloudLimit", "amount": 0.0}))
            }
            "Resource" => body.cloned(),
            _ => None,
        };
        if let Some(op) = legacy {
            value = json!({"CombatPolicy": {"scope": "Status", "op": op}});
        }
        if let Some(mut command) = resource {
            if let Some(amount) = command.get("amount").and_then(Value::as_f64) {
                command["amount"] = json!({"Const": {"Float": amount}});
            }
            value = json!({"Resource": command});
        }
        Action::deserialize(value).map_err(serde::de::Error::custom)
    }
}

/// An expression or predicate that an action evaluates.
#[derive(Clone, Copy)]
pub enum Operand<'a> {
    Expr(&'a Expr),
    Predicate(&'a Predicate),
}

impl Action {
    /// Every expression and predicate this action evaluates, in field order.
    /// Constant event payloads, symbols and variable names are not operands.
    pub fn operands(&self) -> Vec<Operand<'_>> {
        let expressions = match self {
            Self::Status(command) => command.expressions(),
            Self::Object { op, .. } => op.expressions(),
            Self::AddSpeed { x, y } => vec![x, y],
            Self::SetNative { value, .. }
            | Self::AddNative { value, .. }
            | Self::Set { value, .. }
            | Self::Add { value, .. }
            | Self::SetMotionRate { rate: value }
            | Self::AddHeldOpponentDamage { amount: value }
            | Self::AddDamage { amount: value }
            | Self::CombatPolicy {
                op:
                    CombatOp::DamageTaken(value)
                    | CombatOp::KnockbackTaken(value)
                    | CombatOp::DamageDealt(value)
                    | CombatOp::KnockbackDealt(value)
                    | CombatOp::Stat(_, value)
                    | CombatOp::Armor(value),
                ..
            }
            | Self::Resource(super::resource_commands::Command { amount: value, .. }) => {
                vec![value]
            }
            Self::SetNativeFlag { value, .. } | Self::SetFlag { value, .. } => {
                return vec![Operand::Predicate(value)]
            }
            Self::EmitEventValues { arguments, .. } => {
                return arguments
                    .iter()
                    .filter_map(|argument| match &argument.value {
                        EventValue::Constant(_) => None,
                        EventValue::Numeric(expr) => Some(Operand::Expr(expr)),
                        EventValue::Flag(predicate) => Some(Operand::Predicate(predicate)),
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        expressions.into_iter().map(Operand::Expr).collect()
    }
}

/// One constant, typed value carried with a named event. Field order is the
/// declaration order in the matching [`EventBinding`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventArgument {
    pub field: String,
    pub value: Value,
}

/// A computed typed event value. The variant tag is part of the wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum EventValue {
    Constant(Value),
    Numeric(Expr),
    Flag(Predicate),
}

/// A source expression or predicate carried with a named event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueArgument {
    pub field: String,
    pub value: EventValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub from: String,
    pub to: String,
    pub guard: Predicate,
    #[serde(default)]
    pub on_frame: bool,
    #[serde(default)]
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookBinding {
    pub hook: HookName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub article: Option<String>,
    pub transitions: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventBinding {
    pub name: String,
    pub transitions: Vec<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<EventField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventField {
    pub name: String,
    pub kind: VarKind,
}

/// Borrowed view of the exact named receiver currently being evaluated.
/// The schema comes from its `EventBinding` and values from the bounded owned
/// event queue payload; it is never retained between dispatches.
#[derive(Debug, Clone, Copy)]
struct EventContext<'a> {
    event: &'a str,
    fields: &'a [EventField],
    values: &'a [Value],
}

#[derive(Clone, Copy, Default)]
struct ReadScope<'a> {
    event: Option<&'a EventBinding>,
    damage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceSource {
    CloudLimitGroundedNeutral,
    CloudLimitIdleExpiry,
}

impl ResourceSource {
    pub fn fighter(&self) -> &'static str {
        "cloud"
    }

    pub fn hook(&self) -> HookName {
        match self {
            Self::CloudLimitGroundedNeutral => HookName::OnSpend,
            Self::CloudLimitIdleExpiry => HookName::OnTimeout,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusTrigger {
    pub hook: HookName,
    pub symbol: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTrigger {
    pub hook: HookName,
    pub source: ResourceSource,
}

/// Complete owned behavior snapshot. Replacement is whole-program and atomic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveProgram {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_statuses: BTreeMap<String, u8>,
    /// Costume slots this runs on; empty runs on every costume.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub costumes: Vec<u8>,
    pub version: u8,
    pub id: String,
    pub fighter: String,
    pub initial: String,
    pub states: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status_bindings: Vec<super::status_model::StateStatus>,
    pub vars: Vec<VarDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_inputs: Vec<NativeInput>,
    pub transitions: Vec<Transition>,
    pub bindings: Vec<HookBinding>,
    #[serde(default)]
    pub event_bindings: Vec<EventBinding>,
    #[serde(default)]
    pub status_triggers: Vec<StatusTrigger>,
    #[serde(default)]
    pub resource_triggers: Vec<ResourceTrigger>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelError {
    pub item: String,
    pub reason: String,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.item, self.reason)
    }
}

impl LiveProgram {
    #[allow(dead_code)] // Plugin frame sampler; host previews supply their own input snapshot.
    pub fn uses_controller_input(&self) -> bool {
        self.transitions
            .iter()
            .take(MAX_TRANSITIONS)
            .any(transition_uses_input)
    }

    #[allow(dead_code)] // Native plugin frame sampler.
    pub fn uses_phase_input(&self) -> bool {
        self.transitions
            .iter()
            .take(MAX_TRANSITIONS)
            .any(|t| transition_uses_reads(t, true, false, true, false, false))
    }

    /// Validate shape, types, supported hooks, action symbols' shape, and
    /// bounded recursion before a program may be installed.
    pub fn validate(&self) -> Result<(), Vec<ModelError>> {
        let mut errors = Vec::new();
        let issue = |errors: &mut Vec<ModelError>, item: String, reason: String| {
            errors.push(ModelError { item, reason });
        };
        if self.version != MODEL_VERSION
            && !(self.version == 4
                && self.status_bindings.is_empty()
                && !self
                    .required_native_features()
                    .contains(super::status_model::CANCEL_FEATURE))
        {
            issue(
                &mut errors,
                "version".into(),
                format!("unsupported version {}", self.version),
            );
        }
        check_text(&self.id, MAX_PROGRAM_ID_BYTES, "id", &mut errors);
        check_text(&self.fighter, MAX_FIGHTER_BYTES, "fighter", &mut errors);
        let empty = self.states.is_empty()
            && self.vars.is_empty()
            && self.native_inputs.is_empty()
            && self.transitions.is_empty()
            && self.bindings.is_empty()
            && self.event_bindings.is_empty()
            && self.status_triggers.is_empty()
            && self.resource_triggers.is_empty();
        if !empty {
            check_text(&self.initial, MAX_NAME_BYTES, "initial", &mut errors);
        }
        if !empty && self.states.is_empty() {
            issue(
                &mut errors,
                "states".into(),
                "a non-empty program needs states".into(),
            );
        }
        if self.states.len() > MAX_STATES {
            issue(
                &mut errors,
                "states".into(),
                format!("must contain at most {MAX_STATES} states"),
            );
        }
        // State identity is stored separately from the variable table. A reviewed event
        // can dispatch an article action without inventing an unused user variable.
        if self.vars.len() > MAX_VARIABLES {
            issue(
                &mut errors,
                "vars".into(),
                format!("must contain at most {MAX_VARIABLES} variables"),
            );
        }
        if self.native_inputs.len() > MAX_NATIVE_INPUTS {
            issue(
                &mut errors,
                "native_inputs".into(),
                format!("must contain at most {MAX_NATIVE_INPUTS} sources"),
            );
        }
        if !empty && self.transitions.is_empty() {
            issue(
                &mut errors,
                "transitions".into(),
                "a non-empty program needs transitions".into(),
            );
        }
        if self.transitions.len() > MAX_TRANSITIONS {
            issue(
                &mut errors,
                "transitions".into(),
                format!("must contain at most {MAX_TRANSITIONS} transitions"),
            );
        }
        if !empty
            && self.bindings.is_empty()
            && self.event_bindings.is_empty()
            && self.status_bindings.is_empty()
        {
            issue(
                &mut errors,
                "bindings".into(),
                "a non-empty program needs hook or named event bindings".into(),
            );
        }
        if self.bindings.len() > MAX_BINDINGS {
            issue(
                &mut errors,
                "bindings".into(),
                format!("must contain at most {MAX_BINDINGS} bindings"),
            );
        }
        if self.event_bindings.len() > MAX_EVENT_BINDINGS {
            issue(
                &mut errors,
                "event_bindings".into(),
                format!("must contain at most {MAX_EVENT_BINDINGS} event bindings"),
            );
        }
        if self.status_triggers.len() > MAX_BINDINGS || self.resource_triggers.len() > MAX_BINDINGS
        {
            issue(
                &mut errors,
                "triggers".into(),
                format!("each trigger list must contain at most {MAX_BINDINGS} entries"),
            );
        }
        let mut states = BTreeSet::new();
        for (index, state) in self.states.iter().enumerate() {
            check_text(
                state,
                MAX_NAME_BYTES,
                &format!("states[{index}]"),
                &mut errors,
            );
            if !states.insert(state.as_str()) {
                issue(
                    &mut errors,
                    format!("states[{index}]"),
                    "duplicate state".into(),
                );
            }
        }
        if !empty && !states.contains(self.initial.as_str()) {
            issue(
                &mut errors,
                "initial".into(),
                "entry state is not declared".into(),
            );
        }
        let mut vars = BTreeMap::new();
        for (index, var) in self.vars.iter().enumerate() {
            check_text(
                &var.name,
                MAX_NAME_BYTES,
                &format!("vars[{index}].name"),
                &mut errors,
            );
            if vars.insert(var.name.as_str(), var).is_some() {
                issue(
                    &mut errors,
                    format!("vars[{index}].name"),
                    "duplicate variable".into(),
                );
            }
            if !var.initial.finite_and_bounded() {
                issue(
                    &mut errors,
                    format!("vars[{index}].initial"),
                    "number is non-finite or outside the bounded range".into(),
                );
            }
            let correct_kind = var.kind == var.initial.kind()
                || (var.kind == VarKind::Timer && matches!(var.initial, Value::Int(_)));
            if !correct_kind {
                issue(
                    &mut errors,
                    format!("vars[{index}].initial"),
                    "initial value type does not match variable kind".into(),
                );
            }
            if var.kind == VarKind::Timer
                && matches!(var.initial, Value::Int(value) if !(0..=MAX_TIMER_FRAMES).contains(&value))
            {
                issue(
                    &mut errors,
                    format!("vars[{index}].initial"),
                    "timer must be between zero and the frame limit".into(),
                );
            }
        }
        let mut node_budget = MAX_EXPR_NODES;
        // Status phase roots are bound by their owning state's installed callbacks.
        let mut bound_transitions: BTreeSet<usize> = self
            .status_bindings
            .iter()
            .flat_map(|entry| entry.binding.roots.values().flatten().copied())
            .collect();
        let mut bound_hooks = BTreeSet::new();
        let mut event_bindings = BTreeMap::new();
        let mut event_transition_owners = BTreeMap::new();
        for (index, binding) in self.bindings.iter().enumerate() {
            if !bound_hooks.insert(binding.hook) {
                issue(
                    &mut errors,
                    format!("bindings[{index}]"),
                    "hook is bound more than once".into(),
                );
            }
            if binding.transitions.is_empty() {
                issue(
                    &mut errors,
                    format!("bindings[{index}]"),
                    "binding has no transitions".into(),
                );
            }
            if binding.transitions.len() > MAX_TRANSITIONS {
                issue(
                    &mut errors,
                    format!("bindings[{index}].transitions"),
                    format!("must contain at most {MAX_TRANSITIONS} transition indices"),
                );
                continue;
            }
            if binding.transitions.iter().any(|transition| {
                *transition >= self.transitions.len() || !bound_transitions.insert(*transition)
            }) {
                issue(
                    &mut errors,
                    format!("bindings[{index}]"),
                    "transition index is out of range or bound more than once".into(),
                );
            }
        }
        for (index, binding) in self
            .event_bindings
            .iter()
            .take(MAX_EVENT_BINDINGS)
            .enumerate()
        {
            let item = format!("event_bindings[{index}]");
            if binding.fields.len() > MAX_EVENT_FIELDS {
                issue(
                    &mut errors,
                    format!("{item}.fields"),
                    format!("must contain at most {MAX_EVENT_FIELDS} fields"),
                );
                continue;
            }
            if binding.transitions.len() > MAX_TRANSITIONS {
                issue(
                    &mut errors,
                    format!("{item}.transitions"),
                    format!("must contain at most {MAX_TRANSITIONS} transition indices"),
                );
                continue;
            }
            if !event_label_valid(&binding.name) {
                issue(
                    &mut errors,
                    format!("{item}.name"),
                    "event name must be non-empty, bounded, NUL-free, and not whitespace-only"
                        .into(),
                );
            }
            if event_bindings
                .insert(binding.name.as_str(), binding)
                .is_some()
            {
                issue(
                    &mut errors,
                    item.clone(),
                    "event name is bound more than once".into(),
                );
            }
            let mut fields = BTreeSet::new();
            for (field_index, field) in binding.fields.iter().enumerate() {
                let field_item = format!("{item}.fields[{field_index}]");
                if !event_label_valid(&field.name) {
                    issue(
                        &mut errors,
                        field_item.clone(),
                        "event field name must be non-empty, bounded, NUL-free, and not whitespace-only"
                            .into(),
                    );
                }
                if !fields.insert(field.name.as_str()) {
                    issue(
                        &mut errors,
                        field_item.clone(),
                        "duplicate event field".into(),
                    );
                }
                if field.kind == VarKind::Timer {
                    issue(
                        &mut errors,
                        field_item,
                        "event fields cannot use the timer kind".into(),
                    );
                }
            }
            if binding.transitions.is_empty() {
                issue(
                    &mut errors,
                    item.clone(),
                    "binding has no transitions".into(),
                );
            }
            let mut invalid_transition = false;
            for transition in &binding.transitions {
                if *transition >= self.transitions.len() || !bound_transitions.insert(*transition) {
                    invalid_transition = true;
                } else {
                    event_transition_owners.insert(*transition, binding);
                }
            }
            if invalid_transition {
                issue(
                    &mut errors,
                    item,
                    "transition index is out of range or bound more than once".into(),
                );
            }
        }
        if bound_transitions.len() != self.transitions.len() {
            issue(
                &mut errors,
                "bindings".into(),
                "every transition must be bound exactly once".into(),
            );
        }
        let mut status_by_hook: BTreeMap<HookName, Vec<&StatusTrigger>> = BTreeMap::new();
        let mut selected_status_transitions = BTreeSet::new();
        for trigger in self.status_triggers.iter().take(MAX_BINDINGS) {
            status_by_hook
                .entry(trigger.hook)
                .or_default()
                .push(trigger);
        }
        let resource_by_hook: BTreeMap<_, _> = self
            .resource_triggers
            .iter()
            .take(MAX_BINDINGS)
            .map(|v| (v.hook, v))
            .collect();
        let duplicate_status = self
            .status_triggers
            .iter()
            .take(MAX_BINDINGS)
            .enumerate()
            .any(|(index, trigger)| {
                self.status_triggers[..index].iter().any(|prior| {
                    prior.hook == trigger.hook
                        && (!matches!(
                            trigger.hook,
                            HookName::OnStatusEnter | HookName::OnStatusExit
                        ) || prior.symbol == trigger.symbol)
                })
            });
        if duplicate_status || resource_by_hook.len() != self.resource_triggers.len() {
            issue(
                &mut errors,
                "triggers".into(),
                "a hook has duplicate triggers".into(),
            );
        }
        if status_by_hook
            .keys()
            .any(|hook| resource_by_hook.contains_key(hook))
        {
            issue(
                &mut errors,
                "triggers".into(),
                "a hook cannot have both status and resource triggers".into(),
            );
        }
        for trigger in self.status_triggers.iter().take(MAX_BINDINGS) {
            check_text(
                &trigger.symbol,
                MAX_SYMBOL_BYTES,
                "status trigger symbol",
                &mut errors,
            );
            if !matches!(
                trigger.hook,
                HookName::OnSpend | HookName::OnStatusEnter | HookName::OnStatusExit
            ) {
                issue(
                    &mut errors,
                    format!("status trigger {}", trigger.hook.wire_name()),
                    "this hook does not accept a status trigger".into(),
                );
            }
            if trigger.hook == HookName::OnSpend && !trigger.transitions.is_empty() {
                issue(
                    &mut errors,
                    format!("status trigger {}", trigger.hook.wire_name()),
                    "transition selection is supported only for onStatusEnter/onStatusExit".into(),
                );
            }
            if matches!(
                trigger.hook,
                HookName::OnStatusEnter | HookName::OnStatusExit
            ) {
                if trigger.transitions.len() > MAX_TRANSITIONS {
                    issue(
                        &mut errors,
                        format!("status trigger {}", trigger.hook.wire_name()),
                        format!("transition selection exceeds {MAX_TRANSITIONS} entries"),
                    );
                    continue;
                }
                if trigger.transitions.is_empty() {
                    issue(
                        &mut errors,
                        format!("status trigger {}", trigger.hook.wire_name()),
                        format!(
                            "{} must select at least one transition",
                            trigger.hook.wire_name()
                        ),
                    );
                }
                let binding = self
                    .bindings
                    .iter()
                    .find(|binding| binding.hook == trigger.hook);
                for index in &trigger.transitions {
                    if *index >= self.transitions.len()
                        || !binding.is_some_and(|binding| {
                            binding.transitions.len() <= MAX_TRANSITIONS
                                && binding.transitions.contains(index)
                        })
                    {
                        issue(
                            &mut errors,
                            format!("status trigger {}", trigger.hook.wire_name()),
                            format!(
                                "transition index {index} is not in the {} binding",
                                trigger.hook.wire_name()
                            ),
                        );
                    } else if !selected_status_transitions.insert(*index) {
                        issue(
                            &mut errors,
                            format!("status trigger {}", trigger.hook.wire_name()),
                            format!("transition index {index} is selected by more than one status"),
                        );
                    } else if !self.transitions[*index].on_frame {
                        issue(
                            &mut errors,
                            format!("status trigger {}", trigger.hook.wire_name()),
                            format!("transition index {index} must declare an event boundary"),
                        );
                    }
                }
            }
            if !bound_hooks.contains(&trigger.hook) {
                issue(
                    &mut errors,
                    format!("status trigger {}", trigger.hook.wire_name()),
                    "trigger has no matching hook binding".into(),
                );
            }
        }
        for hook in [HookName::OnStatusEnter, HookName::OnStatusExit] {
            if let Some(binding) = self.bindings.iter().find(|binding| binding.hook == hook) {
                for index in binding.transitions.iter().take(MAX_TRANSITIONS) {
                    if !selected_status_transitions.contains(index) {
                        issue(
                            &mut errors,
                            format!("binding {}", hook.wire_name()),
                            format!("transition index {index} has no exact status trigger"),
                        );
                    }
                }
            }
        }
        for trigger in self.resource_triggers.iter().take(MAX_BINDINGS) {
            if trigger.hook != trigger.source.hook() || trigger.source.fighter() != self.fighter {
                issue(
                    &mut errors,
                    format!("resource trigger {}", trigger.hook.wire_name()),
                    "resource source does not match the fighter and hook".into(),
                );
            }
            if !bound_hooks.contains(&trigger.hook) {
                issue(
                    &mut errors,
                    format!("resource trigger {}", trigger.hook.wire_name()),
                    "trigger has no matching hook binding".into(),
                );
            }
        }
        if resource_by_hook.contains_key(&HookName::OnTimeout)
            && self.vars.iter().any(|var| var.kind == VarKind::Timer)
        {
            issue(
                &mut errors,
                "resource trigger onTimeout".into(),
                "native Cloud expiry cannot share onTimeout with authored timer variables; use onChargeTick guards for timer state".into(),
            );
        }
        for (index, binding) in self.bindings.iter().enumerate() {
            if let Some(symbol) = &binding.article {
                if !matches!(
                    binding.hook,
                    HookName::OnArticleSpawn | HookName::OnArticleRemoved
                ) || !NativeInput::article_count(symbol).valid_for(&self.fighter)
                {
                    issue(
                        &mut errors,
                        format!("bindings[{index}].article"),
                        "article filter needs an exact owned article and lifecycle hook".into(),
                    );
                }
            }
            let supported = match binding.hook {
                HookName::OnDealDamage
                | HookName::OnTakeDamage
                | HookName::OnAttackShielded
                | HookName::OnShieldHit
                | HookName::OnIncomingHit
                | HookName::OnReflect
                | HookName::OnAbsorb
                | HookName::OnItemPickup
                | HookName::OnItemThrown
                | HookName::OnGrab
                | HookName::OnThrow
                | HookName::OnGrabbed
                | HookName::OnChargeTick
                | HookName::OnTimeout
                | HookName::OnSwitch
                | HookName::OnStockStart
                | HookName::OnKo
                | HookName::OnArticleSpawn
                | HookName::OnArticleRemoved => true,
                HookName::OnSpend => {
                    status_by_hook.contains_key(&binding.hook)
                        || resource_by_hook.contains_key(&binding.hook)
                }
                HookName::OnStatusEnter | HookName::OnStatusExit => {
                    status_by_hook.contains_key(&binding.hook)
                }
            };
            if !supported {
                issue(
                    &mut errors,
                    format!("bindings[{index}]"),
                    "hook needs an exact status or built-in resource trigger".into(),
                );
            }
        }
        if self.status_bindings.len() > MAX_STATES {
            issue(
                &mut errors,
                "status bindings".into(),
                "too many status bindings".into(),
            );
        } else if let Err(reason) = super::status_model::validate(
            Some(&self.fighter),
            &self.initial,
            self.status_bindings
                .iter()
                .map(|s| (s.state.as_str(), &s.binding)),
            &self
                .transitions
                .iter()
                .map(|t| (t.from.as_str(), t.to.as_str()))
                .collect::<Vec<_>>(),
            &self
                .bindings
                .iter()
                .flat_map(|b| b.transitions.iter().copied())
                .chain(
                    self.event_bindings
                        .iter()
                        .flat_map(|b| b.transitions.iter().copied()),
                )
                .chain(
                    self.status_triggers
                        .iter()
                        .flat_map(|b| b.transitions.iter().copied()),
                )
                .collect(),
        ) {
            issue(&mut errors, "status bindings".into(), reason);
        }
        if let Err(reason) =
            super::status_model::validate_allocations(&self.fighter, &self.custom_statuses)
        {
            issue(&mut errors, "custom statuses".into(), reason);
        }
        for entry in &self.status_bindings {
            if let Err(reason) = entry.binding.target.in_manifest(&self.custom_statuses) {
                issue(&mut errors, "status binding".into(), reason);
            }
            if entry.binding.policy == super::status_model::Policy::Owned {
                for phase in [
                    super::status_model::Phase::Pre,
                    super::status_model::Phase::MainEnter,
                ] {
                    let roots = entry.binding.roots.get(&phase);
                    let valid_last = roots.and_then(|r| r.last()).and_then(|i| self.transitions.get(*i)).is_some_and(|t| {
                        matches!(&t.guard, Predicate::Compare { op: CmpOp::Eq, left: Expr::Const(a), right: Expr::Const(b) } if a == b)
                        && t.actions.iter().filter(|a| matches!((phase, a),
                            (super::status_model::Phase::Pre, Action::Status(super::status_commands::Command::InitializeStatus(_))) |
                            (super::status_model::Phase::MainEnter, Action::Status(super::status_commands::Command::PlayMotion { .. })))).count() == 1
                    });
                    if !valid_last {
                        issue(
                            &mut errors,
                            format!("status {:?}", entry.state),
                            format!(
                                "Owned {phase:?} needs an unconditional final initialization rule"
                            ),
                        );
                    }
                }
            }
            if !self.states.contains(&entry.state) {
                issue(
                    &mut errors,
                    "status bindings".into(),
                    "status owner is not a declared state".into(),
                );
            }
        }
        for (index, transition) in self.transitions.iter().take(MAX_TRANSITIONS).enumerate() {
            let status_phase =
                super::status_model::phase(self.status_bindings.iter().map(|s| &s.binding), index);
            if status_phase.is_none()
                && transition_uses_reads(transition, false, false, false, true, false)
            {
                issue(
                    &mut errors,
                    format!("transitions[{index}]"),
                    "cancel observations require an installed status phase root".into(),
                );
            }
            if status_phase == Some(super::status_model::Phase::Pre)
                && transition_uses_input(transition)
            {
                issue(
                    &mut errors,
                    format!("transitions[{index}]"),
                    "Pre cannot read controller input or motion/status phase observations".into(),
                );
            }
            if let Some(phase) = status_phase {
                if transition
                    .actions
                    .iter()
                    .any(|a| !super::status_model::permits(phase, a))
                {
                    issue(
                        &mut errors,
                        format!("transitions[{index}]"),
                        format!("action is not permitted in status {phase:?}"),
                    );
                }
            }
            for (action_index, action) in transition.actions.iter().enumerate() {
                if let Action::Status(command) = action {
                    if matches!(
                        command,
                        super::status_commands::Command::TryCommonTransition { .. }
                    ) && (transition.actions.len() != 1 || transition.from != transition.to)
                    {
                        issue(
                            &mut errors,
                            format!("transitions[{index}]"),
                            "common transition check must be its own self-edge rule".into(),
                        );
                    }
                    if status_phase.is_none()
                        && !matches!(
                            command,
                            super::status_commands::Command::RequestStatus { .. }
                        )
                    {
                        issue(
                            &mut errors,
                            format!("transitions[{index}]"),
                            "status command requires an exclusive installed phase root".into(),
                        );
                    }
                    if let super::status_commands::Command::RequestStatus { target }
                    | super::status_commands::Command::ChangeStatusNow { symbol: target } =
                        command
                    {
                        if let Err(reason) = target.in_manifest(&self.custom_statuses) {
                            issue(&mut errors, format!("transitions[{index}]"), reason);
                        }
                    }
                    if let Err(reason) = super::status_commands::validate(command, &self.fighter) {
                        issue(&mut errors, format!("transitions[{index}]"), reason);
                    }
                }
                if status_phase.is_some()
                    && matches!(
                        action,
                        Action::RequestFighterStatus { .. }
                            | Action::Status(
                                super::status_commands::Command::ChangeStatusNow { .. }
                                    | super::status_commands::Command::RequestStatus { .. }
                            )
                    )
                    && action_index + 1 != transition.actions.len()
                {
                    issue(
                        &mut errors,
                        format!("transitions[{index}]"),
                        "terminal status action must be last in its rule".into(),
                    );
                }
            }
            let event = event_transition_owners.get(&index).copied();
            let owners: Vec<_> = self
                .bindings
                .iter()
                .filter(|binding| binding.transitions.contains(&index))
                .map(|binding| binding.hook)
                .collect();
            if transition_uses_reads(transition, false, false, false, false, true)
                && (status_phase.is_some()
                    || !transition.on_frame
                    || owners.iter().any(|hook| *hook != HookName::OnChargeTick))
            {
                issue(
                    &mut errors,
                    format!("transitions[{index}]"),
                    "command inputs require a Main frame tick".into(),
                );
            }
            if transition_uses_input(transition)
                && status_phase.is_none()
                && ((!transition.on_frame
                    && transition_uses_phase(transition)
                    && !owners.iter().all(|hook| *hook == HookName::OnIncomingHit))
                    || owners.is_empty()
                    || owners.iter().any(|hook| {
                        *hook != HookName::OnChargeTick
                            && *hook != HookName::OnIncomingHit
                            && (!matches!(
                                hook,
                                HookName::OnItemPickup
                                    | HookName::OnItemThrown
                                    | HookName::OnGrab
                                    | HookName::OnThrow
                                    | HookName::OnGrabbed
                            ) || transition_uses_reads(
                                transition, false, false, false, false, false,
                            ))
                    })
                    || event.is_some()
                    || self
                        .status_triggers
                        .iter()
                        .any(|b| b.transitions.contains(&index)))
            {
                issue(
                    &mut errors,
                    format!("transitions[{index}]"),
                    "controller input requires an exclusive onChargeTick frame-tick binding; phase reads also allow a grab/throw event boundary".into(),
                );
            }
            let damage = !owners.is_empty()
                && owners.iter().all(|hook| {
                    matches!(
                        hook,
                        HookName::OnDealDamage
                            | HookName::OnTakeDamage
                            | HookName::OnAttackShielded
                            | HookName::OnShieldHit
                            | HookName::OnIncomingHit
                            | HookName::OnReflect
                            | HookName::OnAbsorb
                    )
                })
                && event.is_none()
                && !self
                    .event_bindings
                    .iter()
                    .any(|binding| binding.transitions.contains(&index))
                && !self.status_triggers.iter().any(|trigger| {
                    trigger.transitions.contains(&index) || owners.contains(&trigger.hook)
                })
                && !self
                    .resource_triggers
                    .iter()
                    .any(|trigger| owners.contains(&trigger.hook));
            if transition
                .actions
                .iter()
                .any(|a| matches!(a, Action::CancelIncomingHit))
                && !(damage && owners.iter().all(|hook| *hook == HookName::OnIncomingHit))
            {
                issue(
                    &mut errors,
                    format!("transitions[{index}]"),
                    "CancelIncomingHit requires an exclusive onIncomingHit root".into(),
                );
            }
            let event_owner = ReadScope { event, damage };
            check_text(
                &transition.from,
                MAX_NAME_BYTES,
                &format!("transitions[{index}].from"),
                &mut errors,
            );
            check_text(
                &transition.to,
                MAX_NAME_BYTES,
                &format!("transitions[{index}].to"),
                &mut errors,
            );
            if !states.contains(transition.from.as_str())
                || !states.contains(transition.to.as_str())
            {
                issue(
                    &mut errors,
                    format!("transitions[{index}]"),
                    "from/to state is not declared".into(),
                );
            }
            if transition.actions.len() > MAX_ACTIONS_PER_TRANSITION {
                issue(
                    &mut errors,
                    format!("transitions[{index}].actions"),
                    "too many actions".into(),
                );
            }
            let mut phase_guards = vec![&transition.guard];
            for action in &transition.actions {
                match action {
                    Action::SetFlag { value, .. } | Action::SetNativeFlag { value, .. } => {
                        phase_guards.push(value)
                    }
                    Action::EmitEventValues { arguments, .. } => {
                        for a in arguments {
                            if let EventValue::Flag(p) = &a.value {
                                phase_guards.push(p);
                            }
                        }
                    }
                    _ => {}
                }
            }
            let mut phase_budget = MAX_EXPR_NODES;
            while let Some(p) = phase_guards.pop() {
                if phase_budget == 0 {
                    break;
                }
                phase_budget -= 1;
                match p {
                    Predicate::InStatus { symbol } | Predicate::TermEnabled { symbol }
                        if symbol.len() <= MAX_SYMBOL_BYTES
                            && self.fighter.len() <= MAX_FIGHTER_BYTES =>
                    {
                        if let Err(reason) = super::registry::resolve(
                            &self.fighter,
                            symbol,
                            if matches!(p, Predicate::TermEnabled { .. }) {
                                super::registry::IdentifierKind::TransitionTerm
                            } else {
                                super::registry::IdentifierKind::FighterStatus
                            },
                        ) {
                            issue(
                                &mut errors,
                                format!("transitions[{index}]"),
                                reason.to_string(),
                            );
                        }
                    }
                    Predicate::Not(p) => phase_guards.push(p),
                    Predicate::And(a, b) | Predicate::Or(a, b) => {
                        phase_guards.push(a);
                        phase_guards.push(b);
                    }
                    _ => {}
                }
            }
            validate_predicate(
                &transition.guard,
                &vars,
                event_owner,
                1,
                &mut node_budget,
                &mut errors,
                &format!("transitions[{index}].guard"),
            );
            for (action_index, action) in transition
                .actions
                .iter()
                .take(MAX_ACTIONS_PER_TRANSITION)
                .enumerate()
            {
                if let Action::SetNative { input, .. }
                | Action::AddNative { input, .. }
                | Action::SetNativeFlag { input, .. } = action
                {
                    let value = match action {
                        Action::SetNative {
                            value: Expr::Const(Value::Int(value)),
                            ..
                        } => Some(*value),
                        _ => None,
                    };
                    if let Err(reason) = input.validate_write(&self.fighter).and_then(|()| {
                        value.map_or(Ok(()), |value| input.validate_write_int(value))
                    }) {
                        issue(
                            &mut errors,
                            format!("transitions[{index}].actions[{action_index}]"),
                            reason,
                        );
                    }
                }
                if let Action::RequestFighterStatus { symbol } = action {
                    if symbol.len() <= MAX_SYMBOL_BYTES && self.fighter.len() <= MAX_FIGHTER_BYTES {
                        if let Err(error) = super::registry::resolve(
                            &self.fighter,
                            symbol,
                            super::registry::IdentifierKind::FighterStatus,
                        ) {
                            issue(
                                &mut errors,
                                format!("transitions[{index}].actions[{action_index}]"),
                                error.to_string(),
                            );
                        }
                    }
                }
                if let Action::Resource(command) = action {
                    if let Err(reason) = command.check_owner(&self.fighter) {
                        issue(
                            &mut errors,
                            format!("transitions[{index}].actions[{action_index}]"),
                            reason,
                        );
                    }
                }
                if let Action::ChooseHeroMenuSpell { slot } = action {
                    if let Err(reason) =
                        (super::hero_menu_commands::Command::Choose { slot: *slot })
                            .validate(&self.fighter)
                    {
                        issue(
                            &mut errors,
                            format!("transitions[{index}].actions[{action_index}]"),
                            reason,
                        );
                    }
                }
                validate_action(
                    action,
                    &vars,
                    event_owner,
                    &mut node_budget,
                    &mut errors,
                    &format!("transitions[{index}].actions[{action_index}]"),
                    &event_bindings,
                );
            }
        }
        validate_native_inputs(self, &mut errors);
        validate_instant_cycles(self, &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Evaluate installed roots once, using the same bounded transition evaluator.
    pub fn step_status_phase(
        &self,
        state: &mut LiveState,
        owner: &str,
        phase: super::status_model::Phase,
        snapshot: &NativeSnapshot,
    ) -> Result<StepResult, EvaluationError> {
        use super::status_model::Policy;
        let entry = self
            .status_bindings
            .iter()
            .find(|b| b.state == owner)
            .ok_or_else(|| EvaluationError("status owner is missing".into()))?;
        let roots = if entry.binding.policy == Policy::Owned {
            entry
                .binding
                .roots
                .get(&phase)
                .map_or(&[][..], Vec::as_slice)
        } else {
            &[]
        };
        self.step_status_roots(state, owner, phase, roots, true, snapshot)
    }

    /// Continue after an effectful common check without advancing timers twice.
    #[allow(clippy::too_many_arguments)]
    pub fn step_status_roots(
        &self,
        state: &mut LiveState,
        owner: &str,
        phase: super::status_model::Phase,
        roots: &[usize],
        first: bool,
        snapshot: &NativeSnapshot,
    ) -> Result<StepResult, EvaluationError> {
        use super::status_model::Phase;
        let mut staged = state.clone();
        if phase == Phase::Pre {
            staged.state = owner.into();
        }
        // A MainLoop exit transition may already have left the owner; teardown then runs no roots.
        let left = staged.state != owner;
        if left && !matches!(phase, Phase::End | Phase::Exit) {
            return Err(EvaluationError("status chart left its owning state".into()));
        }
        let roots = if left { &[][..] } else { roots };
        let result = self.step_inner(
            &mut staged,
            None,
            (first && phase == Phase::MainLoop).then(BTreeSet::new),
            Some(roots),
            None,
            snapshot,
        )?;
        if phase == Phase::Exit {
            staged.state = self.initial.clone();
        }
        *state = staged;
        Ok(result)
    }

    pub fn initial_state(&self) -> LiveState {
        LiveState {
            random_seed: 1,
            state: self.initial.clone(),
            values: self
                .vars
                .iter()
                .map(|var| (var.name.clone(), var.initial))
                .collect(),
        }
    }

    /// Hook-specific evaluation used by the game plugin outside the frame pass.
    #[allow(dead_code)] // The plugin uses this for non-frame dispatcher events.
    pub fn step(
        &self,
        state: &mut LiveState,
        hook: HookName,
    ) -> Result<StepResult, EvaluationError> {
        self.step_with_snapshot(state, hook, &NativeSnapshot::empty())
    }

    pub fn step_with_snapshot(
        &self,
        state: &mut LiveState,
        hook: HookName,
        snapshot: &NativeSnapshot,
    ) -> Result<StepResult, EvaluationError> {
        if matches!(
            hook,
            HookName::OnDealDamage
                | HookName::OnTakeDamage
                | HookName::OnAttackShielded
                | HookName::OnShieldHit
                | HookName::OnIncomingHit
                | HookName::OnReflect
                | HookName::OnAbsorb
        ) {
            return Err(EvaluationError(
                "damage hooks require an explicit pending-damage input".into(),
            ));
        }
        if matches!(hook, HookName::OnStatusEnter | HookName::OnStatusExit) {
            return Err(EvaluationError(format!(
                "{} requires an exact status trigger",
                hook.wire_name()
            )));
        }
        self.step_inner(
            state,
            Some(hook),
            None,
            None,
            None,
            &snapshot.without_damage(),
        )
    }

    #[allow(dead_code)] // Called by the typed controller adapter once its native gate passes.
    pub fn step_damage_with_snapshot(
        &self,
        state: &mut LiveState,
        input: super::native_inputs::DamageInput,
        snapshot: &NativeSnapshot,
    ) -> Result<StepResult, EvaluationError> {
        let hook = match input.direction() {
            super::native_inputs::DamageDirection::Deal => HookName::OnDealDamage,
            super::native_inputs::DamageDirection::Take => HookName::OnTakeDamage,
            super::native_inputs::DamageDirection::AttackShielded => HookName::OnAttackShielded,
            super::native_inputs::DamageDirection::ShieldHit => HookName::OnShieldHit,
            super::native_inputs::DamageDirection::Incoming => HookName::OnIncomingHit,
            super::native_inputs::DamageDirection::Reflect => HookName::OnReflect,
            super::native_inputs::DamageDirection::Absorb => HookName::OnAbsorb,
        };
        self.step_inner(
            state,
            Some(hook),
            None,
            None,
            None,
            &snapshot.with_damage(input),
        )
    }

    /// Evaluate one externally dispatched named event. The receiver is the
    /// root of this step (depth zero, counted against the event budget), then
    /// any events it emits drain through the normal FIFO queue. This is not a
    /// hook or frame tick, so timers are not decremented here.
    #[allow(dead_code)] // Called by the plugin's timed ACMD event bridge.
    pub fn step_event(
        &self,
        state: &mut LiveState,
        name: &str,
    ) -> Result<StepResult, EvaluationError> {
        self.step_event_with_snapshot(state, name, &NativeSnapshot::empty())
    }

    pub fn step_event_with_snapshot(
        &self,
        state: &mut LiveState,
        name: &str,
        snapshot: &NativeSnapshot,
    ) -> Result<StepResult, EvaluationError> {
        let binding = find_event_binding(self, name)?;
        if !binding.fields.is_empty() {
            return Err(EvaluationError(format!(
                "named event {name:?} requires typed arguments"
            )));
        }
        let mut seen = BTreeSet::new();
        if binding.transitions.is_empty()
            || binding
                .transitions
                .iter()
                .any(|index| *index >= self.transitions.len() || !seen.insert(*index))
        {
            return Err(EvaluationError(format!(
                "named event {name:?} has invalid or duplicate transition selections"
            )));
        }
        self.step_inner(
            state,
            None,
            None,
            None,
            Some((name, &[])),
            &snapshot.without_damage(),
        )
    }

    /// Preflight an exact named root event with constant typed scalar values.
    /// The borrowed field/value pairs must use the receiver's declaration order.
    pub fn validate_named_event_values(
        &self,
        name: &str,
        arguments: &[(&str, Value)],
    ) -> Result<(), EvaluationError> {
        let binding = find_event_binding(self, name)?;
        copy_named_event_values(binding, arguments)?;
        validate_event_transition_selection(binding, self.transitions.len())
    }

    /// Deliver one typed root event. This is separate from the legacy
    /// zero-payload method, which continues to reject typed receivers.
    pub fn step_event_values_with_snapshot(
        &self,
        state: &mut LiveState,
        name: &str,
        arguments: &[(&str, Value)],
        snapshot: &NativeSnapshot,
    ) -> Result<StepResult, EvaluationError> {
        let binding = find_event_binding(self, name)?;
        let payload = copy_named_event_values(binding, arguments)?;
        validate_event_transition_selection(binding, self.transitions.len())?;
        self.step_inner(
            state,
            None,
            None,
            None,
            Some((name, payload.as_slice())),
            &snapshot.without_damage(),
        )
    }

    /// Evaluate only transitions attached to the exact authored status symbol.
    /// Generic hook dispatch cannot invoke this path without selecting a
    /// registered status trigger first.
    #[allow(dead_code)] // The live plugin dispatches this over the shared payload.
    pub fn step_status(
        &self,
        state: &mut LiveState,
        symbol: &str,
    ) -> Result<StepResult, EvaluationError> {
        let trigger = self.find_status_trigger(HookName::OnStatusEnter, symbol)?;
        self.step_exact_status_transitions(HookName::OnStatusEnter, state, &trigger.transitions)
    }

    fn find_status_trigger(
        &self,
        hook: HookName,
        symbol: &str,
    ) -> Result<&StatusTrigger, EvaluationError> {
        if self.status_triggers.len() > MAX_BINDINGS || symbol.len() > MAX_SYMBOL_BYTES {
            return Err(EvaluationError(
                "status trigger lookup exceeds its limit".into(),
            ));
        }
        let mut found = None;
        for trigger in self.status_triggers.iter().take(MAX_BINDINGS) {
            if trigger.hook == hook && trigger.symbol == symbol {
                if found.is_some() {
                    return Err(EvaluationError("duplicate exact status trigger".into()));
                }
                if trigger.transitions.len() > MAX_TRANSITIONS {
                    return Err(EvaluationError(
                        "status trigger transition selection exceeds its limit".into(),
                    ));
                }
                found = Some(trigger);
            }
        }
        found.ok_or_else(|| EvaluationError("unbound exact status trigger".into()))
    }

    /// Evaluate an exact Enter or Exit transition union in binding order.
    /// Symbols resolving to the same native status therefore fire atomically
    /// in serialized binding order.
    pub fn step_exact_status_transitions(
        &self,
        hook: HookName,
        state: &mut LiveState,
        transitions: &[usize],
    ) -> Result<StepResult, EvaluationError> {
        self.step_exact_status_transitions_with_snapshot(
            hook,
            state,
            transitions,
            &NativeSnapshot::empty(),
        )
    }

    pub fn step_exact_status_transitions_with_snapshot(
        &self,
        hook: HookName,
        state: &mut LiveState,
        transitions: &[usize],
        snapshot: &NativeSnapshot,
    ) -> Result<StepResult, EvaluationError> {
        if !matches!(hook, HookName::OnStatusEnter | HookName::OnStatusExit) {
            return Err(EvaluationError("expected an exact status-edge hook".into()));
        }
        if self.status_triggers.len() > MAX_BINDINGS
            || self.bindings.len() > MAX_BINDINGS
            || self.transitions.len() > MAX_TRANSITIONS
            || transitions.len() > MAX_TRANSITIONS
        {
            return Err(EvaluationError(
                "status dispatch table or selection exceeds its limit".into(),
            ));
        }
        if transitions.is_empty() {
            return Err(EvaluationError(format!(
                "{} requires selected transitions",
                hook.wire_name()
            )));
        }
        let mut registered = BTreeSet::new();
        for trigger in self.status_triggers.iter().take(MAX_BINDINGS) {
            if trigger.transitions.len() > MAX_TRANSITIONS {
                return Err(EvaluationError(
                    "status trigger transition selection exceeds its limit".into(),
                ));
            }
            if trigger.hook == hook {
                registered.extend(trigger.transitions.iter().copied());
            }
        }
        let mut seen = BTreeSet::new();
        if transitions
            .iter()
            .any(|index| !registered.contains(index) || !seen.insert(*index))
        {
            return Err(EvaluationError(format!(
                "{} selected transitions are not uniquely registered",
                hook.wire_name()
            )));
        }
        let binding_order = self
            .bindings
            .iter()
            .find(|binding| binding.hook == hook)
            .map(|binding| binding.transitions.as_slice())
            .ok_or_else(|| EvaluationError(format!("{} has no hook binding", hook.wire_name())))?;
        if binding_order.len() > MAX_TRANSITIONS {
            return Err(EvaluationError(
                "status hook transition binding exceeds its limit".into(),
            ));
        }
        let selected: BTreeSet<_> = transitions.iter().copied().collect();
        let ordered: Vec<_> = binding_order
            .iter()
            .filter(|index| selected.contains(index))
            .copied()
            .collect();
        if ordered.len() != selected.len() {
            return Err(EvaluationError(format!(
                "{} selection is outside its hook binding",
                hook.wire_name()
            )));
        }
        self.step_inner(
            state,
            Some(hook),
            None,
            Some(&ordered),
            None,
            &snapshot.without_damage(),
        )
    }

    /// One actual per-frame pass: decrement timers first, evaluate ChargeTick,
    /// then route one synthetic Timeout for newly-expired timers unless the
    /// tick binding refreshed one. The state is committed only on success.
    pub fn step_frame(&self, state: &mut LiveState) -> Result<StepResult, EvaluationError> {
        self.step_frame_with_snapshot(state, &NativeSnapshot::empty())
    }

    pub fn step_frame_with_snapshot(
        &self,
        state: &mut LiveState,
        snapshot: &NativeSnapshot,
    ) -> Result<StepResult, EvaluationError> {
        self.step_inner(
            state,
            Some(HookName::OnChargeTick),
            Some(BTreeSet::new()),
            None,
            None,
            &snapshot.without_damage(),
        )
    }

    fn step_inner(
        &self,
        state: &mut LiveState,
        hook: Option<HookName>,
        frame_mode: Option<BTreeSet<String>>,
        transition_filter: Option<&[usize]>,
        event_root: Option<(&str, &[Value])>,
        snapshot: &NativeSnapshot,
    ) -> Result<StepResult, EvaluationError> {
        if self.vars.len() > MAX_VARIABLES
            || self.native_inputs.len() > MAX_NATIVE_INPUTS
            || self.transitions.len() > MAX_TRANSITIONS
            || self.bindings.len() > MAX_BINDINGS
            || self.event_bindings.len() > MAX_EVENT_BINDINGS
            || self.status_triggers.len() > MAX_BINDINGS
            || self.resource_triggers.len() > MAX_BINDINGS
            || self
                .bindings
                .iter()
                .any(|binding| binding.transitions.len() > MAX_TRANSITIONS)
            || self
                .event_bindings
                .iter()
                .any(|binding| binding.transitions.len() > MAX_TRANSITIONS)
            || self
                .transitions
                .iter()
                .any(|transition| transition.actions.len() > MAX_ACTIONS_PER_TRANSITION)
        {
            return Err(EvaluationError(
                "live behavior dispatch table exceeds a runtime limit".into(),
            ));
        }
        for source in &self.native_inputs {
            if !source.is_supported() || !source.valid_for(&self.fighter) {
                return Err(EvaluationError(format!(
                    "native input {:?} is not eligible for {:?}",
                    source, self.fighter
                )));
            }
            snapshot.require(source).map_err(EvaluationError)?;
        }
        if let Some(phase) = &snapshot.phase {
            phase.validate().map_err(EvaluationError)?;
            if phase.fighter != self.fighter {
                return Err(EvaluationError(
                    "phase snapshot belongs to another fighter".into(),
                ));
            }
        }
        let snapshot = snapshot.clone();
        snapshot.random.set(state.random_seed);
        let snapshot = &snapshot;
        let before = state.snapshot();
        let mut staged = state.clone();
        let mut expired = BTreeSet::new();
        if frame_mode.is_some() {
            for var in &self.vars {
                if var.kind == VarKind::Timer {
                    if let Some(Value::Int(remaining)) = staged.values.get(&var.name).copied() {
                        if remaining > 0 {
                            let next = remaining - 1;
                            staged.values.insert(var.name.clone(), Value::Int(next));
                            if next == 0 {
                                expired.insert(var.name.clone());
                            }
                        }
                    }
                }
            }
        }
        let mut fired = Vec::new();
        let mut event_queue = EventQueue::default();
        if let Some((name, values)) = event_root {
            let binding = find_event_binding(self, name)?;
            let payload = validate_event_values(binding, values)?;
            event_queue.enqueued = 1;
            event_queue.pending.push_back(QueuedEvent {
                name: name.to_owned(),
                depth: 0,
                payload,
            });
        }
        if let Some(transitions) = transition_filter {
            self.eval_transitions(
                &mut staged,
                transitions,
                None,
                &mut fired,
                &mut event_queue,
                0,
                None,
                snapshot,
            )?;
        } else if let Some(hook) = hook {
            self.eval_binding(
                &mut staged,
                hook,
                None,
                &mut fired,
                &mut event_queue,
                0,
                snapshot,
            )?;
        }
        if frame_mode.is_some()
            && transition_filter.is_none()
            && !expired.is_empty()
            && !self
                .resource_triggers
                .iter()
                .any(|trigger| trigger.hook == HookName::OnTimeout)
            && self.bindings.iter().any(|b| b.hook == HookName::OnTimeout)
        {
            let still_expired: BTreeSet<_> = expired.iter().filter(|name| matches!(staged.values.get(*name), Some(Value::Int(value)) if *value <= 0)).cloned().collect();
            if !still_expired.is_empty() {
                self.eval_binding(
                    &mut staged,
                    HookName::OnTimeout,
                    Some(&still_expired),
                    &mut fired,
                    &mut event_queue,
                    0,
                    snapshot,
                )?;
            }
        }
        let event_snapshot = snapshot.without_damage();
        while let Some(event) = event_queue.pending.pop_front() {
            let binding = find_event_binding(self, &event.name)?;
            validate_event_values(binding, event.payload.as_slice())?;
            let event_context = EventContext {
                event: &event.name,
                fields: &binding.fields,
                values: event.payload.as_slice(),
            };
            validate_event_context(&event_context)?;
            self.eval_transitions(
                &mut staged,
                &binding.transitions,
                None,
                &mut fired,
                &mut event_queue,
                event.depth,
                Some(&event_context),
                &event_snapshot,
            )?;
        }
        staged.random_seed = snapshot.random.get();
        let after = staged.snapshot();
        *state = staged;
        let engine_actions = fired
            .iter()
            .flat_map(|transition| transition.engine_actions.iter())
            .cloned()
            .collect();
        Ok(StepResult {
            before,
            after,
            fired,
            expired: expired.into_iter().collect(),
            engine_actions,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_binding(
        &self,
        state: &mut LiveState,
        hook: HookName,
        expired: Option<&BTreeSet<String>>,
        fired: &mut Vec<FiredTransition>,
        event_queue: &mut EventQueue,
        event_depth: usize,
        snapshot: &NativeSnapshot,
    ) -> Result<(), EvaluationError> {
        let Some(binding) = self.bindings.iter().find(|binding| binding.hook == hook) else {
            return Ok(());
        };
        if binding.article.as_ref().is_some_and(|symbol| {
            snapshot.article_event.as_ref()
                != Some(&(hook == HookName::OnArticleSpawn, symbol.clone()))
        }) {
            return Ok(());
        }
        self.eval_transitions(
            state,
            &binding.transitions,
            expired,
            fired,
            event_queue,
            event_depth,
            None,
            snapshot,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_transitions(
        &self,
        state: &mut LiveState,
        transitions: &[usize],
        expired: Option<&BTreeSet<String>>,
        fired: &mut Vec<FiredTransition>,
        event_queue: &mut EventQueue,
        event_depth: usize,
        event_context: Option<&EventContext<'_>>,
        snapshot: &NativeSnapshot,
    ) -> Result<(), EvaluationError> {
        for index in transitions {
            let transition = self.transitions.get(*index).ok_or_else(|| {
                EvaluationError("binding transition index changed after validation".into())
            })?;
            if expired.is_some_and(|set| !predicate_mentions_expired(&transition.guard, set)) {
                continue;
            }
            if state.state != transition.from
                || !eval_predicate(&transition.guard, &state.values, event_context, snapshot)?
            {
                continue;
            }
            let mut engine_actions = Vec::new();
            for action in &transition.actions {
                match action {
                    Action::Status(command) => {
                        let command = command.try_map(|e| {
                            eval_expr_with_snapshot(e, &state.values, event_context, snapshot)
                                .map(Expr::Const)
                        })?;
                        super::status_commands::validate(&command, &self.fighter)
                            .map_err(EvaluationError)?;
                        engine_actions.push(Action::Status(command));
                    }
                    Action::CombatPolicy { scope, op } => {
                        super::native_inputs::validate_policy_scope(*scope)
                            .map_err(EvaluationError)?;
                        let op = op.try_map(|e| {
                            match eval_expr_with_snapshot(
                                e,
                                &state.values,
                                event_context,
                                snapshot,
                            )? {
                                Value::Float(value) if phase_scalar_valid(action, value) => {
                                    Ok(Expr::Const(Value::Float(value)))
                                }
                                _ => Err(EvaluationError(
                                    "combat policy value is outside its bounds".into(),
                                )),
                            }
                        })?;
                        engine_actions.push(Action::CombatPolicy { scope: *scope, op });
                    }
                    Action::AddHeldOpponentDamage { amount: rate }
                    | Action::AddDamage { amount: rate }
                    | Action::SetMotionRate { rate }
                    | Action::AddSpeed { x: rate, .. } => {
                        let resolve = |expr: &Expr| -> Result<Expr, EvaluationError> {
                            let Value::Float(value) = eval_expr_with_snapshot(
                                expr,
                                &state.values,
                                event_context,
                                snapshot,
                            )?
                            else {
                                return Err(EvaluationError(
                                    "native scalar actions require Float expressions".into(),
                                ));
                            };
                            if !phase_scalar_valid(action, value) {
                                return Err(EvaluationError(
                                    "native scalar action is outside its bounds".into(),
                                ));
                            }
                            Ok(Expr::Const(Value::Float(value)))
                        };
                        engine_actions.push(match action {
                            Action::AddSpeed { x, y } => Action::AddSpeed {
                                x: resolve(x)?,
                                y: resolve(y)?,
                            },
                            Action::AddHeldOpponentDamage { amount } => {
                                Action::AddHeldOpponentDamage {
                                    amount: resolve(amount)?,
                                }
                            }
                            Action::AddDamage { amount } => Action::AddDamage {
                                amount: resolve(amount)?,
                            },
                            _ => Action::SetMotionRate {
                                rate: resolve(rate)?,
                            },
                        });
                    }
                    Action::Object { symbol, op } => {
                        let op = op.try_map(|expr| match eval_expr_with_snapshot(expr, &state.values, event_context, snapshot)? {
                            Value::Float(value) if value.is_finite() && value.abs() <= super::native_inputs::ObjectOp::<Expr>::MAX_SPEED => Ok(Expr::Const(Value::Float(value))),
                            _ => Err(EvaluationError("object speed requires a Float within ±10".into())),
                        })?;
                        engine_actions.push(Action::Object {
                            symbol: symbol.clone(),
                            op,
                        });
                    }
                    Action::SetNative { input, value } | Action::AddNative { input, value } => {
                        let result =
                            eval_expr_with_snapshot(value, &state.values, event_context, snapshot)?;
                        let value = Expr::Const(result);
                        engine_actions.push(if matches!(action, Action::AddNative { .. }) {
                            Action::AddNative {
                                input: input.clone(),
                                value,
                            }
                        } else {
                            Action::SetNative {
                                input: input.clone(),
                                value,
                            }
                        });
                    }
                    Action::SetNativeFlag { input, value } => {
                        let result = eval_predicate(value, &state.values, event_context, snapshot)?;
                        engine_actions.push(Action::SetNativeFlag {
                            input: input.clone(),
                            value: Predicate::Compare {
                                op: CmpOp::Eq,
                                left: Expr::Const(Value::Int(i32::from(result))),
                                right: Expr::Const(Value::Int(1)),
                            },
                        });
                    }
                    Action::Resource(command) => {
                        let resolved = command.try_map(|e| {
                            match eval_expr_with_snapshot(
                                e,
                                &state.values,
                                event_context,
                                snapshot,
                            )? {
                                Value::Float(v) => Ok(v),
                                _ => Err(EvaluationError(format!(
                                    "{} needs a Float amount",
                                    command.label()
                                ))),
                            }
                        })?;
                        resolved.validate(&self.fighter).map_err(EvaluationError)?;
                        engine_actions.push(Action::Resource(command.try_map(|_| {
                            Ok::<_, EvaluationError>(Expr::Const(Value::Float(resolved.amount)))
                        })?));
                    }
                    Action::ThrowHeldOpponent { .. }
                    | Action::ReleaseHeldOpponent
                    | Action::CancelIncomingHit
                    | Action::Movement(_)
                    | Action::Item(_)
                    | Action::MeshVisibility { .. }
                    | Action::ChooseHeroMenuSpell { .. }
                    | Action::GenerateArticle { .. }
                    | Action::RemoveArticle { .. }
                    | Action::RemoveNewestArticle { .. }
                    | Action::ArticleMovement { .. }
                    | Action::RequestArticleStatus { .. }
                    | Action::RequestFighterStatus { .. }
                    | Action::RequestWeaponStatus { .. } => {
                        engine_actions.push(action.clone());
                    }
                    _ => apply_action(
                        self,
                        state,
                        action,
                        event_queue,
                        event_depth,
                        event_context,
                        snapshot,
                    )?,
                }
            }
            let phase =
                super::status_model::phase(self.status_bindings.iter().map(|b| &b.binding), *index);
            let destination = self
                .status_bindings
                .iter()
                .find(|b| b.state == transition.to && transition.to != transition.from);
            let requests_status = engine_actions.iter().any(status_terminal);
            if let Some(destination) = destination {
                if !requests_status {
                    engine_actions.push(if phase == Some(super::status_model::Phase::MainLoop) {
                        Action::Status(super::status_commands::Command::ChangeStatusNow {
                            symbol: destination.binding.target.clone(),
                        })
                    } else {
                        Action::Status(super::status_commands::Command::RequestStatus {
                            target: destination.binding.target.clone(),
                        })
                    });
                }
            }
            let terminal = phase.is_some()
                && (engine_actions.iter().any(status_terminal)
                    || engine_actions.iter().any(|a| {
                        matches!(
                            (phase, a),
                            (
                                Some(super::status_model::Phase::Pre),
                                Action::Status(super::status_commands::Command::InitializeStatus(
                                    _
                                ))
                            ) | (
                                Some(super::status_model::Phase::MainEnter),
                                Action::Status(super::status_commands::Command::PlayMotion { .. })
                            )
                        )
                    }));
            let from = state.state.clone();
            // Native entry, rather than an unaccepted request, selects a bound state.
            if destination.is_none() {
                state.state = transition.to.clone();
            }
            fired.push(FiredTransition {
                index: *index,
                from,
                to: transition.to.clone(),
                actions: transition.actions.clone(),
                engine_actions,
            });
            if terminal {
                break;
            }
        }
        Ok(())
    }
}

fn status_terminal(action: &Action) -> bool {
    matches!(
        action,
        Action::RequestFighterStatus { .. }
            | Action::Status(
                super::status_commands::Command::ChangeStatusNow { .. }
                    | super::status_commands::Command::RequestStatus { .. }
            )
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveState {
    pub random_seed: u64,
    pub state: String,
    pub values: BTreeMap<String, Value>,
}

impl LiveState {
    pub fn snapshot(&self) -> StepSnapshot {
        StepSnapshot {
            state: self.state.clone(),
            values: self.values.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepSnapshot {
    pub state: String,
    pub values: BTreeMap<String, Value>,
}

#[derive(Default)]
struct EventQueue {
    pending: VecDeque<QueuedEvent>,
    enqueued: usize,
}

struct QueuedEvent {
    name: String,
    depth: usize,
    payload: InlineEventPayload,
}

#[derive(Clone, Copy)]
struct InlineEventPayload {
    values: [Value; MAX_EVENT_FIELDS],
    len: usize,
}

impl InlineEventPayload {
    fn empty() -> Self {
        Self {
            values: [Value::Flag(false); MAX_EVENT_FIELDS],
            len: 0,
        }
    }

    fn as_slice(&self) -> &[Value] {
        &self.values[..self.len]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FiredTransition {
    pub index: usize,
    pub from: String,
    pub to: String,
    pub actions: Vec<Action>,
    /// Engine effects with scalar arguments resolved at their action position.
    pub engine_actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub before: StepSnapshot,
    pub after: StepSnapshot,
    pub fired: Vec<FiredTransition>,
    pub expired: Vec<String>,
    /// Ordered engine operations requested by fired transitions. The pure
    /// evaluator reports these but never performs them.
    pub engine_actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationError(pub String);

impl std::fmt::Display for EvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EvaluationError {}

fn check_text(text: &str, max: usize, item: &str, errors: &mut Vec<ModelError>) {
    if text.is_empty() || text.len() > max || text.bytes().any(|byte| byte == 0) {
        errors.push(ModelError {
            item: item.into(),
            reason: format!("must be non-empty and at most {max} bytes"),
        });
    }
}

fn event_label_valid(text: &str) -> bool {
    text.len() <= MAX_NAME_BYTES && !text.trim().is_empty() && !text.as_bytes().contains(&0)
}

fn validate_native_inputs(program: &LiveProgram, errors: &mut Vec<ModelError>) {
    if program.native_inputs.len() > MAX_NATIVE_INPUTS {
        return;
    }
    for source in program.damage_status_selectors() {
        if !source.reference_is_bounded() || !source.valid_for(&program.fighter) {
            errors.push(ModelError {
                item: "contact status".into(),
                reason: "contact status selector is invalid or belongs to another receiver fighter"
                    .into(),
            });
        }
    }
    let mut declared = BTreeSet::new();
    for (index, source) in program.native_inputs.iter().enumerate() {
        let item = format!("native_inputs[{index}]");
        if !source.reference_is_bounded() {
            errors.push(ModelError {
                item: item.clone(),
                reason: "native input reference is empty or too long".into(),
            });
        }
        if !source.is_supported() {
            errors.push(ModelError {
                item: item.clone(),
                reason: format!("native input {:?} is unsupported", source),
            });
        } else if !source.valid_for(&program.fighter) {
            errors.push(ModelError {
                item: item.clone(),
                reason: format!(
                    "native input {:?} does not belong to {:?}",
                    source, program.fighter
                ),
            });
        }
        if !declared.insert(source.canonical()) {
            errors.push(ModelError {
                item,
                reason: "duplicate native input declaration".into(),
            });
        }
    }

    let mut used = BTreeSet::new();
    for transition in program.transitions.iter().take(MAX_TRANSITIONS) {
        collect_predicate_native_inputs(&transition.guard, &mut used, 0);
        for action in transition.actions.iter().take(MAX_ACTIONS_PER_TRANSITION) {
            for operand in action.operands() {
                match operand {
                    Operand::Expr(expr) => collect_expr_native_inputs(expr, &mut used, 0),
                    Operand::Predicate(predicate) => {
                        collect_predicate_native_inputs(predicate, &mut used, 0)
                    }
                }
            }
        }
    }
    for source in &used {
        if !source.is_supported() || !source.valid_for(&program.fighter) {
            errors.push(ModelError {
                item: "native input reference".into(),
                reason: format!(
                    "native input {:?} is not eligible for {:?}",
                    source, program.fighter
                ),
            });
        }
        if !declared.contains(source) {
            errors.push(ModelError {
                item: "native input reference".into(),
                reason: format!("native input {:?} has no program declaration", source),
            });
        }
    }
    for source in declared.difference(&used) {
        errors.push(ModelError {
            item: "native_inputs".into(),
            reason: format!("native input {:?} is declared but unused", source),
        });
    }
}

impl LiveProgram {
    fn damage_status_selectors(&self) -> Vec<&NativeInput> {
        let mut result = Vec::new();
        let mut pending = Vec::new();
        for transition in self.transitions.iter().take(MAX_TRANSITIONS) {
            pending.push((&transition.guard, 1));
            for action in transition.actions.iter().take(MAX_ACTIONS_PER_TRANSITION) {
                match action {
                    Action::SetNativeFlag { value, .. } | Action::SetFlag { value, .. } => {
                        pending.push((value, 1))
                    }
                    Action::EmitEventValues { arguments, .. } => {
                        for argument in arguments.iter().take(MAX_EVENT_FIELDS) {
                            if let EventValue::Flag(value) = &argument.value {
                                pending.push((value, 1));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut remaining = MAX_EXPR_NODES.saturating_mul(MAX_TRANSITIONS);
        while let Some((predicate, depth)) = pending.pop() {
            if remaining == 0 {
                break;
            }
            remaining -= 1;
            if depth > MAX_EXPR_DEPTH {
                continue;
            }
            match predicate {
                Predicate::DamageStatusIs(source) => result.push(source),
                Predicate::Not(inner) => pending.push((inner, depth + 1)),
                Predicate::And(left, right) | Predicate::Or(left, right) => {
                    pending.push((left, depth + 1));
                    pending.push((right, depth + 1));
                }
                _ => {}
            }
        }
        result
    }

    pub fn required_native_features(&self) -> BTreeSet<&'static str> {
        let mut result = self
            .native_inputs
            .iter()
            .filter_map(NativeInput::required_feature)
            .collect::<BTreeSet<_>>();
        if !self.status_bindings.is_empty() {
            result.insert(super::status_model::FEATURE);
            result.insert(super::status_model::CANCEL_FEATURE);
        }
        if !self.custom_statuses.is_empty()
            || self.transitions.iter().any(|t| {
                t.actions.iter().any(|a| {
                    matches!(
                        a,
                        Action::Status(
                            super::status_commands::Command::TransitionTerm { .. }
                                | super::status_commands::Command::TransitionGroup { .. }
                                | super::status_commands::Command::EnableCancel
                                | super::status_commands::Command::TryCommonTransition { .. }
                                | super::status_commands::Command::RequestStatus { .. }
                        )
                    )
                })
            })
        {
            result.insert(super::status_model::CANCEL_FEATURE);
        }
        if self.bindings.iter().any(|b| {
            matches!(
                b.hook,
                HookName::OnGrab | HookName::OnThrow | HookName::OnGrabbed
            )
        }) {
            result.insert(super::native_inputs::GRAB_EVENTS_FEATURE);
        }
        if self
            .bindings
            .iter()
            .any(|b| matches!(b.hook, HookName::OnItemPickup | HookName::OnItemThrown))
        {
            result.insert(super::native_inputs::ITEM_EVENTS_FEATURE);
        }
        if self
            .bindings
            .iter()
            .any(|b| matches!(b.hook, HookName::OnReflect | HookName::OnAbsorb))
        {
            result.insert(super::native_inputs::REFLECT_ABSORB_FEATURE);
        }
        if self
            .bindings
            .iter()
            .any(|b| b.hook == HookName::OnIncomingHit)
        {
            result.insert(super::native_inputs::INCOMING_HIT_FEATURE);
        }
        if self.transitions.iter().any(transition_uses_phase) {
            result.insert(super::native_inputs::STATUS_PHASE_FEATURE);
        }
        if self.transitions.iter().flat_map(|t| &t.actions).any(|a| {
            matches!(
                a,
                Action::AddHeldOpponentDamage { .. }
                    | Action::ThrowHeldOpponent { .. }
                    | Action::ReleaseHeldOpponent
            )
        }) {
            result.insert(super::native_inputs::HELD_OPPONENT_FEATURE);
        }
        if self
            .transitions
            .iter()
            .flat_map(|t| &t.actions)
            .any(|a| matches!(a, Action::Resource(_)))
        {
            result.insert(super::resource_commands::FEATURE);
        }
        if self
            .transitions
            .iter()
            .flat_map(|t| &t.actions)
            .any(|a| matches!(a, Action::ChooseHeroMenuSpell { .. }))
        {
            result.insert(super::hero_menu_commands::FEATURE);
        }
        if self
            .transitions
            .iter()
            .flat_map(|t| &t.actions)
            .any(|a| matches!(a, Action::Movement(_) | Action::MeshVisibility { .. }))
        {
            result.insert(super::native_inputs::MOVEMENT_FEATURE);
        }
        if self
            .transitions
            .iter()
            .flat_map(|t| &t.actions)
            .any(|a| matches!(a, Action::Item(_)))
        {
            result.insert(super::native_inputs::ITEMS_FEATURE);
        }
        if !self.damage_status_selectors().is_empty() {
            result.insert(super::native_inputs::DAMAGE_CAPTURE_STATUS_FEATURE);
        }
        if self
            .transitions
            .iter()
            .take(MAX_TRANSITIONS)
            .flat_map(|transition| transition.actions.iter().take(MAX_ACTIONS_PER_TRANSITION))
            .any(|action| {
                matches!(action, Action::Resource(command) if command.resource.entry().writer == super::resource_commands::Writer::CloudLimit)
            })
        {
            result.insert(super::native_inputs::CLOUD_LIMIT_ACTIONS_FEATURE);
        }
        if self
            .bindings
            .iter()
            .any(|binding| binding.article.is_some() || binding.hook == HookName::OnArticleRemoved)
            || self.transitions.iter().any(|transition| {
                transition.actions.iter().any(|action| {
                    matches!(
                        action,
                        Action::RequestArticleStatus { .. }
                            | Action::RemoveNewestArticle { .. }
                            | Action::ArticleMovement { .. }
                            | Action::Object { .. }
                    )
                })
            })
        {
            result.insert(super::native_inputs::OWNED_ARTICLES_FEATURE);
        }
        result
    }
}

fn collect_expr_native_inputs(expr: &Expr, used: &mut BTreeSet<NativeInput>, depth: usize) {
    if depth > MAX_EXPR_DEPTH {
        return;
    }
    match expr {
        Expr::NativeFloat(source) => {
            used.insert(source.canonical());
        }
        Expr::NativeInt(source) => {
            used.insert(source.canonical());
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            collect_expr_native_inputs(left, used, depth + 1);
            collect_expr_native_inputs(right, used, depth + 1);
        }
        Expr::PendingDamage
        | Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. }
        | Expr::Const(_)
        | Expr::Var(_)
        | Expr::TimerRemaining(_)
        | Expr::EventField { .. } => {}
    }
}

fn collect_predicate_native_inputs(
    predicate: &Predicate,
    used: &mut BTreeSet<NativeInput>,
    depth: usize,
) {
    if depth > MAX_EXPR_DEPTH {
        return;
    }
    match predicate {
        Predicate::Compare { left, right, .. } => {
            collect_expr_native_inputs(left, used, depth + 1);
            collect_expr_native_inputs(right, used, depth + 1);
        }
        Predicate::NativeFlag(source) => {
            used.insert(source.canonical());
        }
        Predicate::Not(inner) => collect_predicate_native_inputs(inner, used, depth + 1),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            collect_predicate_native_inputs(left, used, depth + 1);
            collect_predicate_native_inputs(right, used, depth + 1);
        }
        Predicate::DamageIsProjectile
        | Predicate::InStatus { .. }
        | Predicate::InMotion { .. }
        | Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::TermEnabled { .. }
        | Predicate::Situation { .. }
        | Predicate::Button { .. }
        | Predicate::Flag(_)
        | Predicate::EventFlag { .. }
        | Predicate::Expired(_)
        | Predicate::DamageStatusIs(_) => {}
    }
}

fn validate_expr(
    expr: &Expr,
    vars: &BTreeMap<&str, &VarDef>,
    event_owner: ReadScope<'_>,
    depth: usize,
    budget: &mut usize,
    errors: &mut Vec<ModelError>,
    item: &str,
) -> Option<VarKind> {
    if depth > MAX_EXPR_DEPTH || *budget == 0 {
        errors.push(ModelError {
            item: item.into(),
            reason: "expression depth or node budget exceeded".into(),
        });
        return None;
    }
    *budget -= 1;
    match expr {
        Expr::StatusFrame => Some(VarKind::Int),
        Expr::MotionFrame | Expr::StickX { .. } | Expr::StickY => Some(VarKind::Float),
        Expr::RandomInt { min, max } => {
            if !super::native_inputs::random_int_valid(*min, *max) {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "random integer requires bounded min <= max and span <= 1000000".into(),
                });
            }
            Some(VarKind::Int)
        }
        Expr::RandomFloat { min, max } => {
            if !super::native_inputs::random_float_valid(*min, *max) {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "random float requires finite bounded min <= max".into(),
                });
            }
            Some(VarKind::Float)
        }
        Expr::PendingDamage => {
            if !event_owner.damage {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "pending damage requires an exclusively damage/shield contact root transition"
                        .into(),
                });
            }
            Some(VarKind::Float)
        }
        Expr::Const(value) => {
            if !value.finite_and_bounded() {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "constant is non-finite or outside the bounded range".into(),
                });
            }
            Some(value.kind())
        }
        Expr::Var(name) => {
            check_text(name, MAX_NAME_BYTES, item, errors);
            vars.get(name.as_str()).map(|var| var.kind).or_else(|| {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("unknown variable {name:?}"),
                });
                None
            })
        }
        Expr::TimerRemaining(name) => {
            check_text(name, MAX_NAME_BYTES, item, errors);
            match vars.get(name.as_str()).map(|var| var.kind) {
                Some(VarKind::Timer) => Some(VarKind::Int),
                Some(kind) => {
                    errors.push(ModelError {
                        item: item.into(),
                        reason: format!("timer remaining requires a Timer variable, got {kind:?}"),
                    });
                    None
                }
                None => {
                    errors.push(ModelError {
                        item: item.into(),
                        reason: format!("unknown timer variable {name:?}"),
                    });
                    None
                }
            }
        }
        Expr::EventField { event, field } => {
            validate_event_reference(event, field, event_owner.event, false, errors, item)
        }
        Expr::NativeFloat(source) => {
            if source.kind() != Some(NativeKind::Float) {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("native input {source:?} is not a float source"),
                });
            }
            Some(VarKind::Float)
        }
        Expr::NativeInt(source) => {
            if source.kind() != Some(NativeKind::Int) {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("native input {source:?} is not an integer source"),
                });
            }
            Some(VarKind::Int)
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            let a = validate_expr(left, vars, event_owner, depth + 1, budget, errors, item);
            let b = validate_expr(right, vars, event_owner, depth + 1, budget, errors, item);
            match (a, b) {
                (Some(VarKind::Float), Some(VarKind::Float)) => Some(VarKind::Float),
                (Some(VarKind::Int), Some(VarKind::Int)) => Some(VarKind::Int),
                (Some(_), Some(_)) => {
                    errors.push(ModelError {
                        item: item.into(),
                        reason: "numeric operands have incompatible types".into(),
                    });
                    None
                }
                _ => None,
            }
        }
    }
}

fn validate_predicate(
    predicate: &Predicate,
    vars: &BTreeMap<&str, &VarDef>,
    event_owner: ReadScope<'_>,
    depth: usize,
    budget: &mut usize,
    errors: &mut Vec<ModelError>,
    item: &str,
) {
    if depth > MAX_EXPR_DEPTH || *budget == 0 {
        errors.push(ModelError {
            item: item.into(),
            reason: "predicate depth or node budget exceeded".into(),
        });
        return;
    }
    *budget -= 1;
    match predicate {
        Predicate::DamageIsProjectile => {
            if !event_owner.damage {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "projectile damage classification requires an exclusively damage/shield contact root transition".into(),
                });
            }
        }
        Predicate::InStatus { symbol } | Predicate::TermEnabled { symbol } => {
            check_text(symbol, MAX_SYMBOL_BYTES, item, errors);
        }
        Predicate::InMotion { motion } => {
            if !super::native_inputs::motion_name_valid(motion) {
                errors.push(ModelError {
                    item: item.to_string(),
                    reason: format!("{motion:?} is not a motion name"),
                });
            }
        }
        Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::Situation { .. }
        | Predicate::Button { .. } => {}
        Predicate::Flag(name) => {
            check_text(name, MAX_NAME_BYTES, item, errors);
            if !vars
                .get(name.as_str())
                .is_some_and(|var| var.kind == VarKind::Flag)
            {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("{name:?} is not a declared flag"),
                });
            }
        }
        Predicate::EventFlag { event, field } => {
            let _ = validate_event_reference(event, field, event_owner.event, true, errors, item);
        }
        Predicate::DamageStatusIs(source) => {
            if !matches!(source, NativeInput::FighterStatusIs { .. }) || !source.is_supported() {
                errors.push(ModelError { item: item.into(), reason: "contact status requires an exact registered receiver fighter-status selector".into() });
            }
            if !event_owner.damage {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "contact status requires an exclusively damage/shield contact root transition"
                        .into(),
                });
            }
        }
        Predicate::NativeFlag(source) => {
            if source.kind() != Some(NativeKind::Flag) {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("native input {source:?} is not a flag source"),
                });
            }
        }
        Predicate::Expired(name) => {
            check_text(name, MAX_NAME_BYTES, item, errors);
            if !vars
                .get(name.as_str())
                .is_some_and(|var| var.kind == VarKind::Timer)
            {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("{name:?} is not a declared timer"),
                });
            }
        }
        Predicate::Compare { left, right, .. } => {
            let left = validate_expr(left, vars, event_owner, depth + 1, budget, errors, item);
            let right = validate_expr(right, vars, event_owner, depth + 1, budget, errors, item);
            if left != right || matches!(left, Some(VarKind::Flag | VarKind::Timer)) {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "comparison values must have matching numeric types".into(),
                });
            }
        }
        Predicate::Not(inner) => {
            validate_predicate(inner, vars, event_owner, depth + 1, budget, errors, item)
        }
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            validate_predicate(left, vars, event_owner, depth + 1, budget, errors, item);
            validate_predicate(right, vars, event_owner, depth + 1, budget, errors, item);
        }
    }
}

fn validate_event_reference(
    event: &str,
    field: &str,
    event_owner: Option<&EventBinding>,
    flag_only: bool,
    errors: &mut Vec<ModelError>,
    item: &str,
) -> Option<VarKind> {
    if !event_label_valid(event) || !event_label_valid(field) {
        errors.push(ModelError {
            item: item.into(),
            reason: "event field reference must use bounded nonempty event and field names".into(),
        });
        return None;
    }
    let Some(owner) = event_owner else {
        errors.push(ModelError {
            item: item.into(),
            reason: "event field references are only available to named event receivers".into(),
        });
        return None;
    };
    if owner.name != event {
        errors.push(ModelError {
            item: item.into(),
            reason: "event field reference must match this transition's receiver".into(),
        });
        return None;
    }
    let Some(declaration) = owner
        .fields
        .iter()
        .find(|declaration| declaration.name == field)
    else {
        errors.push(ModelError {
            item: item.into(),
            reason: "event field reference is not declared by this receiver".into(),
        });
        return None;
    };
    let valid_kind = if flag_only {
        declaration.kind == VarKind::Flag
    } else {
        matches!(declaration.kind, VarKind::Float | VarKind::Int)
    };
    if !valid_kind {
        errors.push(ModelError {
            item: item.into(),
            reason: if flag_only {
                "event flag predicate requires a flag field".into()
            } else {
                "event numeric expression requires an integer or float field".into()
            },
        });
        None
    } else {
        Some(declaration.kind)
    }
}

fn validate_action(
    action: &Action,
    vars: &BTreeMap<&str, &VarDef>,
    event_owner: ReadScope<'_>,
    budget: &mut usize,
    errors: &mut Vec<ModelError>,
    item: &str,
    event_bindings: &BTreeMap<&str, &EventBinding>,
) {
    match action {
        Action::Status(command) => {
            for e in command.expressions() {
                if !matches!(
                    validate_expr(e, vars, event_owner, 1, budget, errors, item),
                    Some(VarKind::Float | VarKind::Int)
                ) {
                    errors.push(ModelError {
                        item: item.into(),
                        reason: "status arguments must be numeric".into(),
                    });
                }
            }
        }
        Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit => {}
        Action::Movement(op) => {
            if let Err(reason) = op.validate() {
                errors.push(ModelError {
                    item: item.to_string(),
                    reason,
                });
            }
        }
        Action::Resource(command) => {
            if validate_expr(&command.amount, vars, event_owner, 1, budget, errors, item)
                != Some(VarKind::Float)
            {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("{} needs a Float amount", command.label()),
                });
            }
        }
        Action::ChooseHeroMenuSpell { slot } => {
            if *slot >= super::hero_menu_commands::Command::SLOTS {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "Hero menu slot must be 0..=3".into(),
                });
            }
        }
        Action::Item(op) => {
            if let Err(reason) = op.validate() {
                errors.push(ModelError {
                    item: item.to_string(),
                    reason,
                });
            }
        }
        Action::MeshVisibility { mesh, .. } => {
            if let Err(reason) = super::native_inputs::validate_mesh_name(mesh) {
                errors.push(ModelError {
                    item: item.to_string(),
                    reason,
                });
            }
        }
        Action::AddHeldOpponentDamage { amount: rate }
        | Action::AddDamage { amount: rate }
        | Action::CombatPolicy {
            op:
                CombatOp::DamageTaken(rate)
                | CombatOp::KnockbackTaken(rate)
                | CombatOp::DamageDealt(rate)
                | CombatOp::KnockbackDealt(rate)
                | CombatOp::Stat(_, rate)
                | CombatOp::Armor(rate),
            ..
        }
        | Action::SetMotionRate { rate }
        | Action::AddSpeed { x: rate, .. } => {
            if let Action::CombatPolicy { scope, .. } = action {
                if let Err(reason) = super::native_inputs::validate_policy_scope(*scope) {
                    errors.push(ModelError {
                        item: item.into(),
                        reason,
                    });
                }
            }
            let expressions = match action {
                Action::AddSpeed { x, y } => vec![x, y],
                _ => vec![rate],
            };
            for value in expressions {
                if validate_expr(value, vars, event_owner, 1, budget, errors, item)
                    != Some(VarKind::Float)
                {
                    errors.push(ModelError {
                        item: item.into(),
                        reason: "native scalar actions require Float expressions".into(),
                    });
                }
                if let Expr::Const(Value::Float(v)) = value {
                    if !phase_scalar_valid(action, *v) {
                        errors.push(ModelError {
                            item: item.into(),
                            reason: "native scalar action is outside its bounds".into(),
                        });
                    }
                }
            }
        }
        Action::CombatPolicy { scope, .. } => {
            if let Err(reason) = super::native_inputs::validate_policy_scope(*scope) {
                errors.push(ModelError {
                    item: item.into(),
                    reason,
                });
            }
        }
        Action::SetNative { input, value } | Action::AddNative { input, value } => {
            let actual = validate_expr(value, vars, event_owner, 1, budget, errors, item);
            let expected = match input.kind() {
                Some(NativeKind::Float) => Some(VarKind::Float),
                Some(NativeKind::Int) => Some(VarKind::Int),
                _ => None,
            };
            if expected.is_none() || actual != expected {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "native write requires matching numeric types".into(),
                });
            }
        }
        Action::SetNativeFlag { input, value } => {
            validate_predicate(value, vars, event_owner, 1, budget, errors, item);
            if input.kind() != Some(NativeKind::Flag) {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "native flag write requires a Flag field".into(),
                });
            }
        }
        Action::Set { var, value } => {
            check_text(var, MAX_NAME_BYTES, item, errors);
            let kind = validate_expr(value, vars, event_owner, 1, budget, errors, item);
            let Some(target) = vars.get(var.as_str()) else {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("unknown target variable {var:?}"),
                });
                return;
            };
            if kind != Some(target.kind)
                && !(target.kind == VarKind::Timer && kind == Some(VarKind::Int))
            {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "set expression type does not match target".into(),
                });
            }
        }
        Action::SetFlag { var, value } => {
            check_text(var, MAX_NAME_BYTES, item, errors);
            validate_predicate(value, vars, event_owner, 1, budget, errors, item);
            if !vars
                .get(var.as_str())
                .is_some_and(|target| target.kind == VarKind::Flag)
            {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "computed flag assignment requires a Flag predicate and Flag target"
                        .into(),
                });
            }
        }
        Action::Add { var, value } => {
            check_text(var, MAX_NAME_BYTES, item, errors);
            let kind = validate_expr(value, vars, event_owner, 1, budget, errors, item);
            if !vars.get(var.as_str()).is_some_and(|target| {
                matches!(target.kind, VarKind::Float | VarKind::Int) && kind == Some(target.kind)
            }) {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "add requires matching numeric types".into(),
                });
            }
        }
        Action::Raise { var } | Action::Clear { var } => {
            check_text(var, MAX_NAME_BYTES, item, errors);
            if !vars
                .get(var.as_str())
                .is_some_and(|target| target.kind == VarKind::Flag)
            {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "flag action requires a flag target".into(),
                });
            }
        }
        Action::Reset { var } => {
            check_text(var, MAX_NAME_BYTES, item, errors);
            if !vars.contains_key(var.as_str()) {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("unknown target variable {var:?}"),
                });
            }
        }
        Action::GenerateArticle { symbol }
        | Action::RemoveArticle { symbol }
        | Action::RemoveNewestArticle { symbol }
        | Action::ArticleMovement { symbol, .. } => {
            check_text(symbol, MAX_SYMBOL_BYTES, item, errors)
        }
        Action::Object { symbol, op } => {
            check_text(symbol, MAX_SYMBOL_BYTES, item, errors);
            for e in op.expressions() {
                let bounded = match e {
                    Expr::Const(Value::Float(v)) => {
                        v.abs() <= super::native_inputs::ObjectOp::<Expr>::MAX_SPEED
                    }
                    _ => true,
                };
                if validate_expr(e, vars, event_owner, 1, budget, errors, item)
                    != Some(VarKind::Float)
                    || !bounded
                {
                    errors.push(ModelError {
                        item: item.into(),
                        reason: "object speed requires Float expressions within ±10".into(),
                    });
                }
            }
        }
        Action::RequestArticleStatus { symbol, status } => {
            check_text(symbol, MAX_SYMBOL_BYTES, item, errors);
            check_text(status, MAX_SYMBOL_BYTES, item, errors);
        }
        Action::RequestFighterStatus { symbol } => {
            check_text(symbol, MAX_SYMBOL_BYTES, item, errors);
        }
        Action::RequestWeaponStatus { target, symbol } => {
            if target.battle_object_id == 0 {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "weapon target must be nonzero".into(),
                });
            }
            check_text(symbol, MAX_SYMBOL_BYTES, item, errors);
        }
        Action::EmitEvent { name } => {
            if !event_label_valid(name) {
                errors.push(ModelError {
                    item: item.into(),
                    reason:
                        "event name must be non-empty, bounded, NUL-free, and not whitespace-only"
                            .into(),
                });
                return;
            }
            if let Some(binding) = event_bindings.get(name.as_str()) {
                if !binding.fields.is_empty() {
                    errors.push(ModelError {
                        item: item.into(),
                        reason: format!("event {name:?} requires typed arguments"),
                    });
                }
            } else {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "emits an unbound named event".into(),
                });
            }
        }
        Action::EmitEventArguments { name, arguments } => {
            if arguments.len() > MAX_EVENT_FIELDS {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("event arguments exceed {MAX_EVENT_FIELDS} fields"),
                });
                return;
            }
            if !event_label_valid(name) {
                errors.push(ModelError {
                    item: item.into(),
                    reason:
                        "event name must be non-empty, bounded, NUL-free, and not whitespace-only"
                            .into(),
                });
                return;
            }
            let Some(binding) = event_bindings.get(name.as_str()) else {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "emits an unbound named event".into(),
                });
                return;
            };
            if binding.fields.len() > MAX_EVENT_FIELDS {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("event schema exceeds {MAX_EVENT_FIELDS} fields"),
                });
                return;
            }
            if binding.fields.is_empty() {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "typed event arguments require a nonempty schema".into(),
                });
                return;
            }
            if arguments.len() != binding.fields.len() {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("event {name:?} argument count does not match its schema"),
                });
                return;
            }
            for (index, (argument, field)) in arguments.iter().zip(&binding.fields).enumerate() {
                let argument_item = format!("{item}.arguments[{index}]");
                check_text(&argument.field, MAX_NAME_BYTES, &argument_item, errors);
                if argument.field != field.name {
                    errors.push(ModelError {
                        item: argument_item.clone(),
                        reason: "event arguments must use declaration order and exact field names"
                            .into(),
                    });
                }
                if argument.value.kind() != field.kind || !argument.value.finite_and_bounded() {
                    errors.push(ModelError {
                        item: argument_item,
                        reason: "event argument type does not match its field or is outside the bounded range".into(),
                    });
                }
            }
        }
        Action::EmitEventValues { name, arguments } => {
            if arguments.len() > MAX_EVENT_FIELDS {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("event arguments exceed {MAX_EVENT_FIELDS} fields"),
                });
                return;
            }
            if !event_label_valid(name) {
                errors.push(ModelError {
                    item: item.into(),
                    reason:
                        "event name must be non-empty, bounded, NUL-free, and not whitespace-only"
                            .into(),
                });
                return;
            }
            let Some(binding) = event_bindings.get(name.as_str()) else {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "emits an unbound named event".into(),
                });
                return;
            };
            if binding.fields.len() > MAX_EVENT_FIELDS {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("event schema exceeds {MAX_EVENT_FIELDS} fields"),
                });
                return;
            }
            if binding.fields.is_empty() {
                errors.push(ModelError {
                    item: item.into(),
                    reason: "computed event arguments require a nonempty schema".into(),
                });
                return;
            }
            if arguments.len() != binding.fields.len() {
                errors.push(ModelError {
                    item: item.into(),
                    reason: format!("event {name:?} argument count does not match its schema"),
                });
                return;
            }
            for (index, (argument, field)) in arguments.iter().zip(&binding.fields).enumerate() {
                let argument_item = format!("{item}.arguments[{index}]");
                check_text(&argument.field, MAX_NAME_BYTES, &argument_item, errors);
                if argument.field != field.name {
                    errors.push(ModelError {
                        item: argument_item.clone(),
                        reason: "event arguments must use declaration order and exact field names"
                            .into(),
                    });
                }
                let kind = match &argument.value {
                    EventValue::Constant(value) => {
                        if !value.finite_and_bounded() {
                            errors.push(ModelError {
                                item: argument_item.clone(),
                                reason: "event constant is non-finite or outside the bounded range"
                                    .into(),
                            });
                        }
                        Some(value.kind())
                    }
                    EventValue::Numeric(expression) => {
                        let kind = validate_expr(
                            expression,
                            vars,
                            event_owner,
                            1,
                            budget,
                            errors,
                            &argument_item,
                        );
                        if !matches!(kind, Some(VarKind::Float | VarKind::Int)) {
                            errors.push(ModelError {
                                item: argument_item.clone(),
                                reason:
                                    "computed numeric argument must have an integer or float type"
                                        .into(),
                            });
                        }
                        kind
                    }
                    EventValue::Flag(predicate) => {
                        validate_predicate(
                            predicate,
                            vars,
                            event_owner,
                            1,
                            budget,
                            errors,
                            &argument_item,
                        );
                        Some(VarKind::Flag)
                    }
                };
                if kind != Some(field.kind) {
                    errors.push(ModelError {
                        item: argument_item,
                        reason: "event argument type does not match its field".into(),
                    });
                }
            }
        }
    }
}

fn validate_instant_cycles(program: &LiveProgram, errors: &mut Vec<ModelError>) {
    fn visit(
        state: usize,
        states: &[String],
        edges: &[Vec<(usize, bool)>],
        active: &mut Vec<usize>,
        done: &mut BTreeSet<usize>,
    ) -> bool {
        if active.contains(&state) {
            return true;
        }
        if !done.insert(state) {
            return false;
        }
        active.push(state);
        let cycle = edges[state]
            .iter()
            .any(|(to, on_frame)| !on_frame && visit(*to, states, edges, active, done));
        active.pop();
        let _ = states;
        cycle
    }
    let indices: BTreeMap<_, _> = program
        .states
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    let event_transitions: BTreeSet<usize> = program
        .event_bindings
        .iter()
        .take(MAX_EVENT_BINDINGS)
        .flat_map(|binding| binding.transitions.iter().take(MAX_TRANSITIONS).copied())
        .collect();
    let mut edges = vec![Vec::new(); program.states.len()];
    for (index, transition) in program.transitions.iter().take(MAX_TRANSITIONS).enumerate() {
        let emits_event = transition
            .actions
            .iter()
            .take(MAX_ACTIONS_PER_TRANSITION)
            .any(|action| {
                matches!(
                    action,
                    Action::EmitEvent { .. }
                        | Action::EmitEventArguments { .. }
                        | Action::EmitEventValues { .. }
                )
            });
        if !event_transitions.contains(&index)
            && !emits_event
            && super::status_model::phase(program.status_bindings.iter().map(|s| &s.binding), index)
                .is_none()
        {
            if let (Some(from), Some(to)) = (
                indices.get(transition.from.as_str()),
                indices.get(transition.to.as_str()),
            ) {
                edges[*from].push((*to, transition.on_frame));
            }
        }
    }
    for start in 0..edges.len() {
        if visit(
            start,
            &program.states,
            &edges,
            &mut Vec::new(),
            &mut BTreeSet::new(),
        ) {
            errors.push(ModelError {
                item: "transitions".into(),
                reason: "instant transitions contain a cycle without a frame edge".into(),
            });
            return;
        }
    }
}

fn eval_expr_with_snapshot(
    expr: &Expr,
    values: &BTreeMap<String, Value>,
    event_context: Option<&EventContext<'_>>,
    snapshot: &NativeSnapshot,
) -> Result<Value, EvaluationError> {
    match expr {
        Expr::StatusFrame | Expr::MotionFrame => {
            let phase = snapshot.phase.as_ref().ok_or_else(|| {
                EvaluationError("phase read requires a fresh frame snapshot".into())
            })?;
            phase.validate().map_err(EvaluationError)?;
            Ok(if matches!(expr, Expr::StatusFrame) {
                Value::Int(phase.status_frame)
            } else {
                Value::Float(phase.motion_frame)
            })
        }

        Expr::StickX { relative_to_facing } => {
            let input = snapshot.controller.as_ref().ok_or_else(|| {
                EvaluationError("controller input requires a fresh frame snapshot".into())
            })?;
            Ok(Value::Float(
                input.stick_x
                    * if *relative_to_facing {
                        input.facing
                    } else {
                        1.0
                    },
            ))
        }
        Expr::StickY => Ok(Value::Float(
            snapshot
                .controller
                .as_ref()
                .ok_or_else(|| {
                    EvaluationError("controller input requires a fresh frame snapshot".into())
                })?
                .stick_y,
        )),
        Expr::RandomInt { min, max } => {
            if !super::native_inputs::random_int_valid(*min, *max) {
                return Err(EvaluationError("invalid random integer bounds".into()));
            }
            Ok(Value::Int(
                (i64::from(*min)
                    + i64::from(super::controller_random(
                        &snapshot.random,
                        (i64::from(*max) - i64::from(*min) + 1) as i32,
                    ))) as i32,
            ))
        }
        Expr::RandomFloat { min, max } => {
            if !super::native_inputs::random_float_valid(*min, *max) {
                return Err(EvaluationError("invalid random float bounds".into()));
            }
            let fraction =
                f64::from(super::controller_random(&snapshot.random, 1_000_000)) / 999_999.0;
            Ok(Value::Float(
                (f64::from(*min) + (f64::from(*max) - f64::from(*min)) * fraction) as f32,
            ))
        }
        Expr::PendingDamage => snapshot
            .pending_damage()
            .map(Value::Float)
            .map_err(EvaluationError),
        Expr::Const(value) => Ok(*value),
        Expr::Var(name) => values
            .get(name)
            .copied()
            .ok_or_else(|| EvaluationError(format!("missing variable {name:?}"))),
        Expr::TimerRemaining(name) => match values.get(name).copied() {
            Some(Value::Int(remaining)) if (0..=MAX_TIMER_FRAMES).contains(&remaining) => {
                Ok(Value::Int(remaining))
            }
            Some(Value::Int(_)) => Err(EvaluationError(format!(
                "timer {name:?} is outside the bounded countdown range"
            ))),
            Some(_) => Err(EvaluationError(format!(
                "timer {name:?} is not stored as an integer"
            ))),
            None => Err(EvaluationError(format!("missing timer {name:?}"))),
        },
        Expr::EventField { event, field } => {
            let value = event_context_value(event_context, event, field)?;
            if matches!(value, Value::Float(_) | Value::Int(_)) {
                Ok(value)
            } else {
                Err(EvaluationError(
                    "event numeric field is not an integer or float".into(),
                ))
            }
        }
        Expr::NativeFloat(source) => snapshot
            .read(source)
            .map(Value::Float)
            .map_err(EvaluationError),
        Expr::NativeInt(source) => snapshot
            .read_int(source)
            .map(Value::Int)
            .map_err(EvaluationError),
        Expr::Add(a, b) => eval_numeric(
            a,
            b,
            values,
            event_context,
            snapshot,
            |x, y| x + y,
            i32::checked_add,
        ),
        Expr::Sub(a, b) => eval_numeric(
            a,
            b,
            values,
            event_context,
            snapshot,
            |x, y| x - y,
            i32::checked_sub,
        ),
        Expr::Mul(a, b) => eval_numeric(
            a,
            b,
            values,
            event_context,
            snapshot,
            |x, y| x * y,
            i32::checked_mul,
        ),
    }
}

fn eval_numeric(
    expr_a: &Expr,
    expr_b: &Expr,
    values: &BTreeMap<String, Value>,
    event_context: Option<&EventContext<'_>>,
    snapshot: &NativeSnapshot,
    float_op: impl FnOnce(f32, f32) -> f32,
    int_op: impl FnOnce(i32, i32) -> Option<i32>,
) -> Result<Value, EvaluationError> {
    match (
        eval_expr_with_snapshot(expr_a, values, event_context, snapshot)?,
        eval_expr_with_snapshot(expr_b, values, event_context, snapshot)?,
    ) {
        (Value::Float(a), Value::Float(b)) => {
            let result = float_op(a, b);
            if result.is_finite() && f64::from(result).abs() <= MAX_ABS_NUMBER {
                Ok(Value::Float(result))
            } else {
                Err(EvaluationError(
                    "float arithmetic left bounded range".into(),
                ))
            }
        }
        (Value::Int(a), Value::Int(b)) => int_op(a, b)
            .filter(|value| i64::from(*value).abs() <= MAX_ABS_NUMBER as i64)
            .map(Value::Int)
            .ok_or_else(|| EvaluationError("integer arithmetic overflow or out of range".into())),
        _ => Err(EvaluationError("numeric operands disagree".into())),
    }
}

fn eval_predicate(
    predicate: &Predicate,
    values: &BTreeMap<String, Value>,
    event_context: Option<&EventContext<'_>>,
    snapshot: &NativeSnapshot,
) -> Result<bool, EvaluationError> {
    match predicate {
        Predicate::InStatus { symbol } => snapshot
            .phase
            .as_ref()
            .ok_or_else(|| EvaluationError("status requires a fresh frame snapshot".into()))?
            .in_status(symbol)
            .map_err(EvaluationError),
        Predicate::InMotion { motion } => snapshot
            .phase
            .as_ref()
            .ok_or_else(|| EvaluationError("motion requires a fresh frame snapshot".into()))?
            .in_motion(motion)
            .map_err(EvaluationError),
        Predicate::SituationChanged => snapshot
            .situation_changed
            .ok_or_else(|| EvaluationError("situation-change observation is unavailable".into())),
        Predicate::CancelEnabled => snapshot
            .cancel_enabled
            .ok_or_else(|| EvaluationError("cancel observation is unavailable".into())),
        Predicate::TermEnabled { symbol } => snapshot
            .term_enabled
            .get(symbol)
            .copied()
            .ok_or_else(|| EvaluationError("transition term observation is unavailable".into())),
        Predicate::MotionEnded => snapshot
            .motion_ended
            .ok_or_else(|| EvaluationError("motion-end observation is unavailable".into())),
        Predicate::Situation { kind } => {
            let phase = snapshot.phase.as_ref().ok_or_else(|| {
                EvaluationError("situation requires a fresh frame snapshot".into())
            })?;
            phase.validate().map_err(EvaluationError)?;
            Ok(phase.situation == Some(*kind))
        }

        Predicate::DamageIsProjectile => snapshot.damage_is_projectile().map_err(EvaluationError),
        Predicate::Button { button, edge } => snapshot
            .controller
            .as_ref()
            .and_then(|input| input.buttons.get(&(*button, *edge)))
            .copied()
            .ok_or_else(|| {
                EvaluationError("button input requires a fresh complete frame snapshot".into())
            }),
        Predicate::Flag(name) => match values.get(name) {
            Some(Value::Flag(value)) => Ok(*value),
            _ => Err(EvaluationError(format!("{name:?} is not a flag"))),
        },
        Predicate::EventFlag { event, field } => {
            match event_context_value(event_context, event, field)? {
                Value::Flag(value) => Ok(value),
                _ => Err(EvaluationError(
                    "event predicate field is not a flag".into(),
                )),
            }
        }
        Predicate::NativeFlag(source) => snapshot.read_flag(source).map_err(EvaluationError),
        Predicate::DamageStatusIs(source) => {
            snapshot.damage_status_is(source).map_err(EvaluationError)
        }
        Predicate::Expired(name) => match values.get(name) {
            Some(Value::Int(value)) => Ok(*value <= 0),
            _ => Err(EvaluationError(format!("{name:?} is not an integer timer"))),
        },
        Predicate::Compare { op, left, right } => {
            match (
                eval_expr_with_snapshot(left, values, event_context, snapshot)?,
                eval_expr_with_snapshot(right, values, event_context, snapshot)?,
            ) {
                (Value::Float(a), Value::Float(b)) => Ok(cmp(*op, a.partial_cmp(&b))),
                (Value::Int(a), Value::Int(b)) => Ok(cmp(*op, a.partial_cmp(&b))),
                _ => Err(EvaluationError("comparison values disagree".into())),
            }
        }
        Predicate::Not(inner) => Ok(!eval_predicate(inner, values, event_context, snapshot)?),
        Predicate::And(left, right) => Ok(eval_predicate(left, values, event_context, snapshot)?
            && eval_predicate(right, values, event_context, snapshot)?),
        Predicate::Or(left, right) => Ok(eval_predicate(left, values, event_context, snapshot)?
            || eval_predicate(right, values, event_context, snapshot)?),
    }
}

fn event_context_value(
    event_context: Option<&EventContext<'_>>,
    event: &str,
    field: &str,
) -> Result<Value, EvaluationError> {
    if !event_label_valid(event) || !event_label_valid(field) {
        return Err(EvaluationError("event field reference is invalid".into()));
    }
    let context = event_context.ok_or_else(|| {
        EvaluationError("event field is unavailable outside named dispatch".into())
    })?;
    if context.event != event {
        return Err(EvaluationError(
            "event field reference does not match the active receiver".into(),
        ));
    }
    validate_event_context(context)?;
    let index = context
        .fields
        .iter()
        .position(|declaration| declaration.name == field)
        .ok_or_else(|| {
            EvaluationError("event field is not declared by the active receiver".into())
        })?;
    context
        .values
        .get(index)
        .copied()
        .ok_or_else(|| EvaluationError("event receiver payload is missing a declared field".into()))
}

fn validate_event_context(context: &EventContext<'_>) -> Result<(), EvaluationError> {
    if !event_label_valid(context.event)
        || context.fields.len() > MAX_EVENT_FIELDS
        || context.values.len() > MAX_EVENT_FIELDS
        || context.fields.len() != context.values.len()
    {
        return Err(EvaluationError(
            "active named event context is invalid".into(),
        ));
    }
    for (index, (field, value)) in context.fields.iter().zip(context.values).enumerate() {
        if !event_label_valid(&field.name)
            || field.kind == VarKind::Timer
            || context.fields[..index]
                .iter()
                .any(|prior| prior.name == field.name)
            || value.kind() != field.kind
            || !value.finite_and_bounded()
        {
            return Err(EvaluationError(
                "active named event context has an invalid field value".into(),
            ));
        }
    }
    Ok(())
}

fn cmp(op: CmpOp, ordering: Option<std::cmp::Ordering>) -> bool {
    use std::cmp::Ordering::*;
    match op {
        CmpOp::Eq => ordering == Some(Equal),
        CmpOp::Lt => ordering == Some(Less),
        CmpOp::Lte => matches!(ordering, Some(Less | Equal)),
        CmpOp::Gt => ordering == Some(Greater),
        CmpOp::Gte => matches!(ordering, Some(Greater | Equal)),
    }
}

fn validate_event_values(
    binding: &EventBinding,
    values: &[Value],
) -> Result<InlineEventPayload, EvaluationError> {
    if values.len() > MAX_EVENT_FIELDS || binding.fields.len() > MAX_EVENT_FIELDS {
        return Err(EvaluationError(
            "named event payload or schema exceeds the field limit".into(),
        ));
    }
    validate_event_schema(binding)?;
    if values.len() != binding.fields.len() {
        return Err(EvaluationError(format!(
            "named event {:?} payload count does not match its schema",
            binding.name
        )));
    }
    let mut payload = InlineEventPayload::empty();
    for (index, (field, value)) in binding.fields.iter().zip(values).enumerate() {
        if value.kind() != field.kind || !value.finite_and_bounded() {
            return Err(EvaluationError(format!(
                "named event {:?} has an invalid value for field {index}",
                binding.name
            )));
        }
        payload.values[index] = *value;
        payload.len += 1;
    }
    Ok(payload)
}

fn copy_named_event_values(
    binding: &EventBinding,
    arguments: &[(&str, Value)],
) -> Result<InlineEventPayload, EvaluationError> {
    if arguments.len() > MAX_EVENT_FIELDS || binding.fields.len() > MAX_EVENT_FIELDS {
        return Err(EvaluationError(
            "named event arguments or schema exceeds the field limit".into(),
        ));
    }
    validate_event_schema(binding)?;
    if binding.fields.is_empty() {
        return Err(EvaluationError(
            "typed named-event values require a nonempty receiver schema".into(),
        ));
    }
    if arguments.len() != binding.fields.len() {
        return Err(EvaluationError(format!(
            "named event {:?} argument count does not match its schema",
            binding.name
        )));
    }
    let mut values = [Value::Flag(false); MAX_EVENT_FIELDS];
    for (index, ((field_name, value), field)) in arguments.iter().zip(&binding.fields).enumerate() {
        if !event_label_valid(field_name) || *field_name != field.name.as_str() {
            return Err(EvaluationError(format!(
                "named event {:?} has an invalid argument at field {index}",
                binding.name
            )));
        }
        values[index] = *value;
    }
    validate_event_values(binding, &values[..arguments.len()])
}

fn validate_event_transition_selection(
    binding: &EventBinding,
    transition_count: usize,
) -> Result<(), EvaluationError> {
    let mut seen = BTreeSet::new();
    if binding.transitions.is_empty()
        || binding
            .transitions
            .iter()
            .any(|index| *index >= transition_count || !seen.insert(*index))
    {
        return Err(EvaluationError(format!(
            "named event {:?} has invalid or duplicate transition selections",
            binding.name
        )));
    }
    Ok(())
}

fn copy_event_arguments(
    binding: &EventBinding,
    arguments: &[EventArgument],
) -> Result<InlineEventPayload, EvaluationError> {
    if arguments.len() > MAX_EVENT_FIELDS || binding.fields.len() > MAX_EVENT_FIELDS {
        return Err(EvaluationError(
            "named event arguments or schema exceeds the field limit".into(),
        ));
    }
    validate_event_schema(binding)?;
    if binding.fields.is_empty() {
        return Err(EvaluationError(
            "typed event arguments require a nonempty schema".into(),
        ));
    }
    if arguments.len() != binding.fields.len() {
        return Err(EvaluationError(format!(
            "named event {:?} argument count does not match its schema",
            binding.name
        )));
    }
    let mut payload = InlineEventPayload::empty();
    for (index, (argument, field)) in arguments.iter().zip(&binding.fields).enumerate() {
        if !event_label_valid(&argument.field)
            || argument.field != field.name
            || argument.value.kind() != field.kind
            || !argument.value.finite_and_bounded()
        {
            return Err(EvaluationError(format!(
                "named event {:?} has an invalid argument at field {index}",
                binding.name
            )));
        }
        payload.values[index] = argument.value;
        payload.len += 1;
    }
    Ok(payload)
}

fn evaluate_event_arguments(
    binding: &EventBinding,
    arguments: &[ValueArgument],
    values: &BTreeMap<String, Value>,
    event_context: Option<&EventContext<'_>>,
    snapshot: &NativeSnapshot,
) -> Result<InlineEventPayload, EvaluationError> {
    if arguments.len() > MAX_EVENT_FIELDS || binding.fields.len() > MAX_EVENT_FIELDS {
        return Err(EvaluationError(
            "named event arguments or schema exceeds the field limit".into(),
        ));
    }
    validate_event_schema(binding)?;
    if binding.fields.is_empty() {
        return Err(EvaluationError(
            "computed event arguments require a nonempty schema".into(),
        ));
    }
    if arguments.len() != binding.fields.len() {
        return Err(EvaluationError(format!(
            "named event {:?} argument count does not match its schema",
            binding.name
        )));
    }
    let mut payload = InlineEventPayload::empty();
    for (index, (argument, field)) in arguments.iter().zip(&binding.fields).enumerate() {
        if !event_label_valid(&argument.field) || argument.field != field.name {
            return Err(EvaluationError(format!(
                "named event {:?} has an invalid argument at field {index}",
                binding.name
            )));
        }
        let value = match &argument.value {
            EventValue::Constant(value) => *value,
            EventValue::Numeric(expression) => {
                let value = eval_expr_with_snapshot(expression, values, event_context, snapshot)?;
                if !matches!(value, Value::Float(_) | Value::Int(_)) {
                    return Err(EvaluationError(format!(
                        "named event {:?} numeric argument is not numeric at field {index}",
                        binding.name
                    )));
                }
                value
            }
            EventValue::Flag(predicate) => {
                Value::Flag(eval_predicate(predicate, values, event_context, snapshot)?)
            }
        };
        if value.kind() != field.kind || !value.finite_and_bounded() {
            return Err(EvaluationError(format!(
                "named event {:?} has an invalid value for field {index}",
                binding.name
            )));
        }
        payload.values[index] = value;
        payload.len += 1;
    }
    Ok(payload)
}

fn validate_event_schema(binding: &EventBinding) -> Result<(), EvaluationError> {
    if !event_label_valid(&binding.name) || binding.fields.len() > MAX_EVENT_FIELDS {
        return Err(EvaluationError("named event has an invalid schema".into()));
    }
    for (index, field) in binding.fields.iter().enumerate() {
        if !event_label_valid(&field.name)
            || field.kind == VarKind::Timer
            || binding.fields[..index]
                .iter()
                .any(|prior| prior.name == field.name)
        {
            return Err(EvaluationError(format!(
                "named event {:?} has an invalid schema field {index}",
                binding.name
            )));
        }
    }
    Ok(())
}

fn enqueue_event(
    program: &LiveProgram,
    event_queue: &mut EventQueue,
    name: &str,
    depth: usize,
    payload: InlineEventPayload,
) -> Result<(), EvaluationError> {
    if !event_label_valid(name) {
        return Err(EvaluationError("named event name is invalid".into()));
    }
    let next_depth = depth.saturating_add(1);
    if next_depth > MAX_EVENT_DEPTH {
        return Err(EvaluationError(format!(
            "named event causal depth exceeds {MAX_EVENT_DEPTH}"
        )));
    }
    if event_queue.enqueued >= MAX_EVENTS_PER_STEP {
        return Err(EvaluationError(format!(
            "named event count exceeds {MAX_EVENTS_PER_STEP} per step"
        )));
    }
    let binding = find_event_binding(program, name)?;
    validate_event_values(binding, payload.as_slice())?;
    event_queue.enqueued += 1;
    event_queue.pending.push_back(QueuedEvent {
        name: name.to_owned(),
        depth: next_depth,
        payload,
    });
    Ok(())
}

fn find_event_binding<'a>(
    program: &'a LiveProgram,
    name: &str,
) -> Result<&'a EventBinding, EvaluationError> {
    if !event_label_valid(name) {
        return Err(EvaluationError("named event name is invalid".into()));
    }
    if program.event_bindings.len() > MAX_EVENT_BINDINGS {
        return Err(EvaluationError(format!(
            "event receiver table exceeds {MAX_EVENT_BINDINGS} entries"
        )));
    }
    let mut found = None;
    for binding in &program.event_bindings {
        if binding.name == name {
            if found.is_some() {
                return Err(EvaluationError("duplicate named event receiver".into()));
            }
            found = Some(binding);
        }
    }
    let binding = found.ok_or_else(|| EvaluationError(format!("unknown named event {name:?}")))?;
    if program.transitions.len() > MAX_TRANSITIONS || binding.transitions.len() > MAX_TRANSITIONS {
        return Err(EvaluationError(
            "named event transition table exceeds the limit".into(),
        ));
    }
    Ok(binding)
}

fn apply_action(
    program: &LiveProgram,
    state: &mut LiveState,
    action: &Action,
    event_queue: &mut EventQueue,
    event_depth: usize,
    event_context: Option<&EventContext<'_>>,
    snapshot: &NativeSnapshot,
) -> Result<(), EvaluationError> {
    match action {
        Action::Set { var, value } => {
            let result = eval_expr_with_snapshot(value, &state.values, event_context, snapshot)?;
            let target = program
                .vars
                .iter()
                .find(|item| item.name == *var)
                .ok_or_else(|| EvaluationError(format!("unknown variable {var:?}")))?;
            if result.kind() != target.kind
                && !(target.kind == VarKind::Timer && matches!(result, Value::Int(_)))
            {
                return Err(EvaluationError(format!("set type mismatch for {var:?}")));
            }
            if target.kind == VarKind::Timer
                && matches!(result, Value::Int(value) if !(0..=MAX_TIMER_FRAMES).contains(&value))
            {
                return Err(EvaluationError(format!(
                    "timer {var:?} outside frame limit"
                )));
            }
            state.values.insert(var.clone(), result);
        }
        Action::SetFlag { var, value } => {
            let next = eval_predicate(value, &state.values, event_context, snapshot)?;
            if !matches!(state.values.get(var.as_str()), Some(Value::Flag(_))) {
                return Err(EvaluationError(format!(
                    "unknown or non-flag target {var:?}"
                )));
            }
            state.values.insert(var.clone(), Value::Flag(next));
        }
        Action::Add { var, value } => {
            let current = state
                .values
                .get(var)
                .copied()
                .ok_or_else(|| EvaluationError(format!("unknown variable {var:?}")))?;
            let delta = eval_expr_with_snapshot(value, &state.values, event_context, snapshot)?;
            let result = match (current, delta) {
                (Value::Float(a), Value::Float(b))
                    if (a + b).is_finite() && f64::from(a + b).abs() <= MAX_ABS_NUMBER =>
                {
                    Value::Float(a + b)
                }
                (Value::Int(a), Value::Int(b)) => Value::Int(
                    a.checked_add(b)
                        .filter(|v| i64::from(*v).abs() <= MAX_ABS_NUMBER as i64)
                        .ok_or_else(|| {
                            EvaluationError("integer add overflow or out of range".into())
                        })?,
                ),
                _ => return Err(EvaluationError(format!("add type mismatch for {var:?}"))),
            };
            state.values.insert(var.clone(), result);
        }
        Action::Raise { var } => {
            state.values.insert(var.clone(), Value::Flag(true));
        }
        Action::Clear { var } => {
            state.values.insert(var.clone(), Value::Flag(false));
        }
        Action::Reset { var } => {
            let initial = program
                .vars
                .iter()
                .find(|item| item.name == *var)
                .ok_or_else(|| EvaluationError(format!("unknown variable {var:?}")))?
                .initial;
            state.values.insert(var.clone(), initial);
        }
        Action::Status(_)
        | Action::AddHeldOpponentDamage { .. }
        | Action::AddDamage { .. }
        | Action::CombatPolicy { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit
        | Action::Movement(_)
        | Action::Resource(_)
        | Action::Item(_)
        | Action::MeshVisibility { .. }
        | Action::SetMotionRate { .. }
        | Action::AddSpeed { .. }
        | Action::ChooseHeroMenuSpell { .. }
        | Action::SetNative { .. }
        | Action::AddNative { .. }
        | Action::SetNativeFlag { .. }
        | Action::GenerateArticle { .. }
        | Action::RemoveArticle { .. }
        | Action::RemoveNewestArticle { .. }
        | Action::ArticleMovement { .. }
        | Action::Object { .. }
        | Action::RequestArticleStatus { .. }
        | Action::RequestFighterStatus { .. }
        | Action::RequestWeaponStatus { .. } => {}
        Action::EmitEvent { name } => {
            let binding = find_event_binding(program, name)?;
            let payload = validate_event_values(binding, &[])?;
            enqueue_event(program, event_queue, name, event_depth, payload)?;
        }
        Action::EmitEventArguments { name, arguments } => {
            if arguments.len() > MAX_EVENT_FIELDS {
                return Err(EvaluationError(format!(
                    "named event arguments exceed {MAX_EVENT_FIELDS} fields"
                )));
            }
            let binding = find_event_binding(program, name)?;
            let payload = copy_event_arguments(binding, arguments)?;
            enqueue_event(program, event_queue, name, event_depth, payload)?;
        }
        Action::EmitEventValues { name, arguments } => {
            if arguments.len() > MAX_EVENT_FIELDS {
                return Err(EvaluationError(format!(
                    "named event arguments exceed {MAX_EVENT_FIELDS} fields"
                )));
            }
            let binding = find_event_binding(program, name)?;
            let payload = evaluate_event_arguments(
                binding,
                arguments,
                &state.values,
                event_context,
                snapshot,
            )?;
            enqueue_event(program, event_queue, name, event_depth, payload)?;
        }
    }
    Ok(())
}

fn predicate_mentions_expired(predicate: &Predicate, expired: &BTreeSet<String>) -> bool {
    match predicate {
        Predicate::Expired(name) => expired.contains(name),
        Predicate::DamageIsProjectile
        | Predicate::InStatus { .. }
        | Predicate::InMotion { .. }
        | Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::TermEnabled { .. }
        | Predicate::Situation { .. }
        | Predicate::Button { .. }
        | Predicate::Flag(_)
        | Predicate::EventFlag { .. }
        | Predicate::NativeFlag(_)
        | Predicate::DamageStatusIs(_)
        | Predicate::Compare { .. } => false,
        Predicate::Not(inner) => predicate_mentions_expired(inner, expired),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            predicate_mentions_expired(left, expired) || predicate_mentions_expired(right, expired)
        }
    }
}

fn transition_uses_input(t: &Transition) -> bool {
    transition_uses_reads(t, false, false, true, false, false)
}
fn transition_uses_phase(t: &Transition) -> bool {
    transition_uses_reads(t, true, true, true, false, false)
}
fn transition_uses_reads(
    t: &Transition,
    phase_only: bool,
    phase_actions: bool,
    phase_reads: bool,
    cancel_only: bool,
    command_only: bool,
) -> bool {
    enum Node<'a> {
        Expr(&'a Expr, usize),
        Predicate(&'a Predicate, usize),
    }
    let mut pending = vec![Node::Predicate(&t.guard, 1)];
    for action in &t.actions {
        if phase_actions
            && matches!(
                action,
                Action::Status(_)
                    | Action::AddHeldOpponentDamage { .. }
                    | Action::AddDamage { .. }
                    | Action::CombatPolicy { .. }
                    | Action::ThrowHeldOpponent { .. }
                    | Action::ReleaseHeldOpponent
                    | Action::Movement(_)
                    | Action::Item(_)
                    | Action::MeshVisibility { .. }
                    | Action::SetMotionRate { .. }
                    | Action::AddSpeed { .. }
            )
        {
            return true;
        }
        pending.extend(action.operands().into_iter().map(|operand| match operand {
            Operand::Expr(e) => Node::Expr(e, 1),
            Operand::Predicate(p) => Node::Predicate(p, 1),
        }));
    }
    let mut remaining = super::live_model::MAX_EXPR_NODES;
    while let Some(node) = pending.pop() {
        if remaining == 0 {
            break;
        }
        remaining -= 1;
        match node {
            Node::Expr(_, d) | Node::Predicate(_, d) if d > super::live_model::MAX_EXPR_DEPTH => {
                continue
            }
            Node::Predicate(
                Predicate::NativeFlag(super::native_inputs::NativeInput::Command { .. }),
                _,
            ) if command_only => return true,
            Node::Predicate(Predicate::CancelEnabled | Predicate::TermEnabled { .. }, _)
                if cancel_only =>
            {
                return true
            }
            Node::Expr(Expr::StatusFrame | Expr::MotionFrame, _)
            | Node::Predicate(
                Predicate::InStatus { .. }
                | Predicate::InMotion { .. }
                | Predicate::SituationChanged
                | Predicate::MotionEnded
                | Predicate::CancelEnabled
                | Predicate::TermEnabled { .. }
                | Predicate::Situation { .. },
                _,
            ) if phase_reads => return true,
            Node::Expr(Expr::StickX { .. } | Expr::StickY, _)
            | Node::Predicate(
                Predicate::Button { .. }
                | Predicate::NativeFlag(super::native_inputs::NativeInput::Command { .. }),
                _,
            ) if !phase_only && !cancel_only && !command_only => return true,
            Node::Expr(Expr::Add(a, b) | Expr::Sub(a, b) | Expr::Mul(a, b), d) => {
                pending.push(Node::Expr(a, d + 1));
                pending.push(Node::Expr(b, d + 1));
            }
            Node::Predicate(Predicate::Compare { left, right, .. }, d) => {
                pending.push(Node::Expr(left, d + 1));
                pending.push(Node::Expr(right, d + 1));
            }
            Node::Predicate(Predicate::Not(p), d) => pending.push(Node::Predicate(p, d + 1)),
            Node::Predicate(Predicate::And(a, b) | Predicate::Or(a, b), d) => {
                pending.push(Node::Predicate(a, d + 1));
                pending.push(Node::Predicate(b, d + 1));
            }
            _ => {}
        }
    }
    false
}

pub fn phase_scalar_valid(action: &Action, value: f32) -> bool {
    value.is_finite()
        && match action {
            Action::AddHeldOpponentDamage { .. } => (0.0..=999.0).contains(&value),
            Action::AddDamage { .. } => value.abs() <= 999.0,
            Action::CombatPolicy { op, .. } => {
                op.bounds().is_some_and(|range| range.contains(&value))
            }
            Action::SetMotionRate { .. } => (0.05..=20.0).contains(&value),
            Action::AddSpeed { .. } => value.abs() <= 10.0,
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed_event_program() -> LiveProgram {
        let mut program = timer_program(false);
        let gate = || Predicate::Compare {
            op: CmpOp::Gte,
            left: Expr::Var("ticks".into()),
            right: Expr::Const(Value::Int(0)),
        };
        program.transitions = vec![
            Transition {
                from: "active".into(),
                to: "active".into(),
                guard: gate(),
                on_frame: false,
                actions: vec![Action::EmitEventArguments {
                    name: "nested".into(),
                    arguments: vec![
                        EventArgument {
                            field: "amount".into(),
                            value: Value::Float(1.5),
                        },
                        EventArgument {
                            field: "count".into(),
                            value: Value::Int(7),
                        },
                        EventArgument {
                            field: "enabled".into(),
                            value: Value::Flag(true),
                        },
                    ],
                }],
            },
            Transition {
                from: "active".into(),
                to: "done".into(),
                guard: gate(),
                on_frame: false,
                actions: vec![],
            },
        ];
        program.bindings = vec![];
        program.event_bindings = vec![
            EventBinding {
                name: "start".into(),
                transitions: vec![0],
                fields: vec![],
            },
            EventBinding {
                name: "nested".into(),
                transitions: vec![1],
                fields: vec![
                    EventField {
                        name: "amount".into(),
                        kind: VarKind::Float,
                    },
                    EventField {
                        name: "count".into(),
                        kind: VarKind::Int,
                    },
                    EventField {
                        name: "enabled".into(),
                        kind: VarKind::Flag,
                    },
                ],
            },
        ];
        program.status_triggers.clear();
        program.resource_triggers.clear();
        program
    }

    fn computed_event_program() -> LiveProgram {
        let mut program = timer_program(false);
        program.vars.extend([
            VarDef {
                keep_across_form_switch: false,
                name: "scratch".into(),
                kind: VarKind::Int,
                initial: Value::Int(1),
            },
            VarDef {
                keep_across_form_switch: false,
                name: "captured".into(),
                kind: VarKind::Int,
                initial: Value::Int(0),
            },
            VarDef {
                keep_across_form_switch: false,
                name: "ready".into(),
                kind: VarKind::Flag,
                initial: Value::Flag(false),
            },
        ]);
        let always = || Predicate::Compare {
            op: CmpOp::Eq,
            left: Expr::Const(Value::Int(1)),
            right: Expr::Const(Value::Int(1)),
        };
        program.transitions = vec![
            Transition {
                from: "active".into(),
                to: "active".into(),
                guard: always(),
                on_frame: false,
                actions: vec![Action::EmitEventArguments {
                    name: "alpha".into(),
                    arguments: vec![
                        EventArgument {
                            field: "amount".into(),
                            value: Value::Int(5),
                        },
                        EventArgument {
                            field: "enabled".into(),
                            value: Value::Flag(true),
                        },
                    ],
                }],
            },
            Transition {
                from: "active".into(),
                to: "active".into(),
                guard: always(),
                on_frame: false,
                actions: vec![
                    Action::Set {
                        var: "scratch".into(),
                        value: Expr::Const(Value::Int(9)),
                    },
                    Action::EmitEventValues {
                        name: "beta".into(),
                        arguments: vec![
                            ValueArgument {
                                field: "amount".into(),
                                value: EventValue::Numeric(Expr::Add(
                                    Box::new(Expr::EventField {
                                        event: "alpha".into(),
                                        field: "amount".into(),
                                    }),
                                    Box::new(Expr::Var("scratch".into())),
                                )),
                            },
                            ValueArgument {
                                field: "enabled".into(),
                                value: EventValue::Flag(Predicate::And(
                                    Box::new(Predicate::EventFlag {
                                        event: "alpha".into(),
                                        field: "enabled".into(),
                                    }),
                                    Box::new(always()),
                                )),
                            },
                        ],
                    },
                    Action::Set {
                        var: "scratch".into(),
                        value: Expr::Const(Value::Int(100)),
                    },
                ],
            },
            Transition {
                from: "active".into(),
                to: "active".into(),
                guard: always(),
                on_frame: false,
                actions: vec![Action::Set {
                    var: "captured".into(),
                    value: Expr::EventField {
                        event: "beta".into(),
                        field: "amount".into(),
                    },
                }],
            },
            Transition {
                from: "active".into(),
                to: "active".into(),
                guard: Predicate::EventFlag {
                    event: "beta".into(),
                    field: "enabled".into(),
                },
                on_frame: false,
                actions: vec![Action::Raise {
                    var: "ready".into(),
                }],
            },
        ];
        program.bindings.clear();
        program.event_bindings = vec![
            EventBinding {
                name: "start".into(),
                transitions: vec![0],
                fields: vec![],
            },
            EventBinding {
                name: "alpha".into(),
                transitions: vec![1],
                fields: vec![
                    EventField {
                        name: "amount".into(),
                        kind: VarKind::Int,
                    },
                    EventField {
                        name: "enabled".into(),
                        kind: VarKind::Flag,
                    },
                ],
            },
            EventBinding {
                name: "beta".into(),
                transitions: vec![2, 3],
                fields: vec![
                    EventField {
                        name: "amount".into(),
                        kind: VarKind::Int,
                    },
                    EventField {
                        name: "enabled".into(),
                        kind: VarKind::Flag,
                    },
                ],
            },
        ];
        program.status_triggers.clear();
        program.resource_triggers.clear();
        program
    }

    #[test]
    fn typed_constant_arguments_are_copied_in_order_and_nested_dispatch_is_atomic() {
        let program = typed_event_program();
        assert!(program.validate().is_ok(), "{:?}", program.validate());
        let binding = &program.event_bindings[1];
        let Action::EmitEventArguments { arguments, .. } = &program.transitions[0].actions[0]
        else {
            panic!("expected typed event action")
        };
        let payload = copy_event_arguments(binding, arguments).unwrap();
        assert_eq!(
            payload.as_slice(),
            &[Value::Float(1.5), Value::Int(7), Value::Flag(true)]
        );

        let mut state = program.initial_state();
        let result = program.step_event(&mut state, "start").unwrap();
        assert_eq!(result.fired.len(), 2);
        assert_eq!(result.after.state, "done");
        assert_eq!(result.after.values["ticks"], Value::Int(0));
    }

    #[test]
    fn external_typed_root_values_are_bounded_typed_and_atomic() {
        let mut program = typed_event_program();
        for (name, kind, initial) in [
            ("ratio", VarKind::Float, Value::Float(0.0)),
            ("count_seen", VarKind::Int, Value::Int(0)),
            ("enabled_seen", VarKind::Flag, Value::Flag(false)),
        ] {
            program.vars.push(VarDef {
                keep_across_form_switch: false,
                name: name.into(),
                kind,
                initial,
            });
        }
        program.transitions[1].actions = [("ratio", "amount"), ("count_seen", "count")]
            .map(|(var, field)| Action::Set {
                var: var.into(),
                value: Expr::EventField {
                    event: "nested".into(),
                    field: field.into(),
                },
            })
            .to_vec();
        program.transitions[1].actions.push(Action::SetFlag {
            var: "enabled_seen".into(),
            value: Predicate::EventFlag {
                event: "nested".into(),
                field: "enabled".into(),
            },
        });
        assert!(program.validate().is_ok());
        let valid = [
            ("amount", Value::Float(1.5)),
            ("count", Value::Int(7)),
            ("enabled", Value::Flag(true)),
        ];
        let snapshot = NativeSnapshot::empty();
        let mut state = program.initial_state();
        let before = state.snapshot();
        assert!(program
            .step_event_with_snapshot(&mut state, "nested", &snapshot)
            .is_err());
        assert_eq!(state.snapshot(), before);
        program
            .validate_named_event_values("nested", &valid)
            .unwrap();
        program
            .step_event_values_with_snapshot(&mut state, "nested", &valid, &snapshot)
            .unwrap();
        for (name, expected) in [
            ("ratio", valid[0].1),
            ("count_seen", valid[1].1),
            ("enabled_seen", valid[2].1),
        ] {
            assert_eq!(state.values[name], expected);
        }
        let mut payloads = vec![valid[..2].to_vec(), vec![valid[0]; MAX_EVENT_FIELDS + 1]];
        let mut extra = valid.to_vec();
        extra.push(("extra", Value::Int(1)));
        payloads.push(extra);
        let mut wrong_order = valid.to_vec();
        wrong_order.swap(0, 1);
        payloads.push(wrong_order);
        for first in [
            ("other", valid[0].1),
            ("amount", Value::Int(1)),
            ("amount", Value::Float(f32::NAN)),
            ("amount", Value::Float(MAX_ABS_NUMBER as f32 + 1.0)),
        ] {
            let mut invalid = valid.to_vec();
            invalid[0] = first;
            payloads.push(invalid);
        }
        for payload in payloads {
            let mut state = program.initial_state();
            let before = state.snapshot();
            assert!(program
                .validate_named_event_values("nested", &payload)
                .is_err());
            assert!(program
                .step_event_values_with_snapshot(&mut state, "nested", &payload, &snapshot)
                .is_err());
            assert_eq!(state.snapshot(), before);
        }
        for invalid_schema in 0..3 {
            let mut invalid = program.clone();
            let binding = &mut invalid.event_bindings[1];
            match invalid_schema {
                0 => binding.fields[1].name = "amount".into(),
                1 => binding.fields[0].kind = VarKind::Timer,
                _ => binding.transitions = vec![1, 1],
            }
            let mut state = invalid.initial_state();
            let before = state.snapshot();
            assert!(invalid
                .validate_named_event_values("nested", &valid)
                .is_err());
            assert!(invalid
                .step_event_values_with_snapshot(&mut state, "nested", &valid, &snapshot)
                .is_err());
            assert_eq!(state.snapshot(), before);
        }
    }

    #[test]
    fn computed_event_arguments_capture_staged_values_and_forward_receiver_fields() {
        let program = computed_event_program();
        assert!(program.validate().is_ok(), "{:?}", program.validate());

        let mut state = program.initial_state();
        let result = program.step_event(&mut state, "start").unwrap();
        assert_eq!(state.values["scratch"], Value::Int(100));
        assert_eq!(state.values["captured"], Value::Int(14));
        assert_eq!(state.values["ready"], Value::Flag(true));
        assert_eq!(
            result
                .fired
                .iter()
                .map(|fired| fired.index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn timer_remaining_validates_reads_without_mutation_and_counts_down_with_frames() {
        let mut program = timer_program(false);
        program.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Gte,
            left: Expr::TimerRemaining("timer".into()),
            right: Expr::Const(Value::Int(0)),
        };
        assert!(program.validate().is_ok(), "{:?}", program.validate());

        let values = BTreeMap::from([
            ("timer".to_owned(), Value::Int(2)),
            ("ticks".to_owned(), Value::Int(0)),
        ]);
        let before = values.clone();
        assert_eq!(
            eval_expr_with_snapshot(
                &Expr::TimerRemaining("timer".into()),
                &values,
                None,
                &NativeSnapshot::empty(),
            )
            .unwrap(),
            Value::Int(2)
        );
        assert_eq!(values, before, "reading a timer must not tick or mutate it");
        for invalid_value in [-1, MAX_TIMER_FRAMES + 1] {
            let malformed = BTreeMap::from([("timer".to_owned(), Value::Int(invalid_value))]);
            assert!(eval_expr_with_snapshot(
                &Expr::TimerRemaining("timer".into()),
                &malformed,
                None,
                &NativeSnapshot::empty(),
            )
            .is_err());
        }
        let malformed_kind = BTreeMap::from([("timer".to_owned(), Value::Flag(true))]);
        assert!(eval_expr_with_snapshot(
            &Expr::TimerRemaining("timer".into()),
            &malformed_kind,
            None,
            &NativeSnapshot::empty(),
        )
        .is_err());

        program.transitions[0].actions = vec![Action::Set {
            var: "ticks".into(),
            value: Expr::TimerRemaining("timer".into()),
        }];
        let mut state = program.initial_state();
        let first = program.step_frame(&mut state).unwrap();
        assert_eq!(first.after.values["timer"], Value::Int(1));
        assert_eq!(first.after.values["ticks"], Value::Int(1));
        let second = program.step_frame(&mut state).unwrap();
        assert_eq!(second.after.values["timer"], Value::Int(0));
        assert_eq!(second.after.values["ticks"], Value::Int(0));

        let bad_names = vec![
            "missing".to_owned(),
            "ticks".to_owned(),
            String::new(),
            "bad\0name".to_owned(),
            "x".repeat(MAX_NAME_BYTES + 1),
        ];
        for bad in bad_names {
            let mut invalid = timer_program(false);
            invalid.transitions[0].guard = Predicate::Compare {
                op: CmpOp::Eq,
                left: Expr::TimerRemaining(bad.clone()),
                right: Expr::Const(Value::Int(0)),
            };
            assert!(invalid.validate().is_err(), "accepted timer name {bad:?}");
        }

        let mut direct_timer_scalar = timer_program(false);
        direct_timer_scalar.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Eq,
            left: Expr::Var("timer".into()),
            right: Expr::Const(Value::Int(0)),
        };
        assert!(direct_timer_scalar.validate().is_err());
    }

    fn timer_program(refresh_on_expiry: bool) -> LiveProgram {
        let mut tick_actions = vec![Action::Add {
            var: "ticks".into(),
            value: Expr::Const(Value::Int(1)),
        }];
        if refresh_on_expiry {
            tick_actions.push(Action::Set {
                var: "timer".into(),
                value: Expr::Const(Value::Int(3)),
            });
        }
        LiveProgram {
            costumes: Vec::new(),
            custom_statuses: Default::default(),
            status_bindings: Vec::new(),
            version: MODEL_VERSION,
            id: "cloud_timer".into(),
            fighter: "cloud".into(),
            initial: "active".into(),
            states: vec!["active".into(), "done".into()],
            vars: vec![
                VarDef {
                    keep_across_form_switch: false,
                    name: "timer".into(),
                    kind: VarKind::Timer,
                    initial: Value::Int(2),
                },
                VarDef {
                    keep_across_form_switch: false,
                    name: "ticks".into(),
                    kind: VarKind::Int,
                    initial: Value::Int(0),
                },
            ],
            native_inputs: vec![],
            transitions: vec![
                Transition {
                    from: "active".into(),
                    to: "active".into(),
                    guard: if refresh_on_expiry {
                        Predicate::Expired("timer".into())
                    } else {
                        Predicate::Compare {
                            op: CmpOp::Gte,
                            left: Expr::Var("ticks".into()),
                            right: Expr::Const(Value::Int(0)),
                        }
                    },
                    on_frame: true,
                    actions: tick_actions,
                },
                Transition {
                    from: "active".into(),
                    to: "done".into(),
                    guard: Predicate::Expired("timer".into()),
                    on_frame: false,
                    actions: vec![],
                },
            ],
            bindings: vec![
                HookBinding {
                    hook: HookName::OnChargeTick,
                    article: None,
                    transitions: vec![0],
                },
                HookBinding {
                    hook: HookName::OnTimeout,
                    article: None,
                    transitions: vec![1],
                },
            ],
            event_bindings: vec![],
            status_triggers: vec![],
            resource_triggers: vec![],
        }
    }

    fn native_input_program() -> LiveProgram {
        let mut program = timer_program(false);
        program.native_inputs = vec![NativeInput::CloudLimitGauge];
        program.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Gte,
            left: Expr::NativeFloat(NativeInput::CloudLimitGauge),
            right: Expr::Const(Value::Float(50.0)),
        };
        program
    }

    fn native_int_flag_program() -> LiveProgram {
        let mut program = timer_program(false);
        program.native_inputs = vec![
            NativeInput::CloudLimitClearFrame,
            NativeInput::CloudLimitBreak,
        ];
        program.transitions[0].guard = Predicate::And(
            Box::new(Predicate::Not(Box::new(Predicate::NativeFlag(
                NativeInput::CloudLimitBreak,
            )))),
            Box::new(Predicate::Compare {
                op: CmpOp::Gte,
                left: Expr::NativeInt(NativeInput::CloudLimitClearFrame),
                right: Expr::Const(Value::Int(0)),
            }),
        );
        program
    }

    #[test]
    fn native_input_declarations_are_exact_closed_and_owner_bound() {
        let good = native_input_program();
        assert!(good.validate().is_ok(), "{:?}", good.validate());

        let mut missing = good.clone();
        missing.native_inputs.clear();
        assert!(missing.validate().is_err());

        let mut duplicate = good.clone();
        duplicate.native_inputs.push(NativeInput::CloudLimitGauge);
        assert!(duplicate.validate().is_err());

        let mut foreign_owner = good.clone();
        foreign_owner.fighter = "mario".into();
        assert!(foreign_owner.validate().is_err());

        let mut unsupported = good;
        unsupported.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Gte,
            left: Expr::NativeFloat(NativeInput::Unsupported(
                "FIGHTER_MARIO_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
            )),
            right: Expr::Const(Value::Float(50.0)),
        };
        assert!(unsupported.validate().is_err());

        let typed = native_int_flag_program();
        assert!(typed.validate().is_ok(), "{:?}", typed.validate());
        let mut wrong_integer_kind = typed.clone();
        wrong_integer_kind.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Gte,
            left: Expr::NativeInt(NativeInput::CloudLimitGauge),
            right: Expr::Const(Value::Int(0)),
        };
        assert!(wrong_integer_kind.validate().is_err());
        let mut wrong_flag_kind = typed.clone();
        wrong_flag_kind.transitions[0].guard =
            Predicate::NativeFlag(NativeInput::CloudLimitClearFrame);
        assert!(wrong_flag_kind.validate().is_err());
        let mut foreign_typed = typed;
        foreign_typed.fighter = "mario".into();
        assert!(foreign_typed.validate().is_err());
    }

    #[test]
    fn frame_tick_decrements_then_runs_tick_then_synthetic_timeout() {
        let program = timer_program(false);
        assert!(program.validate().is_ok());
        let mut state = program.initial_state();
        let first = program.step_frame(&mut state).unwrap();
        assert_eq!(first.after.values["timer"], Value::Int(1));
        assert_eq!(first.after.values["ticks"], Value::Int(1));
        assert_eq!(
            first
                .fired
                .iter()
                .map(|event| event.index)
                .collect::<Vec<_>>(),
            vec![0]
        );

        let second = program.step_frame(&mut state).unwrap();
        assert_eq!(second.expired, vec!["timer"]);
        assert_eq!(second.after.state, "done");
        assert_eq!(second.after.values["ticks"], Value::Int(2));
        assert_eq!(
            second
                .fired
                .iter()
                .map(|event| event.index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn charge_tick_refresh_suppresses_timeout_for_that_timer() {
        let program = timer_program(true);
        assert!(program.validate().is_ok());
        let mut state = program.initial_state();
        program.step_frame(&mut state).unwrap();
        let second = program.step_frame(&mut state).unwrap();
        assert_eq!(second.expired, vec!["timer"]);
        assert_eq!(second.after.values["timer"], Value::Int(3));
        assert_eq!(second.after.state, "active");
        assert_eq!(
            second
                .fired
                .iter()
                .map(|event| event.index)
                .collect::<Vec<_>>(),
            vec![0]
        );
    }

    #[test]
    fn named_events_are_fifo_same_step_and_depth_overflow_discards_staged_state() {
        let mut program = timer_program(false);
        let always = Predicate::Compare {
            op: CmpOp::Eq,
            left: Expr::Const(Value::Int(1)),
            right: Expr::Const(Value::Int(1)),
        };
        let increment = || Action::Add {
            var: "ticks".into(),
            value: Expr::Const(Value::Int(1)),
        };
        program.transitions = vec![
            Transition {
                from: "active".into(),
                to: "active".into(),
                guard: always.clone(),
                on_frame: true,
                actions: vec![Action::EmitEvent {
                    name: "first".into(),
                }],
            },
            Transition {
                from: "active".into(),
                to: "active".into(),
                guard: always.clone(),
                on_frame: false,
                actions: vec![
                    increment(),
                    Action::EmitEvent {
                        name: "second".into(),
                    },
                ],
            },
            Transition {
                from: "active".into(),
                to: "active".into(),
                guard: always.clone(),
                on_frame: false,
                actions: vec![increment()],
            },
        ];
        program.bindings = vec![HookBinding {
            hook: HookName::OnChargeTick,
            article: None,
            transitions: vec![0],
        }];
        program.event_bindings = vec![
            EventBinding {
                name: "first".into(),
                transitions: vec![1],
                fields: vec![],
            },
            EventBinding {
                name: "second".into(),
                transitions: vec![2],
                fields: vec![],
            },
        ];
        program.resource_triggers.clear();
        assert!(program.validate().is_ok(), "{:?}", program.validate());
        let mut state = program.initial_state();
        let result = program.step(&mut state, HookName::OnChargeTick).unwrap();
        assert_eq!(state.values["ticks"], Value::Int(2));
        assert_eq!(
            result
                .fired
                .iter()
                .map(|item| item.index)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );

        let mut cyclic = timer_program(false);
        let names: Vec<String> = (0..=MAX_EVENT_DEPTH)
            .map(|index| format!("e{index}"))
            .collect();
        cyclic.transitions.clear();
        cyclic.transitions.push(Transition {
            from: "active".into(),
            to: "active".into(),
            guard: always.clone(),
            on_frame: true,
            actions: vec![Action::EmitEvent {
                name: names[0].clone(),
            }],
        });
        for name in &names {
            let index = cyclic.transitions.len();
            let actions = names
                .iter()
                .position(|candidate| candidate == name)
                .filter(|next| *next + 1 < names.len())
                .map(|next| {
                    vec![
                        Action::Add {
                            var: "ticks".into(),
                            value: Expr::Const(Value::Int(1)),
                        },
                        Action::EmitEvent {
                            name: names[next + 1].clone(),
                        },
                    ]
                })
                .unwrap_or_default();
            cyclic.transitions.push(Transition {
                from: "active".into(),
                to: "active".into(),
                guard: always.clone(),
                on_frame: false,
                actions,
            });
            cyclic.event_bindings.push(EventBinding {
                name: name.clone(),
                transitions: vec![index],
                fields: vec![],
            });
        }
        cyclic.bindings = vec![HookBinding {
            hook: HookName::OnChargeTick,
            article: None,
            transitions: vec![0],
        }];
        cyclic.resource_triggers.clear();
        assert!(cyclic.validate().is_ok(), "{:?}", cyclic.validate());
        let mut state = cyclic.initial_state();
        let error = cyclic.step(&mut state, HookName::OnChargeTick).unwrap_err();
        assert!(error.0.contains("causal depth exceeds"), "{error}");
        assert_eq!(state.values["ticks"], Value::Int(0));
    }
    #[test]
    fn generic_float_int_flag_snapshot_survives_event_cascade_and_rejects_partial_batches() {
        use super::super::native_inputs::NativeObservation;

        let float = NativeInput::from_registry(
            "robot",
            "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE",
            NativeKind::Float,
        )
        .unwrap();
        let int = NativeInput::from_registry(
            "robot",
            "FIGHTER_ROBOT_INSTANCE_WORK_ID_INT_BEAM_GLOW_ATTACK",
            NativeKind::Int,
        )
        .unwrap();
        let flag = NativeInput::from_registry(
            "robot",
            "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLAG_ARMSPIN_PUSH_B",
            NativeKind::Flag,
        )
        .unwrap();
        let native_gate = || {
            Predicate::And(
                Box::new(Predicate::NativeFlag(flag.clone())),
                Box::new(Predicate::And(
                    Box::new(Predicate::Compare {
                        op: CmpOp::Gt,
                        left: Expr::NativeFloat(float.clone()),
                        right: Expr::Const(Value::Float(0.0)),
                    }),
                    Box::new(Predicate::Compare {
                        op: CmpOp::Gte,
                        left: Expr::NativeInt(int.clone()),
                        right: Expr::Const(Value::Int(0)),
                    }),
                )),
            )
        };
        let mut program = timer_program(false);
        program.fighter = "robot".into();
        program.bindings.clear();
        program.states.push("native_ready".into());
        program.native_inputs = vec![float.clone(), int.clone(), flag.clone()];
        program.transitions = vec![
            Transition {
                from: "active".into(),
                to: "native_ready".into(),
                guard: native_gate(),
                on_frame: false,
                actions: vec![
                    Action::Set {
                        var: "ticks".into(),
                        value: Expr::NativeInt(int.clone()),
                    },
                    Action::EmitEvent {
                        name: "native_followup".into(),
                    },
                ],
            },
            Transition {
                from: "native_ready".into(),
                to: "done".into(),
                guard: native_gate(),
                on_frame: false,
                actions: vec![Action::Add {
                    var: "ticks".into(),
                    value: Expr::NativeInt(int.clone()),
                }],
            },
        ];
        program.event_bindings = vec![
            EventBinding {
                name: "native_begin".into(),
                transitions: vec![0],
                fields: vec![],
            },
            EventBinding {
                name: "native_followup".into(),
                transitions: vec![1],
                fields: vec![],
            },
        ];
        assert!(program.validate().is_ok(), "{:?}", program.validate());

        let valid = NativeSnapshot::from_observations(
            &[float.clone(), int.clone(), flag.clone()],
            &[
                NativeObservation::Float(6.5),
                NativeObservation::Int(4),
                NativeObservation::Flag(true),
            ],
        )
        .unwrap();
        let mut state = program.initial_state();
        let result = program
            .step_event_with_snapshot(&mut state, "native_begin", &valid)
            .unwrap();
        assert_eq!(result.fired.len(), 2);
        assert_eq!(result.after.state, "done");
        assert_eq!(result.after.values["ticks"], Value::Int(8));

        let mut unchanged = program.initial_state();
        unchanged.values.insert("ticks".into(), Value::Int(91));
        let before = unchanged.snapshot();
        assert!(program
            .step_event_with_snapshot(&mut unchanged, "native_begin", &NativeSnapshot::empty())
            .is_err());
        assert_eq!(unchanged.snapshot(), before);

        assert!(NativeSnapshot::from_observations(
            &[float.clone(), int.clone(), flag.clone()],
            &[
                NativeObservation::Float(6.5),
                NativeObservation::Float(4.0),
                NativeObservation::Flag(true),
            ],
        )
        .is_err());
        assert_eq!(unchanged.snapshot(), before);
    }
}
