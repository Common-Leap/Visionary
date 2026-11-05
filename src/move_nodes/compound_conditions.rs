//! Closed boolean compositions of reviewed native move-condition leaves.
//!
//! Expressions are admitted only when every leaf is one of the existing atomic
//! getters and canonical re-emission exactly matches the imported header.

use crate::move_nodes::model::MoveCondition;

pub const MAX_DEPTH: usize = 8;
pub const MAX_NODES: usize = 255;
pub const MAX_HEADER_BYTES: usize = 4096;

const MAX_PAREN_DEPTH: usize = MAX_DEPTH + 2;

const PREC_OR: u8 = 1;
const PREC_AND: u8 = 2;
const PREC_COMPARISON: u8 = 3;
const PREC_NOT: u8 = 4;
const PREC_ATOM: u8 = 5;

#[derive(Default)]
struct Budget {
    nodes: usize,
}

impl Budget {
    fn visit(&mut self, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err(format!(
                "native condition depth exceeds the supported limit of {MAX_DEPTH}"
            ));
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(format!(
                "native condition node count exceeds the supported limit of {MAX_NODES}"
            ));
        }
        Ok(())
    }
}

/// Emit a validated condition expression inside a canonical `if` header.
pub fn header(fighter: &str, condition: &MoveCondition) -> Result<String, String> {
    let mut budget = Budget::default();
    let expression = emit_expression(fighter, condition, 0, 0, &mut budget)?;
    let header = format!("if {expression} {{");
    if header.len() > MAX_HEADER_BYTES {
        return Err(format!(
            "native condition header exceeds the supported limit of {MAX_HEADER_BYTES} bytes"
        ));
    }
    Ok(header)
}

/// Check only the serialized tree's structural budget, without validating
/// ownership or normalizing any leaf. This is safe for keeping opaque imported
/// conditions visible while refusing to recurse into pathological trees.
pub fn within_limits(condition: &MoveCondition) -> bool {
    let mut pending = vec![(condition, 0usize)];
    let mut nodes = 0usize;
    while let Some((condition, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            return false;
        }
        nodes += 1;
        if nodes > MAX_NODES {
            return false;
        }
        match condition {
            MoveCondition::Not(inner) | MoveCondition::Group(inner) => {
                pending.push((inner, depth + 1));
            }
            MoveCondition::And { left, right } | MoveCondition::Or { left, right } => {
                pending.push((left, depth + 1));
                pending.push((right, depth + 1));
            }
            _ => {}
        }
    }
    true
}

/// Import only bounded, canonical boolean compositions of existing atomic
/// conditions. Unknown leaves leave the complete source header opaque.
pub fn import(fighter: &str, source: &str) -> Option<MoveCondition> {
    if source.len() > MAX_HEADER_BYTES {
        return None;
    }
    let source = source.trim();
    let expression_source = source.strip_prefix("if ")?.strip_suffix(" {")?;
    precheck(expression_source)?;

    if let Some(condition) = super::timeline::import_atomic_condition(fighter, source) {
        return Some(condition);
    }

    let expression = syn::parse_str::<syn::Expr>(expression_source).ok()?;
    let mut budget = Budget::default();
    let condition =
        parse_expression(fighter, expression_source, &expression, 0, &mut budget).ok()?;
    (header(fighter, &condition).ok()?.as_str() == source).then_some(condition)
}

fn precheck(expression: &str) -> Option<()> {
    if expression.is_empty() || expression.len() > MAX_HEADER_BYTES {
        return None;
    }
    let bytes = expression.as_bytes();
    let (mut parens, mut boolean_tokens, mut comparison_tokens, mut generic_depth, mut index) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                // Controller and variable IDs are literal strings. Ignore their
                // contents when counting expression tokens; the closed leaf
                // parser and canonical re-emission validate the actual call.
                index += 1;
                loop {
                    match bytes.get(index)? {
                        b'"' => break,
                        b'\\' => index += 1,
                        b'\n' | b'\r' => return None,
                        _ => {}
                    }
                    index += 1;
                }
            }
            b'(' => {
                parens += 1;
                if parens > MAX_PAREN_DEPTH {
                    return None;
                }
            }
            b')' => {
                parens = parens.checked_sub(1)?;
            }
            b'&' if bytes.get(index + 1) == Some(&b'&') => {
                boolean_tokens += 1;
                index += 1;
            }
            b'|' if bytes.get(index + 1) == Some(&b'|') => {
                boolean_tokens += 1;
                index += 1;
            }
            b'!' if bytes.get(index + 1) == Some(&b'=') => {
                comparison_tokens += 1;
                index += 1;
            }
            b'!' => boolean_tokens += 1,
            b'=' if bytes.get(index + 1) == Some(&b'=') => {
                comparison_tokens += 1;
                index += 1;
            }
            b'<' if bytes.get(index + 1) == Some(&b'=') => {
                comparison_tokens += 1;
                index += 1;
            }
            b'>' if bytes.get(index + 1) == Some(&b'=') => {
                comparison_tokens += 1;
                index += 1;
            }
            b'<' => {
                comparison_tokens += 1;
                if generic_depth > 0 || (index >= 2 && &bytes[index - 2..index] == b"::") {
                    generic_depth += 1;
                    if generic_depth > MAX_DEPTH {
                        return None;
                    }
                }
            }
            b'>' => {
                comparison_tokens += 1;
                generic_depth = generic_depth.saturating_sub(1);
            }
            b'-' => {
                // The reviewed leaf grammar permits a minus only as part of a
                // signed numeric RHS, never as an arithmetic operator.
                let mut previous = index;
                while previous > 0 && bytes[previous - 1].is_ascii_whitespace() {
                    previous -= 1;
                }
                let mut next = index + 1;
                while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                    next += 1;
                }
                let after_comparison = previous >= 2
                    && (&bytes[previous - 2..previous] == b"=="
                        || &bytes[previous - 2..previous] == b"!="
                        || &bytes[previous - 2..previous] == b"<="
                        || &bytes[previous - 2..previous] == b">=")
                    || previous >= 1 && matches!(bytes[previous - 1], b'<' | b'>');
                if !after_comparison || next >= bytes.len() || !bytes[next].is_ascii_digit() {
                    return None;
                }
            }
            b'\n' | b'\r' => return None,
            b' ' | b'\t' | b'_' | b'.' | b',' | b':' | b'*' => {}
            byte if byte.is_ascii_alphanumeric() => {}
            // Braces, brackets, comments, arithmetic, assignment, and
            // every other token are outside the fixed getter-expression form.
            _ => return None,
        }
        if boolean_tokens > MAX_NODES || comparison_tokens > MAX_NODES {
            return None;
        }
        index += 1;
    }
    (parens == 0).then_some(())
}

fn parse_expression(
    fighter: &str,
    source: &str,
    expression: &syn::Expr,
    depth: usize,
    budget: &mut Budget,
) -> Result<MoveCondition, String> {
    budget.visit(depth)?;
    match expression {
        syn::Expr::Paren(paren) => Ok(MoveCondition::Group(Box::new(parse_expression(
            fighter,
            source,
            &paren.expr,
            depth + 1,
            budget,
        )?))),
        syn::Expr::Unary(unary) if matches!(&unary.op, syn::UnOp::Not(_)) => {
            Ok(MoveCondition::Not(Box::new(parse_expression(
                fighter,
                source,
                &unary.expr,
                depth + 1,
                budget,
            )?)))
        }
        syn::Expr::Binary(binary) if matches!(&binary.op, syn::BinOp::And(_)) => {
            Ok(MoveCondition::And {
                left: Box::new(parse_expression(
                    fighter,
                    source,
                    &binary.left,
                    depth + 1,
                    budget,
                )?),
                right: Box::new(parse_expression(
                    fighter,
                    source,
                    &binary.right,
                    depth + 1,
                    budget,
                )?),
            })
        }
        syn::Expr::Binary(binary) if matches!(&binary.op, syn::BinOp::Or(_)) => {
            Ok(MoveCondition::Or {
                left: Box::new(parse_expression(
                    fighter,
                    source,
                    &binary.left,
                    depth + 1,
                    budget,
                )?),
                right: Box::new(parse_expression(
                    fighter,
                    source,
                    &binary.right,
                    depth + 1,
                    budget,
                )?),
            })
        }
        _ => {
            let leaf_source = source_fragment(source, expression)?;
            let header = format!("if {leaf_source} {{");
            super::timeline::import_atomic_condition(fighter, &header).ok_or_else(|| {
                format!("unsupported or foreign native condition leaf {leaf_source:?}")
            })
        }
    }
}

fn source_fragment<'a>(source: &'a str, expression: &syn::Expr) -> Result<&'a str, String> {
    use syn::spanned::Spanned;

    let span = expression.span();
    let start = span.start();
    let end = span.end();
    if start.line != 1 || end.line != 1 || start.column > end.column || end.column > source.len() {
        return Err("native condition leaf has an invalid source span".into());
    }
    // Span columns count characters. Controller names may contain UTF-8,
    // while slicing source requires byte offsets.
    let byte_offset = |column| {
        source
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(source.len()))
            .nth(column)
            .ok_or_else(|| "native condition leaf has an invalid source span".to_owned())
    };
    source
        .get(byte_offset(start.column)?..byte_offset(end.column)?)
        .ok_or_else(|| "native condition leaf is not on a valid source boundary".into())
}

fn emit_expression(
    fighter: &str,
    condition: &MoveCondition,
    parent_precedence: u8,
    depth: usize,
    budget: &mut Budget,
) -> Result<String, String> {
    budget.visit(depth)?;
    let (expression, precedence) = match condition {
        MoveCondition::Not(inner) => (
            format!(
                "!{}",
                emit_expression(fighter, inner, PREC_NOT, depth + 1, budget)?
            ),
            PREC_NOT,
        ),
        MoveCondition::And { left, right } => (
            format!(
                "{} && {}",
                emit_expression(fighter, left, PREC_AND, depth + 1, budget)?,
                emit_expression(fighter, right, PREC_AND, depth + 1, budget)?
            ),
            PREC_AND,
        ),
        MoveCondition::Or { left, right } => (
            format!(
                "{} || {}",
                emit_expression(fighter, left, PREC_OR, depth + 1, budget)?,
                emit_expression(fighter, right, PREC_OR, depth + 1, budget)?
            ),
            PREC_OR,
        ),
        MoveCondition::Group(inner) => (
            format!(
                "({})",
                emit_expression(fighter, inner, 0, depth + 1, budget)?
            ),
            PREC_ATOM,
        ),
        _ => {
            let precedence = if matches!(
                condition,
                MoveCondition::Grounded
                    | MoveCondition::Airborne
                    | MoveCondition::NativeLrNegative
                    | MoveCondition::WorkInt { .. }
                    | MoveCondition::WorkFloat { .. }
                    | MoveCondition::CharacterValue { .. }
            ) {
                PREC_COMPARISON
            } else {
                PREC_ATOM
            };
            (
                super::timeline::atomic_condition_expression(fighter, condition)?,
                precedence,
            )
        }
    };
    Ok(if precedence < parent_precedence {
        format!("({expression})")
    } else {
        expression
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::move_nodes::model::{NativeComparison, NativeIntOperand};

    const BAYONETTA_COSTUME: &str = "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND";

    fn flag(expected: bool) -> MoveCondition {
        MoveCondition::WorkFlag {
            symbol: "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D".into(),
            expected,
        }
    }

    fn costume(choice: &str) -> MoveCondition {
        MoveCondition::WorkInt {
            symbol: BAYONETTA_COSTUME.into(),
            op: NativeComparison::Eq,
            value: NativeIntOperand::Choice(choice.into()),
        }
    }

    #[test]
    fn canonical_boolean_trees_round_trip_and_keep_operator_precedence() {
        let bayonetta_one = "FIGHTER_BAYONETTA_COSTUME_KIND_BAYONETTA_1";
        let cases = [
            (
                flag(false),
                "if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) {",
            ),
            (
                MoveCondition::And {
                    left: Box::new(MoveCondition::Grounded),
                    right: Box::new(costume(bayonetta_one)),
                },
                "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND && WorkModule::get_int(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND) == *FIGHTER_BAYONETTA_COSTUME_KIND_BAYONETTA_1 {",
            ),
            (
                MoveCondition::Or {
                    left: Box::new(MoveCondition::ButtonHeld {
                        button: crate::move_nodes::model::InputButton::Attack,
                    }),
                    right: Box::new(MoveCondition::Not(Box::new(flag(true)))),
                },
                "if ControlModule::check_button_on(agent.module_accessor, *CONTROL_PAD_BUTTON_ATTACK) || !WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) {",
            ),
            (
                MoveCondition::And {
                    left: Box::new(MoveCondition::Grounded),
                    right: Box::new(MoveCondition::Group(Box::new(MoveCondition::Or {
                        left: Box::new(MoveCondition::Airborne),
                        right: Box::new(costume(bayonetta_one)),
                    }))),
                },
                "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND && (StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_AIR || WorkModule::get_int(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND) == *FIGHTER_BAYONETTA_COSTUME_KIND_BAYONETTA_1) {",
            ),
            (
                MoveCondition::Group(Box::new(MoveCondition::Or {
                    left: Box::new(MoveCondition::And {
                        left: Box::new(MoveCondition::Grounded),
                        right: Box::new(flag(true)),
                    }),
                    right: Box::new(MoveCondition::Airborne),
                })),
                "if (StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND && WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) || StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_AIR) {",
            ),
            (
                MoveCondition::Not(Box::new(MoveCondition::Group(Box::new(
                    MoveCondition::And {
                        left: Box::new(MoveCondition::Grounded),
                        right: Box::new(flag(true)),
                    },
                )))),
                "if !(StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND && WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D)) {",
            ),
            (
                MoveCondition::Not(Box::new(MoveCondition::Group(Box::new(
                    MoveCondition::Grounded,
                )))),
                "if !(StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND) {",
            ),
            (
                MoveCondition::Not(Box::new(MoveCondition::Group(Box::new(costume(
                    bayonetta_one,
                ))))),
                "if !(WorkModule::get_int(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND) == *FIGHTER_BAYONETTA_COSTUME_KIND_BAYONETTA_1) {",
            ),
        ];

        for (condition, source) in cases {
            let emitted = header("bayonetta", &condition).unwrap();
            assert_eq!(emitted, source);
            let imported = import("bayonetta", source).unwrap();
            assert_eq!(
                serde_json::to_value(&imported).unwrap(),
                serde_json::to_value(&condition).unwrap()
            );
            assert_eq!(header("bayonetta", &imported).unwrap(), source);
        }
    }

    #[test]
    fn article_exists_conditions_compose_and_reimport_for_the_bound_owner() {
        let article = || MoveCondition::ArticleExists {
            symbol: "FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP".into(),
        };
        let cases = [
            MoveCondition::Not(Box::new(article())),
            MoveCondition::And {
                left: Box::new(MoveCondition::Grounded),
                right: Box::new(article()),
            },
            MoveCondition::Or {
                left: Box::new(article()),
                right: Box::new(MoveCondition::Airborne),
            },
            MoveCondition::Group(Box::new(MoveCondition::Or {
                left: Box::new(article()),
                right: Box::new(MoveCondition::ButtonHeld {
                    button: crate::move_nodes::model::InputButton::Special,
                }),
            })),
        ];

        for condition in cases {
            let source = header("sonic", &condition).unwrap();
            let imported = import("sonic", &source).unwrap();
            assert_eq!(
                serde_json::to_value(&imported).unwrap(),
                serde_json::to_value(&condition).unwrap()
            );
            assert_eq!(header("sonic", &imported).unwrap(), source);
            assert!(import("mario", &source).is_none());
        }
    }

    #[test]
    fn mixed_tree_precedence_and_boolean_truth_tables_are_structural() {
        let a = MoveCondition::Grounded;
        let b = MoveCondition::Airborne;
        let c = flag(true);
        let condition = MoveCondition::Or {
            left: Box::new(a.clone()),
            right: Box::new(MoveCondition::And {
                left: Box::new(b.clone()),
                right: Box::new(c.clone()),
            }),
        };
        let source = "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND || StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_AIR && WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) {";
        let imported = import("bayonetta", source).unwrap();
        assert!(matches!(
            &imported,
            MoveCondition::Or { left, right }
                if matches!(left.as_ref(), MoveCondition::Grounded)
                    && matches!(right.as_ref(), MoveCondition::And { .. })
        ));
        for av in [false, true] {
            for bv in [false, true] {
                for cv in [false, true] {
                    let actual = evaluate(&imported, [av, bv, cv]);
                    assert_eq!(actual, av || (bv && cv));
                }
            }
        }
        assert_eq!(header("bayonetta", &condition).unwrap(), source);
    }

    #[test]
    fn negated_numeric_getter_wraps_the_comparison_not_the_value() {
        let condition = MoveCondition::Not(Box::new(MoveCondition::Group(Box::new(
            MoveCondition::WorkFloat {
                symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                op: NativeComparison::Gte,
                value: 1.5,
            },
        ))));
        let source = "if !(WorkModule::get_float(agent.module_accessor, *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE) >= 1.5) {";
        assert_eq!(header("cloud", &condition).unwrap(), source);
        let imported = import("cloud", source).unwrap();
        assert_eq!(
            serde_json::to_value(&imported).unwrap(),
            serde_json::to_value(&condition).unwrap()
        );
        assert_eq!(header("cloud", &imported).unwrap(), source);
    }

    #[test]
    fn negative_numeric_leaves_are_admitted_inside_compounds() {
        let condition = MoveCondition::And {
            left: Box::new(MoveCondition::Grounded),
            right: Box::new(MoveCondition::WorkFloat {
                symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                op: NativeComparison::Gt,
                value: -1.5,
            }),
        };
        let source = "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND && WorkModule::get_float(agent.module_accessor, *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE) > -1.5 {";
        assert_eq!(header("cloud", &condition).unwrap(), source);
        let imported = import("cloud", source).unwrap();
        assert_eq!(
            serde_json::to_value(&imported).unwrap(),
            serde_json::to_value(&condition).unwrap()
        );
    }

    #[test]
    fn motion_changing_is_a_closed_leaf_in_boolean_compositions() {
        let motion = MoveCondition::MotionChanging;
        let conditions = [
            motion.clone(),
            MoveCondition::Not(Box::new(motion.clone())),
            MoveCondition::And {
                left: Box::new(MoveCondition::Grounded),
                right: Box::new(motion.clone()),
            },
            MoveCondition::Or {
                left: Box::new(motion.clone()),
                right: Box::new(MoveCondition::Airborne),
            },
            MoveCondition::Group(Box::new(MoveCondition::Not(Box::new(motion)))),
        ];
        for condition in conditions {
            let header = header("eflame", &condition).unwrap();
            assert!(header.contains("MotionModule::is_changing(agent.module_accessor)"));
            assert_eq!(
                serde_json::to_value(import("eflame", &header).unwrap()).unwrap(),
                serde_json::to_value(condition).unwrap(),
                "boolean tree did not round-trip: {header}"
            );
        }
        for unsupported in [
            "if MotionModule::is_changing(boma) {",
            "if MotionModule::is_changing(other.module_accessor) {",
            "if MotionModule::is_changing(agent.module_accessor, true) {",
            "if wrapper(MotionModule::is_changing(agent.module_accessor)) {",
            "if MotionModule::is_changing(agent.module_accessor) /* injected */ {",
            "if MotionModule::is_changing(agent.module_accessor) && true {",
        ] {
            assert!(import("eflame", unsupported).is_none(), "{unsupported}");
        }
    }

    fn evaluate(condition: &MoveCondition, leaves: [bool; 3]) -> bool {
        match condition {
            MoveCondition::Grounded => leaves[0],
            MoveCondition::Airborne => leaves[1],
            MoveCondition::WorkFlag { .. } => leaves[2],
            MoveCondition::AttackInfliction { .. } => {
                panic!("infliction leaves are not used by this boolean test helper")
            }
            MoveCondition::Not(inner) => !evaluate(inner, leaves),
            MoveCondition::And { left, right } => evaluate(left, leaves) && evaluate(right, leaves),
            MoveCondition::Or { left, right } => evaluate(left, leaves) || evaluate(right, leaves),
            MoveCondition::Group(inner) => evaluate(inner, leaves),
            _ => panic!("test evaluator received a non-boolean leaf"),
        }
    }

    #[test]
    fn unsafe_noncanonical_or_foreign_compound_headers_are_rejected_whole() {
        for source in [
            "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND&&WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) {",
            "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND && WorkModule::is_flag(agent.module_accessor, *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK) {",
            "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND || arbitrary(agent) {",
            "if WorkModule::get_float(agent.module_accessor, *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE) > 1.0 && WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) {",
            "if WorkModule::get_int(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_INT_COLOR) == *FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT && true {",
            "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND && WorkModule::get_int(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND) == *FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT + injected() {",
            "if !unsafe { WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) } {",
        ] {
            assert!(import("bayonetta", source).is_none(), "{source}");
        }
        assert!(import("ike", "if WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) && StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {").is_none());
    }

    #[test]
    fn boolean_expression_budgets_reject_over_depth_nodes_and_bytes() {
        let leaf = "StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND";
        let too_deep = format!(
            "if {}{leaf}{} {{",
            "(".repeat(MAX_DEPTH + 1),
            ")".repeat(MAX_DEPTH + 1)
        );
        assert!(import("mario", &too_deep).is_none());
        let too_many_tokens = format!(
            "if {} {{",
            std::iter::repeat_n(leaf, MAX_NODES + 2)
                .collect::<Vec<_>>()
                .join(" && ")
        );
        assert!(import("mario", &too_many_tokens).is_none());
        let too_long = format!("if {} {{", " ".repeat(MAX_HEADER_BYTES));
        assert!(import("mario", &too_long).is_none());
        let padded_too_long = format!("{}if {leaf} {{", " ".repeat(MAX_HEADER_BYTES));
        assert!(import("mario", &padded_too_long).is_none());

        let nested = (0..=MAX_DEPTH).fold(leaf.to_owned(), |expr, _| format!("({expr})"));
        let too_deep = format!("if {nested} {{");
        assert!(import("mario", &too_deep).is_none());
    }

    #[test]
    fn precheck_rejects_excess_comparisons_and_non_getter_tokens() {
        let chain = format!(
            "if {} {{",
            std::iter::repeat_n("value <", MAX_NODES + 1)
                .collect::<Vec<_>>()
                .join("")
                + " 0"
        );
        assert!(import("mario", &chain).is_none());

        let leaf = "StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND";
        for suffix in [
            " + 1",
            " / 2",
            " [0]",
            " { nested }",
            " // comment",
            " /* comment */",
            " \"quoted\"",
            "::<Vec<Vec<Vec<Vec<Vec<Vec<Vec<Vec<Vec<T>>>>>>>>>",
        ] {
            let source = format!("if {leaf}{suffix} {{");
            assert!(import("mario", &source).is_none(), "{suffix}");
        }
    }

    #[test]
    fn authored_output_and_structural_only_guard_share_the_same_limits() {
        fn balanced(leaves: usize) -> MoveCondition {
            if leaves == 1 {
                MoveCondition::Grounded
            } else {
                let left_count = leaves / 2;
                MoveCondition::And {
                    left: Box::new(balanced(left_count)),
                    right: Box::new(balanced(leaves - left_count)),
                }
            }
        }

        let too_large = balanced(127);
        assert!(within_limits(&too_large));
        assert!(header("mario", &too_large).is_err());
        assert!(!within_limits(&balanced(129)));

        let too_deep = (0..=MAX_DEPTH).fold(MoveCondition::Grounded, |inner, _| {
            MoveCondition::Group(Box::new(inner))
        });
        assert!(!within_limits(&too_deep));

        let mut too_many = MoveCondition::Grounded;
        for _ in 0..MAX_NODES / 2 {
            too_many = MoveCondition::And {
                left: Box::new(too_many),
                right: Box::new(MoveCondition::Grounded),
            };
        }
        assert!(!within_limits(&too_many));
    }
}
