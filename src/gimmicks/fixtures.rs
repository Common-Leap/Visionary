//! Synthetic dump and manifest fixtures for the F4 regression tests.
//!
//! Test-only: nothing here ships. The synthetic Cloud file mirrors the shape
//! real personal prcs have (nested groups plus an indexed list) without
//! copying any game data — every name, value, and count is invented.

use std::path::Path;

use prc::ParamKind;

/// A Cloud-shaped personal file: a nested group plus an indexed list entry.
///
/// Values are invented; only the nesting shape (group → list → fields) is
/// what the walker and verifier must handle on a real dump.
pub(crate) fn cloud_prc() -> prc::ParamStruct {
    use prc::hash40::Hash40;
    let hash = |name: &str| Hash40(hash40::hash40(name).0);
    prc::ParamStruct(vec![
        (
            hash("param_special_lw"),
            ParamKind::List(prc::ParamList(vec![ParamKind::Struct(prc::ParamStruct(
                vec![
                    (hash("limit_gauge_add"), ParamKind::Float(0.667)),
                    (hash("limit_gauge_take"), ParamKind::Float(1.0)),
                    (hash("charge_per_frame"), ParamKind::Float(0.3)),
                ],
            ))])),
        ),
        (hash("walk_speed_mul"), ParamKind::Float(1.15)),
        (hash("limit_max_frames"), ParamKind::I32(900)),
    ])
}

/// A second fighter's file: flat scalars only, proving multi-fighter indexing.
pub(crate) fn link_prc() -> prc::ParamStruct {
    use prc::hash40::Hash40;
    let hash = |name: &str| Hash40(hash40::hash40(name).0);
    prc::ParamStruct(vec![
        (hash("bomb_fuse"), ParamKind::I32(100)),
        (hash("can_trade_blow"), ParamKind::Bool(true)),
    ])
}

pub(crate) fn write_prc(dir: &Path, rel: &str, tree: &prc::ParamStruct) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    prc::save(&path, tree).unwrap();
}

/// A synthetic dump root: Cloud (nested prc + articles), Link (flat prc, no
/// article dir), Kirby (directory but no prc and no articles).
pub(crate) fn good_dump() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_prc(dir.path(), "fighter/cloud/param/vl.prc", &cloud_prc());
    let article = dir.path().join("fighter/cloud/article");
    std::fs::create_dir_all(article.join("limit_gauge")).unwrap();
    std::fs::write(article.join("sword.prc"), b"placeholder").unwrap();
    write_prc(dir.path(), "fighter/link/param/vl.prc", &link_prc());
    std::fs::create_dir_all(dir.path().join("fighter/kirby")).unwrap();
    dir
}

/// A selected plugin with the two managed generated source slots.
pub(crate) fn plugin(root: &Path) {
    let modules = root.join("src/slight/gimmicks");
    std::fs::create_dir_all(&modules).unwrap();
    for (path, contents) in [
        (
            root.join("Cargo.toml"),
            "[package]\nname = \"slight_replica\"\n",
        ),
        (
            modules.join("generated.rs"),
            "const SLOT: &str = \"visionary.gimmicks.generated-slot.v1\";\n",
        ),
        (
            modules.join("acmd_generated.rs"),
            "const SLOT: &str = \"visionary.gimmicks.generated-acmd-slot.v1\";\n",
        ),
    ] {
        std::fs::write(path, contents).unwrap();
    }
}

use super::behavior::{
    Action, BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, StateDef, Transition, VarDef,
    VarType,
};
use super::codegen::{EmissionPlan, HookBinding};
use super::hooks::HookName;
pub(crate) fn charge_graph() -> BehaviorGraph {
    BehaviorGraph {
        custom_statuses: Default::default(),
        initial: "idle".to_string(),
        states: ["idle", "full"]
            .iter()
            .map(|name| StateDef {
                status: None,
                name: name.to_string(),
            })
            .collect(),
        vars: vec![
            VarDef {
                keep_across_form_switch: false,
                name: "gauge".to_string(),
                kind: VarType::Float,
                initial: ConstValue::Float(0.0),
            },
            VarDef {
                keep_across_form_switch: false,
                name: "powered".to_string(),
                kind: VarType::Flag,
                initial: ConstValue::Flag(false),
            },
        ],
        transitions: vec![
            Transition {
                from: "idle".to_string(),
                to: "full".to_string(),
                guard: Predicate::Compare {
                    op: CmpOp::Gte,
                    left: Expr::Var("gauge".to_string()),
                    right: Expr::Const(ConstValue::Float(100.0)),
                },
                on_frame: true,
                actions: vec![
                    Action::Raise {
                        var: "powered".to_string(),
                    },
                    Action::Reset {
                        var: "gauge".to_string(),
                    },
                ],
            },
            Transition {
                from: "full".to_string(),
                to: "idle".to_string(),
                guard: Predicate::Flag("powered".to_string()),
                on_frame: true,
                actions: vec![Action::Clear {
                    var: "powered".to_string(),
                }],
            },
        ],
        emission_plan: Default::default(),
    }
}

pub(crate) fn charge_plan() -> EmissionPlan {
    EmissionPlan {
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
        resource_triggers: Vec::new(),
        event_bindings: Vec::new(),
        status_triggers: Vec::new(),
    }
}
