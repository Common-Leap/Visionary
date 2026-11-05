//! Selector-based editing of a behavior draft; saved graphs change only on save.

use egui::Ui;

use super::behavior::{Action, BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, VarType};
use super::controller::{ControllerAction, ControllerActionKind};
use super::graph_edit;
use super::hooks::HookName;
use super::native_inputs::{ButtonEdge, FighterValue, InputButton, NativeInput, SituationKind};

#[derive(Default)]
pub struct GraphView {
    state_name: String,
    state_target: String,
    variable_name: String,
    variable_target: String,
    transition: usize,
    action_draft: Option<ControllerAction>,
    timer_target: String,
    timer_frames: i32,
    status: String,
    flag_condition_draft: Option<FlagConditionDraft>,
}

/// Incomplete picker choices stay local until the entire action validates.
struct FlagConditionDraft {
    fighter: String,
    reference: super::field_projection::FieldRef,
    baseline: BehaviorGraph,
    action: Action,
}

impl GraphView {
    fn status_source_ui(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        graph: &mut BehaviorGraph,
        index: usize,
    ) {
        if let Some(source) = graph.emission_plan.status_triggers.get(index).cloned() {
            ui.label(hook_label(source.hook));
            let mut symbol = source.symbol.clone();
            egui::ComboBox::from_id_salt(("status_source", index))
                .selected_text(super::field_canvas::status_name(&symbol))
                .height(360.0)
                .show_ui(ui, |ui| {
                    for entry in super::registry::entries_for(
                        fighter,
                        super::registry::IdentifierKind::FighterStatus,
                    ) {
                        ui.selectable_value(
                            &mut symbol,
                            entry.symbol.to_owned(),
                            super::field_canvas::status_name(entry.symbol),
                        );
                    }
                });
            if symbol != source.symbol {
                let mut updated = graph.clone();
                updated.emission_plan.status_triggers[index].symbol = symbol;
                let result = graph_edit::validate_graph(&updated, fighter).map(|()| updated);
                self.apply(graph, result);
            }
            ui.label(&source.evidence);
            ui.weak(if source.runtime_validated {
                "Runtime evidence recorded"
            } else {
                "Runtime evidence not confirmed"
            });
        }
    }

    /// Inspect one field item without opening the full legacy form editor.
    pub fn field_inspector(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        graph: &mut BehaviorGraph,
        reference: &super::field_projection::FieldRef,
    ) -> bool {
        use super::field_projection::{FieldRef, SourceBindingRef};
        if self.flag_condition_draft.as_ref().is_some_and(|draft| {
            draft.fighter != fighter || draft.reference != *reference || draft.baseline != *graph
        }) {
            self.flag_condition_draft = None;
        }
        let before = graph.clone();
        let (title, detail, _) = super::field_canvas::describe(graph, reference);
        ui.horizontal_wrapped(|ui| {
            ui.label(title);
            ui.weak(detail);
        });
        match reference {
            FieldRef::Graph => {
                let mut initial = graph.initial.clone();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Starts in");
                    state_picker(ui, "field_initial", graph, &mut initial);
                });
                if initial != graph.initial {
                    let result = graph_edit::set_initial(graph, fighter, &initial);
                    self.apply(graph, result);
                }
            }
            FieldRef::State(name) => {
                if self.state_target != *name {
                    self.state_target = name.clone();
                    self.state_name = name.clone();
                }
                ui.horizontal_wrapped(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.state_name).desired_width(160.0));
                    if ui.button("Rename").clicked() {
                        let result =
                            graph_edit::rename_state(graph, fighter, name, &self.state_name);
                        self.apply(graph, result);
                    }
                    if ui.button("Make entry").clicked() {
                        let result = graph_edit::set_initial(graph, fighter, name);
                        self.apply(graph, result);
                    }
                    if ui.button("Remove").clicked() {
                        let result = graph_edit::remove_state(graph, fighter, name);
                        self.apply(graph, result);
                    }
                });
                if graph.initial != *name {
                    let current = graph
                        .states
                        .iter()
                        .find(|state| state.name == *name)
                        .and_then(|state| state.status.as_ref())
                        .map(|binding| binding.target.clone());
                    let mut owned = current.is_some();
                    let mut target = current
                        .clone()
                        .unwrap_or_else(|| "FIGHTER_STATUS_KIND_SPECIAL_S".into());
                    ui.horizontal_wrapped(|ui| {
                        if owned {
                            ui.label("Status");
                            if ui.small_button("Unbind").clicked() {
                                owned = false;
                            }
                        } else if ui.button("Bind fighter status").clicked() {
                            owned = true;
                        }
                        if owned {
                            if ui.button("New custom status").clicked() {
                                let mut ids = graph.custom_statuses.clone();
                                match super::status_model::allocate(&mut ids, name) {
                                    Ok(custom) => target = custom,
                                    Err(error) => self.status = error,
                                }
                            }
                            match &mut target {
                                super::status_model::StatusRef::Native(symbol) => {
                                    super::registry::fighter_status_picker(ui, fighter, symbol)
                                }
                                super::status_model::StatusRef::Custom { key, .. } => {
                                    ui.label(format!("Custom: {key}"));
                                    if ui.small_button("Existing status").clicked() {
                                        target = "FIGHTER_STATUS_KIND_SPECIAL_S".into();
                                    }
                                }
                            }
                        }
                    });
                    if owned && target.is_empty() {
                        target = "FIGHTER_STATUS_KIND_SPECIAL_S".into();
                    }
                    let wanted = owned.then_some(target);
                    if wanted != current {
                        let mut candidate = graph.clone();
                        if let Some(super::status_model::StatusRef::Custom { key, slot }) = &wanted
                        {
                            candidate.custom_statuses.insert(key.clone(), *slot);
                        }
                        let result =
                            graph_edit::set_state_status_ref(&candidate, fighter, name, wanted);
                        self.apply(graph, result);
                    }
                }
                self.owned_state_ui(ui, fighter, graph, name);
            }
            FieldRef::EventField { event, field } => {
                crate::move_nodes::view::event_read_ui(ui, event, field);
            }
            FieldRef::Variable(name) => {
                // Only fighters with native form switches can carry a value to another form.
                if matches!(
                    fighter,
                    "eflame" | "elight" | "pzenigame" | "pfushigisou" | "plizardon"
                ) {
                    if let Some(variable) = graph.vars.iter_mut().find(|v| v.name == *name) {
                        ui.checkbox(
                            &mut variable.keep_across_form_switch,
                            "Keep across form switch",
                        )
                        .on_hover_text(
                            "Copied to the same controller and variable on the next form.",
                        );
                    }
                }

                if super::controller::extract_timer(graph).is_ok()
                    && matches!(name.as_str(), "remaining" | "elapsed_events" | "ready")
                {
                    let mut config =
                        super::controller::extract_timer(graph).expect("checked reusable timer");
                    let original = config.clone();
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Duration");
                        ui.add(
                            egui::DragValue::new(&mut config.duration_frames)
                                .range(1..=super::live_model::MAX_TIMER_FRAMES as u32)
                                .suffix(" frames"),
                        );
                        ui.checkbox(&mut config.repeats, "Repeat");
                    });
                    ui.weak(format!("{:.2} seconds at 60 fps · expiry and stock-start bookkeeping update together", config.duration_frames as f32 / 60.0));
                    if config != original {
                        let result = super::controller::configure_timer(graph, fighter, config)
                            .map(|updated| updated.graph);
                        self.apply(graph, result);
                    }
                    return *graph != before;
                }
                if self.variable_target != *name {
                    self.variable_target = name.clone();
                    self.variable_name = name.clone();
                }
                if let Some(variable) = graph.vars.iter().find(|variable| variable.name == *name) {
                    let mut value = variable.initial;
                    let kind = variable.kind;
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Initial");
                        value_control(ui, &mut value, kind);
                    });
                    if value != variable.initial {
                        let mut updated = graph.clone();
                        updated
                            .vars
                            .iter_mut()
                            .find(|variable| variable.name == *name)
                            .expect("selected variable exists")
                            .initial = value;
                        let result =
                            graph_edit::validate_graph(&updated, fighter).map(|()| updated);
                        self.apply(graph, result);
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.variable_name).desired_width(160.0),
                    );
                    if ui.button("Rename").clicked() {
                        let result =
                            graph_edit::rename_variable(graph, fighter, name, &self.variable_name);
                        self.apply(graph, result);
                    }
                    if ui.button("Remove").clicked() {
                        let result = graph_edit::remove_variable(graph, fighter, name);
                        self.apply(graph, result);
                    }
                });
            }
            FieldRef::Transition(index) => {
                let sources: Vec<_> = graph
                    .emission_plan
                    .status_triggers
                    .iter()
                    .enumerate()
                    .filter(|(_, source)| source.transitions.contains(index))
                    .map(|(i, _)| i)
                    .collect();
                for source in sources {
                    self.status_source_ui(ui, fighter, graph, source);
                }
                if let Some((_, earlier)) = graph_edit::unreachable_frame_rules(graph)
                    .into_iter()
                    .find(|(later, _)| later == index)
                {
                    let (title, detail, _) =
                        super::field_canvas::describe(graph, &FieldRef::Transition(earlier));
                    ui.colored_label(ui.visuals().warn_fg_color, format!(
                        "Never runs: the earlier rule \"{title} · {detail}\" passes whenever this one does and leaves the state first. Run this one earlier or narrow that guard."
                    ));
                }
                if let Some(mut transition) = graph.transitions.get(*index).cloned() {
                    self.transition = *index;
                    let original = transition.clone();
                    ui.add_enabled_ui(
                        !super::field_values::is_managed_timer_transition(graph, *index),
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label("While in");
                                state_picker(
                                    ui,
                                    "field_transition_from",
                                    graph,
                                    &mut transition.from,
                                );
                                ui.label("Then go to");
                                state_picker(ui, "field_transition_to", graph, &mut transition.to);
                            })
                        },
                    );
                    guarded_condition_ui(ui, fighter, graph, *index, &mut transition.guard);
                    if transition != original {
                        let result =
                            graph_edit::replace_transition(graph, fighter, *index, transition);
                        self.apply(graph, result);
                    }
                    self.action_controls(ui, fighter, graph);
                    // Every passing transition on an event runs, in list order, seeing earlier results.
                    ui.horizontal(|ui| {
                        let last = graph.transitions.len().saturating_sub(1);
                        if ui
                            .add_enabled(*index > 0, egui::Button::new("Run earlier").small())
                            .clicked()
                        {
                            let result =
                                graph_edit::reorder_transition(graph, fighter, *index, index - 1);
                            self.apply(graph, result);
                        }
                        if ui
                            .add_enabled(*index < last, egui::Button::new("Run later").small())
                            .clicked()
                        {
                            let result =
                                graph_edit::reorder_transition(graph, fighter, *index, index + 1);
                            self.apply(graph, result);
                        }
                    });
                    if ui.small_button("Remove").clicked() {
                        let result = graph_edit::delete_transition(graph, fighter, *index);
                        self.apply(graph, result);
                    }
                }
            }
            FieldRef::Action { transition, index } => {
                if let Some(action) = graph
                    .transitions
                    .get(*transition)
                    .and_then(|transition| transition.actions.get(*index))
                    .cloned()
                {
                    let managed = managed_timer_action(graph, &action);
                    let mut result = None;
                    if managed {
                        ui.weak(
                            "Managed timer action; edit Duration and Repeat in Quick controls.",
                        );
                    } else if let Some(mut edited) = editable_timer_action(graph, &action) {
                        if let Action::Set {
                            var,
                            value: Expr::Const(value),
                        } = &mut edited
                        {
                            ui.horizontal_wrapped(|ui| {
                                variable_picker(ui, "field_timer_target", graph, var, |kind| {
                                    kind == VarType::Timer
                                });
                                value_control(ui, value, VarType::Timer);
                            });
                        }
                        if edited != action {
                            result = Some(graph_edit::replace_action(
                                graph,
                                fighter,
                                *transition,
                                *index,
                                edited,
                            ));
                        }
                    } else if let Action::SetFlag { var, value } = &action {
                        let (mut var, mut value) = match &self.flag_condition_draft {
                            Some(FlagConditionDraft {
                                action: Action::SetFlag { var, value },
                                ..
                            }) => (var.clone(), value.clone()),
                            _ => (var.clone(), value.clone()),
                        };
                        variable_picker(ui, "field_flag_target", graph, &mut var, |kind| {
                            kind == VarType::Flag
                        });
                        crate::move_nodes::view::guard_value(
                            ui,
                            fighter,
                            &mut value,
                            &graph.vars,
                            0,
                        );
                        if let Ok(scopes) = super::behavior::EventReadScopes::checked(graph) {
                            ui.menu_button("Read incoming", |ui| {
                                for binding in &graph.emission_plan.event_bindings {
                                    for field in binding.fields.iter().filter(|field| {
                                        field.kind == VarType::Flag
                                            && scopes.field_kind(
                                                *transition,
                                                &binding.name,
                                                &field.name,
                                            ) == Ok(VarType::Flag)
                                    }) {
                                        if ui.button(&field.name).clicked() {
                                            value = Predicate::EventFlag {
                                                event: binding.name.clone(),
                                                field: field.name.clone(),
                                            };
                                            ui.close();
                                        }
                                    }
                                }
                            });
                        }
                        let edited = Action::SetFlag { var, value };
                        if edited != action {
                            match graph_edit::replace_action(
                                graph,
                                fighter,
                                *transition,
                                *index,
                                edited.clone(),
                            ) {
                                Ok(updated) => {
                                    self.flag_condition_draft = None;
                                    result = Some(Ok(updated));
                                }
                                Err(error) => {
                                    self.flag_condition_draft = Some(FlagConditionDraft {
                                        fighter: fighter.into(),
                                        reference: reference.clone(),
                                        baseline: graph.clone(),
                                        action: edited,
                                    });
                                    self.status.clear();
                                    ui.weak("Finish choosing this condition to update the draft.")
                                        .on_hover_text(error);
                                }
                            }
                        } else {
                            self.flag_condition_draft = None;
                        }
                    } else if let Ok(mut typed) = ControllerAction::try_from(&action) {
                        if super::editor::action_picker(
                            ui,
                            fighter,
                            graph,
                            &mut typed,
                            super::controller::extract_timer(graph).is_ok(),
                            super::value_edit::ValueScope::of_transition(graph, *transition),
                        ) {
                            result = Some(typed.to_ir(fighter).and_then(|edited| {
                                graph_edit::replace_action(
                                    graph,
                                    fighter,
                                    *transition,
                                    *index,
                                    edited,
                                )
                            }));
                        }
                    } else {
                        ui.weak(advanced_action_label(&action));
                    }
                    if !managed && ui.small_button("Remove action").clicked() {
                        result = Some(graph_edit::remove_action(
                            graph,
                            fighter,
                            *transition,
                            *index,
                        ));
                    }
                    if let Some(result) = result {
                        self.apply(graph, result);
                    }
                }
            }
            FieldRef::SourceBinding(source) => {
                match source {
                    SourceBindingRef::Hook(index) => {
                        if let Some(binding) = graph.emission_plan.bindings.get(*index).cloned() {
                            if matches!(
                                binding.hook,
                                HookName::OnArticleSpawn | HookName::OnArticleRemoved
                            ) {
                                let mut article = binding.article.clone();
                                egui::ComboBox::from_id_salt(("hook_article", *index))
                                    .selected_text(article.as_deref().map_or_else(
                                        || "Any article".into(),
                                        super::editor::article_label,
                                    ))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut article, None, "Any article");
                                        for entry in super::registry::entries_for(
                                            fighter,
                                            super::registry::IdentifierKind::Article,
                                        ) {
                                            ui.selectable_value(
                                                &mut article,
                                                Some(entry.symbol.into()),
                                                super::editor::article_label(entry.symbol),
                                            );
                                        }
                                    });
                                if article != binding.article {
                                    let mut updated = graph.clone();
                                    updated.emission_plan.bindings[*index].article = article;
                                    let result = graph_edit::validate_graph(&updated, fighter)
                                        .map(|()| updated);
                                    self.apply(graph, result);
                                }
                            }
                            ui.label(format!(
                                "Evaluates transitions: {}",
                                binding
                                    .transitions
                                    .iter()
                                    .map(|index| (index + 1).to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        }
                    }
                    SourceBindingRef::Event(index) => {
                        if let Some(binding) =
                            graph.emission_plan.event_bindings.get(*index).cloned()
                        {
                            // The rename buffer is shared with states; the prefix keeps the targets apart.
                            let target = format!("event:{}", binding.name);
                            if self.state_target != target {
                                self.state_target = target;
                                self.state_name = binding.name.clone();
                            }
                            ui.horizontal_wrapped(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.state_name)
                                        .desired_width(160.0),
                                );
                                if ui.button("Rename").clicked() {
                                    let result = graph_edit::rename_event(
                                        graph,
                                        fighter,
                                        *index,
                                        &self.state_name,
                                    );
                                    self.apply(graph, result);
                                }
                            });
                            ui.weak(format!(
                                "Runs {} transitions when a move sends this event. Wire a move's Emit event block here.",
                                binding.transitions.len()
                            ));
                        }
                    }
                    SourceBindingRef::Status(index) => {
                        self.status_source_ui(ui, fighter, graph, *index);
                    }
                    SourceBindingRef::Resource(index) => {
                        if let Some(source) = graph.emission_plan.resource_triggers.get(*index) {
                            ui.label(source.source.description());
                        }
                    }
                }
            }
            FieldRef::Expression { root, path } => {
                self.expression_inspector(ui, fighter, graph, root, path);
            }
            FieldRef::Predicate { transition, path } => {
                self.predicate_inspector(ui, fighter, graph, *transition, path);
            }
            _ => {
                ui.weak("This reference is unresolved. Validation notes explain what needs repair; the original controller is retained.");
            }
        }
        if !self.status.is_empty() {
            ui.weak(&self.status);
        }
        let changed = *graph != before;
        if changed {
            self.flag_condition_draft = None;
        }
        changed
    }

    fn expression_inspector(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        graph: &mut BehaviorGraph,
        root: &super::field_values::ValueRoot,
        path: &super::field_values::ExprPath,
    ) {
        use super::field_values;
        let Ok(original) = field_values::inspect_expression(graph, root, path).cloned() else {
            return;
        };
        let Ok(kind) = field_values::expression_kind(graph, root, path) else {
            return;
        };
        let mut value = original.clone();
        match &mut value {
            Expr::StatusFrame
            | Expr::MotionFrame
            | Expr::StickX { .. }
            | Expr::StickY
            | Expr::RandomInt { .. }
            | Expr::RandomFloat { .. } => {
                ui.weak(source_text(&value));
            }
            Expr::PendingDamage => {
                ui.weak("Contact power / pending damage")
                    .on_hover_text("Positive native units from this damage root.");
            }
            Expr::Const(number) => {
                value_control(ui, number, kind);
            }
            Expr::Var(name) => {
                variable_picker(ui, "field_formula_variable", graph, name, |candidate| {
                    candidate == kind
                });
            }
            Expr::TimerRemaining(name) => {
                variable_picker(ui, "field_remaining_timer", graph, name, |candidate| {
                    candidate == VarType::Timer
                });
                ui.weak("Remaining game ticks").on_hover_text("Integer countdown at this action's evaluation point. Does not tick, restart or transfer the timer.");
            }
            Expr::EventField { event, field } => {
                crate::move_nodes::view::event_read_ui(ui, event, field);
            }
            Expr::NativeFloat(source) => {
                crate::move_nodes::view::native_float_source_ui(ui, fighter, source);
            }
            Expr::NativeInt(NativeInput::ArticleCount { symbol, .. }) => {
                ui.label(format!("Live {}", super::editor::article_label(symbol)));
            }
            Expr::NativeInt(source) => {
                crate::move_nodes::view::native_source_ui(
                    ui,
                    fighter,
                    source,
                    super::native_inputs::NativeKind::Int,
                );
            }
            Expr::Add(..) | Expr::Sub(..) | Expr::Mul(..) => {
                ui.weak("Select a Left or Right card to edit an operand. Drag a numeric variable onto a Value port to replace that subtree.");
            }
        }
        let zero = || {
            Expr::Const(if kind == VarType::Int {
                ConstValue::Int(0)
            } else {
                ConstValue::Float(0.0)
            })
        };
        let one = || {
            Expr::Const(if kind == VarType::Int {
                ConstValue::Int(1)
            } else {
                ConstValue::Float(1.0)
            })
        };
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("Wrap value", |ui| {
                for (label, operator) in [("Add", 0), ("Subtract", 1), ("Multiply", 2)] {
                    if ui.button(label).clicked() {
                        value = match operator {
                            0 => Expr::Add(Box::new(original.clone()), Box::new(zero())),
                            1 => Expr::Sub(Box::new(original.clone()), Box::new(zero())),
                            _ => Expr::Mul(Box::new(original.clone()), Box::new(one())),
                        };
                        ui.close();
                    }
                }
            }).response.on_hover_text("Wrap the existing value intact. Add/Subtract use 0 and Multiply uses 1 until an operand is edited.");
            if ui.small_button("Use constant").clicked() { value = zero(); }
            if kind == VarType::Int && graph.vars.iter().any(|var| var.kind == VarType::Timer) {
                ui.menu_button("Read timer", |ui| {
                    for timer in graph.vars.iter().filter(|var| var.kind == VarType::Timer) {
                        if ui.button(&timer.name).on_hover_text("Remaining game ticks as an Integer. Does not advance, restart or transfer the timer.").clicked() {
                            value = Expr::TimerRemaining(timer.name.clone());
                            ui.close();
                        }
                    }
                });
            }
            let native_kind = if kind == VarType::Float { super::native_inputs::NativeKind::Float } else { super::native_inputs::NativeKind::Int };
            for source in super::native_inputs::NativeInput::catalog().iter().filter(|source| {
                source.valid_for(fighter) && source.kind() == Some(native_kind)
            }) {
                if ui.small_button(format!("Read {}", source.label())).on_hover_text(format!("{}\nRead-only native observation; not an authored timer, inferred capacity, or native-resource write.", source.symbol())).clicked() {
                    value = if kind == VarType::Float { Expr::NativeFloat(source.clone()) } else { Expr::NativeInt(source.clone()) };
                }
            }
            if !super::native_inputs::NativeInput::catalog_for(fighter, native_kind).is_empty() {
                egui::containers::menu::MenuButton::new("Read native value")
                    .config(egui::containers::menu::MenuConfig::new()
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
                    .ui(ui, |ui| {
                    let mut selected = match &original {
                        Expr::NativeFloat(source) | Expr::NativeInt(source) => source.clone(),
                        _ => super::native_inputs::NativeInput::Unsupported("unselected_native_value".into()),
                    };
                    let before = selected.clone();
                    crate::move_nodes::view::native_source_options_ui(ui, fighter, &mut selected, native_kind);
                    if selected != before {
                        value = if kind == VarType::Float { Expr::NativeFloat(selected) } else { Expr::NativeInt(selected) };
                    }
                });
            }
            if let Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) = &original {
                if ui.small_button("Keep left").clicked() { value = *left.clone(); }
                if ui.small_button("Keep right").clicked() { value = *right.clone(); }
            }
        });
        if value != original {
            let result =
                field_values::replace_numeric_expression(graph, fighter, root, path, value);
            self.apply(graph, result);
        }
    }

    fn predicate_inspector(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        graph: &mut BehaviorGraph,
        transition: usize,
        path: &super::field_values::PredicatePath,
    ) {
        use super::field_values;
        let Ok(original) = field_values::inspect_predicate(graph, transition, path).cloned() else {
            return;
        };
        let mut value = original.clone();
        match &mut value {
            Predicate::Flag(name) => {
                variable_picker(ui, "field_condition_flag", graph, name, |kind| {
                    kind == VarType::Flag
                })
            }
            Predicate::Expired(name) => {
                variable_picker(ui, "field_condition_timer", graph, name, |kind| {
                    kind == VarType::Timer
                })
            }
            Predicate::EventFlag { event, field } => {
                crate::move_nodes::view::event_read_ui(ui, event, field);
            }
            Predicate::NativeFlag(source) => {
                crate::move_nodes::view::native_source_ui(
                    ui,
                    fighter,
                    source,
                    super::native_inputs::NativeKind::Flag,
                );
            }
            Predicate::DamageStatusIs(source) => {
                crate::move_nodes::view::damage_status_source_ui(ui, fighter, source)
            }
            Predicate::Compare { op, .. } => {
                egui::ComboBox::from_id_salt("field_condition_compare")
                    .selected_text(op.to_string())
                    .show_ui(ui, |ui| {
                        for candidate in [CmpOp::Eq, CmpOp::Lt, CmpOp::Lte, CmpOp::Gt, CmpOp::Gte] {
                            ui.selectable_value(op, candidate, candidate.to_string());
                        }
                    });
                ui.weak("Select the comparison's number cards to edit Left and Right.");
            }
            _ => {
                ui.weak("Select a child condition to edit it. All requires both; Any requires either; Not inverts its child.");
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("Wrap condition", |ui| {
                for (label, operator) in [("All", 0), ("Any", 1), ("Not", 2)] {
                    if ui.button(label).clicked() {
                        value = match operator {
                            0 => Predicate::And(Box::new(original.clone()), Box::new(always())),
                            1 => Predicate::Or(Box::new(original.clone()), Box::new(Predicate::Not(Box::new(always())))),
                            _ => Predicate::Not(Box::new(original.clone())),
                        };
                        ui.close();
                    }
                }
            }).response.on_hover_text("Keep the existing subtree. All adds an initially true child; Any adds an initially false child.");
            egui::containers::menu::MenuButton::new("Replace with")
                .config(egui::containers::menu::MenuConfig::new()
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
                .ui(ui, |ui| {
                if ui.button("Always").clicked() { value = always(); ui.close(); }
                ui.menu_button("Status at contact", |ui| {
                    for source in super::native_inputs::NativeInput::catalog_for(fighter, super::native_inputs::NativeKind::Flag)
                        .into_iter().filter(|source| matches!(source, super::native_inputs::NativeInput::FighterStatusIs { .. })) {
                        if ui.button(crate::move_nodes::view::native_source_label(&source)).on_hover_text("Receiver status at native contact admission. Requires an owning deal/take root.").clicked() {
                            value = Predicate::DamageStatusIs(source);
                            ui.close();
                        }
                    }
                });
                let attack_flags = super::native_inputs::NativeInput::catalog_for(fighter, super::native_inputs::NativeKind::Flag)
                    .into_iter().filter(|source| matches!(source, super::native_inputs::NativeInput::AttackResultFlag { .. })).collect::<Vec<_>>();
                if !attack_flags.is_empty() {
                    ui.menu_button("Hit or shield check", |ui| {
                        for source in attack_flags {
                            if ui.button(source.label()).on_hover_text("Read this fighter's flag once at the triggering dispatch. Not a collision callback; no victim or projectile identity, deduplication or reset guarantee.").clicked() {
                                value = Predicate::NativeFlag(source);
                                ui.close();
                            }
                        }
                    });
                }
                let article_sources = super::native_inputs::NativeInput::catalog_for(fighter, super::native_inputs::NativeKind::Flag)
                    .into_iter().filter(|source| matches!(source, super::native_inputs::NativeInput::ArticleExists { .. })).collect::<Vec<_>>();
                if !article_sources.is_empty() {
                    ui.menu_button("Article exists", |ui| {
                        ui.set_max_width(230.0);
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        egui::ScrollArea::vertical().id_salt("article_presence_choices").max_height(220.0)
                            .show_rows(ui, ui.spacing().interact_size.y, article_sources.len(), |ui, rows| {
                                for source in &article_sources[rows] {
                                    if ui.button(crate::move_nodes::view::native_source_label(source))
                                        .on_hover_text(format!("{}\nPresence on the triggering fighter, read once per dispatch. Not a spawn or hit event, article count, instance identity or removal cause.", source.reference()))
                                        .clicked() {
                                        value = Predicate::NativeFlag(source.clone());
                                        ui.close();
                                    }
                                }
                            });
                    });
                }
                for source in super::native_inputs::NativeInput::catalog().iter().filter(|source| {
                    source.valid_for(fighter) && source.kind() == Some(super::native_inputs::NativeKind::Flag)
                }) {
                    if ui.button(format!("Read native value · {}", source.label())).on_hover_text(source.symbol()).clicked() { value = Predicate::NativeFlag(source.clone()); ui.close(); }
                }
                if !super::native_inputs::NativeInput::catalog_for(fighter, super::native_inputs::NativeKind::Flag).is_empty() {
                    egui::containers::menu::SubMenuButton::new("Read native value")
                        .config(egui::containers::menu::MenuConfig::new()
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
                        .ui(ui, |ui| {
                        let mut selected = match &original {
                            Predicate::NativeFlag(source) => source.clone(),
                            _ => super::native_inputs::NativeInput::Unsupported("unselected_native_flag".into()),
                        };
                        let before = selected.clone();
                        crate::move_nodes::view::native_source_options_ui(ui, fighter, &mut selected, super::native_inputs::NativeKind::Flag);
                        if selected != before { value = Predicate::NativeFlag(selected); }
                    });
                }
                for variable in &graph.vars {
                    let replacement = match variable.kind {
                        VarType::Flag => Predicate::Flag(variable.name.clone()),
                        VarType::Timer => Predicate::Expired(variable.name.clone()),
                        VarType::Int | VarType::Float => Predicate::Compare { op: CmpOp::Gte, left: Expr::Var(variable.name.clone()), right: Expr::Const(if variable.kind == VarType::Int { ConstValue::Int(0) } else { ConstValue::Float(0.0) }) },
                    };
                    if ui.button(format!("{:?} · {}", variable.kind, variable.name)).clicked() { value = replacement; ui.close(); }
                }
            });
            match &original {
                Predicate::Not(child) => { if ui.small_button("Keep child").clicked() { value = *child.clone(); } }
                Predicate::And(left, right) | Predicate::Or(left, right) => {
                    if ui.small_button("Keep left").clicked() { value = *left.clone(); }
                    if ui.small_button("Keep right").clicked() { value = *right.clone(); }
                }
                _ => {}
            }
        });
        if value != original {
            let result = field_values::replace_predicate(graph, fighter, transition, path, value);
            self.apply(graph, result);
        }
    }

    fn owned_state_ui(
        &mut self,
        ui: &mut Ui,
        fighter: &str,
        graph: &mut BehaviorGraph,
        name: &str,
    ) {
        use super::status_commands::Command;
        use super::status_model::{Phase, Policy};
        use super::status_recipes as recipes;
        let Some(binding) = graph
            .states
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.status.clone())
        else {
            return;
        };
        ui.weak("Export the plugin again after binding a status or adding a custom one.");
        if binding.policy == Policy::Owned {
            let (ground, air) = recipes::motions(graph, name);
            for (is_air, mut motion) in [(false, ground), (true, air)] {
                ui.horizontal(|ui| {
                    ui.label(if is_air {
                        "Animation · air"
                    } else {
                        "Animation · ground"
                    });
                    if is_air && motion.is_empty() {
                        if ui.small_button("Add air animation").clicked() {
                            let result =
                                recipes::insert(graph, fighter, name, recipes::Recipe::GroundAir);
                            self.apply(graph, result);
                        }
                        return;
                    }
                    let before = motion.clone();
                    ui.add(egui::TextEdit::singleline(&mut motion).desired_width(130.0));
                    if motion != before {
                        let result = recipes::edit_motion(graph, fighter, name, is_air, &motion);
                        self.apply(graph, result);
                    }
                });
            }
            let roots: Vec<_> = binding
                .roots
                .values()
                .flatten()
                .filter_map(|i| graph.transitions.get(*i).cloned())
                .collect();
            let mut movement = roots.iter().any(|r| {
                r.actions
                    .iter()
                    .any(|a| matches!(a, Action::Status(Command::ChangeKinetic { .. })))
            });
            let old = movement;
            ui.horizontal(|ui| {
                ui.label("Movement");
                egui::ComboBox::from_id_salt("status_movement")
                    .selected_text(if movement {
                        "Ground stop / air fall"
                    } else {
                        "Keep authored movement"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut movement, false, "Keep authored movement");
                        ui.selectable_value(&mut movement, true, "Ground stop / air fall");
                    });
            });
            if movement != old {
                let result = recipes::movement(graph, fighter, name, movement);
                self.apply(graph, result);
            }
            let mut cancel_frame = roots.iter().find_map(|r| {
                if r.actions
                    .iter()
                    .any(|a| matches!(a, Action::Status(Command::EnableCancel)))
                {
                    if let Predicate::Compare {
                        left: Expr::MotionFrame,
                        right: Expr::Const(ConstValue::Float(frame)),
                        ..
                    } = r.guard
                    {
                        return Some(frame);
                    }
                }
                None
            });
            let old = cancel_frame;
            ui.horizontal(|ui| {
                ui.label("Cancel window");
                let mut enabled = cancel_frame.is_some();
                if ui.checkbox(&mut enabled, "Allow after").changed() {
                    cancel_frame = enabled.then_some(20.0);
                }
                if let Some(frame) = &mut cancel_frame {
                    ui.add(
                        egui::DragValue::new(frame)
                            .range(0.0..=10000.0)
                            .suffix(" frames"),
                    );
                }
            });
            if old != cancel_frame {
                let result = recipes::cancel(graph, fighter, name, cancel_frame);
                self.apply(graph, result);
            }
            let mut finish = roots.iter().any(|r| r.actions.iter().any(|a|matches!(a,Action::Status(Command::ChangeStatusNow {symbol:super::status_model::StatusRef::Native(s)}) if s=="FIGHTER_STATUS_KIND_WAIT" || s=="FIGHTER_STATUS_KIND_FALL")));
            let old = finish;
            ui.horizontal(|ui| {
                ui.label("Finish");
                ui.checkbox(&mut finish, "Idle or fall when the animation ends");
            });
            if old != finish {
                let result = recipes::finish(graph, fighter, name, finish);
                self.apply(graph, result);
            }
            ui.menu_button("Insert recipe", |ui| {
                for recipe in recipes::Recipe::ALL {
                    if ui
                        .button(recipe.label())
                        .on_hover_text(recipe.summary())
                        .clicked()
                    {
                        let result = recipes::insert(graph, fighter, name, recipe);
                        if self.apply(graph, result) {
                            self.status = format!("{} added: {}", recipe.label(), recipe.summary());
                        }
                        ui.close();
                    }
                }
            });
            ui.weak("Recipes add ordinary rules. Select their blocks to customize them.");
        }
        ui.collapsing("Advanced phases", |ui| {
            let mut policy = binding.policy;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut policy, Policy::Native, "Game logic");
                ui.selectable_value(&mut policy, Policy::Owned, "Custom rules");
            });
            if policy != binding.policy {
                let mut updated = graph.clone();
                updated
                    .states
                    .iter_mut()
                    .find(|s| s.name == name)
                    .unwrap()
                    .status
                    .as_mut()
                    .unwrap()
                    .policy = policy;
                let result = graph_edit::validate_graph(&updated, fighter).map(|()| updated);
                self.apply(graph, result);
            }
            for phase in [
                Phase::Pre,
                Phase::Init,
                Phase::MainEnter,
                Phase::MainLoop,
                Phase::Exec,
                Phase::End,
                Phase::Exit,
            ] {
                ui.push_id(format!("{phase:?}"), |ui| {
                    ui.collapsing(format!("{phase:?}"), |ui| {
                        for &index in binding.roots.get(&phase).into_iter().flatten() {
                            if let Some(rule) = graph.transitions.get(index).cloned() {
                                ui.push_id(index, |ui| {
                                    let mut guard = rule.guard.clone();
                                    crate::move_nodes::view::guard_value(
                                        ui,
                                        fighter,
                                        &mut guard,
                                        &graph.vars,
                                        0,
                                    );
                                    if guard != rule.guard {
                                        let mut edited = rule.clone();
                                        edited.guard = guard;
                                        let result = graph_edit::replace_transition(
                                            graph, fighter, index, edited,
                                        );
                                        self.apply(graph, result);
                                    }
                                    self.transition = index;
                                    self.action_controls(ui, fighter, graph);
                                });
                            }
                        }
                        if ui.small_button("Add rule").clicked() {
                            let result = recipes::add_phase_rule(graph, fighter, name, phase);
                            self.apply(graph, result);
                        }
                        ui.weak("Phase rules are also editable as blocks in the workspace.");
                    });
                });
            }
        });
    }

    fn apply(&mut self, graph: &mut BehaviorGraph, result: Result<BehaviorGraph, String>) -> bool {
        match result {
            Ok(updated) => {
                *graph = updated;
                self.status.clear();
                true
            }
            Err(error) => {
                self.status = error;
                false
            }
        }
    }

    fn action_controls(&mut self, ui: &mut Ui, fighter: &str, graph: &mut BehaviorGraph) {
        let actions = graph.transitions[self.transition].actions.clone();
        let managed_timer = super::controller::extract_timer(graph).is_ok();
        let mut applied = None;
        for (index, action) in actions.iter().enumerate() {
            ui.push_id(("draft_action", self.transition, index), |ui| {
                    ui.group(|ui| {
                        let managed = managed_timer_action(graph, action);
                        if managed {
                            ui.weak("Managed timer action")
                                .on_hover_text("Use the saved controller's Duration and Repeat settings. Timer bookkeeping is preserved here.");
                        } else if let Some(mut timer_action) = editable_timer_action(graph, action) {
                            ui.horizontal_wrapped(|ui| {
                                if let Action::Set { var, value: Expr::Const(value) } = &mut timer_action {
                                    ui.label("Start timer");
                                    variable_picker(ui, "draft_timer_edit", graph, var, |kind| kind == VarType::Timer);
                                    value_control(ui, value, VarType::Timer);
                                }
                            });
                            if timer_action != *action {
                                applied = Some(graph_edit::replace_action(graph, fighter, self.transition, index, timer_action));
                            }
                        } else if let Ok(mut typed) = ControllerAction::try_from(action) {
                            if super::editor::action_picker(ui, fighter, graph, &mut typed, managed_timer, super::value_edit::ValueScope::of_transition(graph, self.transition)) {
                                applied = Some(typed.to_ir(fighter).and_then(|action| {
                                    graph_edit::replace_action(graph, fighter, self.transition, index, action)
                                }));
                            }
                        } else {
                            ui.weak(advanced_action_label(action));
                        }
                        if !managed {
                            ui.horizontal_wrapped(|ui| {
                                if ui.add_enabled(index > 0 && !managed_timer_action_at(graph, &actions, index.saturating_sub(1)), egui::Button::new("Up").small()).clicked() {
                                    applied = Some(graph_edit::reorder_action(graph, fighter, self.transition, index, index - 1));
                                }
                                if ui.add_enabled(index + 1 < actions.len() && !managed_timer_action_at(graph, &actions, index + 1), egui::Button::new("Down").small()).clicked() {
                                    applied = Some(graph_edit::reorder_action(graph, fighter, self.transition, index, index + 1));
                                }
                                if ui.small_button("Duplicate").clicked() {
                                    applied = Some(graph_edit::add_action(graph, fighter, self.transition, action.clone()).and_then(|updated| {
                                        let last = updated.transitions[self.transition].actions.len() - 1;
                                        graph_edit::reorder_action(&updated, fighter, self.transition, last, index + 1)
                                    }));
                                }
                                if ui.small_button("Remove action").clicked() {
                                    applied = Some(graph_edit::remove_action(graph, fighter, self.transition, index));
                                }
                            });
                        }
                    });
                });
        }
        if let Some(result) = applied.take() {
            self.apply(graph, result);
        }
        // Closed by default so the next action's picker is not mistaken for one already added.
        egui::CollapsingHeader::new("Add action")
            .id_salt(("add_action", self.transition))
            .show(ui, |ui| {
                let draft = self.action_draft.get_or_insert_with(|| {
                    // Start on something the rule can use right away: a variable write once one exists.
                    let kind = if graph.vars.is_empty() { ControllerActionKind::RequestFighterStatus } else { ControllerActionKind::Set };
                    super::editor::default_action(fighter, graph, kind, managed_timer)
                });
                ui.push_id("draft_action_add", |ui| {
                    super::editor::action_picker(ui, fighter, graph, draft, managed_timer, super::value_edit::ValueScope::of_transition(graph, self.transition));
                    let allowed = super::editor::action_available(fighter, graph, draft, managed_timer);
                    if ui
                        .add_enabled(allowed, egui::Button::new("Add"))
                        .clicked()
                    {
                        applied = Some(draft.to_ir(fighter).and_then(|action| {
                            graph_edit::add_action(graph, fighter, self.transition, action)
                        }));
                    }
                });
                if !managed_timer && graph.vars.iter().any(|var| var.kind == VarType::Flag) {
                    ui.menu_button("Set flag", |ui| {
                        for variable in graph.vars.iter().filter(|var| var.kind == VarType::Flag) {
                            if ui.button(&variable.name).clicked() {
                                applied = Some(graph_edit::add_action(graph, fighter, self.transition, Action::SetFlag {
                                    var: variable.name.clone(),
                                    value: Predicate::Compare { op: CmpOp::Lt, left: Expr::Const(ConstValue::Int(0)), right: Expr::Const(ConstValue::Int(0)) },
                                }));
                                ui.close();
                            }
                        }
                    }).response.on_hover_text("Assign a calculated condition synchronously. Select the new action to choose its condition, or connect a Flag or Timer condition.");
                }
                // A custom authored timer is not a native fighter resource. The reusable
                // timer's internal fields stay controlled by Duration/Repeat instead.
                if !managed_timer && graph.vars.iter().any(|var| var.kind == VarType::Timer) {
                    if self.timer_frames == 0 {
                        self.timer_frames = 60;
                    }
                    ui.horizontal_wrapped(|ui| {
                            variable_picker(ui, "draft_timer_target", graph, &mut self.timer_target, |kind| kind == VarType::Timer);
                            ui.add(egui::DragValue::new(&mut self.timer_frames).range(1..=1_000_000).suffix(" frames"));
                            let valid = graph.vars.iter().any(|var| var.kind == VarType::Timer && var.name == self.timer_target);
                            if ui.add_enabled(valid && self.timer_frames > 0, egui::Button::new("Add timer start"))
                                .on_hover_text("Starts the timer when this event runs. On an Every frame event it restarts every frame; add a condition for a one-time start.")
                                .clicked() {
                                applied = Some(graph_edit::add_action(graph, fighter, self.transition, Action::Set {
                                    var: self.timer_target.clone(), value: Expr::Const(ConstValue::Int(self.timer_frames)),
                                }));
                            }
                        });
                }
            });
        if let Some(result) = applied {
            self.apply(graph, result);
        }
    }
}

fn editable_timer_action(graph: &BehaviorGraph, action: &Action) -> Option<Action> {
    match action {
        Action::Set {
            var,
            value: Expr::Const(ConstValue::Int(_)),
        } if graph
            .vars
            .iter()
            .any(|variable| variable.name == *var && variable.kind == VarType::Timer) =>
        {
            Some(action.clone())
        }
        _ => None,
    }
}

fn managed_timer_action(graph: &BehaviorGraph, action: &Action) -> bool {
    if super::controller::extract_timer(graph).is_err() {
        return false;
    }
    match action {
        Action::Set { var, .. }
        | Action::SetFlag { var, .. }
        | Action::Add { var, .. }
        | Action::Raise { var }
        | Action::Clear { var }
        | Action::Reset { var } => ["remaining", "ready", "elapsed_events"].contains(&var.as_str()),
        _ => false,
    }
}

fn managed_timer_action_at(graph: &BehaviorGraph, actions: &[Action], index: usize) -> bool {
    actions
        .get(index)
        .is_some_and(|action| managed_timer_action(graph, action))
}

fn advanced_action_label(action: &Action) -> String {
    match action {
        Action::EmitEventArguments { .. } => {
            "Event payload · constant arguments preserved; read-only.".into()
        }
        Action::EmitEventValues { .. } => {
            "Calculated payload · value trees preserved; read-only here.".into()
        }
        Action::Reset { var } => format!("Reset {var} to its initial value"),
        Action::RequestFighterStatus { symbol } => format!(
            "Change status: {}",
            super::field_canvas::status_name(symbol)
        ),
        Action::RequestWeaponStatus { symbol, .. } => format!("Weapon status: {symbol}"),
        Action::Set { .. } | Action::Add { .. } | Action::Resource(_) => {
            "Formula · use Edit value blocks to edit its inputs.".into()
        }
        Action::SetFlag { .. } => "Condition · captured when this action runs.".into(),
        _ => "Native action · preserved until edited.".into(),
    }
}

fn state_picker(ui: &mut Ui, id: &str, graph: &BehaviorGraph, selected: &mut String) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(if selected.is_empty() {
            "Choose state…"
        } else {
            selected.as_str()
        })
        .show_ui(ui, |ui| {
            for state in &graph.states {
                ui.selectable_value(selected, state.name.clone(), &state.name);
            }
        });
}

fn variable_picker(
    ui: &mut Ui,
    id: &str,
    graph: &BehaviorGraph,
    selected: &mut String,
    accepts: impl Fn(VarType) -> bool,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(if selected.is_empty() {
            "Choose variable…"
        } else {
            selected.as_str()
        })
        .show_ui(ui, |ui| {
            for variable in graph.vars.iter().filter(|var| accepts(var.kind)) {
                ui.selectable_value(selected, variable.name.clone(), &variable.name);
            }
        });
}

fn value_control(ui: &mut Ui, value: &mut ConstValue, kind: VarType) {
    match value {
        ConstValue::Float(number) => {
            ui.add(
                egui::DragValue::new(number)
                    .speed(0.1)
                    .range(-1_000_000.0..=1_000_000.0),
            );
        }
        ConstValue::Int(number) => {
            ui.add(
                egui::DragValue::new(number).range(if kind == VarType::Timer {
                    0..=1_000_000
                } else {
                    -1_000_000..=1_000_000
                }),
            );
        }
        ConstValue::Flag(flag) => {
            ui.checkbox(flag, "Enabled");
        }
    }
}

pub(crate) fn hook_label(hook: HookName) -> &'static str {
    match hook {
        HookName::OnChargeTick => "Every frame",
        HookName::OnTimeout => "Timer expires",
        HookName::OnSwitch => "Switches form",
        HookName::OnStockStart => "Stock starts",
        HookName::OnKo => "Stock lost",
        HookName::OnArticleSpawn => "Its object appears",
        HookName::OnArticleRemoved => "Its object goes away",
        HookName::OnStatusEnter => "An action starts",
        HookName::OnStatusExit => "An action ends",
        HookName::OnDealDamage => "Deals damage",
        HookName::OnTakeDamage => "Takes damage",
        HookName::OnIncomingHit => "About to be hit",
        HookName::OnAttackShielded => "Attack shielded",
        HookName::OnShieldHit => "Shield hit",
        HookName::OnReflect => "Reflects projectile",
        HookName::OnAbsorb => "Absorbs projectile",
        HookName::OnGrab => "Grabs opponent",
        HookName::OnItemPickup => "Picks up item",
        HookName::OnItemThrown => "Releases item (throw / drop / remove)",
        HookName::OnThrow => "Releases throw",
        HookName::OnGrabbed => "Gets grabbed",
        _ => hook.name(),
    }
}

/// Readable text for the input and random value sources.
pub(crate) fn source_text(expr: &Expr) -> String {
    match expr {
        Expr::StickX {
            relative_to_facing: true,
        } => "Stick forward".into(),
        Expr::StatusFrame => "Status frame".into(),
        Expr::MotionFrame => "Motion frame".into(),
        Expr::StickX { .. } => "Stick right".into(),
        Expr::StickY => "Stick up".into(),
        Expr::RandomInt { min, max } => format!("Random {min}–{max}"),
        Expr::RandomFloat { min, max } => format!("Random {min}–{max}"),
        other => format!("{other:?}"),
    }
}

pub(crate) fn button_text(button: InputButton, edge: ButtonEdge) -> String {
    format!("{button:?} {}", format!("{edge:?}").to_lowercase())
}

/// Canvas title and detail for animation-speed and movement actions.
pub(crate) fn motion_text(action: &Action) -> (&'static str, String) {
    match action {
        Action::Status(c) => (
            c.label(),
            match c {
                super::status_commands::Command::PlayMotion { motion, .. } => motion.clone(),
                _ => String::new(),
            },
        ),
        Action::SetMotionRate { rate } => {
            ("Set animation speed", format!("× {}", value_text(rate)))
        }
        Action::Object { symbol, op } => (
            op.label(),
            match op {
                super::native_inputs::ObjectOp::SetSpeed { x, y } => format!(
                    "{} · x {} · y {}",
                    super::editor::article_label(symbol),
                    value_text(x),
                    value_text(y)
                ),
                _ => super::editor::article_label(symbol),
            },
        ),
        Action::AddSpeed { x, y } => (
            "Add speed",
            format!("forward {} · up {}", value_text(x), value_text(y)),
        ),
        Action::AddHeldOpponentDamage { amount } => {
            ("Add held opponent damage %", value_text(amount))
        }
        Action::ThrowHeldOpponent { direction } => {
            ("Throw held opponent now", format!("{direction:?}"))
        }
        Action::ReleaseHeldOpponent => ("Release held opponent", String::new()),
        Action::AddDamage { amount } => ("Add to damage %", value_text(amount)),
        Action::CombatPolicy { scope, op } => (
            "Combat policy",
            super::editor::combat_text(op, *scope, value_text),
        ),
        Action::CancelIncomingHit => ("Cancel incoming hit", String::new()),
        Action::Resource(command) => (
            "Change a meter",
            format!("{} {}", command.label(), value_text(&command.amount)),
        ),
        Action::ChooseHeroMenuSpell { slot } => ("Choose Hero menu spell", format!("slot {slot}")),
        Action::Movement(op) => (
            op.label(),
            match op {
                super::native_inputs::MovementAction::SetSpeed { x, y } => {
                    let axis = |v: &Option<f32>| v.map_or("keep".to_string(), |v| v.to_string());
                    format!("X {} · Y {}", axis(x), axis(y))
                }
                super::native_inputs::MovementAction::MoveBy { dx, dy } => {
                    format!("world X {dx} · Y {dy}")
                }
                super::native_inputs::MovementAction::SetGravity { enabled } => if *enabled {
                    "enabled"
                } else {
                    "disabled until status ends"
                }
                .into(),
                super::native_inputs::MovementAction::SetFacing(facing) => format!("{facing:?}"),
            },
        ),
        Action::Item(op) => item_text(op),
        Action::MeshVisibility { mesh, visible } => (
            if *visible { "Show mesh" } else { "Hide mesh" },
            mesh.clone(),
        ),
        _ => ("Action", String::new()),
    }
}

/// "ITEM_KIND_THUNDERSWORD" → "Thundersword".
pub(crate) fn item_name(symbol: &str) -> String {
    let name = symbol
        .strip_prefix("ITEM_KIND_")
        .unwrap_or(symbol)
        .replace('_', " ")
        .to_lowercase();
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

pub(crate) fn item_text(op: &super::native_inputs::ItemAction) -> (&'static str, String) {
    use super::native_inputs::ItemAction;
    match op {
        ItemAction::Hold { item } => ("Spawn item into hand", item_name(item)),
        ItemAction::Throw { angle, power } => ("Throw item", format!("{angle}° · power {power}")),
        ItemAction::Drop => ("Drop item", String::new()),
        ItemAction::Remove => ("Remove item", String::new()),
    }
}

/// Canvas title and detail for a game-value write.
pub(crate) fn native_write_text(action: &Action) -> (&'static str, String) {
    match action {
        Action::SetNative { input, value } => (
            "Set game value",
            format!("{} = {}", input.label(), value_text(value)),
        ),
        Action::AddNative { input, value } => (
            "Add to game value",
            format!("{} + {}", input.label(), value_text(value)),
        ),
        Action::SetNativeFlag { input, value } => (
            "Set game flag",
            format!(
                "{} {}",
                input.label(),
                if *value == never() { "off" } else { "on" }
            ),
        ),
        _ => ("Action", String::new()),
    }
}

fn value_text(value: &Expr) -> String {
    super::value_edit::expr_text(value)
}

/// The constant-false guard; `Not(never())` is constant true.
pub(crate) fn never() -> Predicate {
    Predicate::Compare {
        op: CmpOp::Eq,
        left: Expr::Const(ConstValue::Int(0)),
        right: Expr::Const(ConstValue::Int(1)),
    }
}

pub(crate) fn always() -> Predicate {
    Predicate::Compare {
        op: CmpOp::Eq,
        left: Expr::Const(ConstValue::Int(0)),
        right: Expr::Const(ConstValue::Int(0)),
    }
}

fn guarded_condition_ui(
    ui: &mut Ui,
    fighter: &str,
    graph: &BehaviorGraph,
    transition: usize,
    guard: &mut Predicate,
) {
    let managed = super::field_values::is_managed_timer_transition(graph, transition);
    // Input is sampled by the frame tick, so only transitions it runs alone may read it.
    let scope = super::value_edit::ValueScope::of_transition(graph, transition);
    ui.add_enabled_ui(!managed, |ui| {
        guard_control(ui, fighter, graph, guard, scope)
    });
    if managed {
        ui.weak("Timer lifecycle condition · use Duration and Repeat.");
    }
}

fn guard_control(
    ui: &mut Ui,
    fighter: &str,
    graph: &BehaviorGraph,
    guard: &mut Predicate,
    scope: super::value_edit::ValueScope,
) {
    use super::value_edit::{kind_of, value_editor, zero};
    let original = guard.clone();
    let mut kind = match &*guard {
        candidate if *candidate == always() => 0,
        Predicate::Flag(_) => 1,
        Predicate::Not(inner) if matches!(inner.as_ref(), Predicate::Flag(_)) => 2,
        Predicate::Expired(_) => 3,
        Predicate::Button { .. } => 5,
        Predicate::Compare {
            left: Expr::StickX { .. } | Expr::StickY,
            ..
        } => 6,
        Predicate::DamageIsProjectile => 7,
        Predicate::Not(inner) if **inner == Predicate::DamageIsProjectile => 8,
        Predicate::Compare {
            left: Expr::NativeInt(NativeInput::ArticleCount { .. }),
            ..
        } => 9,
        Predicate::InStatus { .. } => 10,
        Predicate::Situation {
            kind: SituationKind::Ground,
        } => 11,
        Predicate::Situation {
            kind: SituationKind::Air,
        } => 12,
        Predicate::Compare {
            left: Expr::StatusFrame,
            ..
        } => 13,
        Predicate::NativeFlag(super::native_inputs::NativeInput::Fighter {
            value: FighterValue::HoldingOpponent,
            ..
        }) => 14,
        Predicate::NativeFlag(super::native_inputs::NativeInput::Fighter {
            value: FighterValue::HoldingItem,
            ..
        }) => 15,
        Predicate::NativeFlag(super::native_inputs::NativeInput::ArticleDeflected { .. }) => 16,
        Predicate::InMotion { .. } => 17,
        Predicate::Compare {
            op: CmpOp::Eq,
            left:
                Expr::NativeInt(super::native_inputs::NativeInput::Fighter {
                    value: FighterValue::HeldItemKind,
                    ..
                }),
            right: Expr::Const(ConstValue::Int(_)),
        } => 18,
        Predicate::Compare { left, .. } if kind_of(graph, left).is_some() => 4,
        Predicate::NativeFlag(super::native_inputs::NativeInput::Command { .. }) => 19,
        _ => 20,
    };
    let old_kind = kind;
    let first_article =
        super::registry::entries_for(fighter, super::registry::IdentifierKind::Article).next();
    let labels = [
        "Always",
        "Flag enabled",
        "Flag disabled",
        "Timer expired",
        "Compare values",
        "Button",
        "Stick",
        "Hit by projectile",
        "Hit by fighter",
        "Article count",
        "In status",
        "On ground",
        "In air",
        "Frames in status",
        "Holding an opponent",
        "Holding an item",
        "Projectile reflected",
        "In animation",
        "Holding item kind",
        "Command input",
        "Advanced condition",
    ];
    ui.horizontal_wrapped(|ui| {
        ui.label("Only if");
        egui::ComboBox::from_id_salt("graph_guard_kind")
            .selected_text(labels[kind])
            .height(400.0)
            .show_ui(ui, |ui| {
                for (index, label) in labels[..20].iter().enumerate() {
                    let (available, why) = match index {
                        5 | 6 | 10..=15 | 17 => (scope.every_frame, "Read every frame. Use an Event · Every frame block."),
                        19 => (scope.every_frame && !super::native_inputs::command_symbols(fighter).is_empty(), "Command inputs need a fighter with command inputs and an Event · Every frame block."),
                        7 | 8 => (scope.damage, "Only damage events know what dealt the hit."),
                        9 => (first_article.is_some(), "This fighter has no articles."),
                        16 => (scope.every_frame && first_article.is_some(), "Read every frame, for a fighter with projectiles."),
                        _ => (true, ""),
                    };
                    ui.add_enabled_ui(available, |ui| {
                        ui.selectable_value(&mut kind, index, *label).on_disabled_hover_text(why);
                    });
                }
            });
    });
    if kind != old_kind {
        let variable = graph.vars.iter().find(|var| match kind {
            1 | 2 => var.kind == VarType::Flag,
            3 => var.kind == VarType::Timer,
            _ => false,
        });
        *guard = match (kind, variable) {
            (0, _) => always(),
            (10, _) => Predicate::InStatus {
                symbol: "FIGHTER_STATUS_KIND_SPECIAL_N".into(),
            },
            (11, _) => Predicate::Situation {
                kind: SituationKind::Ground,
            },
            (14, _) => Predicate::NativeFlag(super::native_inputs::NativeInput::Fighter {
                value: FighterValue::HoldingOpponent,
                fighter: fighter.into(),
            }),
            (19, _) => Predicate::NativeFlag(super::native_inputs::NativeInput::Command {
                fighter: fighter.into(),
                symbol: super::native_inputs::command_symbols(fighter)
                    .first()
                    .copied()
                    .unwrap_or_default()
                    .into(),
            }),
            (18, _) => Predicate::Compare {
                op: CmpOp::Eq,
                left: Expr::NativeInt(super::native_inputs::NativeInput::Fighter {
                    value: FighterValue::HeldItemKind,
                    fighter: fighter.into(),
                }),
                right: Expr::Const(ConstValue::Int(
                    super::native_inputs::ItemAction::item_kind("ITEM_KIND_BEAMSWORD").unwrap(),
                )),
            },
            (17, _) => Predicate::InMotion {
                motion: "throw_f".into(),
            },
            (15, _) => Predicate::NativeFlag(super::native_inputs::NativeInput::Fighter {
                value: FighterValue::HoldingItem,
                fighter: fighter.into(),
            }),
            (16, _) => Predicate::NativeFlag(super::native_inputs::NativeInput::ArticleDeflected {
                fighter: fighter.into(),
                symbol: first_article.map_or_else(String::new, |entry| entry.symbol.into()),
                absorbed: false,
            }),
            (12, _) => Predicate::Situation {
                kind: SituationKind::Air,
            },
            (13, _) => Predicate::Compare {
                op: CmpOp::Gte,
                left: Expr::StatusFrame,
                right: Expr::Const(ConstValue::Int(30)),
            },
            (9, _) => Predicate::Compare {
                op: CmpOp::Gte,
                left: Expr::NativeInt(NativeInput::ArticleCount {
                    fighter: fighter.into(),
                    symbol: first_article.map_or_else(String::new, |entry| entry.symbol.into()),
                }),
                right: Expr::Const(ConstValue::Int(1)),
            },
            (7, _) => Predicate::DamageIsProjectile,
            (8, _) => Predicate::Not(Box::new(Predicate::DamageIsProjectile)),
            (5, _) => Predicate::Button {
                button: InputButton::Attack,
                edge: ButtonEdge::Pressed,
            },
            (6, _) => Predicate::Compare {
                op: CmpOp::Gte,
                left: Expr::StickX {
                    relative_to_facing: true,
                },
                right: Expr::Const(ConstValue::Float(0.5)),
            },
            (1, Some(var)) => Predicate::Flag(var.name.clone()),
            (2, Some(var)) => Predicate::Not(Box::new(Predicate::Flag(var.name.clone()))),
            (3, Some(var)) => Predicate::Expired(var.name.clone()),
            (4, _) => {
                let left = graph
                    .vars
                    .iter()
                    .find(|var| matches!(var.kind, VarType::Int | VarType::Float))
                    .map_or(Expr::Const(ConstValue::Int(0)), |var| {
                        Expr::Var(var.name.clone())
                    });
                let right = zero(kind_of(graph, &left).unwrap_or(VarType::Int));
                Predicate::Compare {
                    op: CmpOp::Gte,
                    left,
                    right,
                }
            }
            _ => {
                ui.weak("Add a compatible variable first.");
                original.clone()
            }
        };
    }
    match guard {
        Predicate::Flag(name) => variable_picker(ui, "graph_guard_flag", graph, name, |kind| {
            kind == VarType::Flag
        }),
        Predicate::Not(inner) => {
            if let Predicate::Flag(name) = inner.as_mut() {
                variable_picker(ui, "graph_guard_not_flag", graph, name, |kind| {
                    kind == VarType::Flag
                });
            }
        }
        Predicate::Expired(name) => variable_picker(ui, "graph_guard_timer", graph, name, |kind| {
            kind == VarType::Timer
        }),
        Predicate::NativeFlag(super::native_inputs::NativeInput::Command { symbol, .. }) => {
            egui::ComboBox::from_id_salt("command_input")
                .selected_text(super::native_inputs::command_label(symbol))
                .show_ui(ui, |ui| {
                    for candidate in super::native_inputs::command_symbols(fighter) {
                        ui.selectable_value(
                            symbol,
                            (*candidate).into(),
                            super::native_inputs::command_label(candidate),
                        );
                    }
                });
        }
        Predicate::InStatus { symbol } => {
            super::registry::fighter_status_picker(ui, fighter, symbol)
        }
        Predicate::InMotion { motion } => {
            ui.add(
                egui::TextEdit::singleline(motion)
                    .desired_width(140.0)
                    .hint_text("throw_f"),
            )
            .on_hover_text("Animation name from the move list, e.g. throw_f, throw_hi, attack_11.");
            motion.retain(|c| c.is_ascii_alphanumeric() || c == '_');
            motion.make_ascii_lowercase();
        }
        Predicate::NativeFlag(super::native_inputs::NativeInput::ArticleDeflected {
            symbol,
            absorbed,
            ..
        }) => {
            ui.horizontal_wrapped(|ui| {
                super::editor::article_picker(ui, fighter, symbol);
                egui::ComboBox::from_id_salt("graph_guard_deflect")
                    .selected_text(if *absorbed { "absorbed" } else { "reflected" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(absorbed, false, "reflected");
                        ui.selectable_value(absorbed, true, "absorbed");
                    });
            });
        }
        Predicate::Button { button, edge } => {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("graph_guard_button")
                    .selected_text(format!("{button:?}"))
                    .show_ui(ui, |ui| {
                        for choice in [
                            InputButton::Attack,
                            InputButton::Special,
                            InputButton::Jump,
                            InputButton::Guard,
                            InputButton::Grab,
                        ] {
                            ui.selectable_value(button, choice, format!("{choice:?}"));
                        }
                    });
                egui::ComboBox::from_id_salt("graph_guard_edge")
                    .selected_text(format!("{edge:?}"))
                    .show_ui(ui, |ui| {
                        for choice in [ButtonEdge::Pressed, ButtonEdge::Held, ButtonEdge::Released]
                        {
                            ui.selectable_value(edge, choice, format!("{choice:?}"));
                        }
                    });
            });
        }
        Predicate::Compare {
            right: Expr::Const(ConstValue::Int(value)),
            ..
        } if kind == 18 => {
            let mut symbol = super::registry::ENTRIES
                .iter()
                .find(|e| {
                    e.kind == super::registry::IdentifierKind::Item
                        && i32::try_from(e.value).ok() == Some(*value)
                })
                .map_or_else(String::new, |e| e.symbol.to_owned());
            super::registry::item_picker(ui, &mut symbol);
            if let Some(id) = super::native_inputs::ItemAction::item_kind(&symbol) {
                *value = id;
            }
        }
        // "Always" is stored as 0 == 0; its operands are not something to edit.
        Predicate::Compare { op, left, right } if kind != 20 && kind != 0 => {
            let before = kind_of(graph, left);
            ui.push_id("compare_left", |ui| {
                value_editor(
                    ui,
                    fighter,
                    graph,
                    left,
                    before.unwrap_or(VarType::Int),
                    scope,
                )
            });
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("graph_guard_operator")
                    .width(36.0)
                    .selected_text(op.to_string())
                    .show_ui(ui, |ui| {
                        for candidate in [CmpOp::Eq, CmpOp::Lt, CmpOp::Lte, CmpOp::Gt, CmpOp::Gte] {
                            ui.selectable_value(op, candidate, candidate.to_string());
                        }
                    });
                let now = kind_of(graph, left).unwrap_or(VarType::Int);
                if Some(now) != before || kind_of(graph, right) != Some(now) {
                    *right = zero(now);
                }
                ui.push_id("compare_right", |ui| {
                    value_editor(ui, fighter, graph, right, now, scope)
                });
            });
        }
        _ => {
            if kind == 20 {
                ui.weak("This compound condition is retained. Select a simple condition to replace it, or use Advanced JSON.");
            }
        }
    }
}

pub(crate) fn phase_predicate_text(value: &Predicate) -> String {
    match value {
        Predicate::SituationChanged => "Situation changed".into(),
        Predicate::MotionEnded => "Motion ended".into(),
        Predicate::CancelEnabled => "Cancel enabled".into(),
        Predicate::TermEnabled { symbol } => format!(
            "{} enabled",
            symbol
                .replace("FIGHTER_STATUS_TRANSITION_TERM_ID_", "")
                .to_lowercase()
                .replace('_', " ")
        ),
        Predicate::InStatus { symbol } => {
            format!("In {}", super::field_canvas::status_name(symbol))
        }
        Predicate::InMotion { motion } => format!("Animation {motion}"),
        Predicate::Situation {
            kind: super::native_inputs::SituationKind::Ground,
        } => "On ground".into(),
        Predicate::Situation {
            kind: super::native_inputs::SituationKind::Air,
        } => "In air".into(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(width: f32, mut draw: impl FnMut(&mut Ui)) {
        let context = egui::Context::default();
        context.memory_mut(|memory| memory.set_everything_is_visible(true));
        for _ in 0..2 {
            let _ = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 1600.0),
                    )),
                    ..Default::default()
                },
                |ui| draw(ui),
            );
        }
    }

    fn text_position(shape: &egui::epaint::Shape, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.size() / 2.0)
            }
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().find_map(|shape| text_position(shape, label))
            }
            _ => None,
        }
    }

    #[test]
    fn field_native_menus_keep_search_open_until_explicit_numeric_or_flag_selection() {
        use super::super::{
            field_values::{ExprPath, PredicatePath, ValueRoot},
            native_inputs::NativeInput,
        };

        for (kind, fighter, query, label, source) in [
            (
                VarType::Float,
                "mario",
                "damage",
                "Damage value",
                NativeInput::Fighter {
                    value: FighterValue::Damage,
                    fighter: "mario".into(),
                },
            ),
            (
                VarType::Int,
                "cloud",
                "clear-frame",
                "Limit clear-frame value",
                NativeInput::CloudLimitClearFrame,
            ),
            (
                VarType::Flag,
                "mario",
                "fireball",
                "Fireball exists",
                NativeInput::ArticleExists {
                    fighter: "mario".into(),
                    symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
                },
            ),
        ] {
            let context = egui::Context::default();
            let mut view = GraphView::default();
            let mut graph = super::super::templates::Preset::Charge.graph();
            graph.vars.push(super::super::behavior::VarDef {
                keep_across_form_switch: false,
                name: "observed".into(),
                kind,
                initial: match kind {
                    VarType::Float => ConstValue::Float(0.0),
                    VarType::Flag => ConstValue::Flag(false),
                    _ => ConstValue::Int(0),
                },
            });
            if kind != VarType::Flag {
                graph.transitions[0].actions = vec![Action::Set {
                    var: "observed".into(),
                    value: super::super::value_edit::zero(kind),
                }];
            }
            let saved = graph.clone();
            let mut frame = |events: Vec<egui::Event>| {
                let output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(800.0, 800.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        if kind == VarType::Flag {
                            view.predicate_inspector(
                                ui,
                                fighter,
                                &mut graph,
                                0,
                                &PredicatePath::default(),
                            );
                        } else {
                            view.expression_inspector(
                                ui,
                                fighter,
                                &mut graph,
                                &ValueRoot::Action {
                                    transition: 0,
                                    index: 0,
                                },
                                &ExprPath::default(),
                            );
                        }
                    },
                );
                (output, graph.clone())
            };
            fn click(
                frame: &mut impl FnMut(Vec<egui::Event>) -> (egui::FullOutput, BehaviorGraph),
                label: &str,
            ) {
                frame(Vec::new());
                let (output, _) = frame(Vec::new());
                let pos = output
                    .shapes
                    .iter()
                    .find_map(|shape| text_position(&shape.shape, label))
                    .unwrap_or_else(|| panic!("missing native menu control {label}"));
                for pressed in [true, false] {
                    frame(vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]);
                }
            }
            if kind == VarType::Flag {
                click(&mut frame, "Replace with");
            }
            click(&mut frame, "Read native value");
            click(&mut frame, "Search sources…");
            frame(vec![egui::Event::Text(query.into())]);
            let (output, after_search) = frame(Vec::new());
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|shape| text_position(&shape.shape, label).is_some()),
                "search must keep the {kind:?} menu open"
            );
            assert_eq!(
                after_search, saved,
                "search must not select a {kind:?} source"
            );
            // The closure owns the graph borrow until after the selection.
            click(&mut frame, label);
            let (output, _) = frame(Vec::new());
            assert!(
                !output
                    .shapes
                    .iter()
                    .any(|shape| text_position(&shape.shape, query).is_some()),
                "explicit selection must close the source search"
            );
            let mut expected = saved;
            if kind == VarType::Flag {
                expected.transitions[0].guard = Predicate::NativeFlag(source);
            } else {
                expected.transitions[0].actions = vec![Action::Set {
                    var: "observed".into(),
                    value: if kind == VarType::Float {
                        Expr::NativeFloat(source)
                    } else {
                        Expr::NativeInt(source)
                    },
                }];
            }
            assert_eq!(
                graph, expected,
                "only explicit source selection may change the graph"
            );
            graph_edit::validate_graph(&graph, fighter).unwrap();
        }
    }

    #[test]
    fn failed_transaction_leaves_draft_unchanged() {
        let mut graph = super::super::templates::Preset::Charge.graph();
        let original = graph.clone();
        let mut view = GraphView::default();
        let result = graph_edit::remove_state(&graph, "mario", &graph.initial);
        assert!(!view.apply(&mut graph, result));
        assert_eq!(graph, original);
        assert!(!view.status.is_empty());
    }

    #[test]
    fn reusable_timer_bookkeeping_is_recognized_and_preserved() {
        let mut graph = super::super::templates::Preset::Timer.graph();
        assert!(super::super::controller::extract_timer(&graph).is_ok());
        for action in &graph.transitions[0].actions {
            assert!(managed_timer_action(&graph, action));
        }
        let article = Action::GenerateArticle {
            symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
        };
        assert!(!managed_timer_action(&graph, &article));
        let original = graph.clone();
        let mut view = GraphView::default();
        render(160.0, |ui| view.action_controls(ui, "mario", &mut graph));
        assert_eq!(graph, original);
        assert!(super::super::controller::extract_timer(&graph).is_ok());
    }
}
