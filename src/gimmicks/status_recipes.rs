//! Compact edits expand to the same visible, validated rules as manual authoring.
use crate::gimmicks::behavior::{
    Action, BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, Transition,
};
use crate::gimmicks::native_inputs::{ButtonEdge, InputButton, SituationKind};
use crate::gimmicks::status_commands::{
    Cliff, Command, CommonTransition, Correction, Energy, EnergyOp, Kinetic, MotionFrame,
};
use crate::gimmicks::status_model::Phase;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recipe {
    GroundAir,
    ChargeRelease,
    Counter,
    Reflector,
    Absorber,
    SuperArmor,
    Hover,
}
impl Recipe {
    pub const ALL: [Self; 7] = [
        Self::GroundAir,
        Self::ChargeRelease,
        Self::Counter,
        Self::Reflector,
        Self::Absorber,
        Self::SuperArmor,
        Self::Hover,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::GroundAir => "Ground/air special",
            Self::ChargeRelease => "Charge → release",
            Self::Counter => "Counter",
            Self::Reflector => "Reflect projectiles",
            Self::Absorber => "Absorb projectiles",
            Self::SuperArmor => "Super armor during move",
            Self::Hover => "Hover/float",
        }
    }
    /// What the recipe adds, in the terms the inspector shows.
    pub fn summary(self) -> &'static str {
        match self {
            Self::GroundAir => "plays the air animation in the air and swaps animation on landing or leaving the ground.",
            Self::ChargeRelease => "loops the animation while Special is held; letting go or 120 frames moves to the new release state.",
            Self::Counter => "hits from frame 6 until frame 28 are cancelled, save incoming damage, and move to the new release state; sets release hitboxes to the saved damage; edit its multiplier and counterattack animation.",
            Self::Reflector => "reflects projectiles during this status; edit size, damage and speed in its entry rule. The exit rule turns it off.",
            Self::Absorber => "absorbs projectiles during this status and heals 10% for each one; edit the heal in its absorb rule. The exit rule turns it off.",
            Self::SuperArmor => "takes hits without flinching for the whole status; edit its entry rule for an armor threshold, a damage multiplier or intangibility instead.",
            Self::Hover => "holding Special in the air turns gravity off.",
        }
    }
}

/// Fighter-specific presets offered beside the generic ones: the defensive
/// specials own Down Special; the steerable projectile drives an owned weapon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Defense {
    Counter,
    Reflector,
    Absorber,
    SuperArmor,
    SteerProjectile,
    FillMeterOnHit,
    ExtraJump,
    AutoFaceOpponent,
    MotionInput,
    CommandGrab,
}
impl Defense {
    pub const ALL: [Self; 10] = [
        Self::Counter,
        Self::Reflector,
        Self::Absorber,
        Self::SuperArmor,
        Self::SteerProjectile,
        Self::FillMeterOnHit,
        Self::ExtraJump,
        Self::AutoFaceOpponent,
        Self::MotionInput,
        Self::CommandGrab,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Counter => "Counter",
            Self::Reflector => "Reflector",
            Self::Absorber => "Absorber",
            Self::SuperArmor => "Super armor",
            Self::SteerProjectile => "Steerable projectile",
            Self::FillMeterOnHit => "Fill meter on hit",
            Self::ExtraJump => "Extra jump",
            Self::AutoFaceOpponent => "Auto face opponent",
            Self::MotionInput => "Motion input",
            Self::CommandGrab => "Command grab",
        }
    }
    pub fn stem(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Reflector => "reflector",
            Self::Absorber => "absorber",
            Self::SuperArmor => "super_armor",
            Self::SteerProjectile => "steer_projectile",
            Self::FillMeterOnHit => "fill_meter_on_hit",
            Self::ExtraJump => "extra_jump",
            Self::AutoFaceOpponent => "auto_face_opponent",
            Self::MotionInput => "motion_input",
            Self::CommandGrab => "command_grab",
        }
    }
    fn recipe(self) -> Recipe {
        match self {
            Self::Counter => Recipe::Counter,
            Self::Reflector => Recipe::Reflector,
            Self::Absorber
            | Self::SteerProjectile
            | Self::FillMeterOnHit
            | Self::ExtraJump
            | Self::AutoFaceOpponent
            | Self::MotionInput
            | Self::CommandGrab => Recipe::Absorber,
            Self::SuperArmor => Recipe::SuperArmor,
        }
    }
    pub fn hover(self) -> String {
        match self {
            Self::SteerProjectile => "While your newest projectile is alive, the stick steers it every frame (up to 2 each way) on top of a base speed of 1 in the direction you faced when it launched.".into(),
            Self::FillMeterOnHit => "Every hit you land adds a tenth of the fighter's first native gauge (Cloud's Limit, Little Mac's KO meter, Hero's MP...).".into(),
            Self::ExtraJump => "One more jump than this fighter normally has, restored if the game resets it.".into(),
            Self::AutoFaceOpponent => "While standing idle, the fighter turns to face the opponent.".into(),
            Self::MotionInput => "Down, down-forward, forward, then Attack within 12 frames per step adds 1% damage; replace the final action.".into(),
            Self::CommandGrab => "Side Special grabs. While holding, the stick walks with the opponent; Attack throws forward, Special lets go, and it throws back after 4 seconds. Letting go also happens when you are hit or leave the ground.".into(),
            _ => format!("Down Special {}", self.recipe().summary()),
        }
    }
    /// An idle state plus a state owning Down Special with the recipe inserted.
    pub fn graph(self, fighter: &str) -> Result<BehaviorGraph, String> {
        if self == Self::MotionInput {
            return motion_input();
        }
        if self == Self::CommandGrab {
            return command_grab(fighter, "catch");
        }
        if self == Self::SteerProjectile {
            return steer_projectile(fighter);
        }
        if self == Self::FillMeterOnHit {
            return fill_meter_on_hit(fighter);
        }
        if self == Self::ExtraJump {
            return extra_jump(fighter);
        }
        if self == Self::AutoFaceOpponent {
            return auto_face_opponent(fighter);
        }
        let state = self.label();
        let graph = BehaviorGraph {
            initial: "Idle".into(),
            states: ["Idle", state]
                .map(|name| crate::gimmicks::behavior::StateDef {
                    name: name.into(),
                    status: None,
                })
                .into(),
            ..Default::default()
        };
        let graph = crate::gimmicks::graph_edit::set_state_status_ref(
            &graph,
            fighter,
            state,
            Some("FIGHTER_STATUS_KIND_SPECIAL_LW".into()),
        )?;
        let graph = insert(&graph, fighter, state, self.recipe())?;
        if self == Self::Counter {
            // The counterattack itself; the stance keeps the common wait animation.
            return edit_motion(
                &graph,
                fighter,
                &format!("{state} release"),
                false,
                "attack_s3_s",
            );
        }
        Ok(graph)
    }
}

/// Side Special redirects to Catch turn, because only the game's catch statuses
/// accept a capture (HDR needed a per-fighter link-event hook to catch elsewhere) and
/// the plain Catch status stays the vanilla grab. Catch pull then leads into an owned
/// Catch wait: the status vanilla keeps a capture in, with its movement replaced.
pub fn command_grab(fighter: &str, motion_name: &str) -> Result<BehaviorGraph, String> {
    use crate::gimmicks::native_inputs::ThrowDirection;
    let status_id = |name: &str| crate::gimmicks::status_model::StatusRef::from(name);
    let frame_is = |op: CmpOp, frame: i32| Predicate::Compare {
        op,
        left: Expr::StatusFrame,
        right: Expr::Const(ConstValue::Int(frame)),
    };
    let holding: Predicate = serde_json::from_value(
        serde_json::json!({"NativeFlag": format!("holding_opponent:{fighter}")}),
    )
    .map_err(|e| e.to_string())?;
    let (start, grab, carry) = ("Side Special", "Grab", "Carry");
    let graph = BehaviorGraph {
        initial: "Idle".into(),
        states: ["Idle", start, grab, carry]
            .map(|name| crate::gimmicks::behavior::StateDef {
                name: name.into(),
                status: None,
            })
            .into(),
        ..Default::default()
    };
    let bind = |graph: &BehaviorGraph, state: &str, target: &str| {
        crate::gimmicks::graph_edit::set_state_status_ref(
            graph,
            fighter,
            state,
            Some(status_id(target)),
        )
    };
    let graph = bind(&graph, start, "FIGHTER_STATUS_KIND_SPECIAL_S")?;
    let graph = bind(&graph, grab, "FIGHTER_STATUS_KIND_CATCH_TURN")?;
    let mut graph = bind(&graph, carry, "FIGHTER_STATUS_KIND_CATCH_WAIT")?;
    // Catch pull is grounded in vanilla, so this slice grabs on the ground only.
    push(
        &mut graph,
        start,
        Phase::MainLoop,
        situation(false),
        vec![status(Command::ChangeStatusNow {
            symbol: status_id("FIGHTER_STATUS_KIND_CATCH_TURN"),
        })],
    );
    push(
        &mut graph,
        start,
        Phase::MainLoop,
        situation(true),
        vec![status(Command::ChangeStatusNow {
            symbol: status_id("FIGHTER_STATUS_KIND_FALL"),
        })],
    );
    push(
        &mut graph,
        grab,
        Phase::MainEnter,
        always(),
        vec![status(Command::ChangeKinetic {
            kind: Kinetic::GroundStop,
        })],
    );
    let box_on = |enabled| {
        status(Command::GrabBox {
            enabled,
            size: 6.0,
            height: 8.0,
            forward: 10.0,
        })
    };
    push(
        &mut graph,
        grab,
        Phase::MainLoop,
        frame_is(CmpOp::Eq, 6),
        vec![box_on(true)],
    );
    push(
        &mut graph,
        grab,
        Phase::MainLoop,
        frame_is(CmpOp::Eq, 14),
        vec![box_on(false)],
    );
    push(
        &mut graph,
        grab,
        Phase::MainLoop,
        holding.clone(),
        vec![status(Command::ChangeStatusNow {
            symbol: status_id("FIGHTER_STATUS_KIND_CATCH_PULL"),
        })],
    );
    let end = |symbol: &str| {
        status(Command::ChangeStatusNow {
            symbol: status_id(symbol),
        })
    };
    let graph = edit_motion(&graph, fighter, grab, false, motion_name)?;
    let mut graph = edit_motion(&graph, fighter, carry, false, "catch_wait")?;
    // The hold ends by rule below, not when the wait animation does: that animation loops.
    remove_finish(&mut graph, carry);
    push(
        &mut graph,
        carry,
        Phase::MainLoop,
        Predicate::MotionEnded,
        vec![motion("catch_wait", false)],
    );
    if let Some(Action::Status(Command::InitializeStatus(settings))) = graph
        .transitions
        .iter_mut()
        .filter(|t| t.from == carry)
        .flat_map(|t| &mut t.actions)
        .find(|a| matches!(a, Action::Status(Command::InitializeStatus(_))))
    {
        settings.status_data.grab_invulnerable = true;
    }
    push(
        &mut graph,
        carry,
        Phase::MainEnter,
        always(),
        vec![
            status(Command::ChangeKinetic {
                kind: Kinetic::GroundStop,
            }),
            status(Command::SetGroundPolicy {
                correction: Correction::Ground,
                cliff: Cliff::None,
            }),
        ],
    );
    let not_holding = and(Predicate::Not(Box::new(holding)), frame_is(CmpOp::Gte, 2));
    let lost = graph.transitions.len();
    graph.transitions.push(Transition {
        from: carry.into(),
        to: "Idle".into(),
        guard: and(not_holding.clone(), situation(false)),
        actions: vec![end("FIGHTER_STATUS_KIND_WAIT")],
        on_frame: false,
    });
    graph.transitions.push(Transition {
        from: carry.into(),
        to: "Idle".into(),
        guard: and(not_holding, situation(true)),
        actions: vec![end("FIGHTER_STATUS_KIND_FALL")],
        on_frame: false,
    });
    graph
        .states
        .iter_mut()
        .find(|s| s.name == carry)
        .unwrap()
        .status
        .as_mut()
        .unwrap()
        .roots
        .get_mut(&Phase::MainLoop)
        .unwrap()
        .extend([lost, lost + 1]);
    push(
        &mut graph,
        carry,
        Phase::MainLoop,
        situation(true),
        vec![Action::ReleaseHeldOpponent],
    );
    let walk: Expr = serde_json::from_value(serde_json::json!({"Mul": [{"StickX": {"relative_to_facing": false}}, {"Const": {"Float": 1.2}}]})).map_err(|e| e.to_string())?;
    push(
        &mut graph,
        carry,
        Phase::MainLoop,
        always(),
        vec![status(Command::ConfigureEnergy {
            energy: Energy::Stop,
            op: EnergyOp::Speed {
                x: walk,
                y: number(0.0),
            },
        })],
    );
    let pressed = |button| Predicate::Button {
        button,
        edge: ButtonEdge::Pressed,
    };
    push(
        &mut graph,
        carry,
        Phase::MainLoop,
        pressed(InputButton::Attack),
        vec![Action::ThrowHeldOpponent {
            direction: ThrowDirection::Forward,
        }],
    );
    push(
        &mut graph,
        carry,
        Phase::MainLoop,
        pressed(InputButton::Special),
        vec![Action::ReleaseHeldOpponent],
    );
    push(
        &mut graph,
        carry,
        Phase::MainLoop,
        frame_is(CmpOp::Eq, 240),
        vec![Action::ThrowHeldOpponent {
            direction: ThrowDirection::Back,
        }],
    );
    crate::gimmicks::graph_edit::validate_graph(&graph, fighter)?;
    Ok(graph)
}

/// Two frame rules: while no projectile is out, remember the fighter's facing;
/// while the newest one is alive, its speed is that launch direction × 1 plus
/// stick × 2, so a centred stick keeps it flying the way it was thrown.
fn steer_projectile(fighter: &str) -> Result<BehaviorGraph, String> {
    use crate::gimmicks::registry::{self, IdentifierKind};
    // The article behind the fighter's first reviewed weapon Work field (Mario: fireball).
    let article = registry::ENTRIES
        .iter()
        .filter(|entry| entry.fighter == fighter && entry.symbol.starts_with("WEAPON_"))
        .find_map(|entry| registry::projectile_work(fighter, entry.symbol, entry.kind))
        .map(|(_, article)| article.symbol)
        .or_else(|| {
            registry::entries_for(fighter, IdentifierKind::Article)
                .next()
                .map(|entry| entry.symbol)
        })
        .ok_or_else(|| format!("{fighter} has no owned projectile"))?;
    let stick =
        |axis: serde_json::Value| serde_json::json!({"Mul": [axis, {"Const": {"Float": 2.0}}]});
    let alive =
        serde_json::json!({"NativeFlag": format!("owned_object:{fighter}:{article}:alive")});
    serde_json::from_value(serde_json::json!({
        "initial": "Steer", "states": [{"name": "Steer"}],
        "vars": [{"name": "launch_dir", "kind": "Float", "initial": {"Float": 1.0}}],
        "transitions": [{
            "from": "Steer", "to": "Steer", "on_frame": true, "guard": alive,
            "actions": [{"Object": {"symbol": article, "op": {"SetSpeed": {
                "x": {"Add": [{"Var": "launch_dir"}, stick(serde_json::json!({"StickX": {"relative_to_facing": false}}))]},
                "y": stick(serde_json::json!("StickY")),
            }}}}],
        }, {
            "from": "Steer", "to": "Steer", "on_frame": true, "guard": {"Not": alive},
            "actions": [{"Set": {"var": "launch_dir", "value": {"NativeFloat": format!("facing:{fighter}")}}}],
        }],
        "emission_plan": {"bindings": [{"hook": "onChargeTick", "transitions": [0, 1]}], "event_bindings": []},
    }))
    .map_err(|error| error.to_string())
}

/// One onDealDamage rule: Add a tenth of the fighter's first resource row's bound per hit.
fn fill_meter_on_hit(fighter: &str) -> Result<BehaviorGraph, String> {
    let resource = super::resource_commands::Resource::for_fighter(fighter)
        .next()
        .ok_or_else(|| format!("{fighter} has no native resource gauge"))?;
    serde_json::from_value(serde_json::json!({
        "initial": "Ready", "states": [{"name": "Ready"}], "vars": [],
        "transitions": [{
            "from": "Ready", "to": "Ready", "on_frame": true, "guard": always(),
            "actions": [{"Resource": {"op": "add", "resource": resource, "amount": {"Const": {"Float": (resource.entry().max / 10.0).ceil()}}}}],
        }],
        "emission_plan": {"bindings": [{"hook": "onDealDamage", "transitions": [0]}], "event_bindings": []},
    }))
    .map_err(|error| error.to_string())
}

/// Remember the fighter's own maximum on the first tick, because it differs per
/// fighter, then hold it one higher every frame.
fn extra_jump(fighter: &str) -> Result<BehaviorGraph, String> {
    let max = format!("work_int:{fighter}:FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT_MAX");
    let raised = serde_json::json!({"Add": [{"Var": "base"}, {"Const": {"Int": 1}}]});
    serde_json::from_value(serde_json::json!({
        "initial": "Measure", "states": [{"name": "Measure"}, {"name": "Raised"}],
        "vars": [{"name": "base", "kind": "Int", "initial": {"Int": 0}}],
        "transitions": [
            {"from": "Measure", "to": "Raised", "on_frame": true, "guard": always(), "actions": [
                {"Set": {"var": "base", "value": {"NativeInt": max}}},
            ]},
            {"from": "Raised", "to": "Raised", "on_frame": true,
             "guard": {"Compare": {"op": "Lt", "left": {"NativeInt": max}, "right": raised}},
             "actions": [{"SetNative": {"input": max, "value": raised}}]},
        ],
        "emission_plan": {"bindings": [{"hook": "onChargeTick", "transitions": [0, 1]}], "event_bindings": []},
    }))
    .map_err(|error| error.to_string())
}

fn motion_input() -> Result<BehaviorGraph, String> {
    use serde_json::{json, Value};
    let cmp = |op: &str, left: Value, right: Value| json!({"Compare": {"op": op, "left": left, "right": right}});
    let x = json!({"StickX": {"relative_to_facing": true}});
    let y = json!("StickY");
    let c = |v: f32| json!({"Const": {"Float": v}});
    let and = |a: Value, b: Value| json!({"And": [a, b]});
    let down = and(
        cmp("Lt", y.clone(), c(-0.5)),
        and(cmp("Gt", x.clone(), c(-0.4)), cmp("Lt", x.clone(), c(0.4))),
    );
    let diagonal = and(cmp("Lt", y.clone(), c(-0.5)), cmp("Gt", x.clone(), c(0.5)));
    let forward = and(
        cmp("Gt", x, c(0.5)),
        and(cmp("Gt", y.clone(), c(-0.4)), cmp("Lt", y, c(0.4))),
    );
    let attack = json!({"Button": {"button": "attack", "edge": "Pressed"}});
    let reset = json!({"Set": {"var": "age", "value": {"Const": {"Int": 0}}}});
    let mut transitions = vec![
        json!({"from":"Idle","to":"Down","on_frame":true,"guard":and(down.clone(), json!({"Not":attack.clone()})),"actions":[reset.clone()]}),
    ];
    for (from, to, held, next) in [
        ("Down", "Diagonal", down, diagonal.clone()),
        ("Diagonal", "Forward", diagonal, forward.clone()),
        (
            "Forward",
            "Idle",
            forward.clone(),
            and(forward, attack.clone()),
        ),
    ] {
        let timely = cmp("Lt", json!({"Var":"age"}), json!({"Const":{"Int":12}}));
        let advance = and(timely.clone(), next.clone());
        let advance = if from == "Forward" {
            advance
        } else {
            and(advance, json!({"Not":attack.clone()}))
        };
        let actions = if from == "Forward" {
            json!([reset.clone(), {"AddDamage":{"amount":{"Const":{"Float":1.0}}}}])
        } else {
            json!([reset.clone()])
        };
        transitions.push(
            json!({"from":from,"to":to,"on_frame":true,"guard":advance.clone(),"actions":actions}),
        );
        let valid = and(timely, and(held, json!({"Not":attack.clone()})));
        transitions.push(json!({"from":from,"to":from,"on_frame":true,"guard":valid.clone(),"actions":[{"Set":{"var":"age","value":{"Add":[{"Var":"age"},{"Const":{"Int":1}}]}}}]}));
        transitions.push(json!({"from":from,"to":"Reset","on_frame":true,"guard":{"Not":{"Or":[valid, advance]}},"actions":[reset.clone()]}));
    }
    transitions.push(json!({"from":"Reset","to":"Idle","on_frame":true,
        "guard":and(cmp("Gt", json!("StickY"), c(-0.4)), and(cmp("Gt", json!({"StickX":{"relative_to_facing":true}}), c(-0.4)), cmp("Lt", json!({"StickX":{"relative_to_facing":true}}), c(0.4)))),
        "actions":[reset]}));
    // The runtime evaluates every rule in order; visit later stages first.
    let bindings: Vec<_> = (1..transitions.len() - 1)
        .collect::<Vec<_>>()
        .chunks(3)
        .rev()
        .flatten()
        .copied()
        .chain([0, transitions.len() - 1])
        .collect();
    serde_json::from_value(json!({
        "initial":"Idle", "states":[{"name":"Idle"},{"name":"Down"},{"name":"Diagonal"},{"name":"Forward"},{"name":"Reset"}],
        "vars":[{"name":"age","kind":"Int","initial":{"Int":0}}],
        "transitions":transitions,
        "emission_plan":{"bindings":[{"hook":"onChargeTick","transitions":bindings}],"event_bindings":[]}
    })).map_err(|e| e.to_string())
}

fn auto_face_opponent(fighter: &str) -> Result<BehaviorGraph, String> {
    serde_json::from_value(serde_json::json!({
        "initial": "Active", "states": [{"name": "Active"}], "vars": [],
        "transitions": [{
            "from": "Active", "to": "Active", "on_frame": true,
            "guard": {"NativeFlag": format!("status_is:{fighter}:FIGHTER_STATUS_KIND_WAIT")},
            "actions": [{"Movement": {"SetFacing": "Opponent"}}],
        }],
        "emission_plan": {"bindings": [{"hook": "onChargeTick", "transitions": [0]}], "event_bindings": []},
    }))
    .map_err(|error| error.to_string())
}

use super::templates::always;
fn number(n: f32) -> Expr {
    Expr::Const(ConstValue::Float(n))
}
fn and(a: Predicate, b: Predicate) -> Predicate {
    Predicate::And(Box::new(a), Box::new(b))
}
fn situation(air: bool) -> Predicate {
    Predicate::Situation {
        kind: if air {
            SituationKind::Air
        } else {
            SituationKind::Ground
        },
    }
}
fn button(edge: ButtonEdge) -> Predicate {
    Predicate::Button {
        button: InputButton::Special,
        edge,
    }
}
fn status(command: Command<Expr>) -> Action {
    Action::Status(command)
}
fn motion(name: &str, inherit: bool) -> Action {
    status(Command::PlayMotion {
        motion: name.into(),
        frame: if inherit {
            MotionFrame::Inherit
        } else {
            MotionFrame::Start(number(0.0))
        },
        rate: number(1.0),
    })
}
fn push(
    graph: &mut BehaviorGraph,
    state: &str,
    phase: Phase,
    guard: Predicate,
    actions: Vec<Action>,
) {
    let initialization = (phase == Phase::MainEnter
        && !actions
            .iter()
            .any(|a| matches!(a, Action::Status(Command::PlayMotion { .. }))))
        || (phase == Phase::Pre
            && !actions
                .iter()
                .any(|a| matches!(a, Action::Status(Command::InitializeStatus(_)))));
    let index = graph.transitions.len();
    graph.transitions.push(Transition {
        from: state.into(),
        to: state.into(),
        guard,
        actions,
        on_frame: false,
    });
    let roots = graph
        .states
        .iter_mut()
        .find(|s| s.name == state)
        .unwrap()
        .status
        .as_mut()
        .unwrap()
        .roots
        .entry(phase)
        .or_default();
    if initialization {
        roots.insert(0, index);
    } else {
        roots.push(index);
    }
}

fn remove_rules(
    graph: &mut BehaviorGraph,
    state: &str,
    matches: impl Fn(Phase, &Transition) -> bool,
) {
    let mut indices: Vec<_> = graph
        .states
        .iter()
        .find(|s| s.name == state)
        .and_then(|s| s.status.as_ref())
        .into_iter()
        .flat_map(|b| b.roots.iter())
        .flat_map(|(phase, indices)| indices.iter().map(move |i| (*phase, *i)))
        .filter(|(phase, i)| matches(*phase, &graph.transitions[*i]))
        .map(|(_, i)| i)
        .collect();
    indices.sort_unstable();
    indices.dedup();
    for i in indices.into_iter().rev() {
        crate::gimmicks::graph_edit::remove_transition_at(graph, i);
    }
}

fn require_binding(graph: &BehaviorGraph, state: &str) -> Result<(), String> {
    graph
        .states
        .iter()
        .find(|s| s.name == state)
        .and_then(|s| s.status.as_ref())
        .map(|_| ())
        .ok_or_else(|| "Choose a status state".into())
}

pub fn add_phase_rule(
    graph: &BehaviorGraph,
    fighter: &str,
    state: &str,
    phase: Phase,
) -> Result<BehaviorGraph, String> {
    require_binding(graph, state)?;
    let mut updated = graph.clone();
    push(&mut updated, state, phase, always(), vec![]);
    crate::gimmicks::graph_edit::validate_graph(&updated, fighter)?;
    Ok(updated)
}

/// Motion rows edit existing PlayMotion blocks, including inherited situation swaps.
pub fn motions(graph: &BehaviorGraph, state: &str) -> (String, String) {
    let mut names = (String::new(), String::new());
    if let Some(binding) = graph
        .states
        .iter()
        .find(|s| s.name == state)
        .and_then(|s| s.status.as_ref())
    {
        for index in binding.roots.values().flatten() {
            if let Some(rule) = graph.transitions.get(*index) {
                for action in &rule.actions {
                    if let Action::Status(Command::PlayMotion { motion, .. }) = action {
                        if is_air(&rule.guard) {
                            names.1 = motion.clone();
                        } else {
                            names.0 = motion.clone();
                        }
                    }
                }
            }
        }
    }
    names
}
fn is_air(guard: &Predicate) -> bool {
    match guard {
        Predicate::Situation {
            kind: SituationKind::Air,
        } => true,
        Predicate::And(a, b) => is_air(a) || is_air(b),
        _ => false,
    }
}
pub fn edit_motion(
    graph: &BehaviorGraph,
    fighter: &str,
    state: &str,
    air: bool,
    name: &str,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    let binding = updated
        .states
        .iter()
        .find(|s| s.name == state)
        .and_then(|s| s.status.as_ref())
        .ok_or("select a status state")?
        .clone();
    let mut found = false;
    for index in binding.roots.values().flatten() {
        let rule = &mut updated.transitions[*index];
        if is_air(&rule.guard) == air {
            for action in &mut rule.actions {
                if let Action::Status(Command::PlayMotion { motion, .. }) = action {
                    *motion = name.into();
                    found = true;
                }
            }
        }
    }
    if !found {
        return Err("Insert Ground/air special to add a separate air motion".into());
    }
    crate::gimmicks::graph_edit::validate_graph(&updated, fighter)?;
    Ok(updated)
}

/// Insert once; existing advanced roots and unrelated receivers stay intact.
pub fn insert(
    graph: &BehaviorGraph,
    fighter: &str,
    state: &str,
    recipe: Recipe,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    let binding = updated
        .states
        .iter()
        .find(|s| s.name == state)
        .and_then(|s| s.status.as_ref())
        .ok_or("Choose an owned status first")?
        .clone();
    if binding.policy != crate::gimmicks::status_model::Policy::Owned {
        return Err("Choose Custom rules in Advanced phases first".into());
    }
    let (ground, air) = motions(graph, state);
    let ground = if ground.is_empty() {
        "special_s".into()
    } else {
        ground
    };
    let air = if air.is_empty() {
        ground
            .strip_prefix("special_")
            .map_or_else(|| ground.clone(), |suffix| format!("special_air_{suffix}"))
    } else {
        air
    };
    match recipe {
        Recipe::GroundAir => {
            // Replace only motion-only rules; preserve other authored entry/loop work.
            remove_rules(&mut updated, state, |phase, rule| {
                (phase == Phase::MainEnter
                    || (phase == Phase::MainLoop
                        && matches!(&rule.guard, Predicate::And(a, _) if **a == Predicate::SituationChanged)))
                    && !rule.actions.is_empty()
                    && rule
                        .actions
                        .iter()
                        .all(|a| matches!(a, Action::Status(Command::PlayMotion { .. })))
            });
            for air_side in [true, false] {
                let name = if air_side { &air } else { &ground };
                push(
                    &mut updated,
                    state,
                    Phase::MainEnter,
                    if air_side { situation(true) } else { always() },
                    vec![motion(name, false)],
                );
                push(
                    &mut updated,
                    state,
                    Phase::MainLoop,
                    and(Predicate::SituationChanged, situation(air_side)),
                    vec![motion(name, true)],
                );
            }
        }
        Recipe::ChargeRelease => {
            let target = release_state(&mut updated, fighter, state, &ground)?;
            // Charge loops its selected motion while held; release routes to an ordinary custom status.
            remove_finish(&mut updated, state);
            push(
                &mut updated,
                state,
                Phase::MainLoop,
                and(button(ButtonEdge::Held), Predicate::MotionEnded),
                vec![motion(&ground, false)],
            );
            push(
                &mut updated,
                state,
                Phase::MainLoop,
                Predicate::Or(
                    Box::new(Predicate::Not(Box::new(button(ButtonEdge::Held)))),
                    Box::new(Predicate::Compare {
                        op: CmpOp::Gte,
                        left: Expr::StatusFrame,
                        right: Expr::Const(ConstValue::Int(120)),
                    }),
                ),
                vec![status(Command::ChangeStatusNow { symbol: target })],
            );
        }
        Recipe::Reflector => {
            for (phase, enabled) in [(Phase::MainEnter, true), (Phase::Exit, false)] {
                push(
                    &mut updated,
                    state,
                    phase,
                    always(),
                    vec![status(Command::Reflector {
                        enabled,
                        size: 6.0,
                        damage: 1.5,
                        speed: 1.5,
                    })],
                );
            }
        }
        Recipe::Absorber => {
            if has_contact_rule(&updated, state, crate::gimmicks::hooks::HookName::OnAbsorb) {
                return Err("This state already has an absorb rule".into());
            }
            for (phase, enabled) in [(Phase::MainEnter, true), (Phase::Exit, false)] {
                push(
                    &mut updated,
                    state,
                    phase,
                    always(),
                    vec![status(Command::Absorber { enabled, size: 8.0 })],
                );
            }
            let index = updated.transitions.len();
            updated.transitions.push(Transition {
                from: state.into(),
                to: state.into(),
                on_frame: true,
                guard: always(),
                actions: vec![Action::AddDamage {
                    amount: number(-10.0),
                }],
            });
            updated
                .emission_plan
                .bindings
                .push(crate::gimmicks::codegen::HookBinding {
                    hook: crate::gimmicks::hooks::HookName::OnAbsorb,
                    article: None,
                    transitions: vec![index],
                });
        }
        Recipe::Counter => {
            if has_contact_rule(
                &updated,
                state,
                crate::gimmicks::hooks::HookName::OnIncomingHit,
            ) {
                return Err("This state already has an incoming-hit rule".into());
            }
            let target = release_state(&mut updated, fighter, state, &ground)?;
            let pending = format!("counter_{}", updated.vars.len());
            let damage = format!("counter_damage_{}", updated.vars.len());
            updated.vars.push(crate::gimmicks::behavior::VarDef {
                keep_across_form_switch: false,
                name: damage.clone(),
                kind: crate::gimmicks::behavior::VarType::Float,
                initial: ConstValue::Float(0.0),
            });
            let release = updated
                .states
                .iter()
                .find(|s| s.status.as_ref().is_some_and(|b| b.target == target))
                .unwrap()
                .name
                .clone();
            push(
                &mut updated,
                &release,
                Phase::Exec,
                always(),
                (0..8)
                    .map(|id| {
                        status(Command::SetAttackDamage {
                            id,
                            damage: Expr::Mul(
                                Box::new(Expr::Var(damage.clone())),
                                Box::new(number(1.0)),
                            ),
                        })
                    })
                    .collect(),
            );
            updated.vars.push(crate::gimmicks::behavior::VarDef {
                keep_across_form_switch: false,
                name: pending.clone(),
                kind: crate::gimmicks::behavior::VarType::Flag,
                initial: ConstValue::Flag(false),
            });
            push(
                &mut updated,
                state,
                Phase::MainEnter,
                always(),
                vec![
                    Action::Clear {
                        var: pending.clone(),
                    },
                    Action::Reset {
                        var: damage.clone(),
                    },
                ],
            );
            push(
                &mut updated,
                state,
                Phase::MainLoop,
                Predicate::Flag(pending.clone()),
                vec![
                    Action::Clear {
                        var: pending.clone(),
                    },
                    status(Command::ChangeStatusNow { symbol: target }),
                ],
            );
            let index = updated.transitions.len();
            updated.transitions.push(Transition {
                from: state.into(),
                to: state.into(),
                on_frame: true,
                guard: and(
                    Predicate::Not(Box::new(Predicate::Flag(pending.clone()))),
                    and(
                        Predicate::Compare {
                            op: CmpOp::Gte,
                            left: Expr::MotionFrame,
                            right: number(6.0),
                        },
                        Predicate::Compare {
                            op: CmpOp::Lt,
                            left: Expr::MotionFrame,
                            right: number(28.0),
                        },
                    ),
                ),
                actions: vec![
                    Action::Set {
                        var: damage,
                        value: Expr::PendingDamage,
                    },
                    Action::CancelIncomingHit,
                    Action::Raise { var: pending },
                ],
            });
            updated
                .emission_plan
                .bindings
                .push(crate::gimmicks::codegen::HookBinding {
                    hook: crate::gimmicks::hooks::HookName::OnIncomingHit,
                    article: None,
                    transitions: vec![index],
                });
        }
        Recipe::SuperArmor => {
            // Status scope: the channel returns to normal when the status ends, so no exit rule.
            push(
                &mut updated,
                state,
                Phase::MainEnter,
                always(),
                vec![Action::CombatPolicy {
                    scope: crate::gimmicks::native_inputs::PolicyScope::Status,
                    op: crate::gimmicks::native_inputs::CombatOp::SuperArmor,
                }],
            );
        }
        Recipe::Hover => {
            updated = movement(&updated, fighter, state, true)?;
            for held in [true, false] {
                push(
                    &mut updated,
                    state,
                    Phase::MainLoop,
                    and(
                        situation(true),
                        if held {
                            button(ButtonEdge::Held)
                        } else {
                            Predicate::Not(Box::new(button(ButtonEdge::Held)))
                        },
                    ),
                    vec![status(Command::ConfigureEnergy {
                        energy: Energy::Gravity,
                        op: if held {
                            EnergyOp::Disable
                        } else {
                            EnergyOp::Enable
                        },
                    })],
                );
            }
        }
    }
    if recipe == Recipe::GroundAir {
        updated = movement(&updated, fighter, state, true)?;
        updated = finish(&updated, fighter, state, true)?;
    }
    crate::gimmicks::graph_edit::validate_graph(&updated, fighter)?;
    Ok(updated)
}
fn has_contact_rule(
    graph: &BehaviorGraph,
    state: &str,
    hook: crate::gimmicks::hooks::HookName,
) -> bool {
    graph.emission_plan.bindings.iter().any(|b| {
        b.hook == hook
            && b.transitions
                .iter()
                .any(|i| graph.transitions[*i].from == state)
    })
}
fn release_state(
    graph: &mut BehaviorGraph,
    fighter: &str,
    state: &str,
    motion_name: &str,
) -> Result<crate::gimmicks::status_model::StatusRef, String> {
    let name = format!("{state} release");
    if graph.states.iter().any(|s| s.name == name) {
        return Err("A release state already exists; edit its ordinary rules".into());
    }
    let target = crate::gimmicks::status_model::allocate(&mut graph.custom_statuses, &name)?;
    graph.states.push(crate::gimmicks::behavior::StateDef {
        name: name.clone(),
        status: None,
    });
    *graph = crate::gimmicks::graph_edit::set_state_status_ref(
        graph,
        fighter,
        &name,
        Some(target.clone()),
    )?;
    *graph = edit_motion(graph, fighter, &name, false, motion_name)?;
    *graph = finish(graph, fighter, &name, true)?;
    Ok(target)
}
fn remove_finish(graph: &mut BehaviorGraph, state: &str) {
    remove_rules(graph, state, |phase, rule| {
        phase == Phase::MainLoop
        && (matches!(rule.guard, Predicate::MotionEnded)
            || matches!(&rule.guard, Predicate::And(a, _) if **a == Predicate::MotionEnded))
        && !rule.actions.is_empty()
        && rule.actions.iter().all(|a| matches!(a,
            Action::Status(Command::ChangeStatusNow { symbol: crate::gimmicks::status_model::StatusRef::Native(s) })
                if s == "FIGHTER_STATUS_KIND_WAIT" || s == "FIGHTER_STATUS_KIND_FALL"))
    });
}
pub fn finish(
    graph: &BehaviorGraph,
    fighter: &str,
    state: &str,
    enabled: bool,
) -> Result<BehaviorGraph, String> {
    require_binding(graph, state)?;
    let mut updated = graph.clone();
    remove_finish(&mut updated, state);
    if enabled {
        for air in [false, true] {
            push(
                &mut updated,
                state,
                Phase::MainLoop,
                and(Predicate::MotionEnded, situation(air)),
                vec![status(Command::ChangeStatusNow {
                    symbol: if air {
                        "FIGHTER_STATUS_KIND_FALL"
                    } else {
                        "FIGHTER_STATUS_KIND_WAIT"
                    }
                    .into(),
                })],
            );
        }
    }
    crate::gimmicks::graph_edit::validate_graph(&updated, fighter)?;
    Ok(updated)
}
pub fn movement(
    graph: &BehaviorGraph,
    fighter: &str,
    state: &str,
    enabled: bool,
) -> Result<BehaviorGraph, String> {
    require_binding(graph, state)?;
    let mut updated = graph.clone();
    // Recognize only this preset's exact rule shape; custom movement stays visible.
    remove_rules(&mut updated, state, |phase, rule| {
        [Phase::MainEnter, Phase::MainLoop].contains(&phase)
            && [false, true].iter().any(|&air| {
                rule.guard
                    == if phase == Phase::MainEnter {
                        situation(air)
                    } else {
                        and(Predicate::SituationChanged, situation(air))
                    }
            })
            && matches!(
                rule.actions.as_slice(),
                [
                    Action::Status(Command::ChangeKinetic {
                        kind: Kinetic::Fall | Kinetic::GroundStop
                    }),
                    Action::Status(Command::SetGroundPolicy {
                        correction: Correction::Air | Correction::Ground,
                        cliff: Cliff::AlwaysBothSides
                    })
                ]
            )
    });
    if enabled {
        for air in [false, true] {
            for phase in [Phase::MainEnter, Phase::MainLoop] {
                push(
                    &mut updated,
                    state,
                    phase,
                    if phase == Phase::MainEnter {
                        situation(air)
                    } else {
                        and(Predicate::SituationChanged, situation(air))
                    },
                    vec![
                        status(Command::ChangeKinetic {
                            kind: if air {
                                Kinetic::Fall
                            } else {
                                Kinetic::GroundStop
                            },
                        }),
                        status(Command::SetGroundPolicy {
                            correction: if air {
                                Correction::Air
                            } else {
                                Correction::Ground
                            },
                            cliff: Cliff::AlwaysBothSides,
                        }),
                    ],
                );
            }
        }
    }
    crate::gimmicks::graph_edit::validate_graph(&updated, fighter)?;
    Ok(updated)
}
pub fn cancel(
    graph: &BehaviorGraph,
    fighter: &str,
    state: &str,
    frame: Option<f32>,
) -> Result<BehaviorGraph, String> {
    require_binding(graph, state)?;
    let mut updated = graph.clone();
    remove_rules(&mut updated, state, |phase, rule| {
        phase == Phase::MainLoop
            && matches!(
                rule.guard,
                Predicate::Compare {
                    op: CmpOp::Gte,
                    left: Expr::MotionFrame,
                    right: Expr::Const(ConstValue::Float(_))
                }
            )
            && matches!(
                rule.actions.as_slice(),
                [Action::Status(
                    Command::EnableCancel
                        | Command::TryCommonTransition {
                            kind: CommonTransition::GroundWait | CommonTransition::AirFall
                        }
                )]
            )
    });
    if let Some(frame) = frame {
        for command in [
            Command::EnableCancel,
            Command::TryCommonTransition {
                kind: CommonTransition::GroundWait,
            },
            Command::TryCommonTransition {
                kind: CommonTransition::AirFall,
            },
        ] {
            push(
                &mut updated,
                state,
                Phase::MainLoop,
                Predicate::Compare {
                    op: CmpOp::Gte,
                    left: Expr::MotionFrame,
                    right: number(frame),
                },
                vec![status(command)],
            );
        }
    }
    crate::gimmicks::graph_edit::validate_graph(&updated, fighter)?;
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owned() -> BehaviorGraph {
        let graph = BehaviorGraph {
            initial: "Idle".into(),
            states: vec![
                crate::gimmicks::behavior::StateDef {
                    name: "Idle".into(),
                    status: None,
                },
                crate::gimmicks::behavior::StateDef {
                    name: "Start".into(),
                    status: None,
                },
            ],
            ..Default::default()
        };
        let graph = crate::gimmicks::graph_edit::set_state_status(
            &graph,
            "mario",
            "Start",
            Some("FIGHTER_STATUS_KIND_SPECIAL_S"),
        )
        .unwrap();
        edit_motion(&graph, "mario", "Start", false, "special_s").unwrap()
    }
    #[test]
    fn recipes_are_normal_serialized_graphs_and_validate_through_both_paths() {
        for recipe in Recipe::ALL {
            let graph = insert(&owned(), "mario", "Start", recipe)
                .unwrap_or_else(|e| panic!("{recipe:?}: {e}"));
            let restored: BehaviorGraph =
                serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
            assert_eq!(graph, restored);
            crate::gimmicks::graph_edit::validate_graph(&restored, "mario").unwrap();
            crate::gimmicks::live_protocol::program_from_graph("mario", &restored).unwrap();
            let projection = crate::gimmicks::field_projection::project(&restored).unwrap();
            assert!(
                !projection
                    .diagnostics
                    .iter()
                    .any(|d| d.message.contains("not bound")),
                "{:?}",
                projection.diagnostics
            );
        }
    }
    #[test]
    fn compact_edits_preserve_advanced_roots_and_reindex_receivers() {
        let mut graph = insert(&owned(), "mario", "Start", Recipe::Counter).unwrap();
        let incoming = graph.emission_plan.bindings[0].transitions[0];
        let contact = graph.transitions[incoming].clone();
        graph = insert(&graph, "mario", "Start", Recipe::GroundAir).unwrap();
        graph = edit_motion(&graph, "mario", "Start", true, "special_air_s").unwrap();
        graph = movement(&graph, "mario", "Start", true).unwrap();
        graph = cancel(&graph, "mario", "Start", Some(20.0)).unwrap();
        graph = finish(&graph, "mario", "Start", true).unwrap();
        for phase in [Phase::Pre, Phase::MainEnter] {
            graph = add_phase_rule(&graph, "mario", "Start", phase).unwrap();
        }
        assert_eq!(
            contact,
            graph.transitions[graph.emission_plan.bindings[0].transitions[0]]
        );
        assert_eq!(
            motions(&graph, "Start"),
            ("special_s".into(), "special_air_s".into())
        );
        let live = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        assert_eq!(live.status_bindings.len(), 2);
    }
    #[test]
    fn owned_recipe_project_and_controller_asset_retain_bindings_and_layout() {
        use crate::mod_project::{ControllerChart, FighterMod, ModProjectFile};
        let mut chart = ControllerChart {
            graph: insert(&owned(), "mario", "Start", Recipe::GroundAir).unwrap(),
            ..Default::default()
        };
        chart
            .layout
            .positions
            .insert("state/Start".into(), [123.0, 45.0]);
        let asset = crate::gimmicks::controller_asset::ControllerAsset::capture(
            "mario", "special", "Special", &chart,
        )
        .unwrap();
        assert_eq!(
            crate::gimmicks::controller_asset::ControllerAsset::decode(&asset.encode().unwrap())
                .unwrap()
                .chart(),
            &chart
        );
        let mut fighter = FighterMod::default();
        fighter
            .controller_charts
            .insert("special".into(), chart.clone());
        let mut project = ModProjectFile::default();
        project.fighters.insert("mario".into(), fighter);
        let restored: ModProjectFile =
            serde_json::from_str(&serde_json::to_string(&project).unwrap()).unwrap();
        assert_eq!(
            restored.fighters["mario"].controller_charts["special"],
            chart
        );
    }

    #[test]
    fn status_diagnostics_point_to_the_owning_card() {
        let mut graph = owned();
        graph.transitions[0].actions = vec![motion("special_s", false)];
        let projection = crate::gimmicks::field_projection::project(&graph).unwrap();
        assert!(projection.diagnostics.iter().any(|d| d.reference
            == crate::gimmicks::field_projection::FieldRef::State("Start".into())
            && d.message.contains("not permitted")));
        graph.transitions[0].actions.clear();
        graph.transitions[1].actions.clear();
        let projection = crate::gimmicks::field_projection::project(&graph).unwrap();
        assert!(projection.diagnostics.iter().any(|d| d.reference
            == crate::gimmicks::field_projection::FieldRef::State("Start".into())
            && d.message.contains("no ground motion")));
    }

    #[test]
    fn ground_air_recipe_swaps_once_and_finishes_in_the_correct_situation() {
        use crate::gimmicks::{
            live_model,
            native_inputs::{NativeSnapshot, PhaseInput},
        };
        let graph = insert(&owned(), "mario", "Start", Recipe::GroundAir).unwrap();
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        for (kind, expected) in [
            (SituationKind::Ground, "FIGHTER_STATUS_KIND_WAIT"),
            (SituationKind::Air, "FIGHTER_STATUS_KIND_FALL"),
        ] {
            let mut snapshot = NativeSnapshot::empty();
            snapshot.phase = Some(PhaseInput {
                fighter: "mario".into(),
                status: 0,
                status_frame: 0,
                motion_frame: 12.0,
                motion: 0,
                situation: Some(kind),
            });
            snapshot.motion_ended = Some(false);
            snapshot.situation_changed = Some(true);
            let mut state = program.initial_state();
            program
                .step_status_phase(&mut state, "Start", Phase::Pre, &snapshot)
                .unwrap();
            let entered = program
                .step_status_phase(&mut state, "Start", Phase::MainEnter, &snapshot)
                .unwrap();
            assert_eq!(
                entered
                    .engine_actions
                    .iter()
                    .filter(|a| matches!(
                        a,
                        live_model::Action::Status(Command::PlayMotion {
                            frame: MotionFrame::Start(_),
                            ..
                        })
                    ))
                    .count(),
                1
            );
            assert!(
                entered.engine_actions.iter().any(|a| matches!(a,
                live_model::Action::Status(Command::ChangeKinetic { kind: Kinetic::GroundStop })
                    if kind == SituationKind::Ground))
                    || entered.engine_actions.iter().any(|a| matches!(a,
                live_model::Action::Status(Command::ChangeKinetic { kind: Kinetic::Fall })
                    if kind == SituationKind::Air))
            );
            let swapped = program
                .step_status_phase(&mut state, "Start", Phase::MainLoop, &snapshot)
                .unwrap();
            assert_eq!(
                swapped
                    .engine_actions
                    .iter()
                    .filter(|a| matches!(
                        a,
                        live_model::Action::Status(Command::PlayMotion {
                            frame: MotionFrame::Inherit,
                            ..
                        })
                    ))
                    .count(),
                1
            );
            snapshot.situation_changed = Some(false);
            assert!(program
                .step_status_phase(&mut state, "Start", Phase::MainLoop, &snapshot)
                .unwrap()
                .engine_actions
                .is_empty());
            snapshot.motion_ended = Some(true);
            let ended = program
                .step_status_phase(&mut state, "Start", Phase::MainLoop, &snapshot)
                .unwrap();
            assert!(ended.engine_actions.iter().any(|a|matches!(a,live_model::Action::Status(Command::ChangeStatusNow {symbol:crate::gimmicks::status_model::StatusRef::Native(s)}) if s==expected)));
        }
    }

    #[test]
    fn owned_counter_skips_frame_dispatch_and_still_receives_contact() {
        use crate::gimmicks::{
            controller_runtime::ControllerCoordinator,
            native_inputs::{DamageDirection, DamageInput, NativeSnapshot, PhaseInput},
        };
        let graph = insert(&owned(), "mario", "Start", Recipe::Counter).unwrap();
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let status = program
            .status_bindings
            .iter()
            .find(|b| b.state == "Start")
            .unwrap()
            .binding
            .target
            .resolve("mario")
            .unwrap() as i32;
        let coordinator = ControllerCoordinator::new(std::collections::BTreeMap::from([(
            "counter".into(),
            program,
        )]))
        .unwrap();
        let mut state = coordinator.initial_state();
        let mut snapshot = NativeSnapshot::empty();
        snapshot.phase = Some(PhaseInput {
            fighter: "mario".into(),
            status,
            status_frame: 10,
            motion_frame: 10.0,
            motion: 0,
            situation: Some(SituationKind::Ground),
        });
        coordinator
            .step_status_phase(&mut state, status, Phase::Pre, &snapshot)
            .unwrap();
        coordinator
            .step_status_phase(&mut state, status, Phase::MainEnter, &snapshot)
            .unwrap();
        assert!(coordinator
            .step_frame(&mut state, &snapshot)
            .unwrap()
            .charts
            .is_empty());
        let hit = DamageInput::new(DamageDirection::Incoming, 12.0).unwrap();
        let contact = coordinator.step_damage(&mut state, hit, &snapshot).unwrap();
        assert!(contact.engine_actions.iter().any(|a| matches!(
            a.action,
            crate::gimmicks::live_model::Action::CancelIncomingHit
        )));
        assert!(coordinator
            .step_damage(&mut state, hit, &snapshot)
            .unwrap()
            .engine_actions
            .is_empty());
    }

    #[test]
    fn fill_meter_on_hit_adds_to_the_fighters_first_gauge_on_each_landed_hit() {
        use crate::gimmicks::live_model::{Action, Expr, Value};
        use crate::gimmicks::native_inputs::{DamageDirection, DamageInput, NativeSnapshot};
        for (fighter, key, amount) in [
            ("cloud", "cloudLimit", 10.0),
            ("littlemac", "littleMacKoGauge", 10.0),
            ("buddy", "banjoFeathers", 1.0),
        ] {
            let graph = Defense::FillMeterOnHit.graph(fighter).unwrap();
            crate::gimmicks::graph_edit::validate_graph(&graph, fighter).unwrap();
            assert!(crate::gimmicks::behavior::requires_controller_runtime(
                &graph
            ));
            let program =
                crate::gimmicks::live_protocol::program_from_graph(fighter, &graph).unwrap();
            let mut state = program.initial_state();
            let hit = DamageInput::new(DamageDirection::Deal, 12.0).unwrap();
            let step = program
                .step_damage_with_snapshot(&mut state, hit, &NativeSnapshot::empty())
                .unwrap();
            let Action::Resource(command) = &step.engine_actions[0] else {
                panic!("{:?}", step.engine_actions)
            };
            assert_eq!(
                (command.resource.entry().key, &command.amount),
                (key, &Expr::Const(Value::Float(amount)))
            );
            let taken = DamageInput::new(DamageDirection::Take, 12.0).unwrap();
            assert!(program
                .step_damage_with_snapshot(&mut state, taken, &NativeSnapshot::empty())
                .unwrap()
                .engine_actions
                .is_empty());
        }
        assert!(Defense::FillMeterOnHit.graph("mario").is_err());
    }

    #[test]
    fn extra_jump_raises_each_fighters_own_maximum_and_restores_it_after_a_reset() {
        use crate::gimmicks::live_model::{Action, Expr, Value};
        use crate::gimmicks::native_inputs::{NativeInput, NativeObservation, NativeSnapshot};
        let graph = Defense::ExtraJump.graph("kirby").unwrap();
        crate::gimmicks::graph_edit::validate_graph(&graph, "kirby").unwrap();
        let program = crate::gimmicks::live_protocol::program_from_graph("kirby", &graph).unwrap();
        let max = NativeInput::from_reference(
            "work_int:kirby:FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT_MAX",
        );
        let mut state = program.initial_state();
        let mut tick = |value: i32| {
            let snapshot = NativeSnapshot::from_observations(
                std::slice::from_ref(&max),
                &[NativeObservation::Int(value)],
            )
            .unwrap();
            program
                .step_frame_with_snapshot(&mut state, &snapshot)
                .unwrap()
                .engine_actions
        };
        let write = |value| {
            vec![Action::SetNative {
                input: max.clone(),
                value: Expr::Const(Value::Int(value)),
            }]
        };
        assert_eq!(tick(6), write(7), "the first tick measures vanilla's six");
        assert!(
            tick(7).is_empty(),
            "held at the raised maximum, nothing is written"
        );
        assert_eq!(tick(6), write(7), "a reset to vanilla is repaired");
    }

    #[test]
    fn motion_input_sequence_order_timeout_and_facing() {
        use crate::gimmicks::native_inputs::{ControllerInput, NativeSnapshot};
        let graph = Defense::MotionInput.graph("mario").unwrap();
        crate::gimmicks::graph_edit::validate_graph(&graph, "mario").unwrap();
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let run = |facing: f32, inputs: &[(f32, f32, bool)]| {
            let mut state = program.initial_state();
            let mut fired = 0;
            for &(x, y, attack) in inputs {
                let mut input = ControllerInput {
                    stick_x: x,
                    stick_y: y,
                    facing,
                    ..Default::default()
                };
                input
                    .buttons
                    .insert((InputButton::Attack, ButtonEdge::Pressed), attack);
                let mut snapshot = NativeSnapshot::empty();
                snapshot.controller = Some(input);
                fired += program
                    .step_frame_with_snapshot(&mut state, &snapshot)
                    .unwrap()
                    .engine_actions
                    .len();
            }
            fired
        };
        let correct = [
            (0.0, -1.0, false),
            (1.0, -1.0, false),
            (1.0, 0.0, false),
            (1.0, 0.0, true),
        ];
        assert_eq!(run(1.0, &correct), 1);
        assert_eq!(
            run(
                1.0,
                &[
                    (1.0, -1.0, false),
                    (0.0, -1.0, false),
                    (1.0, 0.0, false),
                    (1.0, 0.0, true)
                ]
            ),
            0
        );
        let mut slow = vec![(0.0, -1.0, false); 15];
        slow.extend_from_slice(&correct[1..]);
        assert_eq!(run(1.0, &slow), 0);
        let mirrored: Vec<_> = correct.iter().map(|&(x, y, a)| (-x, y, a)).collect();
        assert_eq!(run(-1.0, &mirrored), 1);
        assert_eq!(run(-1.0, &correct), 0);
    }

    #[test]
    fn auto_face_opponent_turns_only_while_idle() {
        use crate::gimmicks::live_model::Action;
        use crate::gimmicks::native_inputs::{
            Facing, MovementAction, NativeInput, NativeObservation, NativeSnapshot,
        };
        let graph = Defense::AutoFaceOpponent.graph("ryu").unwrap();
        crate::gimmicks::graph_edit::validate_graph(&graph, "ryu").unwrap();
        let program = crate::gimmicks::live_protocol::program_from_graph("ryu", &graph).unwrap();
        let idle = NativeInput::from_reference("status_is:ryu:FIGHTER_STATUS_KIND_WAIT");
        let mut state = program.initial_state();
        for (waiting, expected) in [
            (false, vec![]),
            (
                true,
                vec![Action::Movement(MovementAction::SetFacing(
                    Facing::Opponent,
                ))],
            ),
        ] {
            let snapshot = NativeSnapshot::from_observations(
                std::slice::from_ref(&idle),
                &[NativeObservation::Flag(waiting)],
            )
            .unwrap();
            assert_eq!(
                program
                    .step_frame_with_snapshot(&mut state, &snapshot)
                    .unwrap()
                    .engine_actions,
                expected
            );
        }
    }

    /// The Apply button runs every preset through this path. Validating only the graph let
    /// Extra jump, which reads a fighter-owned native value, fail in the editor alone.
    #[test]
    fn every_preset_survives_the_editor_apply_path() {
        use crate::gimmicks::templates::BehaviorTemplate;
        for defense in Defense::ALL {
            for fighter in ["mario", "cloud", "fox"] {
                let Ok(graph) = defense.graph(fighter) else {
                    continue;
                };
                BehaviorTemplate::from_graph("Controller", Some(fighter), graph)
                    .and_then(|template| template.instantiate(fighter))
                    .unwrap_or_else(|error| panic!("{defense:?} {fighter}: {error}"));
            }
        }
    }

    #[test]
    fn defensive_presets_build_validated_down_special_controllers() {
        for defense in Defense::ALL.into_iter().filter(|d| {
            !matches!(
                d,
                Defense::SteerProjectile
                    | Defense::FillMeterOnHit
                    | Defense::ExtraJump
                    | Defense::AutoFaceOpponent
                    | Defense::MotionInput
                    | Defense::CommandGrab
            )
        }) {
            for fighter in ["mario", "ness", "fox"] {
                let graph = defense
                    .graph(fighter)
                    .unwrap_or_else(|e| panic!("{defense:?} {fighter}: {e}"));
                crate::gimmicks::graph_edit::validate_graph(&graph, fighter).unwrap();
                let program =
                    crate::gimmicks::live_protocol::program_from_graph(fighter, &graph).unwrap();
                assert!(program
                    .status_bindings
                    .iter()
                    .any(|b| b.state == defense.label()));
            }
        }
        let counter = Defense::Counter.graph("mario").unwrap();
        assert_eq!(motions(&counter, "Counter").0, "wait");
        assert_eq!(motions(&counter, "Counter release").0, "attack_s3_s");
    }

    #[test]
    fn command_grab_builds_for_any_fighter_and_lands_in_catch_statuses() {
        for fighter in ["mario", "ganon", "koopa"] {
            let graph = Defense::CommandGrab.graph(fighter).unwrap();
            crate::gimmicks::graph_edit::validate_graph(&graph, fighter).unwrap();
            let program =
                crate::gimmicks::live_protocol::program_from_graph(fighter, &graph).unwrap();
            let bound = |state: &str| {
                program
                    .status_bindings
                    .iter()
                    .find(|b| b.state == state)
                    .unwrap()
                    .binding
                    .target
                    .resolve(fighter)
                    .unwrap()
            };
            let symbol = |name: &str| {
                crate::gimmicks::registry::resolve(
                    fighter,
                    name,
                    crate::gimmicks::registry::IdentifierKind::FighterStatus,
                )
                .unwrap()
                .value
            };
            assert_eq!(bound("Grab"), symbol("FIGHTER_STATUS_KIND_CATCH_TURN"));
            assert_eq!(bound("Carry"), symbol("FIGHTER_STATUS_KIND_CATCH_WAIT"));
        }
        let graph = Defense::CommandGrab.graph("mario").unwrap();
        let carry_actions: Vec<_> = graph
            .transitions
            .iter()
            .filter(|t| t.from == "Carry")
            .flat_map(|t| &t.actions)
            .collect();
        assert!(carry_actions
            .iter()
            .any(|a| matches!(a, Action::ThrowHeldOpponent { .. })));
        assert!(carry_actions
            .iter()
            .any(|a| matches!(a, Action::ReleaseHeldOpponent)));
        let grab_actions: Vec<_> = graph
            .transitions
            .iter()
            .filter(|t| t.from == "Grab")
            .flat_map(|t| &t.actions)
            .collect();
        assert!(grab_actions
            .iter()
            .any(|a| matches!(a, Action::Status(Command::GrabBox { enabled: true, .. }))));
        assert!(grab_actions
            .iter()
            .any(|a| matches!(a, Action::Status(Command::GrabBox { enabled: false, .. }))));
        // A finish rule here would leave the status, and the exit guard would drop the opponent.
        assert!(!graph.transitions.iter().any(|t| t.from == "Carry"
            && t.guard == Predicate::MotionEnded
            && t.actions
                .iter()
                .any(|a| !matches!(a, Action::Status(Command::PlayMotion { .. })))));
        assert!(graph
            .transitions
            .iter()
            .any(|t| t.from == "Carry" && t.guard == Predicate::MotionEnded));
        assert!(!graph.transitions.iter().any(|t| t.from == "Carry"
            && matches!(&t.guard, Predicate::And(a, _) if **a == Predicate::MotionEnded)));
    }

    #[test]
    fn steerable_projectile_sets_the_live_fireball_speed_from_the_stick() {
        use crate::gimmicks::{
            live_model,
            native_inputs::{
                ControllerInput, NativeInput, NativeObservation, NativeSnapshot, ObjectField,
                ObjectOp,
            },
        };
        let graph = Defense::SteerProjectile.graph("mario").unwrap();
        crate::gimmicks::graph_edit::validate_graph(&graph, "mario").unwrap();
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let alive = NativeInput::OwnedObject {
            fighter: "mario".into(),
            symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
            field: ObjectField::Alive,
        };
        let facing = NativeInput::from_reference("facing:mario");
        let mut state = program.initial_state();
        let mut step = |live: bool, lr: f32, stick_x: f32| {
            let observations: Vec<_> = program
                .native_inputs
                .iter()
                .map(|input| {
                    if *input == alive {
                        NativeObservation::Flag(live)
                    } else {
                        assert_eq!(*input, facing);
                        NativeObservation::Float(lr)
                    }
                })
                .collect();
            let mut snapshot =
                NativeSnapshot::from_observations(&program.native_inputs, &observations).unwrap();
            snapshot.controller = Some(ControllerInput {
                stick_x,
                stick_y: 1.0,
                facing: lr,
                ..Default::default()
            });
            program
                .step_frame_with_snapshot(&mut state, &snapshot)
                .unwrap()
                .engine_actions
        };
        let float = |v| live_model::Expr::Const(live_model::Value::Float(v));
        let speed = |x| {
            vec![live_model::Action::Object {
                symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
                op: ObjectOp::SetSpeed {
                    x: float(x),
                    y: float(2.0),
                },
            }]
        };
        // Thrown facing left: a centred stick keeps it flying left; the stick steers on top.
        assert!(step(false, -1.0, 0.0).is_empty());
        assert_eq!(step(true, 1.0, 0.0), speed(-1.0));
        assert_eq!(step(true, 1.0, 0.5), speed(0.0));
        assert!(step(false, 1.0, 0.0).is_empty());
        assert_eq!(step(true, -1.0, -0.5), speed(0.0));
        assert!(Defense::SteerProjectile.graph("cloud").is_ok());
    }

    #[test]
    fn frame_rules_after_a_leaving_sibling_with_a_looser_guard_are_flagged() {
        use crate::gimmicks::graph_edit::unreachable_frame_rules;
        use crate::gimmicks::native_inputs::{NativeInput, NativeObservation, NativeSnapshot};
        // Steer (alive) then Remove (alive and Shield): both stay in the state, so both run.
        let mut graph = Defense::SteerProjectile.graph("mario").unwrap();
        let mut remove = graph.transitions[0].clone();
        remove.guard = and(remove.guard.clone(), button(ButtonEdge::Pressed));
        remove.actions = vec![serde_json::from_value(serde_json::json!({"Object": {"symbol": "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL", "op": "Remove"}})).unwrap()];
        graph.transitions.push(remove);
        graph.emission_plan.bindings[0].transitions.push(2);
        assert!(unreachable_frame_rules(&graph).is_empty());
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let observations: Vec<_> = program
            .native_inputs
            .iter()
            .map(|i| {
                if *i == NativeInput::from_reference("facing:mario") {
                    NativeObservation::Float(1.0)
                } else {
                    NativeObservation::Flag(true)
                }
            })
            .collect();
        let mut snapshot =
            NativeSnapshot::from_observations(&program.native_inputs, &observations).unwrap();
        snapshot.controller = Some(crate::gimmicks::native_inputs::ControllerInput {
            buttons: [((InputButton::Special, ButtonEdge::Pressed), true)].into(),
            ..Default::default()
        });
        let fired: Vec<_> = program
            .step_frame_with_snapshot(&mut program.initial_state(), &snapshot)
            .unwrap()
            .fired
            .iter()
            .map(|f| f.index)
            .collect();
        assert_eq!(fired, vec![0, 2]);
        // Once Steer leaves the state, Remove (a narrower guard) is dead; Not-alive is not.
        graph.states.push(crate::gimmicks::behavior::StateDef {
            name: "Done".into(),
            status: None,
        });
        graph.transitions[0].to = "Done".into();
        assert_eq!(unreachable_frame_rules(&graph), vec![(2, 0)]);
        let fired = |graph: &crate::gimmicks::behavior::BehaviorGraph| -> Vec<usize> {
            let program =
                crate::gimmicks::live_protocol::program_from_graph("mario", graph).unwrap();
            program
                .step_frame_with_snapshot(&mut program.initial_state(), &snapshot)
                .unwrap()
                .fired
                .iter()
                .map(|f| f.index)
                .collect()
        };
        assert_eq!(fired(&graph), vec![0]);
        // A status-bound target only requests its status, so the siblings still run.
        graph.states.last_mut().unwrap().status =
            Some(crate::gimmicks::status_model::StatusBinding {
                target: "FIGHTER_STATUS_KIND_GUARD_ON".into(),
                policy: Default::default(),
                roots: Default::default(),
            });
        assert!(unreachable_frame_rules(&graph).is_empty());
        assert_eq!(fired(&graph), vec![0, 2]);
        graph.states.last_mut().unwrap().status = None;
        // The same guard and no guard shadow too; running the narrower rule first does not.
        graph.transitions[2].guard = graph.transitions[0].guard.clone();
        assert_eq!(unreachable_frame_rules(&graph), vec![(2, 0)]);
        graph.transitions[0].guard = always();
        assert_eq!(unreachable_frame_rules(&graph), vec![(1, 0), (2, 0)]);
        graph.emission_plan.bindings[0].transitions = vec![2, 1, 0];
        assert!(unreachable_frame_rules(&graph).is_empty());
    }

    #[test]
    fn absorber_recipe_toggles_the_volume_and_heals_on_absorb() {
        use crate::gimmicks::native_inputs::{DamageDirection, DamageInput, NativeSnapshot};
        let graph = insert(&owned(), "mario", "Start", Recipe::Absorber).unwrap();
        assert!(insert(&graph, "mario", "Start", Recipe::Absorber).is_err());
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let mut state = program.initial_state();
        let snapshot = NativeSnapshot::empty();
        program
            .step_status_phase(&mut state, "Start", Phase::Pre, &snapshot)
            .unwrap();
        for (phase, expected) in [(Phase::MainEnter, true), (Phase::Exit, false)] {
            let result = program
                .step_status_phase(&mut state, "Start", phase, &snapshot)
                .unwrap();
            assert!(result.engine_actions.iter().any(|a| matches!(a, crate::gimmicks::live_model::Action::Status(Command::Absorber {enabled,..}) if *enabled == expected)), "{phase:?}");
        }
        // Paired: an absorb contact heals; a reflect contact on the same state does not.
        let mut state = program.initial_state();
        program
            .step_status_phase(&mut state, "Start", Phase::Pre, &snapshot)
            .unwrap();
        program
            .step_status_phase(&mut state, "Start", Phase::MainEnter, &snapshot)
            .unwrap();
        let heal = |direction| {
            let mut state = state.clone();
            let input = DamageInput::new(direction, 12.0)
                .unwrap()
                .with_projectile(true);
            program
                .step_damage_with_snapshot(&mut state, input, &snapshot)
                .unwrap()
                .engine_actions
                .into_iter()
                .any(|a| matches!(a, crate::gimmicks::live_model::Action::AddDamage { .. }))
        };
        assert!(heal(DamageDirection::Absorb));
        assert!(!heal(DamageDirection::Reflect));
    }

    #[test]
    fn super_armor_preset_sets_a_status_scoped_policy_on_entry() {
        use crate::gimmicks::native_inputs::{CombatOp, PolicyScope};
        let graph = Defense::SuperArmor.graph("mario").unwrap();
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let mut state = program.initial_state();
        let snapshot = crate::gimmicks::native_inputs::NativeSnapshot::empty();
        let policy = crate::gimmicks::live_model::Action::CombatPolicy {
            scope: PolicyScope::Status,
            op: CombatOp::SuperArmor,
        };
        program
            .step_status_phase(&mut state, "Super armor", Phase::Pre, &snapshot)
            .unwrap();
        let entry = program
            .step_status_phase(&mut state, "Super armor", Phase::MainEnter, &snapshot)
            .unwrap();
        assert!(
            entry.engine_actions.contains(&policy),
            "{:?}",
            entry.engine_actions
        );
        let exit = program
            .step_status_phase(&mut state, "Super armor", Phase::Exit, &snapshot)
            .unwrap();
        assert!(!exit.engine_actions.contains(&policy));
    }

    #[test]
    fn reflector_recipe_enables_on_entry_and_disables_on_exit() {
        let graph = insert(&owned(), "mario", "Start", Recipe::Reflector).unwrap();
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let mut state = program.initial_state();
        let snapshot = crate::gimmicks::native_inputs::NativeSnapshot::empty();
        program
            .step_status_phase(&mut state, "Start", Phase::Pre, &snapshot)
            .unwrap();
        for (phase, expected) in [(Phase::MainEnter, true), (Phase::Exit, false)] {
            let result = program
                .step_status_phase(&mut state, "Start", phase, &snapshot)
                .unwrap();
            assert!(result.engine_actions.iter().any(|a|matches!(a,crate::gimmicks::live_model::Action::Status(Command::Reflector {enabled,..}) if *enabled==expected)));
        }
    }

    #[test]
    fn marth_window_captures_only_the_first_contact_and_resets_on_entry() {
        use crate::gimmicks::native_inputs::{
            DamageDirection, DamageInput, NativeSnapshot, PhaseInput,
        };
        // Vanilla game_speciallw enables SHIELD at 6 and disables it at 28.
        let graph = insert(&owned(), "mario", "Start", Recipe::Counter).unwrap();
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        for (frame, accepted) in [(5.0, false), (6.0, true), (27.0, true), (28.0, false)] {
            let mut state = program.initial_state();
            let mut snapshot = NativeSnapshot::empty();
            snapshot.phase = Some(PhaseInput {
                fighter: "mario".into(),
                status: 0,
                status_frame: 100,
                motion_frame: frame,
                motion: 0,
                situation: Some(SituationKind::Ground),
            });
            program
                .step_status_phase(&mut state, "Start", Phase::Pre, &snapshot)
                .unwrap();
            program
                .step_status_phase(&mut state, "Start", Phase::MainEnter, &snapshot)
                .unwrap();
            let hit = DamageInput::new(DamageDirection::Incoming, 12.0).unwrap();
            let first = program
                .step_damage_with_snapshot(&mut state, hit, &snapshot)
                .unwrap();
            assert_eq!(!first.engine_actions.is_empty(), accepted, "frame {frame}");
            let damage = state
                .values
                .iter()
                .find(|(name, _)| name.starts_with("counter_damage_"))
                .unwrap()
                .1;
            assert_eq!(
                *damage,
                crate::gimmicks::live_model::Value::Float(if accepted { 12.0 } else { 0.0 })
            );
            if accepted {
                let second = program
                    .step_damage_with_snapshot(
                        &mut state,
                        DamageInput::new(DamageDirection::Incoming, 30.0).unwrap(),
                        &snapshot,
                    )
                    .unwrap();
                assert!(second.engine_actions.is_empty());
                let release = program
                    .status_bindings
                    .iter()
                    .find(|b| b.state != "Start")
                    .unwrap()
                    .state
                    .clone();
                program
                    .step_status_phase(&mut state, &release, Phase::Pre, &snapshot)
                    .unwrap();
                let scaled = program
                    .step_status_phase(&mut state, &release, Phase::Exec, &snapshot)
                    .unwrap();
                assert!(scaled.engine_actions.iter().any(|a| matches!(
                    a,
                    crate::gimmicks::live_model::Action::Status(Command::SetAttackDamage {
                        id: 0,
                        damage: crate::gimmicks::live_model::Expr::Const(
                            crate::gimmicks::live_model::Value::Float(12.0)
                        )
                    })
                )));
                program
                    .step_status_phase(&mut state, "Start", Phase::Pre, &snapshot)
                    .unwrap();
                program
                    .step_status_phase(&mut state, "Start", Phase::MainEnter, &snapshot)
                    .unwrap();
                assert!(!program
                    .step_damage_with_snapshot(&mut state, hit, &snapshot)
                    .unwrap()
                    .engine_actions
                    .is_empty());
            }
        }
    }

    #[test]
    fn invalid_motion_keeps_the_original_graph() {
        let graph = owned();
        let before = graph.clone();
        assert!(edit_motion(&graph, "mario", "Start", false, "").is_err());
        assert_eq!(graph, before);
    }
}
