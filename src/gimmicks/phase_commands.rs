//! Bounded phase commands shared by native delivery and host verification.
use super::live_model::{self as model, Action, Expr, Value};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Movement(super::native_inputs::MovementAction),
    HeldDamage(f32),
    ThrowHeldOpponent(super::native_inputs::ThrowDirection),
    ReleaseHeldOpponent,
    AddDamage(f32),
    Combat(
        super::native_inputs::PolicyScope,
        super::native_inputs::CombatOp<f32>,
    ),
    Rate(f32),
    Speed {
        x: f32,
        y: f32,
    },
    HoldItem(i32),
    ThrowItem {
        angle: f32,
        power: f32,
    },
    DropItem,
    RemoveItem,
    MeshVisibility {
        mesh: u64,
        visible: bool,
    },
}

pub fn resolve(action: &Action) -> Result<Command, String> {
    let scalar = |expr: &Expr| match expr {
        Expr::Const(Value::Float(value)) if model::phase_scalar_valid(action, *value) => Ok(*value),
        _ => Err("phase action requires a resolved bounded Float value".to_string()),
    };
    match action {
        Action::AddHeldOpponentDamage { amount } => Ok(Command::HeldDamage(scalar(amount)?)),
        Action::ThrowHeldOpponent { direction } => Ok(Command::ThrowHeldOpponent(*direction)),
        Action::ReleaseHeldOpponent => Ok(Command::ReleaseHeldOpponent),
        Action::AddDamage { amount } => Ok(Command::AddDamage(scalar(amount)?)),
        Action::CombatPolicy { scope, op } => {
            super::native_inputs::validate_policy_scope(*scope)?;
            Ok(Command::Combat(*scope, op.try_map(scalar)?))
        }
        Action::SetMotionRate { rate } => Ok(Command::Rate(scalar(rate)?)),
        Action::AddSpeed { x, y } => Ok(Command::Speed {
            x: scalar(x)?,
            y: scalar(y)?,
        }),
        Action::Movement(op) => {
            op.validate()?;
            Ok(Command::Movement(*op))
        }
        Action::Item(op) => {
            use super::native_inputs::ItemAction;
            op.validate()?;
            Ok(match op {
                ItemAction::Hold { item } => {
                    Command::HoldItem(ItemAction::item_kind(item).ok_or("unknown item kind")?)
                }
                ItemAction::Throw { angle, power } => Command::ThrowItem {
                    angle: *angle,
                    power: *power,
                },
                ItemAction::Drop => Command::DropItem,
                ItemAction::Remove => Command::RemoveItem,
            })
        }
        Action::MeshVisibility { mesh, visible } => {
            super::native_inputs::validate_mesh_name(mesh)?;
            Ok(Command::MeshVisibility {
                mesh: super::native_inputs::hash40(mesh),
                visible: *visible,
            })
        }
        _ => Err("unsupported phase action".into()),
    }
}

/// Prove identity around every engine read/write. Facing is sampled at the
/// command's delivery point, after any preceding authored status request.
pub fn execute_guarded(
    command: Command,
    mut check: impl FnMut() -> Result<(), String>,
    mut facing: impl FnMut() -> f32,
    mut apply: impl FnMut(Command),
) -> Result<(), String> {
    check()?;
    let command = match command {
        Command::Speed { x, y } => {
            let lr = facing();
            check()?;
            if !matches!(lr, -1.0 | 1.0) {
                return Err("invalid fighter facing".into());
            }
            Command::Speed { x: x * lr, y }
        }
        rate => rate,
    };
    check()?;
    apply(command);
    check()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_loss_prevents_delivery_and_facing_only_changes_x() {
        let command = Command::Speed { x: 2.0, y: -0.5 };
        let mut delivered = None;
        execute_guarded(command, || Ok(()), || -1.0, |c| delivered = Some(c)).unwrap();
        assert_eq!(delivered, Some(Command::Speed { x: -2.0, y: -0.5 }));
        for fail_at in 0..3 {
            let mut checks = 0;
            let mut delivered = false;
            assert!(execute_guarded(
                command,
                || {
                    let current = checks;
                    checks += 1;
                    if current == fail_at {
                        Err("identity changed".into())
                    } else {
                        Ok(())
                    }
                },
                || -1.0,
                |_| delivered = true
            )
            .is_err());
            assert!(!delivered);
        }
        for lr in [0.0, f32::NAN] {
            assert!(execute_guarded(
                command,
                || Ok(()),
                || lr,
                |_| panic!("invalid facing delivered")
            )
            .is_err());
        }
        execute_guarded(
            Command::Rate(1.5),
            || Ok(()),
            || panic!("rate read facing"),
            |c| assert_eq!(c, Command::Rate(1.5)),
        )
        .unwrap();
    }
}
