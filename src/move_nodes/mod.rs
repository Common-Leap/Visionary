//! Typed move authoring, lowered into the existing ACMD and gimmick models.
pub mod block_clipboard;
pub mod canvas;
mod character_values;
mod chart_event_view;
pub mod compound_conditions;
pub mod coverage;
pub mod effects;
pub mod flow;
pub mod live_steps;
pub mod live_support;
pub mod model;
pub mod native_calls;
pub mod native_conditions;
pub mod native_status_flags;
pub mod native_work;
pub mod plain;
pub mod project;
pub mod timeline;
pub mod trails;
pub mod view;

#[cfg(test)]
mod export_tests;

#[cfg(test)]
pub(crate) mod test_support {
    macro_rules! assert_json_eq {
        ($left:expr, $right:expr $(, $message:expr)* $(,)?) => {
            assert_eq!(
                serde_json::to_value($left).unwrap(),
                serde_json::to_value($right).unwrap()
                $(, $message)*
            )
        };
    }
    pub(crate) use assert_json_eq;

    use super::model::{MoveNode, MoveTrack, NodeKind, Track};
    use crate::data::{EffectMacro, ExcuteStmt};
    use crate::gimmicks::behavior::{ConstValue, VarDef, VarType};

    pub(crate) fn node(id: u32, kind: NodeKind) -> MoveNode {
        MoveNode { id, kind }
    }
    pub(crate) fn execute(id: u32, commands: Vec<ExcuteStmt>) -> MoveNode {
        node(id, NodeKind::Execute { commands })
    }
    pub(crate) fn effect_execute(id: u32, commands: Vec<EffectMacro>) -> MoveNode {
        node(id, NodeKind::EffectExecute { commands })
    }
    pub(crate) fn frame(id: u32, frame: f32) -> MoveNode {
        node(id, NodeKind::Frame { frame })
    }
    pub(crate) fn wait(id: u32, frames: f32) -> MoveNode {
        node(id, NodeKind::Wait { frames })
    }
    /// A condition the importer did not type: a whole preserved block, or a branch whose
    /// header is kept verbatim with typed blocks under it.
    pub(crate) fn untyped_condition(kind: &NodeKind) -> bool {
        matches!(
            kind,
            NodeKind::Preserved { .. }
                | NodeKind::Branch {
                    condition: super::model::MoveCondition::Source { .. },
                    ..
                }
        )
    }
    pub(crate) fn track(track: Track, nodes: Vec<MoveNode>) -> MoveTrack {
        MoveTrack {
            track,
            nodes,
            baseline: Vec::new(),
            effect_execute_baseline: Vec::new(),
        }
    }
    pub(crate) fn var(name: String, kind: VarType, initial: ConstValue) -> VarDef {
        VarDef {
            keep_across_form_switch: false,
            name,
            kind,
            initial,
        }
    }

    pub(crate) fn set(
        var: String,
        value: crate::gimmicks::behavior::Expr,
    ) -> crate::gimmicks::behavior::Action {
        crate::gimmicks::behavior::Action::Set { var, value }
    }
    pub(crate) fn controller_event(
        name: &str,
        actions: Vec<crate::gimmicks::behavior::Action>,
    ) -> crate::gimmicks::behavior::BehaviorGraph {
        use crate::gimmicks::{behavior::*, codegen::*};
        BehaviorGraph {
            initial: "active".into(),
            states: vec![StateDef {
                status: None,
                name: "active".into(),
            }],
            transitions: vec![Transition {
                from: "active".into(),
                to: "active".into(),
                guard: crate::gimmicks::graph_view::always(),
                on_frame: true,
                actions,
            }],
            emission_plan: EmissionPlan {
                event_bindings: vec![EventBinding {
                    name: name.into(),
                    transitions: vec![0],
                    fields: vec![],
                }],
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod execute_branches_tests;

#[cfg(test)]
mod native_scope_tests;
