//! Bounded, pure validation helpers for authored named-event payload schemas.
//!
//! Values reuse behavior constants, numeric expressions and predicates; Timer
//! is not an event scalar. Source and live lowering preserve schemas and
//! canonicalize sends into receiver order without rewriting authored argument
//! order. Runtime evaluators capture computed values at the send point.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::behavior::{ConstValue, VarType};

pub const MAX_FIELDS_PER_EVENT: usize = 8;
pub const MAX_FIELD_NAME_BYTES: usize = super::live_model::MAX_NAME_BYTES;

// Schemas and payloads are the live model's types; saved projects and the
// live wire share one spelling, so lowering only reorders arguments.
pub use super::live_model::{
    EventArgument as Argument, EventField as FieldDef, EventValue, ValueArgument,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub owner: String,
    pub event: String,
    pub field: Option<String>,
    pub reason: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.field {
            Some(field) => write!(
                formatter,
                "{} event {:?} field {:?}: {}",
                self.owner, self.event, field, self.reason
            ),
            None => write!(
                formatter,
                "{} event {:?}: {}",
                self.owner, self.event, self.reason
            ),
        }
    }
}

fn bounded_label(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes.saturating_sub(3);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

fn error(
    owner: &str,
    event: &str,
    field: Option<&str>,
    reason: impl Into<String>,
) -> ValidationError {
    ValidationError {
        owner: bounded_label(owner, 96),
        event: bounded_label(event, MAX_FIELD_NAME_BYTES),
        field: field.map(|field| bounded_label(field, MAX_FIELD_NAME_BYTES)),
        reason: reason.into(),
    }
}

fn name_error(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        Some("must be non-empty")
    } else if name.len() > MAX_FIELD_NAME_BYTES {
        Some("exceeds the 48-byte UTF-8 name limit")
    } else if name.contains('\0') {
        Some("must not contain NUL")
    } else {
        None
    }
}

fn const_kind(value: ConstValue) -> VarType {
    match value {
        ConstValue::Float(_) => VarType::Float,
        ConstValue::Int(_) => VarType::Int,
        ConstValue::Flag(_) => VarType::Flag,
    }
}

fn scalar_value_error(value: ConstValue) -> Option<&'static str> {
    match value {
        ConstValue::Float(value)
            if !value.is_finite() || f64::from(value).abs() > super::live_model::MAX_ABS_NUMBER =>
        {
            Some("must be finite and within MAX_ABS_NUMBER")
        }
        ConstValue::Int(value)
            if i64::from(value).abs() > super::live_model::MAX_ABS_NUMBER as i64 =>
        {
            Some("must be within MAX_ABS_NUMBER")
        }
        ConstValue::Float(_) | ConstValue::Int(_) | ConstValue::Flag(_) => None,
    }
}

/// Validate the bounded, schema-independent portion of an authored argument
/// vector. This runs before any map allocation in behavior validation.
pub fn validate_argument_shape(
    owner: &str,
    event: &str,
    arguments: &[Argument],
) -> Result<(), Vec<ValidationError>> {
    if arguments.len() > MAX_FIELDS_PER_EVENT {
        return Err(vec![error(
            owner,
            event,
            None,
            format!(
                "received {} arguments; maximum is {MAX_FIELDS_PER_EVENT}",
                arguments.len()
            ),
        )]);
    }
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for argument in arguments {
        if let Some(reason) = name_error(&argument.field) {
            errors.push(error(owner, event, Some(&argument.field), reason));
        }
        if !seen.insert(argument.field.as_str()) {
            errors.push(error(
                owner,
                event,
                Some(&argument.field),
                "argument field is duplicated",
            ));
        }
        if let Some(reason) = scalar_value_error(argument.value) {
            errors.push(error(owner, event, Some(&argument.field), reason));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validate the bounded, schema-independent portion of dynamic arguments.
/// Expression shape and receiver-scoped references are bounded/validated by
/// the behavior graph walk; this pass validates field labels and constants.
pub fn validate_value_argument_shape(
    owner: &str,
    event: &str,
    arguments: &[ValueArgument],
) -> Result<(), Vec<ValidationError>> {
    if arguments.len() > MAX_FIELDS_PER_EVENT {
        return Err(vec![error(
            owner,
            event,
            None,
            format!(
                "received {} arguments; maximum is {MAX_FIELDS_PER_EVENT}",
                arguments.len()
            ),
        )]);
    }
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for argument in arguments {
        if let Some(reason) = name_error(&argument.field) {
            errors.push(error(owner, event, Some(&argument.field), reason));
        }
        if !seen.insert(argument.field.as_str()) {
            errors.push(error(
                owner,
                event,
                Some(&argument.field),
                "argument field is duplicated",
            ));
        }
        if let EventValue::Constant(value) = &argument.value {
            if let Some(reason) = scalar_value_error(*value) {
                errors.push(error(owner, event, Some(&argument.field), reason));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validate field count, names, uniqueness, and the existing closed scalar
/// kinds. Errors identify owner, event, and (when applicable) field.
pub fn validate_schema(
    owner: &str,
    event: &str,
    fields: &[FieldDef],
) -> Result<(), Vec<ValidationError>> {
    if fields.len() > MAX_FIELDS_PER_EVENT {
        return Err(vec![error(
            owner,
            event,
            None,
            format!(
                "has {} fields; maximum is {MAX_FIELDS_PER_EVENT}",
                fields.len()
            ),
        )]);
    }
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for field in fields {
        if let Some(reason) = name_error(&field.name) {
            errors.push(error(owner, event, Some(&field.name), reason));
        }
        if !seen.insert(field.name.as_str()) {
            errors.push(error(
                owner,
                event,
                Some(&field.name),
                "field name is duplicated",
            ));
        }
        if field.kind == VarType::Timer {
            errors.push(error(
                owner,
                event,
                Some(&field.name),
                "Timer is not a supported event field kind; use Flag, Int, or Float",
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validate named arguments and return immutable scalar values in declaration
/// order. Neither input is modified, including on any failure path.
pub fn canonicalize_arguments(
    owner: &str,
    event: &str,
    fields: &[FieldDef],
    arguments: &[Argument],
) -> Result<Vec<ConstValue>, Vec<ValidationError>> {
    // Check both collection lengths before validating either one's contents.
    // This keeps an oversized argument list from forcing a schema walk (and
    // vice versa), including when both inputs are oversized.
    let schema_oversized = fields.len() > MAX_FIELDS_PER_EVENT;
    let arguments_oversized = arguments.len() > MAX_FIELDS_PER_EVENT;
    if schema_oversized || arguments_oversized {
        let mut errors = Vec::new();
        if schema_oversized {
            errors.extend(validate_schema(owner, event, fields).unwrap_err());
        }
        if arguments_oversized {
            errors.extend(validate_argument_shape(owner, event, arguments).unwrap_err());
        }
        return Err(errors);
    }
    validate_schema(owner, event, fields)?;
    validate_argument_shape(owner, event, arguments)?;
    let mut errors = Vec::new();
    if arguments.len() != fields.len() {
        errors.push(error(
            owner,
            event,
            None,
            format!(
                "received {} arguments for {} declared fields",
                arguments.len(),
                fields.len()
            ),
        ));
    }

    let declared: BTreeMap<&str, &FieldDef> = fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect();
    let mut supplied = BTreeMap::new();
    for argument in arguments {
        supplied.insert(argument.field.as_str(), argument.value);
        let Some(field) = declared.get(argument.field.as_str()) else {
            errors.push(error(
                owner,
                event,
                Some(&argument.field),
                "argument does not exist in the event schema",
            ));
            continue;
        };
        let actual = const_kind(argument.value);
        if actual != field.kind {
            errors.push(error(
                owner,
                event,
                Some(&argument.field),
                format!("expected {:?}, received {:?}", field.kind, actual),
            ));
        }
    }
    for field in fields {
        if !supplied.contains_key(field.name.as_str()) {
            errors.push(error(
                owner,
                event,
                Some(&field.name),
                "required argument is missing",
            ));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(fields
        .iter()
        .map(|field| supplied[field.name.as_str()])
        .collect())
}

/// Validate exact dynamic argument/schema correspondence and borrow the
/// arguments in declaration order. `value_kind` uses the owning transition's
/// graph scope to typecheck Numeric and Flag expressions without evaluating
/// them. Payload expressions are intentionally not copied or evaluated here.
pub fn canonicalize_value_arguments<'a>(
    owner: &str,
    event: &str,
    fields: &[FieldDef],
    arguments: &'a [ValueArgument],
    mut value_kind: impl FnMut(&EventValue) -> Result<VarType, String>,
) -> Result<Vec<&'a ValueArgument>, Vec<ValidationError>> {
    let schema_oversized = fields.len() > MAX_FIELDS_PER_EVENT;
    let arguments_oversized = arguments.len() > MAX_FIELDS_PER_EVENT;
    if schema_oversized || arguments_oversized {
        let mut errors = Vec::new();
        if schema_oversized {
            errors.extend(validate_schema(owner, event, fields).unwrap_err());
        }
        if arguments_oversized {
            errors.extend(validate_value_argument_shape(owner, event, arguments).unwrap_err());
        }
        return Err(errors);
    }
    validate_schema(owner, event, fields)?;
    validate_value_argument_shape(owner, event, arguments)?;
    let mut errors = Vec::new();
    if arguments.len() != fields.len() {
        errors.push(error(
            owner,
            event,
            None,
            format!(
                "received {} arguments for {} declared fields",
                arguments.len(),
                fields.len()
            ),
        ));
    }
    let declared: BTreeMap<&str, &FieldDef> = fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect();
    let mut supplied = BTreeMap::new();
    for argument in arguments {
        supplied.insert(argument.field.as_str(), argument);
        let Some(field) = declared.get(argument.field.as_str()) else {
            errors.push(error(
                owner,
                event,
                Some(&argument.field),
                "argument does not exist in the event schema",
            ));
            continue;
        };
        let actual = match value_kind(&argument.value) {
            Ok(kind) => kind,
            Err(reason) => {
                errors.push(error(owner, event, Some(&argument.field), reason));
                continue;
            }
        };
        if actual != field.kind {
            errors.push(error(
                owner,
                event,
                Some(&argument.field),
                format!("expected {:?}, received {actual:?}", field.kind),
            ));
        }
    }
    for field in fields {
        if !supplied.contains_key(field.name.as_str()) {
            errors.push(error(
                owner,
                event,
                Some(&field.name),
                "required argument is missing",
            ));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(fields
        .iter()
        .map(|field| supplied[field.name.as_str()])
        .collect())
}

#[cfg(test)]
/// Check an event-field read against a proven receiver context. `None` is the
/// ordinary hook/timer/status/resource path and intentionally fails closed.
pub fn validate_receiver_field_access(
    owner: &str,
    receiver_event: Option<&str>,
    referenced_event: &str,
    fields: &[FieldDef],
    field_name: &str,
    expected_kind: VarType,
) -> Result<(), ValidationError> {
    if let Err(mut errors) = validate_schema(owner, referenced_event, fields) {
        return Err(errors.remove(0));
    }
    let Some(receiver_event) = receiver_event else {
        return Err(error(
            owner,
            referenced_event,
            Some(field_name),
            "event fields are unavailable outside a named-event receiver",
        ));
    };
    if receiver_event != referenced_event {
        return Err(error(
            owner,
            referenced_event,
            Some(field_name),
            format!(
                "receiver is for event {:?}, not this event",
                bounded_label(receiver_event, MAX_FIELD_NAME_BYTES)
            ),
        ));
    }
    if expected_kind == VarType::Timer {
        return Err(error(
            owner,
            referenced_event,
            Some(field_name),
            "Timer is not a supported event field kind",
        ));
    }
    let Some(field) = fields.iter().find(|field| field.name == field_name) else {
        return Err(error(
            owner,
            referenced_event,
            Some(field_name),
            "field does not exist in the event schema",
        ));
    };
    if field.kind != expected_kind {
        return Err(error(
            owner,
            referenced_event,
            Some(field_name),
            format!(
                "expected {:?}, schema declares {:?}",
                expected_kind, field.kind
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::behavior::{Expr, Predicate};
    use crate::gimmicks::codegen::EventBinding;

    fn field(name: &str, kind: VarType) -> FieldDef {
        FieldDef {
            name: name.into(),
            kind,
        }
    }

    #[test]
    fn schema_limits_names_and_timer_kind_are_checked() {
        let eight = (0..MAX_FIELDS_PER_EVENT)
            .map(|index| field(&format!("f{index}"), VarType::Int))
            .collect::<Vec<_>>();
        assert!(validate_schema("cloud", "ready", &eight).is_ok());
        let nine = (0..=MAX_FIELDS_PER_EVENT)
            .map(|index| field(&format!("f{index}"), VarType::Int))
            .collect::<Vec<_>>();
        assert!(validate_schema("cloud", "ready", &nine).is_err());
        assert!(validate_schema("cloud", "ready", &[field("", VarType::Int)]).is_err());
        assert!(validate_schema("cloud", "ready", &[field("x\0y", VarType::Int)]).is_err());
        assert!(validate_schema(
            "cloud",
            "ready",
            &[field("same", VarType::Int), field("same", VarType::Float)]
        )
        .is_err());
        assert!(validate_schema("cloud", "ready", &[field("clock", VarType::Timer)]).is_err());
        assert!(validate_schema("cloud", "ready", &[field(&"é".repeat(24), VarType::Int)]).is_ok());
        assert!(
            validate_schema("cloud", "ready", &[field(&"é".repeat(25), VarType::Int)]).is_err()
        );
    }

    #[test]
    fn legacy_event_binding_defaults_to_zero_fields_and_omits_them() {
        let binding: EventBinding =
            serde_json::from_str(r#"{"name":"ready","transitions":[0]}"#).unwrap();
        assert!(binding.fields.is_empty());
        assert_eq!(
            serde_json::to_string(&binding).unwrap(),
            r#"{"name":"ready","transitions":[0]}"#
        );
    }

    #[test]
    fn arguments_are_checked_and_canonicalized_without_mutating_inputs() {
        let fields = vec![
            field("count", VarType::Int),
            field("active", VarType::Flag),
            field("ratio", VarType::Float),
        ];
        let args = [
            ("ratio", ConstValue::Float(0.25)),
            ("active", ConstValue::Flag(true)),
            ("count", ConstValue::Int(7)),
        ]
        .map(|(field, value)| Argument {
            field: field.into(),
            value,
        })
        .to_vec();
        let before = (fields.clone(), args.clone());
        assert_eq!(
            canonicalize_arguments("cloud", "ready", &fields, &args).unwrap(),
            [
                ConstValue::Int(7),
                ConstValue::Flag(true),
                ConstValue::Float(0.25)
            ]
        );
        for (index, replacement) in [
            (2, None),
            (3, Some(args[2].clone())),
            (
                2,
                Some(Argument {
                    field: "count".into(),
                    value: ConstValue::Flag(true),
                }),
            ),
            (
                2,
                Some(Argument {
                    field: "missing".into(),
                    value: ConstValue::Int(1),
                }),
            ),
            (2, Some(args[0].clone())),
        ] {
            let mut invalid = args.clone();
            if index < invalid.len() {
                invalid.remove(index);
            }
            if let Some(value) = replacement {
                invalid.insert(index, value);
            }
            assert!(canonicalize_arguments("cloud", "ready", &fields, &invalid).is_err());
        }
        assert_eq!((fields, args), before);
    }

    #[test]
    fn dynamic_canonicalization_borrows_in_schema_order_and_checks_kinds() {
        let fields = vec![field("count", VarType::Int), field("active", VarType::Flag)];
        let arguments = vec![
            ValueArgument {
                field: "active".into(),
                value: EventValue::Flag(Predicate::Flag("ready".into())),
            },
            ValueArgument {
                field: "count".into(),
                value: EventValue::Numeric(Expr::Var("count".into())),
            },
        ];
        let ordered = canonicalize_value_arguments(
            "cloud",
            "ready",
            &fields,
            &arguments,
            |value| match value {
                EventValue::Constant(ConstValue::Float(_)) => Ok(VarType::Float),
                EventValue::Constant(ConstValue::Int(_)) => Ok(VarType::Int),
                EventValue::Constant(ConstValue::Flag(_)) | EventValue::Flag(_) => {
                    Ok(VarType::Flag)
                }
                EventValue::Numeric(Expr::Var(_) | Expr::TimerRemaining(_)) => Ok(VarType::Int),
                EventValue::Numeric(_) => Err("invalid numeric expression".into()),
            },
        )
        .unwrap();
        assert_eq!(ordered[0].field, "count");
        assert_eq!(ordered[1].field, "active");
        assert!(matches!(&ordered[1].value, EventValue::Flag(_)));

        let wrong_kind = canonicalize_value_arguments(
            "cloud",
            "ready",
            &fields,
            &arguments,
            |value| match value {
                EventValue::Flag(_) => Ok(VarType::Int),
                EventValue::Numeric(_) => Ok(VarType::Int),
                EventValue::Constant(_) => Err("invalid constant".into()),
            },
        );
        assert!(wrong_kind
            .unwrap_err()
            .iter()
            .any(|error| error.reason.contains("expected Flag")));
    }

    #[test]
    fn dynamic_argument_shape_rejects_duplicates_and_unknown_record_fields() {
        let duplicate = vec![
            ValueArgument {
                field: "x".into(),
                value: EventValue::Constant(ConstValue::Int(1)),
            },
            ValueArgument {
                field: "x".into(),
                value: EventValue::Constant(ConstValue::Int(2)),
            },
        ];
        assert!(validate_value_argument_shape("cloud", "ready", &duplicate).is_err());
        let unknown: Result<ValueArgument, _> =
            serde_json::from_str(r#"{"field":"x","value":{"Constant":{"Int":1}},"surprise":true}"#);
        assert!(unknown.is_err());
    }

    #[test]
    fn argument_shape_is_bounded_and_rejects_bad_labels_and_scalars() {
        let valid = (0..MAX_FIELDS_PER_EVENT)
            .map(|index| Argument {
                field: format!("f{index}"),
                value: ConstValue::Int(index as i32),
            })
            .collect::<Vec<_>>();
        assert!(validate_argument_shape("cloud", "ready", &valid).is_ok());
        let mut too_many = valid;
        too_many.push(Argument {
            field: "extra".into(),
            value: ConstValue::Int(1),
        });
        let errors = validate_argument_shape("cloud", "ready", &too_many).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].field.is_none());
        for (name, value) in [
            ("", ConstValue::Int(1)),
            ("ratio", ConstValue::Float(f32::NAN)),
            ("count", ConstValue::Int(i32::MAX)),
        ] {
            assert!(validate_argument_shape(
                "cloud",
                "ready",
                &[Argument {
                    field: name.into(),
                    value
                }]
            )
            .is_err());
        }
        let duplicate = Argument {
            field: "same".into(),
            value: ConstValue::Int(1),
        };
        assert!(
            validate_argument_shape("cloud", "ready", &[duplicate.clone(), duplicate]).is_err()
        );
    }

    #[test]
    fn oversized_inputs_fail_once_before_semantic_aggregation() {
        let fields = (0..=MAX_FIELDS_PER_EVENT)
            .map(|index| field(&format!("f{index}"), VarType::Timer))
            .collect::<Vec<_>>();
        let schema_errors = validate_schema("cloud", "ready", &fields).unwrap_err();
        assert_eq!(schema_errors.len(), 1);
        assert!(schema_errors[0].field.is_none());

        let arguments = (0..=MAX_FIELDS_PER_EVENT)
            .map(|index| Argument {
                field: format!("unknown{index}"),
                value: ConstValue::Flag(false),
            })
            .collect::<Vec<_>>();
        let argument_errors =
            canonicalize_arguments("cloud", "ready", &[], &arguments).unwrap_err();
        assert_eq!(argument_errors.len(), 1);
        assert!(argument_errors[0].field.is_none());

        let both_oversized =
            canonicalize_arguments("cloud", "ready", &fields, &arguments).unwrap_err();
        assert_eq!(both_oversized.len(), 2);
        assert!(both_oversized.iter().all(|error| error.field.is_none()));
    }

    #[test]
    fn oversized_invalid_names_do_not_escape_into_diagnostics() {
        let name = "x".repeat(10_000);
        let errors = validate_schema("cloud", "ready", &[field(&name, VarType::Int)]).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].field.as_ref().unwrap().len() <= MAX_FIELD_NAME_BYTES);
    }

    #[test]
    fn scalar_values_must_be_finite_and_within_the_shared_bound() {
        for value in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            (super::super::live_model::MAX_ABS_NUMBER as f32) * 2.0,
        ] {
            assert!(canonicalize_arguments(
                "cloud",
                "ready",
                &[field("x", VarType::Float)],
                &[Argument {
                    field: "x".into(),
                    value: ConstValue::Float(value)
                }]
            )
            .is_err());
        }
        assert!(canonicalize_arguments(
            "cloud",
            "ready",
            &[field("x", VarType::Int)],
            &[Argument {
                field: "x".into(),
                value: ConstValue::Int(i32::MAX)
            }]
        )
        .is_err());
    }

    #[test]
    fn field_access_requires_the_exact_named_receiver_and_kind() {
        let fields = [field("count", VarType::Int)];
        for (receiver, name, kind, accepted) in [
            (Some("ready"), "count", VarType::Int, true),
            (None, "count", VarType::Int, false),
            (Some("other"), "count", VarType::Int, false),
            (Some("ready"), "missing", VarType::Int, false),
            (Some("ready"), "count", VarType::Float, false),
            (Some("ready"), "count", VarType::Timer, false),
        ] {
            assert_eq!(
                validate_receiver_field_access("cloud", receiver, "ready", &fields, name, kind)
                    .is_ok(),
                accepted
            );
        }
    }
}
