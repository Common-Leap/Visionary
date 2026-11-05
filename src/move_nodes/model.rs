use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::data::{AcmdStmt, ExcuteStmt};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Track {
    #[default]
    Game,
    Sound,
    Expression,
    Effect,
}

/// Structured sequencing deliberately excludes unbounded loops and arbitrary Rust.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveNode {
    pub id: u32,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum NodeKind {
    Frame {
        frame: f32,
    },
    /// Exact native sv_animcmd::execute timing. Direct root source export only.
    NativeExecuteFrame {
        frame: f32,
    },
    /// Exact native motion-loop synchronization. Root presentation tracks, source export only.
    NativeMotionLoopSync,
    /// Native `wait_loop_clear` as written at the root of a dumped script. Root Game and
    /// Expression placement, source export only.
    NativeWaitLoopClear,
    Wait {
        frames: f32,
    },
    /// Animation playback rate changed at this point in the timeline.
    MotionRate {
        rate: f32,
    },
    /// A typed ACMD command written outside an `is_excute` block.
    ///
    /// Only source-measured bare command families are accepted by the timeline compiler.
    BareCommand {
        command: Box<ExcuteStmt>,
    },
    Execute {
        commands: Vec<ExcuteStmt>,
    },
    /// An imported `is_excute` block with editable typed commands interleaved with exact
    /// source commands that remain immutable.
    MixedExecute {
        commands: Vec<MoveCommand>,
    },
    EffectExecute {
        commands: Vec<crate::data::EffectMacro>,
    },
    /// An imported effect execute wrapper with typed calls interleaved with exact,
    /// immutable source calls. `source_commands` is checked against the track baseline.
    MixedEffectExecute {
        commands: Vec<EffectCommandSlot>,
        source_commands: Vec<crate::data::EffectMacro>,
    },
    Repeat {
        count: u32,
        body: Vec<MoveNode>,
        /// Repeat every motion loop (native `loop { body; wait }`) instead of `count` times.
        /// `count` must be 1. Root placement, last in its track.
        #[serde(default, skip_serializing_if = "crate::data::MotionLoop::is_no")]
        motion_loop: crate::data::MotionLoop,
    },
    Branch {
        condition: MoveCondition,
        then_nodes: Vec<MoveNode>,
        #[serde(default)]
        else_nodes: Vec<MoveNode>,
    },
    /// Immutable imported structure; authoring validation proves it against the baseline.
    Preserved {
        statement: AcmdStmt,
    },
}

/// One command inside a partially editable native execute block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum MoveCommand {
    Editable(ExcuteStmt),
    Preserved(ExcuteStmt),
}

/// One command inside a partially editable native effect execute block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum EffectCommandSlot {
    Editable(crate::data::EffectMacro),
    Preserved(crate::data::EffectMacro),
}

/// Closed query windows accepted for source-only AttackModule infliction tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum InflictionQuery {
    CurrentHitlag,
    CurrentStatus,
}

/// Collision result masks admitted by the source-only infliction condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum InflictionMask {
    Hit,
    Shield,
}

/// Literal compared with a character controller variable at an ACMD branch.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CharacterValue {
    Int(i32),
    Float(f32),
    Flag(bool),
}

/// Read-only native and controller conditions; no raw source expressions or numeric IDs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum MoveCondition {
    Stick {
        axis: StickAxis,
        op: NativeComparison,
        value: f32,
        #[serde(default)]
        relative_to_facing: bool,
    },
    /// The native ACMD execution predicate, evaluated at this exact source location.
    IsExecute,
    Grounded,
    Airborne,
    /// Exact native `MotionModule::is_changing(agent.module_accessor)` predicate.
    MotionChanging,
    /// Source-only `get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0` test.
    /// This preserves the authored comparison without assigning left/right semantics or
    /// exposing a general native getter, symbol, operator, or threshold.
    NativeLrNegative,
    /// Source-only `ItemModule::is_have_item(agent.module_accessor, 0)` read.
    /// The measured slot is fixed; this does not select or mutate a live item.
    ItemHeld,
    /// `StatusModule::status_kind(..) == *SYMBOL` for one registered status of this fighter.
    InStatus {
        symbol: String,
    },
    /// `ItemModule::get_have_item_kind(.., 0) == *ITEM_KIND_*` for one registered item kind.
    HoldingItemKind {
        item: String,
    },
    /// Read-only existence check for one exact article registered to this fighter.
    ArticleExists {
        symbol: String,
    },
    WorkFlag {
        symbol: String,
        expected: bool,
    },
    ButtonHeld {
        button: InputButton,
    },
    /// Source-only boolean check at the ACMD branch location, not a collision event/input.
    AttackInfliction {
        query: InflictionQuery,
        mask: InflictionMask,
    },
    WorkInt {
        symbol: String,
        op: NativeComparison,
        value: NativeIntOperand,
    },
    WorkFloat {
        symbol: String,
        op: NativeComparison,
        value: f32,
    },
    CharacterValue {
        chart_id: String,
        var: String,
        op: NativeComparison,
        value: CharacterValue,
    },
    Not(Box<MoveCondition>),
    And {
        left: Box<MoveCondition>,
        right: Box<MoveCondition>,
    },
    Or {
        left: Box<MoveCondition>,
        right: Box<MoveCondition>,
    },
    Group(Box<MoveCondition>),
    /// An imported `if`/`else` header no typed condition reads — a dumped address call such
    /// as `if(0x1cb400(-460198162, 1)){`, a lone `else {` — kept byte-exact so the blocks
    /// under it stay typed and editable. Never authored: the compiler accepts it only against
    /// an import-time baseline witness, so the header is always the source's own text.
    Source {
        header: String,
    },
}

#[cfg(test)]
mod native_lr_condition_serde_tests {
    use super::MoveCondition;

    #[test]
    fn native_lr_negative_is_a_closed_fieldless_variant_and_old_conditions_still_load() {
        let condition = MoveCondition::NativeLrNegative;
        let encoded = serde_json::to_value(&condition).unwrap();
        assert_eq!(encoded, serde_json::json!("native_lr_negative"));
        let decoded: MoveCondition = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);

        assert!(serde_json::from_value::<MoveCondition>(serde_json::json!({
            "native_lr_negative": { "symbol": "SO_VAR_FLOAT_LR", "threshold": 0.0 }
        }))
        .is_err());
        let legacy: MoveCondition = serde_json::from_value(serde_json::json!("grounded")).unwrap();
        assert_eq!(
            serde_json::to_value(legacy).unwrap(),
            serde_json::json!("grounded")
        );
    }
}

#[cfg(test)]
mod attack_infliction_serde_tests {
    use super::{InflictionMask, InflictionQuery, MoveCondition};

    #[test]
    fn attack_infliction_uses_closed_enums_and_preserves_legacy_conditions() {
        let condition = MoveCondition::AttackInfliction {
            query: InflictionQuery::CurrentStatus,
            mask: InflictionMask::Shield,
        };
        let encoded = serde_json::to_value(&condition).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({
                "attack_infliction": {
                    "query": "current_status",
                    "mask": "shield"
                }
            })
        );
        let decoded: MoveCondition = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);

        for invalid in [
            serde_json::json!({"attack_infliction": {"query": "current_status", "mask": 4}}),
            serde_json::json!({"attack_infliction": {"query": "current_status", "mask": "hs"}}),
            serde_json::json!({"attack_infliction": {"query": "all", "mask": "hit"}}),
            serde_json::json!({"attack_infliction": {"query": "current_status", "mask": "shield", "extra": true}}),
        ] {
            assert!(serde_json::from_value::<MoveCondition>(invalid).is_err());
        }

        let legacy: MoveCondition = serde_json::from_value(serde_json::json!("grounded")).unwrap();
        assert_eq!(
            serde_json::to_value(legacy).unwrap(),
            serde_json::json!("grounded")
        );
    }
}

/// Closed source-level comparison operators, not an inferred native event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeComparison {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
}

impl NativeComparison {
    pub const ALL: [Self; 6] = [Self::Eq, Self::Ne, Self::Lt, Self::Lte, Self::Gt, Self::Gte];

    pub fn token(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Lte => "<=",
            Self::Gt => ">",
            Self::Gte => ">=",
        }
    }
}

/// A literal integer, an exact pinned fighter-kind symbol, or a closed
/// source-specific native enum symbol. This is not a battle-object ID,
/// writable Work slot, or source-code expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeIntOperand {
    Literal(i32),
    FighterKind(String),
    Choice(String),
}

pub use crate::gimmicks::native_inputs::InputButton;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveTrack {
    pub track: Track,
    #[serde(default)]
    pub nodes: Vec<MoveNode>,
    /// Original statements that may be preserved but not replaced with authored source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baseline: Vec<AcmdStmt>,
    /// Typed source-command vectors for mixed effect wrappers, retained independently from
    /// the mutable node slots so opaque source cannot be promoted by editing both slot and node
    /// witness. Each imported mixed wrapper consumes exactly one matching entry at compile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effect_execute_baseline: Vec<Vec<crate::data::EffectMacro>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveGraph {
    pub version: u32,
    #[serde(default)]
    pub tracks: Vec<MoveTrack>,
    /// Editor-only positions keyed by node ID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub positions: BTreeMap<String, [f32; 2]>,
}
impl Default for MoveGraph {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            tracks: Vec::new(),
            positions: BTreeMap::new(),
        }
    }
}
impl MoveGraph {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeError {
    pub subject: String,
    pub reason: String,
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.subject, self.reason)
    }
}

impl std::error::Error for NodeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StickAxis {
    X,
    Y,
}

#[cfg(test)]
mod shape_tests {
    use super::*;
    #[test]
    fn move_variants_reject_unknown_payload_fields() {
        assert!(
            serde_json::from_str::<NodeKind>(r#"{"frame":{"frame":2.0,"extra":true}}"#).is_err()
        );
        assert!(serde_json::from_str::<MoveCondition>(
            r#"{"work_flag":{"symbol":"FIGHTER_FLAG","expected":true,"id":7}}"#
        )
        .is_err());
        assert!(serde_json::from_str::<MoveGraph>(r#"{"version":1,"triggers":[]}"#).is_err());
    }
}
