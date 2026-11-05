//! Typed commands admitted only by an installed status phase.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MotionFrame<E> {
    Start(E),
    Inherit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Command<E> {
    Reflector {
        enabled: bool,
        size: f32,
        damage: f32,
        speed: f32,
    },
    /// A reflector-shaped volume whose contacts arrive as OnAbsorb and remove
    /// the projectile instead of sending it back.
    Absorber {
        enabled: bool,
        size: f32,
    },
    /// One CATCH box in front of the fighter. The game forms the capture link only
    /// while the status is one of its catch statuses, so bind the state to one.
    GrabBox {
        enabled: bool,
        size: f32,
        height: f32,
        forward: f32,
    },
    SetAttackDamage {
        id: u8,
        damage: E,
    },
    TransitionTerm {
        symbol: String,
        enabled: bool,
    },
    TransitionGroup {
        symbol: String,
        enabled: bool,
    },
    EnableCancel,
    TryCommonTransition {
        kind: CommonTransition,
    },
    ChangeKinetic {
        kind: Kinetic,
    },
    ConfigureEnergy {
        energy: Energy,
        op: EnergyOp<E>,
    },
    SetSituation {
        situation: Situation,
    },
    SetGroundPolicy {
        correction: Correction,
        cliff: Cliff,
    },
    InitializeStatus(Settings),
    PlayMotion {
        motion: String,
        frame: MotionFrame<E>,
        rate: E,
    },
    ChangeStatusNow {
        symbol: super::status_model::StatusRef,
    },
    RequestStatus {
        target: super::status_model::StatusRef,
    },
}

impl<E> Command<E> {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Reflector { .. } => "Reflect projectiles",
            Self::Absorber { .. } => "Absorb projectiles",
            Self::GrabBox { .. } => "Grab box",
            Self::SetAttackDamage { .. } => "Set hitbox damage",
            Self::TransitionTerm { .. } => "Allow transition",
            Self::TransitionGroup { .. } => "Allow transition group",
            Self::EnableCancel => "Enable cancel",
            Self::TryCommonTransition { .. } => "Try common transition",
            Self::ChangeKinetic { .. } => "Change kinetics",
            Self::ConfigureEnergy { .. } => "Configure energy",
            Self::SetSituation { .. } => "Set situation",
            Self::SetGroundPolicy { .. } => "Ground / cliff policy",
            Self::InitializeStatus(_) => "Initialize status",
            Self::PlayMotion { .. } => "Play motion",
            Self::RequestStatus { .. } => "Request status",
            Self::ChangeStatusNow { .. } => "Change status now",
        }
    }
    pub fn try_map<T, F>(&self, mut f: impl FnMut(&E) -> Result<T, F>) -> Result<Command<T>, F> {
        Ok(match self {
            Self::Reflector {
                enabled,
                size,
                damage,
                speed,
            } => Command::Reflector {
                enabled: *enabled,
                size: *size,
                damage: *damage,
                speed: *speed,
            },
            Self::GrabBox {
                enabled,
                size,
                height,
                forward,
            } => Command::GrabBox {
                enabled: *enabled,
                size: *size,
                height: *height,
                forward: *forward,
            },
            Self::Absorber { enabled, size } => Command::Absorber {
                enabled: *enabled,
                size: *size,
            },
            Self::SetAttackDamage { id, damage } => Command::SetAttackDamage {
                id: *id,
                damage: f(damage)?,
            },
            Self::TransitionTerm { symbol, enabled } => Command::TransitionTerm {
                symbol: symbol.clone(),
                enabled: *enabled,
            },
            Self::TransitionGroup { symbol, enabled } => Command::TransitionGroup {
                symbol: symbol.clone(),
                enabled: *enabled,
            },
            Self::EnableCancel => Command::EnableCancel,
            Self::TryCommonTransition { kind } => Command::TryCommonTransition { kind: *kind },
            Self::ChangeKinetic { kind } => Command::ChangeKinetic { kind: *kind },
            Self::ConfigureEnergy { energy, op } => Command::ConfigureEnergy {
                energy: *energy,
                op: op.try_map(&mut f)?,
            },
            Self::SetSituation { situation } => Command::SetSituation {
                situation: *situation,
            },
            Self::SetGroundPolicy { correction, cliff } => Command::SetGroundPolicy {
                correction: *correction,
                cliff: *cliff,
            },
            Self::InitializeStatus(s) => Command::InitializeStatus(s.clone()),
            Self::PlayMotion {
                motion,
                frame,
                rate,
            } => Command::PlayMotion {
                motion: motion.clone(),
                frame: match frame {
                    MotionFrame::Start(e) => MotionFrame::Start(f(e)?),
                    MotionFrame::Inherit => MotionFrame::Inherit,
                },
                rate: f(rate)?,
            },
            Self::RequestStatus { target } => Command::RequestStatus {
                target: target.clone(),
            },
            Self::ChangeStatusNow { symbol } => Command::ChangeStatusNow {
                symbol: symbol.clone(),
            },
        })
    }

    pub fn expressions_mut(&mut self) -> Vec<&mut E> {
        match self {
            Self::SetAttackDamage { damage, .. } => vec![damage],
            Self::ConfigureEnergy { op, .. } => op.expressions_mut(),
            Self::PlayMotion { frame, rate, .. } => match frame {
                MotionFrame::Start(frame) => vec![frame, rate],
                MotionFrame::Inherit => vec![rate],
            },
            _ => vec![],
        }
    }
    pub fn expressions(&self) -> Vec<&E> {
        match self {
            Self::SetAttackDamage { damage, .. } => vec![damage],
            Self::ConfigureEnergy { op, .. } => op.expressions(),
            Self::PlayMotion { frame, rate, .. } => match frame {
                MotionFrame::Start(frame) => vec![frame, rate],
                MotionFrame::Inherit => vec![rate],
            },
            _ => vec![],
        }
    }
    pub fn permits(&self, phase: super::status_model::Phase) -> bool {
        use super::status_model::Phase;
        match self {
            Self::Reflector { .. } | Self::Absorber { .. } | Self::GrabBox { .. } => matches!(
                phase,
                Phase::MainEnter | Phase::MainLoop | Phase::Exec | Phase::End | Phase::Exit
            ),
            Self::SetAttackDamage { .. } => matches!(phase, Phase::MainLoop | Phase::Exec),
            Self::TryCommonTransition { .. } => phase == Phase::MainLoop,
            Self::TransitionTerm { .. } | Self::TransitionGroup { .. } | Self::EnableCancel => {
                matches!(
                    phase,
                    Phase::Init | Phase::MainEnter | Phase::MainLoop | Phase::Exec
                )
            }
            Self::InitializeStatus(_) => phase == Phase::Pre,
            Self::ChangeKinetic { .. }
            | Self::ConfigureEnergy { .. }
            | Self::SetSituation { .. }
            | Self::SetGroundPolicy { .. }
            | Self::PlayMotion { .. } => matches!(
                phase,
                Phase::MainEnter | Phase::MainLoop | Phase::Init | Phase::Exec
            ),
            Self::ChangeStatusNow { .. } | Self::RequestStatus { .. } => phase == Phase::MainLoop,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Situation {
    #[default]
    Keep,
    Ground,
    Air,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kinetic {
    #[default]
    Unique,
    GroundStop,
    Fall,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Correction {
    #[default]
    Keep,
    Ground,
    Air,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cliff {
    #[default]
    None,
    Always,
    AlwaysBothSides,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkKeep {
    #[default]
    None,
    All,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tread {
    #[default]
    NoReaction,
    Enable,
    Disable,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecialKind {
    Neutral,
    #[default]
    Side,
    Up,
    Down,
}

/// SDK booleans with unspecified names retain their pinned argument numbers;
/// no inferred engine semantics are attached to those fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StatusData {
    pub sdk_arg2: bool,
    pub tread: Tread,
    pub sdk_arg4: bool,
    pub grab_invulnerable: bool,
    pub sdk_arg6: bool,
    pub special: SpecialKind,
    pub log_attack: bool,
    pub log_action_trigger: bool,
    pub start_turn: bool,
    pub power_up_special: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub situation: Situation,
    pub kinetic: Kinetic,
    pub correction: Correction,
    pub cliff: Cliff,
    pub jostle: bool,
    pub keep_flags: WorkKeep,
    pub keep_ints: WorkKeep,
    pub keep_floats: WorkKeep,
    pub status_data: StatusData,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            situation: Situation::Keep,
            kinetic: Kinetic::Unique,
            correction: Correction::Keep,
            cliff: Cliff::None,
            jostle: true,
            keep_flags: WorkKeep::None,
            keep_ints: WorkKeep::None,
            keep_floats: WorkKeep::None,
            status_data: StatusData::default(),
        }
    }
}

pub fn validate(command: &Command<super::live_model::Expr>, fighter: &str) -> Result<(), String> {
    use super::live_model::{Expr, Value};
    match command {
        Command::Absorber { size, .. } => {
            if !size.is_finite() || !(0.1..=50.0).contains(size) {
                return Err("Absorber size outside bounds".into());
            }
        }
        Command::Reflector {
            size,
            damage,
            speed,
            ..
        } => {
            if !size.is_finite()
                || !(0.1..=50.0).contains(size)
                || !damage.is_finite()
                || !(0.1..=10.0).contains(damage)
                || !speed.is_finite()
                || !(0.1..=10.0).contains(speed)
            {
                return Err("Reflector size or multiplier outside bounds".into());
            }
        }
        Command::GrabBox {
            size,
            height,
            forward,
            ..
        } => {
            if !size.is_finite()
                || !(0.1..=50.0).contains(size)
                || !height.is_finite()
                || height.abs() > 100.0
                || !forward.is_finite()
                || forward.abs() > 100.0
            {
                return Err("Grab box size or position outside bounds".into());
            }
        }
        Command::SetAttackDamage { id, damage } => {
            if *id > 7 {
                return Err("hitbox ID must be 0 through 7".into());
            }
            if let Expr::Const(value) = damage {
                let n = match value {
                    Value::Float(v) => *v,
                    Value::Int(v) => *v as f32,
                    _ => return Err("hitbox damage must be numeric".into()),
                };
                if !n.is_finite() || !(0.0..=999.0).contains(&n) {
                    return Err("hitbox damage must be 0 through 999".into());
                }
            }
        }
        Command::TransitionTerm { symbol, .. } | Command::TransitionGroup { symbol, .. } => {
            let kind = if matches!(command, Command::TransitionTerm { .. }) {
                super::registry::IdentifierKind::TransitionTerm
            } else {
                super::registry::IdentifierKind::TransitionGroup
            };
            super::registry::resolve(fighter, symbol, kind).map_err(|e| e.to_string())?;
        }
        Command::EnableCancel | Command::TryCommonTransition { .. } => {}
        Command::PlayMotion {
            motion,
            frame,
            rate,
        } => {
            if motion.is_empty()
                || motion.len() > 64
                || !motion
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            {
                return Err("motion must be a bounded animation name".into());
            }
            for (expr, min, max) in std::iter::once((rate, 0.05, 20.0)).chain(match frame {
                MotionFrame::Start(frame) => Some((frame, 0.0, 100000.0)),
                MotionFrame::Inherit => None,
            }) {
                if let Expr::Const(value) = expr {
                    let n = match value {
                        Value::Float(v) => *v,
                        Value::Int(v) => *v as f32,
                        _ => return Err("motion argument must be numeric".into()),
                    };
                    if !n.is_finite() || !(min..=max).contains(&n) {
                        return Err("motion argument outside bounded range".into());
                    }
                }
            }
        }
        Command::ChangeStatusNow { symbol: target } | Command::RequestStatus { target } => {
            target.resolve(fighter)?;
        }
        Command::ConfigureEnergy { energy, op } => {
            op.validate(*energy)?;
            for expr in op.expressions() {
                if let Expr::Const(value) = expr {
                    let n = match value {
                        Value::Float(v) => *v,
                        Value::Int(v) => *v as f32,
                        _ => return Err("energy argument must be numeric".into()),
                    };
                    if !n.is_finite() || n.abs() > 10.0 {
                        return Err("energy argument outside ±10".into());
                    }
                }
            }
            if *energy == Energy::Gravity {
                if let Some(Expr::Const(value)) = op.expressions().first() {
                    if !matches!(value, Value::Float(0.0) | Value::Int(0)) {
                        return Err("gravity has no horizontal component".into());
                    }
                }
            }
        }
        Command::InitializeStatus(_)
        | Command::ChangeKinetic { .. }
        | Command::SetSituation { .. }
        | Command::SetGroundPolicy { .. } => {}
    }
    Ok(())
}

/// Closed SDK energy IDs and reset modes. No saved numeric engine identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Energy {
    Stop,
    Gravity,
    Control,
    Motion,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reset {
    StopGround,
    StopAir,
    Gravity,
    ControlFallAdjust,
    MotionGround,
    MotionAir,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum EnergyOp<E> {
    Reset { kind: Reset, x: E, y: E },
    Enable,
    Disable,
    Speed { x: E, y: E },
    Accel { x: E, y: E },
    Brake { x: E, y: E },
    Limit { x: E, y: E },
    Stable { x: E, y: E },
}
impl<E> EnergyOp<E> {
    pub fn validate(&self, energy: Energy) -> Result<(), String> {
        use {Energy as E, EnergyOp as O, Reset as R};
        let valid = match self {
            O::Enable | O::Disable => true,
            O::Reset { kind, .. } => matches!(
                (energy, kind),
                (E::Stop, R::StopGround | R::StopAir)
                    | (E::Gravity, R::Gravity)
                    | (E::Control, R::ControlFallAdjust)
                    | (E::Motion, R::MotionGround | R::MotionAir)
            ),
            O::Accel { .. } => matches!(energy, E::Stop | E::Gravity),
            O::Speed { .. } | O::Brake { .. } | O::Limit { .. } | O::Stable { .. } => {
                energy != E::Motion
            }
        };
        if valid {
            Ok(())
        } else {
            Err("unsupported energy operation or reset mode".into())
        }
    }
    pub fn expressions(&self) -> Vec<&E> {
        match self {
            Self::Enable | Self::Disable => vec![],
            Self::Reset { x, y, .. }
            | Self::Speed { x, y }
            | Self::Accel { x, y }
            | Self::Brake { x, y }
            | Self::Limit { x, y }
            | Self::Stable { x, y } => vec![x, y],
        }
    }
    pub fn expressions_mut(&mut self) -> Vec<&mut E> {
        match self {
            Self::Enable | Self::Disable => vec![],
            Self::Reset { x, y, .. }
            | Self::Speed { x, y }
            | Self::Accel { x, y }
            | Self::Brake { x, y }
            | Self::Limit { x, y }
            | Self::Stable { x, y } => vec![x, y],
        }
    }
    pub fn try_map<T, F>(&self, mut f: impl FnMut(&E) -> Result<T, F>) -> Result<EnergyOp<T>, F> {
        Ok(match self {
            Self::Enable => EnergyOp::Enable,
            Self::Disable => EnergyOp::Disable,
            Self::Reset { kind, x, y } => EnergyOp::Reset {
                kind: *kind,
                x: f(x)?,
                y: f(y)?,
            },
            Self::Speed { x, y } => EnergyOp::Speed { x: f(x)?, y: f(y)? },
            Self::Accel { x, y } => EnergyOp::Accel { x: f(x)?, y: f(y)? },
            Self::Brake { x, y } => EnergyOp::Brake { x: f(x)?, y: f(y)? },
            Self::Limit { x, y } => EnergyOp::Limit { x: f(x)?, y: f(y)? },
            Self::Stable { x, y } => EnergyOp::Stable { x: f(x)?, y: f(y)? },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::live_model::{Expr, Value};
    use super::*;
    fn n(v: f32) -> Expr {
        Expr::Const(Value::Float(v))
    }
    #[test]
    fn reflector_roundtrip_bounds_and_phase() {
        for (size, damage, speed, valid) in [
            (6.0, 1.5, 1.5, true),
            (0.0, 1.5, 1.5, false),
            (6.0, -1.0, 1.5, false),
            (6.0, 1.5, f32::NAN, false),
        ] {
            let command: Command<Expr> = Command::Reflector {
                enabled: true,
                size,
                damage,
                speed,
            };
            assert_eq!(validate(&command, "mario").is_ok(), valid);
            assert!(command.permits(super::super::status_model::Phase::Exit));
            assert!(!command.permits(super::super::status_model::Phase::Pre));
            if valid {
                assert_eq!(
                    serde_json::from_str::<Command<Expr>>(
                        &serde_json::to_string(&command).unwrap()
                    )
                    .unwrap(),
                    command
                );
            }
        }
    }

    #[test]
    fn grab_box_roundtrip_bounds_and_phase() {
        for (size, height, forward, valid) in [
            (6.0, 8.0, 10.0, true),
            (0.0, 8.0, 10.0, false),
            (6.0, f32::NAN, 10.0, false),
            (6.0, 8.0, 101.0, false),
        ] {
            let command: Command<Expr> = Command::GrabBox {
                enabled: true,
                size,
                height,
                forward,
            };
            assert_eq!(validate(&command, "mario").is_ok(), valid);
            assert!(command.permits(super::super::status_model::Phase::MainLoop));
            assert!(!command.permits(super::super::status_model::Phase::Pre));
            if valid {
                assert_eq!(
                    serde_json::from_str::<Command<Expr>>(
                        &serde_json::to_string(&command).unwrap()
                    )
                    .unwrap(),
                    command
                );
            }
        }
    }

    #[test]
    fn absorber_roundtrip_bounds_and_phase() {
        for (size, valid) in [
            (6.0, true),
            (50.0, true),
            (0.0, false),
            (51.0, false),
            (f32::NAN, false),
        ] {
            let command: Command<Expr> = Command::Absorber {
                enabled: true,
                size,
            };
            assert_eq!(validate(&command, "ness").is_ok(), valid);
            assert!(command.permits(super::super::status_model::Phase::MainEnter));
            assert!(command.permits(super::super::status_model::Phase::Exit));
            assert!(!command.permits(super::super::status_model::Phase::Pre));
            if valid {
                assert_eq!(
                    serde_json::from_str::<Command<Expr>>(
                        &serde_json::to_string(&command).unwrap()
                    )
                    .unwrap(),
                    command
                );
            }
        }
    }

    #[test]
    fn hitbox_damage_roundtrips_and_rejects_invalid_values() {
        for (id, damage, valid) in [
            (0, 8.0, true),
            (7, 50.0, true),
            (8, 8.0, false),
            (0, -1.0, false),
            (0, f32::NAN, false),
            (0, 1000.0, false),
        ] {
            let command = Command::SetAttackDamage {
                id,
                damage: n(damage),
            };
            assert_eq!(validate(&command, "mario").is_ok(), valid);
            assert!(command.permits(super::super::status_model::Phase::Exec));
            assert!(!command.permits(super::super::status_model::Phase::Pre));
            if valid {
                let restored: Command<Expr> =
                    serde_json::from_str(&serde_json::to_string(&command).unwrap()).unwrap();
                assert_eq!(restored, command);
                assert_eq!(
                    command.try_map(|_| Ok::<_, ()>(12)).unwrap().expressions(),
                    vec![&12]
                );
            }
        }
    }

    #[test]
    fn energy_operation_and_reset_matrix() {
        use Energy::*;
        for energy in [Stop, Gravity, Control, Motion] {
            for op in [
                EnergyOp::Enable,
                EnergyOp::Disable,
                EnergyOp::Speed { x: 0, y: 0 },
                EnergyOp::Accel { x: 0, y: 0 },
                EnergyOp::Brake { x: 0, y: 0 },
                EnergyOp::Limit { x: 0, y: 0 },
                EnergyOp::Stable { x: 0, y: 0 },
            ] {
                let expected = match op {
                    EnergyOp::Enable | EnergyOp::Disable => true,
                    EnergyOp::Accel { .. } => matches!(energy, Stop | Gravity),
                    _ => energy != Motion,
                };
                assert_eq!(op.validate(energy).is_ok(), expected, "{energy:?}/{op:?}");
            }
            for kind in [
                Reset::StopGround,
                Reset::StopAir,
                Reset::Gravity,
                Reset::ControlFallAdjust,
                Reset::MotionGround,
                Reset::MotionAir,
            ] {
                let expected = match kind {
                    Reset::StopGround | Reset::StopAir => Stop,
                    Reset::Gravity => Gravity,
                    Reset::ControlFallAdjust => Control,
                    Reset::MotionGround | Reset::MotionAir => Motion,
                };
                assert_eq!(
                    EnergyOp::Reset { kind, x: 0, y: 0 }
                        .validate(energy)
                        .is_ok(),
                    energy == expected
                );
            }
        }
    }
    #[test]
    fn bounded_numeric_and_phase_admission() {
        for value in [f32::NAN, f32::INFINITY, -10.01, 10.01] {
            assert!(validate(
                &Command::ConfigureEnergy {
                    energy: Energy::Stop,
                    op: EnergyOp::Speed {
                        x: n(value),
                        y: n(0.0)
                    }
                },
                "mario"
            )
            .is_err());
        }
        let gravity = Command::ConfigureEnergy {
            energy: Energy::Gravity,
            op: EnergyOp::Speed {
                x: n(1.0),
                y: n(2.0),
            },
        };
        assert!(validate(&gravity, "mario").is_err());
        for phase in [
            super::super::status_model::Phase::Pre,
            super::super::status_model::Phase::End,
            super::super::status_model::Phase::Exit,
        ] {
            assert!(!gravity.permits(phase));
        }
        assert!(serde_json::from_str::<Settings>(r#"{"keep_flags":42}"#).is_err());
        assert!(serde_json::from_str::<Command<Expr>>(r#"{"ChangeKinetic":{"kind":5}}"#).is_err());
    }
    #[test]
    fn energy_mapping_retains_both_coordinates_and_reset() {
        let command = Command::ConfigureEnergy {
            energy: Energy::Stop,
            op: EnergyOp::Reset {
                kind: Reset::StopAir,
                x: 2,
                y: 3,
            },
        };
        let mapped = command.try_map(|v| Ok::<_, ()>(v * 2)).unwrap();
        assert_eq!(mapped.expressions(), vec![&4, &6]);
        assert_eq!(
            serde_json::from_str::<Command<i32>>(&serde_json::to_string(&mapped).unwrap()).unwrap(),
            mapped
        );
    }
}

/// Effectful native checks. A handled result terminates this MainLoop invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommonTransition {
    AirCliff,
    GroundWait,
    AirFall,
    AirLanding,
    AirSpecial,
}

#[cfg(test)]
mod cancel_tests {
    use super::*;
    #[test]
    fn transition_identifier_kind_and_phase_are_exact() {
        let term = "FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_SQUAT";
        let group = "FIGHTER_STATUS_TRANSITION_GROUP_CHK_GROUND";
        for (command, valid) in [
            (
                Command::TransitionTerm {
                    symbol: term.into(),
                    enabled: true,
                },
                true,
            ),
            (
                Command::TransitionGroup {
                    symbol: group.into(),
                    enabled: false,
                },
                true,
            ),
            (
                Command::TransitionTerm {
                    symbol: group.into(),
                    enabled: true,
                },
                false,
            ),
            (
                Command::TransitionGroup {
                    symbol: term.into(),
                    enabled: true,
                },
                false,
            ),
            (
                Command::TransitionTerm {
                    symbol: "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE".into(),
                    enabled: true,
                },
                false,
            ),
        ] {
            assert_eq!(validate(&command, "mario").is_ok(), valid);
            assert!(validate(&command, "unknown").is_err());
        }
        let check = Command::<super::super::live_model::Expr>::TryCommonTransition {
            kind: CommonTransition::AirCliff,
        };
        assert!(check.permits(super::super::status_model::Phase::MainLoop));
        assert!(!check.permits(super::super::status_model::Phase::Exec));
    }
}
