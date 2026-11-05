use crate::gimmicks::{
    behavior::{Action, Expr, Predicate, VarDef, VarType},
    codegen::ResourceSource,
    hooks::HookName,
};
use crate::move_nodes::model::MoveTrack;
pub use crate::move_nodes::model::NodeError;
pub(super) use crate::move_nodes::model::SCHEMA_VERSION;
use serde::Deserialize;
use std::collections::BTreeMap;
/// Whether a move recipe's declared variable belongs to the whole character or
/// is namespaced to this recipe when custom triggers are lowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariableScope {
    /// Legacy behavior: recipes intentionally share identical declarations.
    #[default]
    CharacterShared,
    /// Keep this variable independent from same-named variables in other moves.
    MoveLocal,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum TriggerSource {
    Hook(HookName),
    TimerExpired {
        timer: String,
    },
    Resource(ResourceSource),
    /// Fire only when this fighter's identity-aware status tracker observes entry
    /// into the exact fighter-owned status constant.
    StatusEntered {
        symbol: String,
    },
    /// Fire only when this fighter's identity-aware status tracker observes exit
    /// from the exact fighter-owned status constant.
    StatusExited {
        symbol: String,
    },
    Event {
        name: String,
        /// Ordered receiver schema. Missing/empty is the legacy zero-payload form.
        #[serde(default)]
        fields: Vec<crate::gimmicks::event_payload::FieldDef>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomTrigger {
    pub name: String,
    pub source: TriggerSource,
    /// Omitted means unconditional; otherwise uses the existing typed gimmick predicates.
    #[serde(default)]
    pub guard: Option<Predicate>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

/// A synchronous insertion of a named action routine before one ordinary
/// trigger action. `before == actions.len()` places the call at the end.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineCall {
    pub routine: String,
    pub before: usize,
    /// Required named scalar values captured at the call's ordered position.
    #[serde(default)]
    pub arguments: Vec<RoutineArgument>,
    /// Required named mutable scalar destinations populated when the routine returns.
    #[serde(default)]
    pub outputs: Vec<RoutineOutputBinding>,
}

/// One read-only scalar input declared by an action-routine definition.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineInput {
    pub name: String,
    pub kind: VarType,
}

/// One mutable scalar output declared by an action-routine definition.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineOutput {
    pub name: String,
    pub kind: VarType,
}

/// A caller-owned variable receiving one routine output after the call returns.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineOutputBinding {
    pub output: String,
    pub destination: String,
}

/// Typed scalar bound to a routine input at a callsite. The untagged encoding
/// keeps legacy numeric argument JSON unchanged while predicates encode Flag.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum RoutineArgumentValue {
    Numeric(Expr),
    Flag(Predicate),
}

impl From<Expr> for RoutineArgumentValue {
    fn from(value: Expr) -> Self {
        Self::Numeric(value)
    }
}

impl From<Predicate> for RoutineArgumentValue {
    fn from(value: Predicate) -> Self {
        Self::Flag(value)
    }
}

/// One typed scalar bound to a routine input at a callsite.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineArgument {
    pub input: String,
    pub value: RoutineArgumentValue,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveGraph {
    pub version: u32,
    #[serde(default)]
    pub tracks: Vec<MoveTrack>,
    /// Editor-only canvas positions keyed as `node:<id>`, `trigger:<name>`, or
    /// `action:<trigger-name>:<index>`. These coordinates do not affect behavior.
    #[serde(default)]
    pub positions: BTreeMap<String, [f32; 2]>,
    #[serde(default)]
    pub variables: Vec<VarDef>,
    /// Optional per-declaration scope. Missing entries retain the historical
    /// character-shared behavior so older project files keep their semantics.
    #[serde(default)]
    pub variable_scopes: BTreeMap<String, VariableScope>,
    #[serde(default)]
    pub triggers: Vec<CustomTrigger>,
    /// Reusable flat action bodies. These are expanded before live lowering.
    #[serde(default)]
    pub routines: BTreeMap<String, Vec<Action>>,
    /// Ordered, typed input signatures keyed by character-wide routine name.
    /// Missing signatures retain the existing no-argument routine behavior.
    #[serde(default)]
    pub routine_inputs: BTreeMap<String, Vec<RoutineInput>>,
    /// Ordered, typed mutable output signatures keyed by routine name.
    #[serde(default)]
    pub routine_outputs: BTreeMap<String, Vec<RoutineOutput>>,
    /// Calls keyed by trigger name. Vector order is stable for calls at an anchor.
    #[serde(default)]
    pub routine_calls: BTreeMap<String, Vec<RoutineCall>>,
    /// Calls keyed by routine definition name. These share the same before-action
    /// anchors and vector ordering as trigger calls.
    #[serde(default)]
    pub routine_calls_in_routines: BTreeMap<String, Vec<RoutineCall>>,
}

impl Default for MoveGraph {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            tracks: Vec::new(),
            positions: BTreeMap::new(),
            variables: Vec::new(),
            variable_scopes: BTreeMap::new(),
            triggers: Vec::new(),
            routines: BTreeMap::new(),
            routine_inputs: BTreeMap::new(),
            routine_outputs: BTreeMap::new(),
            routine_calls: BTreeMap::new(),
            routine_calls_in_routines: BTreeMap::new(),
        }
    }
}

impl MoveGraph {
    pub fn scope(&self, name: &str) -> VariableScope {
        self.variable_scopes.get(name).copied().unwrap_or_default()
    }
}
