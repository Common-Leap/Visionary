//! Compact desktop authoring and preview for HUD-meter bindings (F3.2).

use std::collections::BTreeMap;

use egui::{Color32, RichText, Ui};

use super::meter::{self, MeterBinding, MeterStyle, MeterValue};
use super::registry::{self, IdentifierKind};
use crate::game_link::{GameLink, LinkStatus, LiveGimmickMeterValue};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ValueDraftKind {
    #[default]
    Float,
    Integer,
    Flag,
    Pips,
    Cycle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PreviewBoundary {
    Empty,
    #[default]
    Partial,
    Full,
    Overflow,
    Unknown,
}

impl PreviewBoundary {
    const ALL: [Self; 5] = [
        Self::Empty,
        Self::Partial,
        Self::Full,
        Self::Overflow,
        Self::Unknown,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Empty => "Empty",
            Self::Partial => "Partial",
            Self::Full => "Full",
            Self::Overflow => "Overflow",
            Self::Unknown => "Unknown",
        }
    }

    fn fraction(self) -> Option<f32> {
        match self {
            Self::Empty => Some(0.0),
            Self::Partial => Some(0.5),
            Self::Full | Self::Overflow => Some(1.0),
            Self::Unknown => None,
        }
    }
}

impl ValueDraftKind {
    const ALL: [Self; 5] = [
        Self::Float,
        Self::Integer,
        Self::Flag,
        Self::Pips,
        Self::Cycle,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Float => "Decimal value",
            Self::Integer => "Whole-number value",
            Self::Flag => "On / off",
            Self::Pips => "Pip counter",
            Self::Cycle => "Named states",
        }
    }

    fn registry_kind(self) -> IdentifierKind {
        match self {
            Self::Float => IdentifierKind::WorkFloat,
            Self::Integer | Self::Pips | Self::Cycle => IdentifierKind::WorkInt,
            Self::Flag => IdentifierKind::WorkFlag,
        }
    }

    fn styles(self) -> &'static [MeterStyle] {
        match self {
            Self::Float => &[MeterStyle::Bar],
            Self::Integer => &[MeterStyle::Bar, MeterStyle::StockArrows],
            Self::Flag | Self::Cycle => &[MeterStyle::Icon],
            Self::Pips => &[MeterStyle::Pips],
        }
    }
}

#[derive(Default)]
struct MeterDraft {
    kind: ValueDraftKind,
    symbol: String,
    first: String,
    last: String,
    labels: String,
}

impl MeterDraft {
    fn incomplete_guidance(&self) -> Option<&'static str> {
        if self.symbol.trim().is_empty() {
            return Some("Choose a source for the meter.");
        }
        match self.kind {
            ValueDraftKind::Float | ValueDraftKind::Integer
                if self.first.trim().is_empty() || self.last.trim().is_empty() =>
            {
                Some("Enter the meter range.")
            }
            ValueDraftKind::Pips if self.last.trim().is_empty() => {
                Some("Enter the number of pips.")
            }
            ValueDraftKind::Cycle if self.labels.trim().is_empty() => {
                Some("Enter the cycle labels.")
            }
            _ => None,
        }
    }

    fn binding(&self, fighter: &str, style: MeterStyle) -> Result<MeterBinding, String> {
        let parse = |text: &str, name: &str| {
            text.trim()
                .parse::<f32>()
                .map_err(|_| format!("{name} must be a number"))
        };
        let symbol = self.symbol.trim().to_string();
        let value = match self.kind {
            ValueDraftKind::Float => MeterValue::Float {
                symbol,
                min: parse(&self.first, "Minimum")?,
                max: parse(&self.last, "Maximum")?,
            },
            ValueDraftKind::Integer => MeterValue::Integer {
                symbol,
                min: self
                    .first
                    .trim()
                    .parse()
                    .map_err(|_| "Minimum must be an integer".to_string())?,
                max: self
                    .last
                    .trim()
                    .parse()
                    .map_err(|_| "Maximum must be an integer".to_string())?,
            },
            ValueDraftKind::Flag => MeterValue::Flag { symbol },
            ValueDraftKind::Pips => MeterValue::Pips {
                symbol,
                count: self
                    .last
                    .trim()
                    .parse()
                    .map_err(|_| "Pip count must be a positive integer".to_string())?,
            },
            ValueDraftKind::Cycle => MeterValue::Cycle {
                symbol,
                labels: self
                    .labels
                    .split(',')
                    .map(str::trim)
                    .map(str::to_string)
                    .collect(),
            },
        };
        let binding = MeterBinding {
            fighter: fighter.to_string(),
            enabled: true,
            style,
            value,
        };
        let errors = meter::validate(&binding);
        if errors.is_empty() {
            Ok(binding)
        } else {
            Err(errors.join("; "))
        }
    }
}

pub struct MeterView {
    fighter: String,
    style: MeterStyle,
    draft: MeterDraft,
    preview_boundary: PreviewBoundary,
    message: String,
}

impl Default for MeterView {
    fn default() -> Self {
        Self {
            fighter: String::new(),
            style: MeterStyle::Bar,
            draft: MeterDraft {
                first: "0".to_string(),
                last: "100".to_string(),
                ..Default::default()
            },
            preview_boundary: PreviewBoundary::default(),
            message: String::new(),
        }
    }
}

impl MeterView {
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        meters: &mut BTreeMap<String, Vec<MeterBinding>>,
        link: &GameLink,
    ) {
        if fighter.is_empty() {
            return;
        }
        if self.fighter != fighter {
            self.reset_for_fighter(fighter);
        }
        link.ensure_started();
        ui.add_space(8.0);
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Meter definition").strong());
                    ui.label(RichText::new("Preview only").small().weak())
                        .on_hover_text(
                            "The plugin can sample this value after export, but this build does not yet draw a custom in-game HUD.",
                        );
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Value");
                    egui::ComboBox::from_id_salt("gimmick_meter_value_kind")
                        .selected_text(self.draft.kind.label())
                        .show_ui(ui, |ui| {
                            for kind in ValueDraftKind::ALL {
                                if ui
                                    .selectable_value(&mut self.draft.kind, kind, kind.label())
                                    .changed()
                                {
                                    self.draft.symbol.clear();
                                    self.style = match kind {
                                        ValueDraftKind::Float | ValueDraftKind::Integer => {
                                            MeterStyle::Bar
                                        }
                                        ValueDraftKind::Flag | ValueDraftKind::Cycle => {
                                            MeterStyle::Icon
                                        }
                                        ValueDraftKind::Pips => MeterStyle::Pips,
                                    };
                                }
                            }
                        });
                    ui.label("Display");
                    egui::ComboBox::from_id_salt("gimmick_meter_style")
                        .selected_text(style_label(self.style))
                        .show_ui(ui, |ui| {
                            for &style in self.draft.kind.styles() {
                                ui.selectable_value(&mut self.style, style, style_label(style));
                            }
                        });
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Game value");
                    let choices: Vec<&str> =
                        registry::entries_for(fighter, self.draft.kind.registry_kind())
                            .map(|entry| entry.symbol)
                            .collect();
                    egui::ComboBox::from_id_salt("gimmick_meter_symbol")
                        .selected_text(if self.draft.symbol.is_empty() {
                            "Choose a value…".to_string()
                        } else {
                            source_label(&self.draft.symbol)
                        })
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            for symbol in choices {
                                ui.selectable_value(
                                    &mut self.draft.symbol,
                                    symbol.to_string(),
                                    source_label(symbol),
                                )
                                .on_hover_text(symbol);
                            }
                        });
                });
                self.draw_value_fields(ui);
                self.draw_live_value(ui, fighter, link);
                let verdict = self.draft.binding(fighter, self.style);
                let existing = meters.get(fighter).and_then(|saved| {
                    saved
                        .iter()
                        .position(|binding| value_symbol(&binding.value) == self.draft.symbol)
                });
                ui.horizontal_wrapped(|ui| {
                    if let Some(guidance) = self.draft.incomplete_guidance() {
                        ui.add_enabled(false, egui::Button::new("Add to build"));
                        ui.label(RichText::new(guidance).small().weak());
                    } else {
                        match verdict {
                            Ok(binding) => {
                                ui.label("Preview");
                                preview(ui, &binding, self.preview_boundary);
                                egui::ComboBox::from_id_salt("gimmick_meter_preview_boundary")
                                    .selected_text(self.preview_boundary.label())
                                    .show_ui(ui, |ui| {
                                        for boundary in PreviewBoundary::ALL {
                                            ui.selectable_value(
                                                &mut self.preview_boundary,
                                                boundary,
                                                boundary.label(),
                                            );
                                        }
                                    });
                                let action = if existing.is_some() {
                                    "Update build"
                                } else {
                                    "Add to build"
                                };
                                if ui.small_button(action).clicked() {
                                    if store_binding(meters, fighter, binding) {
                                        self.message = "Updated. The next plugin build will use this definition."
                                            .to_string();
                                    } else {
                                        self.message =
                                            "Added. It will be included in the next plugin build."
                                                .to_string();
                                    }
                                }
                            }
                            Err(reason) => {
                                ui.add_enabled(false, egui::Button::new("Add to build"));
                                ui.colored_label(
                                    Color32::from_rgb(240, 150, 130),
                                    RichText::new(reason).small(),
                                );
                            }
                        }
                    }
                });
                if !self.message.is_empty() {
                    ui.label(RichText::new(&self.message).small().weak());
                }
            });
        self.draw_saved(ui, fighter, meters);
    }

    fn reset_for_fighter(&mut self, fighter: &str) {
        self.fighter = fighter.to_string();
        self.style = MeterStyle::Bar;
        self.preview_boundary = PreviewBoundary::Partial;
        self.message.clear();
        self.draft = MeterDraft {
            first: "0".to_string(),
            last: "100".to_string(),
            ..Default::default()
        };
        if fighter == "cloud" {
            const LIMIT: &str = "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE";
            if registry::resolve(fighter, LIMIT, IdentifierKind::WorkFloat).is_ok() {
                self.draft.symbol = LIMIT.to_string();
            }
        }
    }

    fn draw_value_fields(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| match self.draft.kind {
            ValueDraftKind::Float | ValueDraftKind::Integer => {
                ui.label("Min");
                ui.add(egui::TextEdit::singleline(&mut self.draft.first).desired_width(70.0));
                ui.label("Max");
                ui.add(egui::TextEdit::singleline(&mut self.draft.last).desired_width(70.0));
            }
            ValueDraftKind::Pips => {
                ui.label("Count");
                ui.add(egui::TextEdit::singleline(&mut self.draft.last).desired_width(70.0));
            }
            ValueDraftKind::Cycle => {
                ui.label("Labels");
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.labels)
                        .desired_width(300.0)
                        .hint_text("state 1, state 2, state 3"),
                );
            }
            ValueDraftKind::Flag => {}
        });
    }

    fn draw_live_value(&self, ui: &mut Ui, fighter: &str, link: &GameLink) {
        if self.draft.symbol.is_empty() {
            return;
        }
        let values = link.gimmick_meter_values(fighter, &self.draft.symbol);
        if values.is_empty() {
            if link.status() == LinkStatus::Connected {
                ui.label(
                    RichText::new("Live value unavailable in the current plugin build")
                        .small()
                        .weak(),
                )
                .on_hover_text(
                    "Add this definition, build the plugin, deploy it, and restart the game to enable live sampling.",
                );
            }
            return;
        }
        for meter in values {
            ui.horizontal_wrapped(|ui| {
                let player = if meter.entry_id >= 0 {
                    format!("P{}", meter.entry_id + 1)
                } else {
                    "Live".to_string()
                };
                ui.label(RichText::new(format!("Live {player}")).small().strong());
                match meter.value {
                    LiveGimmickMeterValue::Float(value) => {
                        let min = self.draft.first.trim().parse::<f32>().ok();
                        let max = self.draft.last.trim().parse::<f32>().ok();
                        match (min, max) {
                            (Some(min), Some(max)) if min < max => {
                                let fraction = ((value - min) / (max - min)).clamp(0.0, 1.0);
                                ui.add(
                                    egui::ProgressBar::new(fraction)
                                        .desired_width(160.0)
                                        .text(format!("{value:.1} / {max:.1}")),
                                );
                            }
                            _ => {
                                ui.label(format!("{value:.3}"));
                            }
                        }
                    }
                    LiveGimmickMeterValue::Int(value) => {
                        ui.label(value.to_string());
                    }
                    LiveGimmickMeterValue::Flag(active) => {
                        ui.label(if active { "On" } else { "Off" });
                    }
                }
            });
        }
    }

    fn draw_saved(
        &self,
        ui: &mut Ui,
        fighter: &str,
        meters: &mut BTreeMap<String, Vec<MeterBinding>>,
    ) {
        let mut remove = None;
        if let Some(saved) = meters.get_mut(fighter) {
            if !saved.is_empty() {
                ui.label(RichText::new("Included in plugin build").small().strong());
            }
            for (index, binding) in saved.iter_mut().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut binding.enabled, "");
                    ui.label(RichText::new(source_label(value_symbol(&binding.value))).small())
                        .on_hover_text(value_symbol(&binding.value));
                    preview(ui, binding, self.preview_boundary);
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
            }
            if let Some(index) = remove {
                saved.remove(index);
            }
        }
        if meters.get(fighter).is_some_and(Vec::is_empty) {
            meters.remove(fighter);
        }
    }
}

fn style_label(style: MeterStyle) -> &'static str {
    match style {
        MeterStyle::Bar => "Progress bar",
        MeterStyle::Icon => "Icon",
        MeterStyle::Pips => "Pips",
        MeterStyle::StockArrows => "Stock arrows",
    }
}

fn source_label(symbol: &str) -> String {
    let suffix = symbol
        .split_once("_WORK_ID_")
        .map_or(symbol, |(_, suffix)| suffix)
        .strip_prefix("FLOAT_")
        .or_else(|| {
            symbol
                .split_once("_WORK_ID_")
                .map(|(_, suffix)| suffix)
                .and_then(|suffix| suffix.strip_prefix("INT_"))
        })
        .or_else(|| {
            symbol
                .split_once("_WORK_ID_")
                .map(|(_, suffix)| suffix)
                .and_then(|suffix| suffix.strip_prefix("FLAG_"))
        })
        .unwrap_or_else(|| {
            symbol
                .split_once("_WORK_ID_")
                .map_or(symbol, |(_, suffix)| suffix)
        });
    suffix
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn value_symbol(value: &MeterValue) -> &str {
    match value {
        MeterValue::Float { symbol, .. }
        | MeterValue::Integer { symbol, .. }
        | MeterValue::Flag { symbol }
        | MeterValue::Pips { symbol, .. }
        | MeterValue::Cycle { symbol, .. } => symbol,
    }
}

/// Store one source once per fighter. Re-authoring the same game value edits
/// its presentation instead of making the plugin sample duplicate bindings.
/// Returns true when an existing definition was replaced.
fn store_binding(
    meters: &mut BTreeMap<String, Vec<MeterBinding>>,
    fighter: &str,
    binding: MeterBinding,
) -> bool {
    let saved = meters.entry(fighter.to_string()).or_default();
    if let Some(index) = saved
        .iter()
        .position(|known| value_symbol(&known.value) == value_symbol(&binding.value))
    {
        saved[index] = binding;
        true
    } else {
        saved.push(binding);
        false
    }
}

fn preview(ui: &mut Ui, binding: &MeterBinding, boundary: PreviewBoundary) {
    match (&binding.style, &binding.value) {
        (MeterStyle::Bar, _) => {
            let mut bar =
                egui::ProgressBar::new(boundary.fraction().unwrap_or(0.0)).desired_width(100.0);
            bar = match boundary {
                PreviewBoundary::Unknown => bar.text("?"),
                PreviewBoundary::Overflow => bar.text(">100%"),
                _ => bar.show_percentage(),
            };
            ui.add(bar);
        }
        (MeterStyle::Pips, MeterValue::Pips { count, .. }) => {
            let shown = usize::from((*count).min(12));
            let filled = match boundary {
                PreviewBoundary::Empty | PreviewBoundary::Unknown => 0,
                PreviewBoundary::Partial => shown / 2,
                PreviewBoundary::Full | PreviewBoundary::Overflow => shown,
            };
            ui.label(
                RichText::new(if boundary == PreviewBoundary::Unknown {
                    "?".to_string()
                } else {
                    format!("{}{}", "●".repeat(filled), "○".repeat(shown - filled))
                })
                .small(),
            );
        }
        (MeterStyle::StockArrows, _) => {
            let text = match boundary {
                PreviewBoundary::Empty => "△ △ △",
                PreviewBoundary::Partial => "▲ △ △",
                PreviewBoundary::Full | PreviewBoundary::Overflow => "▲ ▲ ▲",
                PreviewBoundary::Unknown => "?",
            };
            ui.label(RichText::new(text).small());
        }
        (MeterStyle::Icon, MeterValue::Cycle { labels, .. }) => {
            let index = match boundary {
                PreviewBoundary::Empty => Some(0),
                PreviewBoundary::Partial => Some(labels.len() / 2),
                PreviewBoundary::Full | PreviewBoundary::Overflow => labels.len().checked_sub(1),
                PreviewBoundary::Unknown => None,
            };
            ui.label(
                RichText::new(
                    index
                        .and_then(|index| labels.get(index))
                        .map_or("?", String::as_str),
                )
                .small(),
            );
        }
        (MeterStyle::Icon, _) => {
            ui.label(
                RichText::new(if matches!(boundary, PreviewBoundary::Empty) {
                    "○"
                } else if matches!(boundary, PreviewBoundary::Unknown) {
                    "?"
                } else {
                    "●"
                })
                .small(),
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_builds_only_a_valid_allowlisted_binding() {
        let draft = MeterDraft {
            kind: ValueDraftKind::Float,
            symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".to_string(),
            first: "0".to_string(),
            last: "100".to_string(),
            labels: String::new(),
        };
        assert!(draft.binding("cloud", MeterStyle::Bar).is_ok());
        assert!(draft.binding("mario", MeterStyle::Bar).is_err());
        assert!(draft.binding("cloud", MeterStyle::Pips).is_err());
    }

    #[test]
    fn untouched_meter_fields_are_guidance_not_validation_failures() {
        let mut draft = MeterDraft {
            first: "0".to_string(),
            last: "100".to_string(),
            ..Default::default()
        };
        assert_eq!(
            draft.incomplete_guidance(),
            Some("Choose a source for the meter.")
        );
        draft.symbol = "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".to_string();
        assert_eq!(draft.incomplete_guidance(), None);
        draft.last.clear();
        assert_eq!(draft.incomplete_guidance(), Some("Enter the meter range."));
    }

    #[test]
    fn preview_boundaries_are_deterministic_and_clamped() {
        assert_eq!(PreviewBoundary::Empty.fraction(), Some(0.0));
        assert_eq!(PreviewBoundary::Partial.fraction(), Some(0.5));
        assert_eq!(PreviewBoundary::Full.fraction(), Some(1.0));
        assert_eq!(PreviewBoundary::Overflow.fraction(), Some(1.0));
        assert_eq!(PreviewBoundary::Unknown.fraction(), None);
    }

    #[test]
    fn cloud_opens_with_a_readable_limit_preset() {
        let mut view = MeterView::default();
        view.reset_for_fighter("cloud");
        assert_eq!(
            view.draft.symbol,
            "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE"
        );
        assert_eq!(view.draft.first, "0");
        assert_eq!(view.draft.last, "100");
        assert_eq!(source_label(&view.draft.symbol), "Limit Gauge");
    }

    #[test]
    fn value_kind_offers_only_compatible_display_styles() {
        assert_eq!(ValueDraftKind::Float.styles(), &[MeterStyle::Bar]);
        assert_eq!(
            ValueDraftKind::Integer.styles(),
            &[MeterStyle::Bar, MeterStyle::StockArrows]
        );
        assert_eq!(ValueDraftKind::Flag.styles(), &[MeterStyle::Icon]);
        assert_eq!(ValueDraftKind::Pips.styles(), &[MeterStyle::Pips]);
        assert_eq!(ValueDraftKind::Cycle.styles(), &[MeterStyle::Icon]);
    }

    #[test]
    fn saving_the_same_source_updates_instead_of_duplicating_it() {
        let mut meters = BTreeMap::new();
        let first = MeterBinding {
            fighter: "cloud".into(),
            enabled: true,
            style: MeterStyle::Bar,
            value: MeterValue::Float {
                symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                min: 0.0,
                max: 100.0,
            },
        };
        assert!(!store_binding(&mut meters, "cloud", first));

        let updated = MeterBinding {
            fighter: "cloud".into(),
            enabled: false,
            style: MeterStyle::Bar,
            value: MeterValue::Float {
                symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                min: 10.0,
                max: 80.0,
            },
        };
        assert!(store_binding(&mut meters, "cloud", updated.clone()));
        assert_eq!(meters["cloud"], vec![updated]);
    }
}
