//! Direct controls for resolved scalar leaves in a fighter's personal prc.
//!
//! This adapter keeps the control model small and typed. Paths come from the
//! same named-leaf walk used by the personal-parameter editor, and sparse
//! values are never coerced while being displayed or saved.

use std::collections::HashMap;

use crate::mod_project::{ParamMod, ParamValue};

use super::params::{
    path_is_resolved, personal_edits, personal_path_for, read_leaf, record_personal_edit,
};
use super::paths::NestedPath;

/// One named scalar in a fighter's personal parameter file.
#[derive(Clone, Debug, PartialEq)]
pub struct ParameterControl {
    /// Stable nested path used by the sparse project model.
    pub path: String,
    /// Humanized top-level group, or `General` for a root scalar.
    pub group: String,
    /// Humanized leaf name. Keep [`Self::path`] available for exact inspection.
    pub label: String,
    /// Current effective value, including a valid sparse override.
    pub value: ParamValue,
    /// Value read from the personal prc tree.
    pub base: ParamValue,
    /// Whether the project currently has a sparse override for this path.
    pub overridden: bool,
}

/// Build controls from resolved scalar leaves in the actual personal prc tree.
///
/// Only named booleans and numeric leaves are exposed. A matching sparse edit
/// must have the same exact `ParamValue` variant as the base and finite floats;
/// corrupt values produce an error instead of being hidden or coerced.
pub fn controls(
    tree: &prc::ParamStruct,
    fighter: &str,
    labels: &HashMap<u64, String>,
    params: &ParamMod,
) -> Result<Vec<ParameterControl>, String> {
    personal_path_for(fighter).map_err(|error| error.to_string())?;

    let mut controls = Vec::new();
    for spelling in super::view::editable_paths(tree, labels) {
        let path = NestedPath::parse(&spelling).map_err(|error| error.to_string())?;
        if !path_is_resolved(&path) {
            continue;
        }
        let base = read_leaf(tree, fighter, &path).map_err(|error| error.to_string())?;
        if !is_control_value(base) {
            continue;
        }
        validate_value(&spelling, base)?;

        let stored = personal_edits(params, fighter).get(&spelling).copied();
        let (value, overridden) = match stored {
            Some(value) => {
                if !same_variant(base, value) {
                    return Err(format!(
                        "fighter {fighter}: {spelling}: stored value is {}, but the personal prc leaf is {}",
                        super::params::value_kind_name(&value),
                        super::params::value_kind_name(&base),
                    ));
                }
                validate_value(&spelling, value)?;
                (value, true)
            }
            None => (base, false),
        };

        let segments = &path.0;
        let first = &segments[0];
        let group = if segments.len() > 1 || first.index.is_some() {
            match first.index {
                Some(index) => format!("{} [{index}]", humanize(&first.field)),
                None => humanize(&first.field),
            }
        } else {
            "General".to_string()
        };
        let label = humanize(&segments.last().expect("nonempty parsed path").field);
        controls.push(ParameterControl {
            path: spelling,
            group,
            label,
            value,
            base,
            overridden,
        });
    }

    controls.sort_by(|left, right| {
        left.group
            .cmp(&right.group)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(controls)
}

/// Save a direct control edit after rechecking its stable path and value type.
/// Returning to the base value removes the sparse project entry.
pub fn save(
    params: &mut ParamMod,
    fighter: &str,
    control: &ParameterControl,
    value: ParamValue,
) -> Result<(), String> {
    personal_path_for(fighter).map_err(|error| error.to_string())?;
    let path = NestedPath::parse(&control.path).map_err(|error| error.to_string())?;
    if !path_is_resolved(&path) || path.to_string() != control.path {
        return Err(format!(
            "fighter {fighter}: {} is not a resolved personal parameter path",
            control.path
        ));
    }
    if !is_control_value(control.base) {
        return Err(format!(
            "fighter {fighter}: {} has unsupported base type {}",
            control.path,
            super::params::value_kind_name(&control.base)
        ));
    }
    if !same_variant(control.base, value) {
        return Err(format!(
            "fighter {fighter}: {} expects {}, got {}",
            control.path,
            super::params::value_kind_name(&control.base),
            super::params::value_kind_name(&value)
        ));
    }
    validate_value(&control.path, control.base)?;
    validate_value(&control.path, value)?;

    if let Some(stored) = personal_edits(params, fighter).get(&control.path).copied() {
        if !same_variant(control.base, stored) {
            return Err(format!(
                "fighter {fighter}: {} has a stored {} override, but its base is {}",
                control.path,
                super::params::value_kind_name(&stored),
                super::params::value_kind_name(&control.base)
            ));
        }
        validate_value(&control.path, stored)?;
    }

    record_personal_edit(params, fighter, &path, value, Some(control.base))
        .map_err(|error| error.to_string())
}

fn is_control_value(value: ParamValue) -> bool {
    matches!(
        value,
        ParamValue::Bool(_)
            | ParamValue::I8(_)
            | ParamValue::U8(_)
            | ParamValue::I16(_)
            | ParamValue::U16(_)
            | ParamValue::I32(_)
            | ParamValue::U32(_)
            | ParamValue::Float(_)
    )
}

fn same_variant(left: ParamValue, right: ParamValue) -> bool {
    matches!(
        (left, right),
        (ParamValue::Bool(_), ParamValue::Bool(_))
            | (ParamValue::I8(_), ParamValue::I8(_))
            | (ParamValue::U8(_), ParamValue::U8(_))
            | (ParamValue::I16(_), ParamValue::I16(_))
            | (ParamValue::U16(_), ParamValue::U16(_))
            | (ParamValue::I32(_), ParamValue::I32(_))
            | (ParamValue::U32(_), ParamValue::U32(_))
            | (ParamValue::Float(_), ParamValue::Float(_))
            | (ParamValue::Hash(_), ParamValue::Hash(_))
    )
}

fn validate_value(path: &str, value: ParamValue) -> Result<(), String> {
    if matches!(value, ParamValue::Float(number) if !number.is_finite()) {
        return Err(format!("{path}: float value must be finite"));
    }
    Ok(())
}

fn humanize(raw: &str) -> String {
    raw.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::params::{personal_edits, record_personal_edit};
    use crate::mod_project::ParamValue;
    use prc::hash40::Hash40;
    use prc::{ParamKind, ParamList, ParamStruct};

    fn field(name: &str, value: ParamKind) -> (Hash40, ParamKind) {
        (Hash40(hash40::hash40(name).0), value)
    }

    fn tree() -> ParamStruct {
        ParamStruct(vec![
            field(
                "param_special_lw",
                ParamKind::List(ParamList(vec![ParamKind::Struct(ParamStruct(vec![
                    field("limit_gauge_add", ParamKind::Float(0.5)),
                    field("limit_max", ParamKind::I32(100)),
                    field("charge_enabled", ParamKind::Bool(true)),
                    field("display_name", ParamKind::Str("Cloud".into())),
                    field(
                        "nested_group",
                        ParamKind::Struct(ParamStruct(vec![field("inner", ParamKind::U16(12))])),
                    ),
                ]))])),
            ),
            field("walk_speed_mul", ParamKind::Float(1.15)),
            field("attack_rate", ParamKind::U8(3)),
            field("unknown_root", ParamKind::I16(7)),
        ])
    }

    fn labels() -> HashMap<u64, String> {
        [
            "param_special_lw",
            "limit_gauge_add",
            "limit_max",
            "charge_enabled",
            "display_name",
            "nested_group",
            "inner",
            "walk_speed_mul",
            "attack_rate",
        ]
        .into_iter()
        .map(|name| (hash40::hash40(name).0, name.to_string()))
        .collect()
    }

    #[test]
    fn controls_only_named_numeric_and_boolean_leaves_and_sort_groups() {
        let controls = controls(&tree(), "cloud", &labels(), &ParamMod::default()).unwrap();
        assert_eq!(
            controls
                .iter()
                .map(|item| item.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "attack_rate",
                "walk_speed_mul",
                "param_special_lw[0].charge_enabled",
                "param_special_lw[0].limit_gauge_add",
                "param_special_lw[0].limit_max",
                "param_special_lw[0].nested_group.inner",
            ]
        );
        assert!(!controls
            .iter()
            .any(|item| item.path.contains("display_name")));
        assert!(!controls
            .iter()
            .any(|item| item.path.contains("unknown_root")));
        let group = &controls[2];
        assert_eq!(group.group, "Param Special Lw [0]");
        assert_eq!(group.label, "Charge Enabled");
        assert_eq!(group.value, ParamValue::Bool(true));
        assert_eq!(controls[0].group, "General");
    }

    #[test]
    fn sparse_override_reset_and_invalid_types_are_checked() {
        let mut params = ParamMod::default();
        let path = NestedPath::parse("param_special_lw[0].limit_max").unwrap();
        record_personal_edit(
            &mut params,
            "cloud",
            &path,
            ParamValue::I32(140),
            Some(ParamValue::I32(100)),
        )
        .unwrap();
        let neighbor = NestedPath::parse("param_special_lw[0].charge_enabled").unwrap();
        record_personal_edit(
            &mut params,
            "cloud",
            &neighbor,
            ParamValue::Bool(false),
            Some(ParamValue::Bool(true)),
        )
        .unwrap();
        let encoded = serde_json::to_string(&params).unwrap();
        params = serde_json::from_str(&encoded).unwrap();
        let rows = controls(&tree(), "cloud", &labels(), &params).unwrap();
        let control = rows
            .iter()
            .find(|row| row.path == path.to_string())
            .unwrap();
        assert_eq!(control.base, ParamValue::I32(100));
        assert_eq!(control.value, ParamValue::I32(140));
        assert!(control.overridden);
        save(&mut params, "cloud", control, ParamValue::I32(100)).unwrap();
        assert!(!personal_edits(&params, "cloud").contains_key(&path.to_string()));
        assert_eq!(
            personal_edits(&params, "cloud").get(&neighbor.to_string()),
            Some(&ParamValue::Bool(false))
        );
        assert!(save(&mut params, "cloud", control, ParamValue::Float(2.0)).is_err());
        assert!(save(&mut params, "cloud", control, ParamValue::I8(2)).is_err());
    }

    #[test]
    fn malformed_stored_values_and_nonfinite_base_values_are_rejected() {
        let mut params = ParamMod::default();
        let path = NestedPath::parse("walk_speed_mul").unwrap();
        record_personal_edit(
            &mut params,
            "cloud",
            &path,
            ParamValue::I32(2),
            Some(ParamValue::Float(1.15)),
        )
        .unwrap();
        assert!(controls(&tree(), "cloud", &labels(), &params)
            .unwrap_err()
            .contains("stored value is i32"));

        let mut nonfinite = tree();
        if let ParamKind::Float(value) = &mut nonfinite.0[1].1 {
            *value = f32::INFINITY;
        }
        assert!(
            controls(&nonfinite, "cloud", &labels(), &ParamMod::default())
                .unwrap_err()
                .contains("finite")
        );
    }

    #[test]
    fn save_rejects_bad_fighter_unresolved_path_and_nonfinite_value() {
        let control = ParameterControl {
            path: "walk_speed_mul".into(),
            group: "General".into(),
            label: "Walk Speed Mul".into(),
            value: ParamValue::Float(1.15),
            base: ParamValue::Float(1.15),
            overridden: false,
        };
        let mut params = ParamMod::default();
        assert!(save(&mut params, "cloud/name", &control, ParamValue::Float(2.0)).is_err());
        let mut unresolved = control.clone();
        unresolved.path = "0x123456789".into();
        assert!(save(&mut params, "cloud", &unresolved, ParamValue::Float(2.0)).is_err());
        assert!(
            save(&mut params, "cloud", &control, ParamValue::Float(f32::NAN))
                .unwrap_err()
                .contains("finite")
        );
    }
}
