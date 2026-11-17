//! Typed editing and desktop simulation helpers for reusable behavior controllers.
//!
//! These helpers operate on the validated behavior IR. They do not read or
//! write native fighter resources; fighter-specific symbols are admitted only
//! through the pinned registry and adapter gates.

use std::collections::BTreeMap;

use super::behavior::{Action, BehaviorGraph, ConstValue, Expr, Predicate, VarType};
use super::hooks::HookName;
use super::native_inputs::{ItemAction, NativeInput};
use super::registry::{self, IdentifierKind};

const TIMER_VAR: &str = "remaining";
const TIMER_COUNT_VAR: &str = "elapsed_events";
const TIMER_READY_VAR: &str = "ready";

/// Closed action choices suitable for a controller action picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerActionKind {
    Reflector,
    Absorber,
    GrabBox,
    SetAttackDamage,
    RequestStatus,
    TransitionTerm,
    TransitionGroup,
    EnableCancel,
    TryCommonTransition,
    ChangeKinetic,
    ConfigureEnergy,
    SetSituation,
    SetGroundPolicy,
    PlayMotion,
    InitializeStatus,
    ChangeStatusNow,
    Set,
    Add,
    Raise,
    Clear,
    RequestFighterStatus,
    GenerateArticle,
    RemoveArticle,
    RemoveNewestArticle,
    MoveArticle,
    SetObjectSpeed,
    RemoveObject,
    RequestArticleStatus,
    Resource,
    ChooseHeroMenuSpell,
    AddHeldOpponentDamage,
    ThrowHeldOpponent,
    ReleaseHeldOpponent,
    AddDamage,
    CombatPolicy,
    CancelIncomingHit,
    SetMeshVisibility,
    SetMotionRate,
    AddSpeed,
    SetProjectileValue,
    SetNative,
    AddNative,
    SetNativeFlag,
    SetSpeed,
    SetGravity,
    MoveBy,
    SetFacing,
    HoldItem,
    ThrowItem,
    DropItem,
    RemoveItem,
}

impl ControllerActionKind {
    pub const ALL: [Self; 51] = [
        Self::Reflector,
        Self::Absorber,
        Self::GrabBox,
        Self::SetAttackDamage,
        Self::RequestStatus,
        Self::TransitionTerm,
        Self::TransitionGroup,
        Self::EnableCancel,
        Self::TryCommonTransition,
        Self::ChangeKinetic,
        Self::ConfigureEnergy,
        Self::SetSituation,
        Self::SetGroundPolicy,
        Self::PlayMotion,
        Self::InitializeStatus,
        Self::ChangeStatusNow,
        Self::Set,
        Self::Add,
        Self::Raise,
        Self::Clear,
        Self::RequestFighterStatus,
        Self::SetMotionRate,
        Self::SetMeshVisibility,
        Self::AddSpeed,
        Self::AddDamage,
        Self::CombatPolicy,
        Self::CancelIncomingHit,
        Self::AddHeldOpponentDamage,
        Self::ThrowHeldOpponent,
        Self::ReleaseHeldOpponent,
        Self::GenerateArticle,
        Self::RemoveArticle,
        Self::RemoveNewestArticle,
        Self::MoveArticle,
        Self::SetObjectSpeed,
        Self::RemoveObject,
        Self::SetProjectileValue,
        Self::RequestArticleStatus,
        Self::SetSpeed,
        Self::SetGravity,
        Self::MoveBy,
        Self::SetFacing,
        Self::HoldItem,
        Self::ThrowItem,
        Self::DropItem,
        Self::RemoveItem,
        Self::SetNative,
        Self::AddNative,
        Self::SetNativeFlag,
        Self::Resource,
        Self::ChooseHeroMenuSpell,
    ];

    /// Heading the picker lists this choice under.
    pub fn group(self) -> &'static str {
        match self {
            Self::Resource | Self::ChooseHeroMenuSpell => "Character specials",
            Self::Reflector
            | Self::Absorber
            | Self::GrabBox
            | Self::SetAttackDamage
            | Self::RequestStatus
            | Self::TransitionTerm
            | Self::TransitionGroup
            | Self::EnableCancel
            | Self::TryCommonTransition
            | Self::ChangeKinetic
            | Self::ConfigureEnergy
            | Self::SetSituation
            | Self::SetGroundPolicy
            | Self::PlayMotion
            | Self::InitializeStatus
            | Self::ChangeStatusNow => "Status phases",
            Self::Set | Self::Add | Self::Raise | Self::Clear => "Variables",
            Self::RequestFighterStatus | Self::SetMotionRate | Self::SetMeshVisibility => "Fighter",
            Self::AddSpeed | Self::SetSpeed | Self::SetGravity | Self::MoveBy | Self::SetFacing => {
                "Movement"
            }
            Self::AddHeldOpponentDamage | Self::ThrowHeldOpponent | Self::ReleaseHeldOpponent => {
                "Held opponent"
            }
            Self::AddDamage | Self::CombatPolicy | Self::CancelIncomingHit => "Damage & armor",
            Self::GenerateArticle
            | Self::RemoveArticle
            | Self::RemoveNewestArticle
            | Self::MoveArticle
            | Self::SetObjectSpeed
            | Self::RemoveObject
            | Self::RequestArticleStatus => "Articles",
            Self::HoldItem | Self::ThrowItem | Self::DropItem | Self::RemoveItem => "Items",
            Self::SetProjectileValue => "Articles",
            Self::SetNative | Self::AddNative | Self::SetNativeFlag => "Game values",
        }
    }
}

/// A typed user-authored action. Symbolic article targets remain exact registry
/// symbols and are checked against the selected fighter before application.
#[derive(Debug, Clone, PartialEq)]
pub enum ControllerAction {
    Status(super::status_commands::Command<Expr>),
    AddHeldOpponentDamage {
        amount: Expr,
    },
    ThrowHeldOpponent {
        direction: super::native_inputs::ThrowDirection,
    },
    ReleaseHeldOpponent,
    /// Values are numbers, variables, game values, random rolls or sums of them.
    Set {
        variable: String,
        value: Expr,
    },
    Add {
        variable: String,
        value: Expr,
    },
    Raise {
        variable: String,
    },
    Clear {
        variable: String,
    },
    RequestFighterStatus {
        symbol: String,
    },
    GenerateArticle {
        symbol: String,
    },
    RemoveArticle {
        symbol: String,
    },
    RemoveNewestArticle {
        symbol: String,
    },
    ArticleMovement {
        symbol: String,
        op: super::native_inputs::MovementAction,
    },
    /// Set the speed of, or remove, the newest live object of an owned article kind.
    Object {
        symbol: String,
        op: super::native_inputs::ObjectOp<Expr>,
    },
    RequestArticleStatus {
        symbol: String,
        status: String,
    },
    /// Add to or set one row of the native resource table.
    Resource(super::resource_commands::Command<Expr>),
    ChooseHeroMenuSpell {
        slot: u8,
    },
    MeshVisibility {
        mesh: String,
        visible: bool,
    },
    /// Signed percent delta (-999..=999); healing is clamped to current percent.
    AddDamage {
        amount: Expr,
    },
    /// How this fighter takes or deals hits, until the scope ends.
    CombatPolicy {
        scope: super::native_inputs::PolicyScope,
        op: super::native_inputs::CombatOp<Expr>,
    },
    CancelIncomingHit,
    SetMotionRate {
        rate: Expr,
    },
    /// Facing-relative speed added this frame.
    AddSpeed {
        x: Expr,
        y: Expr,
    },
    /// Write a plain game (WorkModule) value; `value` has the input's kind.
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
        on: bool,
    },
    Movement(super::native_inputs::MovementAction),
    Item(ItemAction),
}

/// The editable policy for the built-in portable timer controller.
#[derive(Debug, Clone, PartialEq)]
pub struct TimerControllerConfig {
    pub duration_frames: u32,
    pub repeats: bool,
    pub expiry_actions: Vec<ControllerAction>,
}

/// A successful typed edit carries both the updated graph and the values that
/// were actually applied, allowing callers to confirm persisted changes.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedController<T> {
    pub graph: BehaviorGraph,
    pub applied: T,
}

/// Read the reusable timer's settings from a graph. The portable timer uses
/// fixed names so serialized templates can be inspected and reconfigured
/// without out-of-band metadata.
pub fn extract_timer(graph: &BehaviorGraph) -> Result<TimerControllerConfig, String> {
    let (timer_var, count_var, ready_var, expiry) = timer_shape(graph)?;
    let ConstValue::Int(duration) = timer_var.initial else {
        return Err("timer duration is not an integer frame count".into());
    };
    if duration <= 0 {
        return Err("timer duration must be greater than zero".into());
    }
    let stock_start = &graph.transitions[stock_start_transition_index(graph)?];
    if !stock_start.actions.iter().any(|action| {
        matches!(action, Action::Set { var, value: Expr::Const(ConstValue::Int(value)) }
            if var == TIMER_VAR && *value == duration)
    }) {
        return Err("stock-start reset duration does not match the timer duration".into());
    }
    let timer_write = expiry.actions.iter().rev().find_map(|action| match action {
        Action::Set {
            var,
            value: Expr::Const(ConstValue::Int(value)),
        } if var == TIMER_VAR => Some(*value),
        _ => None,
    });
    let repeats = match timer_write {
        Some(value) if value == duration => true,
        Some(0) => false,
        _ => return Err("timer expiry policy is malformed".into()),
    };
    let mut internal_prefix = vec![
        Action::Add {
            var: TIMER_COUNT_VAR.into(),
            value: Expr::Const(ConstValue::Int(1)),
        },
        Action::Raise {
            var: TIMER_READY_VAR.into(),
        },
    ];
    // Verify that the named state is the standard observable, per-life timer.
    if count_var.kind != VarType::Int || ready_var.kind != VarType::Flag {
        return Err("timer event-count or ready variable has the wrong type".into());
    }
    let timer_set = Action::Set {
        var: TIMER_VAR.into(),
        value: Expr::Const(ConstValue::Int(timer_write.unwrap_or_default())),
    };
    internal_prefix.push(timer_set);
    if expiry.actions.len() < internal_prefix.len()
        || expiry.actions[..internal_prefix.len()] != internal_prefix
    {
        return Err("timer expiry actions do not match the reusable timer contract".into());
    }
    let expiry_actions = expiry.actions[internal_prefix.len()..]
        .iter()
        .map(ControllerAction::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TimerControllerConfig {
        duration_frames: duration as u32,
        repeats,
        expiry_actions,
    })
}

/// Update the reusable timer while retaining unrelated graph states,
/// variables, and hook bindings. Its duration, expiry policy, and stock-start
/// reset duration are changed together.
pub fn configure_timer(
    graph: &BehaviorGraph,
    fighter: &str,
    config: TimerControllerConfig,
) -> Result<AppliedController<TimerControllerConfig>, String> {
    if config.duration_frames == 0
        || config.duration_frames > super::live_model::MAX_TIMER_FRAMES as u32
    {
        return Err(format!(
            "timer duration must be between 1 and {} frames",
            super::live_model::MAX_TIMER_FRAMES
        ));
    }
    let mut updated = graph.clone();
    timer_shape(&updated)?;
    let timer_index = updated
        .vars
        .iter()
        .position(|var| var.name == TIMER_VAR && var.kind == VarType::Timer)
        .ok_or_else(|| "graph has no reusable timer named `remaining`".to_string())?;
    let expiry_index = updated
        .emission_plan
        .bindings
        .iter()
        .filter(|binding| binding.hook == HookName::OnTimeout)
        .flat_map(|binding| binding.transitions.iter().copied())
        .next()
        .ok_or_else(|| "reusable timer needs one onTimeout transition".to_string())?;
    let stock_start_index = stock_start_transition_index(&updated)?;
    updated.vars[timer_index].initial = ConstValue::Int(config.duration_frames as i32);
    let mut stock_timer_sets = 0;
    for action in &mut updated.transitions[stock_start_index].actions {
        if let Action::Set {
            var,
            value: Expr::Const(ConstValue::Int(value)),
        } = action
        {
            if var == TIMER_VAR {
                *value = config.duration_frames as i32;
                stock_timer_sets += 1;
            }
        }
    }
    if stock_timer_sets != 1 {
        return Err("stock-start transition must set the reusable timer exactly once".into());
    }

    let mut actions = vec![
        Action::Add {
            var: TIMER_COUNT_VAR.into(),
            value: Expr::Const(ConstValue::Int(1)),
        },
        Action::Raise {
            var: TIMER_READY_VAR.into(),
        },
    ];
    for action in &config.expiry_actions {
        if action.targets_timer_control() {
            return Err("expiry actions cannot write the timer, event count, or ready flag".into());
        }
    }
    actions.push(Action::Set {
        var: TIMER_VAR.into(),
        value: Expr::Const(ConstValue::Int(if config.repeats {
            config.duration_frames as i32
        } else {
            0
        })),
    });
    for action in &config.expiry_actions {
        actions.push(action.to_ir(fighter)?);
    }
    updated.transitions[expiry_index].actions = actions;
    validate_for_target(&updated, fighter)?;
    Ok(AppliedController {
        graph: updated,
        applied: config,
    })
}

impl ControllerAction {
    fn targets_timer_control(&self) -> bool {
        match self {
            Self::Set { variable, .. }
            | Self::Add { variable, .. }
            | Self::Raise { variable }
            | Self::Clear { variable } => matches!(
                variable.as_str(),
                TIMER_VAR | TIMER_COUNT_VAR | TIMER_READY_VAR
            ),
            Self::RequestFighterStatus { .. }
            | Self::GenerateArticle { .. }
            | Self::RemoveArticle { .. }
            | Self::RemoveNewestArticle { .. }
            | Self::ArticleMovement { .. }
            | Self::Object { .. }
            | Self::RequestArticleStatus { .. }
            | Self::SetNative { .. }
            | Self::AddNative { .. }
            | Self::SetNativeFlag { .. }
            | Self::AddHeldOpponentDamage { .. }
            | Self::AddDamage { .. }
            | Self::CombatPolicy { .. }
            | Self::ThrowHeldOpponent { .. }
            | Self::ReleaseHeldOpponent
            | Self::CancelIncomingHit
            | Self::SetMotionRate { .. }
            | Self::AddSpeed { .. }
            | Self::Movement(_)
            | Self::Status(_)
            | Self::Resource(_)
            | Self::Item(_)
            | Self::MeshVisibility { .. }
            | Self::ChooseHeroMenuSpell { .. } => false,
        }
    }

    pub(crate) fn to_ir(&self, fighter: &str) -> Result<Action, String> {
        Ok(match self {
            Self::Status(c) => Action::Status(c.clone()),
            Self::Set { variable, value } => Action::Set {
                var: variable.clone(),
                value: value.clone(),
            },
            Self::RemoveNewestArticle { symbol } => {
                registry::resolve(fighter, symbol, IdentifierKind::Article)
                    .map_err(|error| error.to_string())?;
                Action::RemoveNewestArticle {
                    symbol: symbol.clone(),
                }
            }
            Self::ArticleMovement { symbol, op } => {
                registry::resolve(fighter, symbol, IdentifierKind::Article)
                    .map_err(|error| error.to_string())?;
                op.validate_for_article()?;
                Action::ArticleMovement {
                    symbol: symbol.clone(),
                    op: *op,
                }
            }
            Self::Object { symbol, op } => {
                registry::resolve(fighter, symbol, IdentifierKind::Article)
                    .map_err(|error| error.to_string())?;
                Action::Object {
                    symbol: symbol.clone(),
                    op: op.clone(),
                }
            }
            Self::RequestArticleStatus { symbol, status } => {
                registry::resolve(fighter, symbol, IdentifierKind::Article)
                    .map_err(|error| error.to_string())?;
                registry::resolve(fighter, status, IdentifierKind::WeaponStatus)
                    .map_err(|error| error.to_string())?;
                Action::RequestArticleStatus {
                    symbol: symbol.clone(),
                    status: status.clone(),
                }
            }
            Self::AddHeldOpponentDamage { amount } => Action::AddHeldOpponentDamage {
                amount: amount.clone(),
            },
            Self::AddDamage { amount } => Action::AddDamage {
                amount: amount.clone(),
            },
            Self::CombatPolicy { scope, op } => {
                super::native_inputs::validate_policy_scope(*scope)?;
                Action::CombatPolicy {
                    scope: *scope,
                    op: op.clone(),
                }
            }
            Self::ThrowHeldOpponent { direction } => Action::ThrowHeldOpponent {
                direction: *direction,
            },
            Self::ReleaseHeldOpponent => Action::ReleaseHeldOpponent,
            Self::CancelIncomingHit => Action::CancelIncomingHit,
            Self::Movement(op) => {
                op.validate()?;
                Action::Movement(*op)
            }
            Self::Resource(command) => {
                command.check_owner(fighter)?;
                if let Expr::Const(ConstValue::Float(v)) = command.amount {
                    command.try_map(|_| Ok::<_, String>(v))?.validate(fighter)?;
                }
                Action::Resource(command.clone())
            }
            Self::ChooseHeroMenuSpell { slot } => {
                (super::hero_menu_commands::Command::Choose { slot: *slot }).validate(fighter)?;
                Action::ChooseHeroMenuSpell { slot: *slot }
            }
            Self::Item(op) => {
                op.validate()?;
                Action::Item(op.clone())
            }
            Self::MeshVisibility { mesh, visible } => {
                super::native_inputs::validate_mesh_name(mesh)?;
                Action::MeshVisibility {
                    mesh: mesh.clone(),
                    visible: *visible,
                }
            }
            Self::SetMotionRate { rate } => Action::SetMotionRate { rate: rate.clone() },
            Self::AddSpeed { x, y } => Action::AddSpeed {
                x: x.clone(),
                y: y.clone(),
            },
            Self::SetNative { input, value } => Action::SetNative {
                input: input.clone(),
                value: value.clone(),
            },
            Self::AddNative { input, value } => Action::AddNative {
                input: input.clone(),
                value: value.clone(),
            },
            Self::SetNativeFlag { input, on } => Action::SetNativeFlag {
                input: input.clone(),
                value: if *on {
                    Predicate::Not(Box::new(super::graph_view::never()))
                } else {
                    super::graph_view::never()
                },
            },
            Self::Add { variable, value } => Action::Add {
                var: variable.clone(),
                value: value.clone(),
            },
            Self::Raise { variable } => Action::Raise {
                var: variable.clone(),
            },
            Self::Clear { variable } => Action::Clear {
                var: variable.clone(),
            },
            Self::RequestFighterStatus { symbol } => {
                registry::resolve(fighter, symbol, IdentifierKind::FighterStatus)
                    .map_err(|error| error.to_string())?;
                Action::RequestFighterStatus {
                    symbol: symbol.clone(),
                }
            }
            Self::GenerateArticle { symbol } => {
                registry::resolve(fighter, symbol, IdentifierKind::Article)
                    .map_err(|error| error.to_string())?;
                Action::GenerateArticle {
                    symbol: symbol.clone(),
                }
            }
            Self::RemoveArticle { symbol } => {
                registry::resolve(fighter, symbol, IdentifierKind::Article)
                    .map_err(|error| error.to_string())?;
                Action::RemoveArticle {
                    symbol: symbol.clone(),
                }
            }
        })
    }
}

impl TryFrom<&Action> for ControllerAction {
    type Error = String;

    fn try_from(action: &Action) -> Result<Self, Self::Error> {
        match action {
            Action::Status(c) => Ok(Self::Status(c.clone())),
            Action::Set { var, value } => Ok(Self::Set {
                variable: var.clone(),
                value: value.clone(),
            }),
            Action::Add { var, value } => Ok(Self::Add {
                variable: var.clone(),
                value: value.clone(),
            }),
            Action::Raise { var } => Ok(Self::Raise {
                variable: var.clone(),
            }),
            Action::Clear { var } => Ok(Self::Clear {
                variable: var.clone(),
            }),
            Action::RequestFighterStatus { symbol } => Ok(Self::RequestFighterStatus {
                symbol: symbol.clone(),
            }),
            Action::GenerateArticle { symbol } => Ok(Self::GenerateArticle {
                symbol: symbol.clone(),
            }),
            Action::RemoveArticle { symbol } => Ok(Self::RemoveArticle {
                symbol: symbol.clone(),
            }),
            Action::RemoveNewestArticle { symbol } => Ok(Self::RemoveNewestArticle {
                symbol: symbol.clone(),
            }),
            Action::ArticleMovement { symbol, op } => Ok(Self::ArticleMovement {
                symbol: symbol.clone(),
                op: *op,
            }),
            Action::Object { symbol, op } => Ok(Self::Object {
                symbol: symbol.clone(),
                op: op.clone(),
            }),
            Action::RequestArticleStatus { symbol, status } => Ok(Self::RequestArticleStatus {
                symbol: symbol.clone(),
                status: status.clone(),
            }),
            Action::AddHeldOpponentDamage { amount } => Ok(Self::AddHeldOpponentDamage {
                amount: amount.clone(),
            }),
            Action::AddDamage { amount } => Ok(Self::AddDamage {
                amount: amount.clone(),
            }),
            Action::CombatPolicy { scope, op } => Ok(Self::CombatPolicy {
                scope: *scope,
                op: op.clone(),
            }),
            Action::ThrowHeldOpponent { direction } => Ok(Self::ThrowHeldOpponent {
                direction: *direction,
            }),
            Action::ReleaseHeldOpponent => Ok(Self::ReleaseHeldOpponent),
            Action::CancelIncomingHit => Ok(Self::CancelIncomingHit),
            Action::Movement(op) => Ok(Self::Movement(*op)),
            Action::Resource(command) => Ok(Self::Resource(command.clone())),
            Action::ChooseHeroMenuSpell { slot } => Ok(Self::ChooseHeroMenuSpell { slot: *slot }),
            Action::Item(op) => Ok(Self::Item(op.clone())),
            Action::MeshVisibility { mesh, visible } => Ok(Self::MeshVisibility {
                mesh: mesh.clone(),
                visible: *visible,
            }),
            Action::SetMotionRate { rate } => Ok(Self::SetMotionRate { rate: rate.clone() }),
            Action::AddSpeed { x, y } => Ok(Self::AddSpeed {
                x: x.clone(),
                y: y.clone(),
            }),
            Action::SetNative { input, value } => Ok(Self::SetNative {
                input: input.clone(),
                value: value.clone(),
            }),
            Action::AddNative { input, value } => Ok(Self::AddNative {
                input: input.clone(),
                value: value.clone(),
            }),
            Action::SetNativeFlag { input, value } if value == &super::graph_view::never() => {
                Ok(Self::SetNativeFlag {
                    input: input.clone(),
                    on: false,
                })
            }
            Action::SetNativeFlag {
                input,
                value: Predicate::Not(inner),
            } if **inner == super::graph_view::never() => Ok(Self::SetNativeFlag {
                input: input.clone(),
                on: true,
            }),
            _ => Err("timer expiry contains an action outside the typed controller set".into()),
        }
    }
}

fn timer_shape(
    graph: &BehaviorGraph,
) -> Result<
    (
        &super::behavior::VarDef,
        &super::behavior::VarDef,
        &super::behavior::VarDef,
        &super::behavior::Transition,
    ),
    String,
> {
    let timer = graph
        .vars
        .iter()
        .find(|var| var.name == TIMER_VAR && var.kind == VarType::Timer)
        .ok_or_else(|| "graph has no reusable timer named `remaining`".to_string())?;
    let count = graph
        .vars
        .iter()
        .find(|var| var.name == TIMER_COUNT_VAR)
        .ok_or_else(|| "graph has no per-life `elapsed_events` counter".to_string())?;
    let ready = graph
        .vars
        .iter()
        .find(|var| var.name == TIMER_READY_VAR)
        .ok_or_else(|| "graph has no expiry `ready` flag".to_string())?;
    let timeout_indices = graph
        .emission_plan
        .bindings
        .iter()
        .filter(|binding| binding.hook == HookName::OnTimeout)
        .flat_map(|binding| binding.transitions.iter().copied())
        .collect::<Vec<_>>();
    if timeout_indices.len() != 1 {
        return Err("reusable timer needs exactly one onTimeout transition".into());
    }
    let transition = graph
        .transitions
        .get(timeout_indices[0])
        .ok_or_else(|| "onTimeout transition index is out of range".to_string())?;
    if !matches!(&transition.guard, Predicate::Expired(name) if name == TIMER_VAR) {
        return Err("onTimeout transition must be guarded by expiry of `remaining`".into());
    }
    Ok((timer, count, ready, transition))
}

fn stock_start_transition_index(graph: &BehaviorGraph) -> Result<usize, String> {
    let indices = graph
        .emission_plan
        .bindings
        .iter()
        .filter(|binding| binding.hook == HookName::OnStockStart)
        .flat_map(|binding| binding.transitions.iter().copied())
        .collect::<Vec<_>>();
    if indices.len() != 1 {
        return Err("reusable timer needs exactly one onStockStart transition".into());
    }
    let transition = graph
        .transitions
        .get(indices[0])
        .ok_or_else(|| "onStockStart transition index is out of range".to_string())?;
    let resets_count = transition.actions.iter().any(|action| {
        matches!(action, Action::Set { var, value: Expr::Const(ConstValue::Int(0)) }
            if var == TIMER_COUNT_VAR)
    });
    let clears_ready = transition
        .actions
        .iter()
        .any(|action| matches!(action, Action::Clear { var } if var == TIMER_READY_VAR));
    if !resets_count || !clears_ready {
        return Err("onStockStart must reset the event count and ready flag".into());
    }
    Ok(indices[0])
}

fn validate_for_target(graph: &BehaviorGraph, fighter: &str) -> Result<(), String> {
    let template = super::templates::BehaviorTemplate::from_graph(
        "Controller edit",
        Some(fighter),
        graph.clone(),
    )?;
    template.instantiate(fighter).map(|_| ())
}

/// Desktop simulation result with per-frame snapshots. `flags` is a compact
/// projection for timeline consumers; `step` contains ordered IR transitions.
#[derive(Debug, Clone, PartialEq)]
pub struct ControllerFrame {
    pub frame: u32,
    pub flags: BTreeMap<String, bool>,
    pub step: super::live_model::StepResult,
}

/// Simulate the actual graph for a bounded number of frame ticks using the
/// same pure evaluator as the plugin behavior runtime.
pub fn simulate_frames(
    fighter: &str,
    graph: &BehaviorGraph,
    frames: u32,
) -> Result<Vec<ControllerFrame>, String> {
    const MAX_PREVIEW_FRAMES: u32 = 36_000;
    if frames > MAX_PREVIEW_FRAMES {
        return Err(format!("preview is limited to {MAX_PREVIEW_FRAMES} frames"));
    }
    let program = super::live_protocol::program_from_graph(fighter, graph)?;
    let mut state = program.initial_state();
    (0..frames)
        .map(|frame| {
            let step = program
                .step_frame(&mut state)
                .map_err(|error| error.to_string())?;
            let flags = step
                .after
                .values
                .iter()
                .filter_map(|(name, value)| match value {
                    super::live_model::TypedValue::Flag(value) => Some((name.clone(), *value)),
                    _ => None,
                })
                .collect();
            Ok(ControllerFrame {
                frame: frame + 1,
                flags,
                step,
            })
        })
        .collect()
}

#[cfg(test)]
/// Append one typed user action to an existing transition. Validation is
/// transactional: the input graph is unchanged on any failure.
pub fn append_action(
    graph: &BehaviorGraph,
    fighter: &str,
    transition_index: usize,
    action: ControllerAction,
) -> Result<AppliedController<ControllerAction>, String> {
    let mut updated = graph.clone();
    reject_timer_control_write(graph, transition_index, &action)?;
    let transition = updated
        .transitions
        .get_mut(transition_index)
        .ok_or_else(|| format!("transition index {transition_index} is out of range"))?;
    transition.actions.push(action.to_ir(fighter)?);
    validate_for_target(&updated, fighter)?;
    Ok(AppliedController {
        graph: updated,
        applied: action,
    })
}

#[cfg(test)]
fn reject_timer_control_write(
    graph: &BehaviorGraph,
    _transition_index: usize,
    action: &ControllerAction,
) -> Result<(), String> {
    if has_reusable_timer_lifecycle(graph) && action.targets_timer_control() {
        return Err("timer actions cannot write the timer, event count, or ready flag".into());
    }
    Ok(())
}

#[cfg(test)]
/// Recognize the reserved timer prefix without requiring its editable suffix
/// to be valid. This keeps generic edits from shifting or replacing the
/// timer's own bookkeeping even while a user is repairing the suffix.
fn timer_internal_prefix_len(graph: &BehaviorGraph, transition_index: usize) -> usize {
    let timeout_is_transition = graph
        .emission_plan
        .bindings
        .iter()
        .filter(|binding| binding.hook == HookName::OnTimeout)
        .flat_map(|binding| binding.transitions.iter().copied())
        .any(|index| index == transition_index);
    if !timeout_is_transition {
        return 0;
    }
    let Some(transition) = graph.transitions.get(transition_index) else {
        return 0;
    };
    if !matches!(&transition.guard, Predicate::Expired(name) if name == TIMER_VAR)
        || !graph
            .vars
            .iter()
            .any(|var| var.name == TIMER_VAR && var.kind == VarType::Timer)
    {
        return 0;
    }
    let expected = [
        Action::Add {
            var: TIMER_COUNT_VAR.into(),
            value: Expr::Const(ConstValue::Int(1)),
        },
        Action::Raise {
            var: TIMER_READY_VAR.into(),
        },
    ];
    if transition.actions.len() < 3
        || transition.actions[..2] != expected
        || !matches!(
            &transition.actions[2],
            Action::Set {
                var,
                value: Expr::Const(ConstValue::Int(_))
            } if var == TIMER_VAR
        )
    {
        return 0;
    }
    3
}

#[cfg(test)]
fn has_reusable_timer_lifecycle(graph: &BehaviorGraph) -> bool {
    if !graph
        .vars
        .iter()
        .any(|var| var.name == TIMER_VAR && var.kind == VarType::Timer)
        || !graph
            .vars
            .iter()
            .any(|var| var.name == TIMER_COUNT_VAR && var.kind == VarType::Int)
        || !graph
            .vars
            .iter()
            .any(|var| var.name == TIMER_READY_VAR && var.kind == VarType::Flag)
    {
        return false;
    }
    let timeout_indices = graph
        .emission_plan
        .bindings
        .iter()
        .filter(|binding| binding.hook == HookName::OnTimeout)
        .flat_map(|binding| binding.transitions.iter().copied())
        .collect::<Vec<_>>();
    if timeout_indices.len() != 1 || timer_internal_prefix_len(graph, timeout_indices[0]) != 3 {
        return false;
    }
    let Ok(stock_start_index) = stock_start_transition_index(graph) else {
        return false;
    };
    graph.transitions[stock_start_index]
        .actions
        .iter()
        .any(|action| {
            matches!(
                action,
                Action::Set {
                    var,
                    value: Expr::Const(ConstValue::Int(_))
                } if var == TIMER_VAR
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::native_inputs::{Facing, MovementAction};
    use crate::gimmicks::templates::Preset;

    #[test]
    fn timer_settings_are_extracted_and_reconfigured_without_touching_other_graph_data() {
        let source = Preset::Timer.graph();
        let before_bindings = source.emission_plan.bindings.clone();
        let before_transition = source.transitions[0].clone();
        let config = TimerControllerConfig {
            duration_frames: 3,
            repeats: false,
            expiry_actions: vec![],
        };
        let changed = configure_timer(&source, "mario", config.clone()).unwrap();
        assert_eq!(changed.applied, config);
        assert_eq!(changed.graph.emission_plan.bindings, before_bindings);
        assert_eq!(changed.graph.transitions[0], before_transition);
        assert_eq!(extract_timer(&changed.graph).unwrap(), config);
        assert_eq!(changed.graph.vars[0].initial, ConstValue::Int(3));
        assert!(changed.graph.transitions[1].actions.contains(&Action::Set {
            var: TIMER_VAR.into(),
            value: Expr::Const(ConstValue::Int(0)),
        }));
    }

    #[test]
    fn timer_duration_must_be_positive_and_bounded() {
        let graph = Preset::Timer.graph();
        let mut config = extract_timer(&graph).unwrap();
        config.duration_frames = 0;
        assert!(configure_timer(&graph, "mario", config.clone())
            .unwrap_err()
            .contains("between 1 and 1000000"));
        config.duration_frames = crate::gimmicks::live_model::MAX_TIMER_FRAMES as u32 + 1;
        assert!(configure_timer(&graph, "mario", config)
            .unwrap_err()
            .contains("1000000"));
    }

    #[test]
    fn shared_preview_reports_expiry_pulse_count_repeat_and_stock_reset() {
        use crate::gimmicks::live_model::{HookName as LiveHook, Value};

        let graph = configure_timer(
            &Preset::Timer.graph(),
            "mario",
            TimerControllerConfig {
                duration_frames: 3,
                repeats: true,
                expiry_actions: vec![],
            },
        )
        .unwrap()
        .graph;
        let frames = simulate_frames("mario", &graph, 4).unwrap();
        assert_eq!(frames.len(), 4);
        assert_eq!(frames[2].step.expired, vec![TIMER_VAR.to_owned()]);
        assert_eq!(frames[2].flags.get(TIMER_READY_VAR), Some(&true));
        assert_eq!(
            frames[2].step.after.values.get(TIMER_COUNT_VAR),
            Some(&Value::Int(1))
        );
        assert_eq!(frames[3].flags.get(TIMER_READY_VAR), Some(&false));

        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let mut state = program.initial_state();
        for _ in 0..3 {
            program.step_frame(&mut state).unwrap();
        }
        program.step(&mut state, LiveHook::OnStockStart).unwrap();
        assert_eq!(state.values.get(TIMER_COUNT_VAR), Some(&Value::Int(0)));
        assert_eq!(state.values.get(TIMER_READY_VAR), Some(&Value::Flag(false)));
        assert_eq!(state.values.get(TIMER_VAR), Some(&Value::Int(3)));
    }

    #[test]
    fn charge_preset_exposes_full_gauge_before_its_ready_pulse_and_reset() {
        use crate::gimmicks::live_model::Value;

        let graph = Preset::Charge.graph();
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let mut state = program.initial_state();
        for _ in 0..100 {
            program.step_frame(&mut state).unwrap();
        }
        assert_eq!(state.values.get("gauge"), Some(&Value::Float(100.0)));
        assert_eq!(state.values.get("ready"), Some(&Value::Flag(false)));
        program.step_frame(&mut state).unwrap();
        assert_eq!(state.values.get("gauge"), Some(&Value::Float(0.0)));
        assert_eq!(state.values.get("ready"), Some(&Value::Flag(true)));
        program.step_frame(&mut state).unwrap();
        assert_eq!(state.values.get("gauge"), Some(&Value::Float(1.0)));
        assert_eq!(state.values.get("ready"), Some(&Value::Flag(false)));
    }

    #[test]
    fn typed_article_actions_resolve_exact_fighter_targets() {
        let graph = Preset::KoTally.graph();
        let original_actions = graph.transitions[0].actions.clone();
        let cloud_article = registry::entries_for("cloud", IdentifierKind::Article)
            .next()
            .unwrap()
            .symbol;
        let mario_article = registry::entries_for("mario", IdentifierKind::Article)
            .next()
            .unwrap()
            .symbol;
        let result = append_action(
            &graph,
            "cloud",
            0,
            ControllerAction::GenerateArticle {
                symbol: cloud_article.into(),
            },
        )
        .unwrap();
        assert_eq!(
            result.applied,
            ControllerAction::GenerateArticle {
                symbol: cloud_article.into()
            }
        );
        assert!(append_action(
            &graph,
            "cloud",
            0,
            ControllerAction::RemoveArticle {
                symbol: mario_article.into(),
            },
        )
        .unwrap_err()
        .contains("belongs to fighter"));
        assert_eq!(graph.transitions[0].actions, original_actions);
    }

    #[test]
    fn item_actions_need_a_real_item_and_a_bounded_throw() {
        for op in [
            ItemAction::Hold {
                item: "ITEM_KIND_NONE".into(),
            },
            ItemAction::Hold {
                item: "FIGHTER_STATUS_KIND_WAIT".into(),
            },
            ItemAction::Throw {
                angle: 400.0,
                power: 1.0,
            },
            ItemAction::Throw {
                angle: 0.0,
                power: f32::NAN,
            },
        ] {
            assert!(ControllerAction::Item(op).to_ir("mario").is_err());
        }
    }

    #[test]
    fn suggested_action_values_are_closed_and_match_supported_ir_actions() {
        assert_eq!(ControllerActionKind::ALL.len(), 51);
        assert!(ControllerActionKind::ALL.contains(&ControllerActionKind::Resource));
        assert!(ControllerActionKind::ALL.contains(&ControllerActionKind::Set));
        assert!(ControllerActionKind::ALL.contains(&ControllerActionKind::GenerateArticle));
    }

    #[test]
    fn formula_and_game_value_actions_round_trip_through_the_ir() {
        for (fighter, action) in [
            (
                "mario",
                ControllerAction::Set {
                    variable: "count".into(),
                    value: Expr::RandomInt { min: 1, max: 6 },
                },
            ),
            (
                "mario",
                ControllerAction::Add {
                    variable: "gauge".into(),
                    value: Expr::Mul(
                        Box::new(Expr::PendingDamage),
                        Box::new(Expr::Const(ConstValue::Float(0.5))),
                    ),
                },
            ),
            (
                "mario",
                ControllerAction::SetNative {
                    input: NativeInput::from_registry(
                        "mario",
                        "WEAPON_MARIO_FIREBALL_INSTANCE_WORK_ID_INT_BOUND_COUNT",
                        crate::gimmicks::native_inputs::NativeKind::Int,
                    )
                    .unwrap(),
                    value: Expr::Const(ConstValue::Int(99)),
                },
            ),
            (
                "samus",
                ControllerAction::SetNative {
                    input: NativeInput::from_registry(
                        "samus",
                        "WEAPON_SAMUS_CSHOT_INSTANCE_WORK_ID_FLOAT_CHARGE",
                        crate::gimmicks::native_inputs::NativeKind::Float,
                    )
                    .unwrap(),
                    value: Expr::Const(ConstValue::Float(0.5)),
                },
            ),
            (
                "cloud",
                ControllerAction::SetNativeFlag {
                    input: NativeInput::from_registry(
                        "cloud",
                        "WEAPON_CLOUD_WAVE_INSTANCE_WORK_ID_FLAG_ATTACK_POWER_LOW",
                        crate::gimmicks::native_inputs::NativeKind::Flag,
                    )
                    .unwrap(),
                    on: true,
                },
            ),
            (
                "robot",
                ControllerAction::SetNative {
                    input: NativeInput::RobotBeamEnergy,
                    value: Expr::Var("gauge".into()),
                },
            ),
            (
                "robot",
                ControllerAction::AddNative {
                    input: NativeInput::RobotBeamEnergy,
                    value: Expr::Const(ConstValue::Float(-1.0)),
                },
            ),
            (
                "reflet",
                ControllerAction::SetNativeFlag {
                    input: NativeInput::RefletThunderSword,
                    on: true,
                },
            ),
            (
                "reflet",
                ControllerAction::SetNativeFlag {
                    input: NativeInput::RefletThunderSword,
                    on: false,
                },
            ),
            (
                "mario",
                ControllerAction::SetMotionRate {
                    rate: Expr::Var("gauge".into()),
                },
            ),
            (
                "mario",
                ControllerAction::AddSpeed {
                    x: Expr::Const(ConstValue::Float(1.5)),
                    y: Expr::StickY,
                },
            ),
            (
                "cloud",
                ControllerAction::RemoveNewestArticle {
                    symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
                },
            ),
            (
                "peach",
                ControllerAction::Item(ItemAction::Hold {
                    item: "ITEM_KIND_BEAMSWORD".into(),
                }),
            ),
            (
                "peach",
                ControllerAction::Item(ItemAction::Throw {
                    angle: 45.0,
                    power: 2.0,
                }),
            ),
            (
                "cloud",
                ControllerAction::RequestArticleStatus {
                    symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
                    status: "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR".into(),
                },
            ),
        ] {
            let ir = action.to_ir(fighter).unwrap();
            assert_eq!(ControllerAction::try_from(&ir).unwrap(), action, "{ir:?}");
        }
        for op in [
            MovementAction::MoveBy { dx: 5.0, dy: -5.0 },
            MovementAction::MoveBy { dx: 0.0, dy: 2.0 },
        ] {
            let action = ControllerAction::ArticleMovement {
                symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
                op,
            };
            let ir = action.to_ir("cloud").unwrap();
            assert_eq!(ControllerAction::try_from(&ir).unwrap(), action);
            assert!(action.to_ir("mario").is_err());
        }
        for op in [
            MovementAction::SetSpeed {
                x: Some(2.0),
                y: None,
            },
            MovementAction::SetGravity { enabled: false },
            MovementAction::SetFacing(Facing::Turn),
            MovementAction::MoveBy { dx: 99.0, dy: 0.0 },
        ] {
            let action = ControllerAction::ArticleMovement {
                symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
                op,
            };
            assert!(action.to_ir("cloud").is_err(), "{op:?}");
        }
        let wrong_owner = ControllerAction::RemoveNewestArticle {
            symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
        };
        assert!(wrong_owner.to_ir("mario").is_err());
    }
}
