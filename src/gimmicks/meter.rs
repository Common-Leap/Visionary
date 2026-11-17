//! Typed HUD-meter bindings (slice F3.1).
//!
//! This is deliberately presentation-free. A binding names one allowlisted
//! fighter work symbol and describes how its value is interpreted; desktop
//! preview and game rendering consume only bindings that pass [`validate`].

use serde::{Deserialize, Serialize};

use super::registry::{self, IdentifierKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MeterStyle {
    Bar,
    Icon,
    Pips,
    StockArrows,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MeterValue {
    Float { symbol: String, min: f32, max: f32 },
    Integer { symbol: String, min: i32, max: i32 },
    Flag { symbol: String },
    Pips { symbol: String, count: u16 },
    Cycle { symbol: String, labels: Vec<String> },
}

impl MeterValue {
    fn symbol(&self) -> &str {
        match self {
            Self::Float { symbol, .. }
            | Self::Integer { symbol, .. }
            | Self::Flag { symbol }
            | Self::Pips { symbol, .. }
            | Self::Cycle { symbol, .. } => symbol,
        }
    }

    fn identifier_kind(&self) -> IdentifierKind {
        match self {
            Self::Float { .. } => IdentifierKind::WorkFloat,
            Self::Integer { .. } | Self::Pips { .. } | Self::Cycle { .. } => {
                IdentifierKind::WorkInt
            }
            Self::Flag { .. } => IdentifierKind::WorkFlag,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeterBinding {
    pub fighter: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    pub style: MeterStyle,
    pub value: MeterValue,
}

const fn enabled_by_default() -> bool {
    true
}

/// Refuse malformed ranges, empty display states, incompatible styles, and
/// every identifier not present in the pinned per-fighter registry.
pub fn validate(binding: &MeterBinding) -> Vec<String> {
    let mut errors = Vec::new();
    if binding.fighter.is_empty() {
        errors.push("meter fighter is empty".to_string());
    }
    match &binding.value {
        MeterValue::Float { min, max, .. } => {
            if !min.is_finite() || !max.is_finite() || min >= max {
                errors.push("float meter range must be finite with min < max".to_string());
            }
        }
        MeterValue::Integer { min, max, .. } if min >= max => {
            errors.push("integer meter range must have min < max".to_string());
        }
        MeterValue::Pips { count, .. } if *count == 0 => {
            errors.push("pip meter must contain at least one pip".to_string());
        }
        MeterValue::Cycle { labels, .. } => {
            if labels.is_empty() {
                errors.push("cycle meter must contain at least one label".to_string());
            } else if labels.iter().any(|label| label.trim().is_empty()) {
                errors.push("cycle meter labels must not be empty".to_string());
            }
        }
        _ => {}
    }
    let style_ok = matches!(
        (&binding.style, &binding.value),
        (
            MeterStyle::Bar,
            MeterValue::Float { .. } | MeterValue::Integer { .. }
        ) | (
            MeterStyle::Icon,
            MeterValue::Flag { .. } | MeterValue::Cycle { .. }
        ) | (MeterStyle::Pips, MeterValue::Pips { .. })
            | (MeterStyle::StockArrows, MeterValue::Integer { .. })
    );
    if !style_ok {
        errors.push("meter style is incompatible with its value kind".to_string());
    }
    if let Err(error) = registry::resolve(
        &binding.fighter,
        binding.value.symbol(),
        binding.value.identifier_kind(),
    ) {
        errors.push(format!("meter identifier is not allowlisted: {error:?}"));
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloud_limit() -> MeterBinding {
        MeterBinding {
            fighter: "cloud".to_string(),
            enabled: true,
            style: MeterStyle::Bar,
            value: MeterValue::Float {
                symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".to_string(),
                min: 0.0,
                max: 100.0,
            },
        }
    }

    #[test]
    fn binding_round_trips_and_defaults_enabled() {
        let binding = cloud_limit();
        let json = serde_json::to_string(&binding).unwrap();
        assert_eq!(
            serde_json::from_str::<MeterBinding>(&json).unwrap(),
            binding
        );
        let without_enabled = json.replace("\"enabled\":true,", "");
        assert!(
            serde_json::from_str::<MeterBinding>(&without_enabled)
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn cloud_limit_is_a_clean_allowlisted_float_bar() {
        assert!(validate(&cloud_limit()).is_empty());
    }

    #[test]
    fn wrong_fighter_kind_style_and_ranges_fail_closed() {
        let mut binding = cloud_limit();
        binding.fighter = "mario".to_string();
        binding.style = MeterStyle::Pips;
        binding.value = MeterValue::Integer {
            symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".to_string(),
            min: 4,
            max: 4,
        };
        let errors = validate(&binding).join("\n");
        assert!(errors.contains("min < max"), "{errors}");
        assert!(errors.contains("incompatible"), "{errors}");
        assert!(errors.contains("allowlisted"), "{errors}");
    }

    #[test]
    fn empty_pips_and_cycle_labels_are_rejected() {
        let mut binding = cloud_limit();
        binding.style = MeterStyle::Pips;
        binding.value = MeterValue::Pips {
            symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_BREAK_CLEAR_FRAME".to_string(),
            count: 0,
        };
        assert!(validate(&binding).iter().any(|error| error.contains("pip")));
        binding.style = MeterStyle::Icon;
        binding.value = MeterValue::Cycle {
            symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_BREAK_CLEAR_FRAME".to_string(),
            labels: vec![" ".to_string()],
        };
        assert!(validate(&binding)
            .iter()
            .any(|error| error.contains("labels")));
    }
}
