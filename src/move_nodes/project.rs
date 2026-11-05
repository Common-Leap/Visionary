//! Transactional lowering of saved move graphs into the project's existing records.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::data::{AcmdScript, EditRecord};
use crate::mod_project::{FighterMod, ModProjectFile};

use super::model::{NodeError, Track};

pub(crate) const MAX_FIGHTER_MOVES: usize = crate::gimmicks::live_model::MAX_TRANSITIONS;

pub struct CompiledRecipes {
    pub behavior: crate::gimmicks::behavior::BehaviorGraph,
    pub tracks: BTreeMap<String, Vec<(Track, AcmdScript)>>,
    controller_receivers: ReceiverTables,
}

type ReceiverTables =
    BTreeMap<String, BTreeMap<String, Vec<crate::gimmicks::event_payload::FieldDef>>>;

#[derive(Clone, Copy)]
struct ReceiverValidationOptions {
    include_legacy_main_events: bool,
    require_canonical_typed_arguments: bool,
}

struct TypedEventValidation<'a> {
    fighter: &'a str,
    move_name: &'a str,
    track: Track,
    receiver_tables: &'a ReceiverTables,
    require_canonical_arguments: bool,
}

/// Validate the complete character draft before saving any recipe or changing live state.
/// A receiver edited in one move can invalidate a timed emitter in another move.
#[cfg(test)]
pub fn compile_recipes(
    fighter: &str,
    base: &crate::gimmicks::behavior::BehaviorGraph,
    graphs: &BTreeMap<String, super::model::MoveGraph>,
) -> Result<CompiledRecipes, Vec<NodeError>> {
    compile_recipes_with_controllers(fighter, base, &BTreeMap::new(), graphs)
}

/// Compile move recipes against the complete same-fighter receiver set. The
/// legacy wrapper above deliberately supplies no extra charts, preserving its
/// old main-only validation behavior.
pub fn compile_recipes_with_controllers(
    fighter: &str,
    base: &crate::gimmicks::behavior::BehaviorGraph,
    charts: &BTreeMap<String, crate::mod_project::ControllerChart>,
    graphs: &BTreeMap<String, super::model::MoveGraph>,
) -> Result<CompiledRecipes, Vec<NodeError>> {
    compile_recipes_with_controllers_internal(fighter, base, charts, graphs, false)
}

/// Return controller charts referenced by typed commands and conditions in a move graph.
/// The graph is bounded before its recursively stored move/ACMD nodes are inspected.
pub fn referenced_controller_charts(
    graph: &super::model::MoveGraph,
) -> Result<BTreeSet<String>, String> {
    super::block_clipboard::preflight_graph(graph)?;

    fn stmt(stmts: &[crate::data::AcmdStmt], out: &mut BTreeSet<String>) {
        for statement in stmts {
            match statement {
                crate::data::AcmdStmt::Excute(commands) => {
                    for command in commands {
                        if let Some(chart_id) = command_chart_id(command) {
                            out.insert(chart_id.to_owned());
                        }
                    }
                }
                crate::data::AcmdStmt::Bare(command) => {
                    if let Some(chart_id) = command_chart_id(command.as_ref()) {
                        out.insert(chart_id.to_owned());
                    }
                }
                crate::data::AcmdStmt::Loop { body, .. }
                | crate::data::AcmdStmt::RawBlock { body, .. } => stmt(body, out),
                _ => {}
            }
        }
    }

    fn condition(value: &super::model::MoveCondition, out: &mut BTreeSet<String>) {
        use super::model::MoveCondition;
        match value {
            MoveCondition::CharacterValue { chart_id, .. } => {
                out.insert(chart_id.clone());
            }
            MoveCondition::Not(inner) | MoveCondition::Group(inner) => condition(inner, out),
            MoveCondition::And { left, right } | MoveCondition::Or { left, right } => {
                condition(left, out);
                condition(right, out);
            }
            _ => {}
        }
    }

    fn nodes(items: &[super::model::MoveNode], out: &mut BTreeSet<String>) {
        for node in items {
            match &node.kind {
                super::model::NodeKind::BareCommand { command } => {
                    if let Some(chart_id) = command_chart_id(command.as_ref()) {
                        out.insert(chart_id.to_owned());
                    }
                }
                super::model::NodeKind::Execute { commands } => {
                    for command in commands {
                        if let Some(chart_id) = command_chart_id(command) {
                            out.insert(chart_id.to_owned());
                        }
                    }
                }
                super::model::NodeKind::MixedExecute { commands } => {
                    for command in commands {
                        let command = match command {
                            super::model::MoveCommand::Editable(command)
                            | super::model::MoveCommand::Preserved(command) => command,
                        };
                        if let Some(chart_id) = command_chart_id(command) {
                            out.insert(chart_id.to_owned());
                        }
                    }
                }
                super::model::NodeKind::Preserved { statement } => {
                    stmt(std::slice::from_ref(statement), out)
                }
                super::model::NodeKind::Repeat { body, .. } => nodes(body, out),
                super::model::NodeKind::Branch {
                    condition: value,
                    then_nodes,
                    else_nodes,
                } => {
                    condition(value, out);
                    nodes(then_nodes, out);
                    nodes(else_nodes, out);
                }
                _ => {}
            }
        }
    }

    let mut referenced = BTreeSet::new();
    for track in &graph.tracks {
        nodes(&track.nodes, &mut referenced);
        stmt(&track.baseline, &mut referenced);
    }
    Ok(referenced)
}

fn command_chart_id(command: &crate::data::ExcuteStmt) -> Option<&str> {
    match command {
        crate::data::ExcuteStmt::GimmickChartEvent { chart_id, .. }
        | crate::data::ExcuteStmt::GimmickChartEventValues { chart_id, .. } => Some(chart_id),
        _ => None,
    }
}

fn compile_recipes_with_controllers_internal(
    fighter: &str,
    base: &crate::gimmicks::behavior::BehaviorGraph,
    charts: &BTreeMap<String, crate::mod_project::ControllerChart>,
    graphs: &BTreeMap<String, super::model::MoveGraph>,
    persisted_chart_events_present: bool,
) -> Result<CompiledRecipes, Vec<NodeError>> {
    let mut errors = Vec::new();
    let mut tracks = BTreeMap::new();
    for (move_name, graph) in graphs {
        match super::timeline::compile(fighter, move_name, graph) {
            Ok(compiled) => {
                tracks.insert(move_name.clone(), compiled);
            }
            Err(track_errors) => errors.extend(track_errors),
        }
    }
    let behavior = match crate::gimmicks::controller_lower::compile(fighter, base) {
        Ok(behavior) => Some(behavior),
        Err(trigger_errors) => {
            errors.extend(trigger_errors);
            None
        }
    };
    let chart_event_source_present = tracks
        .values()
        .flatten()
        .any(|(_, script)| script_has_chart_event(script));
    let requires_compiled_candidate =
        !charts.is_empty() || chart_event_source_present || persisted_chart_events_present;
    let controller_receivers = if let Some(behavior) = &behavior {
        let receiver_result = if requires_compiled_candidate {
            compiled_controller_receivers(fighter, behavior, charts)
        } else {
            legacy_main_receivers(fighter, behavior)
        };
        match receiver_result {
            Ok(receivers) => Some(receivers),
            Err(error) => {
                errors.push(error);
                None
            }
        }
    } else {
        None
    };
    if let Some(behavior) = &behavior {
        for (move_name, scripts) in &tracks {
            for (track, script) in scripts {
                validate_character_value_conditions(
                    fighter,
                    move_name,
                    *track,
                    &script.stmts,
                    behavior,
                    charts,
                    &mut errors,
                );
            }
        }
    }
    if let Some((_behavior, receiver_tables)) = behavior.as_ref().zip(controller_receivers.as_ref())
    {
        for (move_name, scripts) in &mut tracks {
            for (track, script) in scripts {
                if let Err(receiver_errors) = validate_gimmick_event_receivers_with_tables(
                    fighter,
                    move_name,
                    *track,
                    script,
                    receiver_tables,
                    ReceiverValidationOptions {
                        include_legacy_main_events: true,
                        require_canonical_typed_arguments: false,
                    },
                ) {
                    errors.extend(receiver_errors);
                } else if let Err(receiver_errors) = canonicalize_chart_event_scripts(
                    fighter,
                    move_name,
                    *track,
                    script,
                    receiver_tables,
                ) {
                    errors.extend(receiver_errors);
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(CompiledRecipes {
            behavior: behavior.expect("successful trigger compilation"),
            tracks,
            controller_receivers: controller_receivers.unwrap_or_default(),
        })
    } else {
        Err(errors)
    }
}

fn validate_character_value_conditions(
    fighter: &str,
    move_name: &str,
    track: Track,
    statements: &[crate::data::AcmdStmt],
    main: &crate::gimmicks::behavior::BehaviorGraph,
    charts: &BTreeMap<String, crate::mod_project::ControllerChart>,
    errors: &mut Vec<NodeError>,
) {
    for statement in statements {
        match statement {
            crate::data::AcmdStmt::RawBlock { header, body } => {
                if let Some(condition) = super::timeline::import_condition(Some(fighter), header) {
                    if let Err(reason) = super::character_values::validate(&condition, main, charts)
                    {
                        errors.push(NodeError {
                            subject: format!("{fighter}/{move_name} {track:?} condition"),
                            reason,
                        });
                    }
                }
                validate_character_value_conditions(
                    fighter, move_name, track, body, main, charts, errors,
                );
            }
            crate::data::AcmdStmt::Loop { body, .. } => {
                validate_character_value_conditions(
                    fighter, move_name, track, body, main, charts, errors,
                );
            }
            _ => {}
        }
    }
}

/// Expand every saved move graph into ordinary project edits.
///
/// The input is never modified. Any invalid name, compiler diagnostic, or collision rejects the
/// whole expansion; callers can therefore persist or export the returned value atomically.
pub fn expand(project: &ModProjectFile) -> Result<ModProjectFile, Vec<NodeError>> {
    let mut errors = Vec::new();

    validate_structured_fighter_identities(project, &mut errors);
    for (fighter, data) in &project.fighters {
        if let Err(reason) =
            crate::gimmicks::controller_project::preflight(&data.behavior, &data.controller_charts)
        {
            errors.push(NodeError {
                subject: format!(
                    "fighter {} controllers",
                    fighter.chars().take(64).collect::<String>()
                ),
                reason,
            });
        }
        if let Err(reason) = crate::gimmicks::behavior::preflight_bounds(&data.behavior) {
            errors.push(NodeError {
                subject: format!(
                    "fighter {} behavior",
                    fighter.chars().take(64).collect::<String>()
                ),
                reason,
            });
        }
        for (move_name, graph) in &data.move_graphs {
            if let Err(reason) = super::block_clipboard::preflight_graph(graph) {
                errors.push(NodeError {
                    subject: format!(
                        "fighter {} move {}",
                        fighter.chars().take(64).collect::<String>(),
                        move_name.chars().take(64).collect::<String>()
                    ),
                    reason,
                });
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut expanded = project.clone();

    let mut fighter_names = project.fighters.keys().collect::<Vec<_>>();
    fighter_names.sort();

    for fighter_name in fighter_names {
        let Some(source_fighter) = project.fighters.get(fighter_name) else {
            continue;
        };
        if source_fighter.move_graphs.is_empty() {
            if has_persisted_chart_event(source_fighter) {
                match compiled_controller_receivers(
                    fighter_name,
                    &source_fighter.behavior,
                    &source_fighter.controller_charts,
                ) {
                    Ok(receivers) => errors.extend(validate_persisted_chart_event_scripts(
                        fighter_name,
                        source_fighter,
                        &receivers,
                    )),
                    Err(error) => errors.push(error),
                }
            }
            continue;
        }
        let fighter_error_start = errors.len();

        validate_identifier(fighter_name, "fighter", &mut errors);
        let mut move_names = source_fighter.move_graphs.keys().collect::<Vec<_>>();
        move_names.sort();
        for move_name in &move_names {
            validate_identifier(
                move_name,
                &format!("fighter {fighter_name} move"),
                &mut errors,
            );
        }

        if errors.len() != fighter_error_start {
            continue;
        }

        let Some(output_fighter) = expanded.fighters.get_mut(fighter_name) else {
            continue;
        };

        let compiled = match compile_recipes_with_controllers_internal(
            fighter_name,
            &source_fighter.behavior,
            &source_fighter.controller_charts,
            &source_fighter.move_graphs,
            has_persisted_chart_event(source_fighter),
        ) {
            Ok(compiled) => compiled,
            Err(compile_errors) => {
                errors.extend(compile_errors);
                continue;
            }
        };
        errors.extend(validate_persisted_chart_event_scripts(
            fighter_name,
            source_fighter,
            &compiled.controller_receivers,
        ));
        if errors.len() != fighter_error_start {
            continue;
        }
        output_fighter.behavior = compiled.behavior;
        if let Err(reason) = crate::gimmicks::controller_project::preflight(
            &output_fighter.behavior,
            &output_fighter.controller_charts,
        ) {
            errors.push(NodeError {
                subject: format!("fighter {fighter_name} controllers"),
                reason,
            });
            continue;
        }

        for (move_name, tracks) in compiled.tracks {
            let mut generated_categories = BTreeSet::new();
            for (track, script) in tracks {
                let category = match track {
                    Track::Game => "game",
                    Track::Sound => "sound",
                    Track::Expression => "expression",
                    Track::Effect => "effect",
                };
                if !generated_categories.insert(category) {
                    errors.push(NodeError {
                        subject: format!("fighter {fighter_name} move {move_name}"),
                        reason: format!("compiler produced more than one {category} track"),
                    });
                    continue;
                }
                merge_track(
                    output_fighter,
                    fighter_name,
                    &move_name,
                    track,
                    script,
                    &mut errors,
                );
            }
        }

        output_fighter.move_graphs.clear();
    }

    if errors.is_empty() {
        Ok(expanded)
    } else {
        Err(errors)
    }
}

/// Validate every timed named-event receiver in a compiled ACMD script against the compiled
/// character behavior's event bindings. Recurses through preserved control-flow blocks and
/// loops so nested source cannot bypass the receiver check.
///
/// Call this after behavior compilation and before applying/committing generated move scripts.
/// The helper is read-only and does not add producers or alter source/runtime semantics.
#[cfg(test)]
pub fn validate_gimmick_event_receivers(
    fighter: &str,
    move_name: &str,
    track: Track,
    script: &AcmdScript,
    compiled_behavior: &crate::gimmicks::behavior::BehaviorGraph,
) -> Result<(), Vec<NodeError>> {
    validate_gimmick_event_receivers_with_controllers(
        fighter,
        move_name,
        track,
        script,
        compiled_behavior,
        &BTreeMap::new(),
    )
}

/// Validate main-target and explicitly chart-targeted events against exactly
/// one graph for this fighter. An additional-chart command cannot use `main`
/// or search another owner/chart for a matching event name.
#[cfg(test)]
pub fn validate_gimmick_event_receivers_with_controllers(
    fighter: &str,
    move_name: &str,
    track: Track,
    script: &AcmdScript,
    compiled_behavior: &crate::gimmicks::behavior::BehaviorGraph,
    charts: &BTreeMap<String, crate::mod_project::ControllerChart>,
) -> Result<(), Vec<NodeError>> {
    let receiver_tables = if charts.is_empty() && !script_has_chart_event(script) {
        legacy_main_receivers(fighter, compiled_behavior)
    } else {
        compiled_controller_receivers(fighter, compiled_behavior, charts)
    }
    .map_err(|error| vec![error])?;
    validate_gimmick_event_receivers_with_tables(
        fighter,
        move_name,
        track,
        script,
        &receiver_tables,
        ReceiverValidationOptions {
            include_legacy_main_events: true,
            require_canonical_typed_arguments: false,
        },
    )
}

fn legacy_main_receivers(
    fighter: &str,
    graph: &crate::gimmicks::behavior::BehaviorGraph,
) -> Result<ReceiverTables, NodeError> {
    let subject = format!("fighter {fighter} behavior");
    if graph.emission_plan.event_bindings.len() > crate::gimmicks::codegen::MAX_EVENT_BINDINGS {
        return Err(NodeError {
            subject,
            reason: "named event receiver table exceeds its binding limit".into(),
        });
    }
    let mut receivers = BTreeMap::new();
    for binding in &graph.emission_plan.event_bindings {
        if crate::gimmicks::event_payload::validate_schema(fighter, &binding.name, &binding.fields)
            .is_err()
            || receivers
                .insert(binding.name.clone(), binding.fields.clone())
                .is_some()
        {
            return Err(NodeError {
                subject,
                reason: "named event receiver has an invalid schema or duplicate binding".into(),
            });
        }
    }
    Ok(BTreeMap::from([(
        crate::gimmicks::controller_project::MAIN_CHART.to_owned(),
        receivers,
    )]))
}

fn compiled_controller_receivers(
    fighter: &str,
    main: &crate::gimmicks::behavior::BehaviorGraph,
    charts: &BTreeMap<String, crate::mod_project::ControllerChart>,
) -> Result<ReceiverTables, NodeError> {
    let subject = format!("fighter {fighter} controllers");
    let mut receiver_tables = BTreeMap::new();
    receiver_tables.insert(
        crate::gimmicks::controller_project::MAIN_CHART.to_owned(),
        BTreeMap::new(),
    );
    if main.is_empty() && charts.is_empty() {
        return Ok(receiver_tables);
    }
    let programs =
        crate::gimmicks::controller_project::compile(fighter, main, charts).map_err(|reason| {
            NodeError {
                subject: subject.clone(),
                reason,
            }
        })?;
    for record in programs {
        let mut receivers = BTreeMap::new();
        for binding in &record.program.event_bindings {
            if receivers
                .insert(
                    binding.name.clone(),
                    binding
                        .fields
                        .iter()
                        .map(|field| crate::gimmicks::event_payload::FieldDef {
                            name: field.name.clone(),
                            kind: match field.kind {
                                crate::gimmicks::live_model::VarKind::Float => {
                                    crate::gimmicks::behavior::VarType::Float
                                }
                                crate::gimmicks::live_model::VarKind::Int => {
                                    crate::gimmicks::behavior::VarType::Int
                                }
                                crate::gimmicks::live_model::VarKind::Flag => {
                                    crate::gimmicks::behavior::VarType::Flag
                                }
                                crate::gimmicks::live_model::VarKind::Timer => {
                                    crate::gimmicks::behavior::VarType::Timer
                                }
                            },
                        })
                        .collect(),
                )
                .is_some()
            {
                return Err(NodeError {
                    subject,
                    reason: format!(
                        "controller chart {:?} contains a duplicate compiled event binding",
                        record.chart_id
                    ),
                });
            }
        }
        receiver_tables.insert(record.chart_id, receivers);
    }
    Ok(receiver_tables)
}

fn validate_persisted_chart_event_scripts(
    fighter: &str,
    data: &FighterMod,
    receiver_tables: &ReceiverTables,
) -> Vec<NodeError> {
    let mut errors = Vec::new();
    for (move_name, record) in &data.acmd {
        if script_has_chart_event(&record.script) {
            if let Err(found) = validate_gimmick_event_receivers_with_tables(
                fighter,
                move_name,
                Track::Game,
                &record.script,
                receiver_tables,
                ReceiverValidationOptions {
                    include_legacy_main_events: false,
                    require_canonical_typed_arguments: true,
                },
            ) {
                errors.extend(found);
            }
        }
    }
    for (move_name, script) in &data.sound_scripts {
        if script_has_chart_event(script) {
            if let Err(found) = validate_gimmick_event_receivers_with_tables(
                fighter,
                move_name,
                Track::Sound,
                script,
                receiver_tables,
                ReceiverValidationOptions {
                    include_legacy_main_events: false,
                    require_canonical_typed_arguments: true,
                },
            ) {
                errors.extend(found);
            }
        }
    }
    for (move_name, script) in &data.expression_scripts {
        if script_has_chart_event(script) {
            if let Err(found) = validate_gimmick_event_receivers_with_tables(
                fighter,
                move_name,
                Track::Expression,
                script,
                receiver_tables,
                ReceiverValidationOptions {
                    include_legacy_main_events: false,
                    require_canonical_typed_arguments: true,
                },
            ) {
                errors.extend(found);
            }
        }
    }
    errors
}

fn has_persisted_chart_event(data: &FighterMod) -> bool {
    data.acmd
        .values()
        .any(|record| script_has_chart_event(&record.script))
        || data.sound_scripts.values().any(script_has_chart_event)
        || data.expression_scripts.values().any(script_has_chart_event)
}

fn script_has_chart_event(script: &AcmdScript) -> bool {
    script.stmts.iter().any(statement_has_chart_event)
}

fn statement_has_chart_event(statement: &crate::data::AcmdStmt) -> bool {
    match statement {
        crate::data::AcmdStmt::Excute(commands) => commands.iter().any(|command| {
            matches!(
                command,
                crate::data::ExcuteStmt::GimmickChartEvent { .. }
                    | crate::data::ExcuteStmt::GimmickChartEventValues { .. }
            )
        }),
        crate::data::AcmdStmt::Bare(command) => {
            matches!(
                command.as_ref(),
                crate::data::ExcuteStmt::GimmickChartEvent { .. }
                    | crate::data::ExcuteStmt::GimmickChartEventValues { .. }
            )
        }
        crate::data::AcmdStmt::RawBlock { body, .. } | crate::data::AcmdStmt::Loop { body, .. } => {
            body.iter().any(statement_has_chart_event)
        }
        _ => false,
    }
}

fn validate_gimmick_event_receivers_with_tables(
    fighter: &str,
    move_name: &str,
    track: Track,
    script: &AcmdScript,
    receiver_tables: &ReceiverTables,
    options: ReceiverValidationOptions,
) -> Result<(), Vec<NodeError>> {
    let mut errors = Vec::new();
    collect_gimmick_event_receiver_errors(
        fighter,
        move_name,
        track,
        &script.stmts,
        receiver_tables,
        options,
        &mut errors,
    );
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn valid_controller_chart_id(chart_id: &str) -> bool {
    chart_id != crate::gimmicks::controller_project::MAIN_CHART
        && !chart_id.trim().is_empty()
        && chart_id.len() <= crate::gimmicks::controller_protocol::MAX_CHART_ID_BYTES
        && !chart_id.as_bytes().contains(&0)
}

fn collect_gimmick_event_receiver_errors(
    fighter: &str,
    move_name: &str,
    track: Track,
    statements: &[crate::data::AcmdStmt],
    receiver_tables: &ReceiverTables,
    options: ReceiverValidationOptions,
    errors: &mut Vec<NodeError>,
) {
    for statement in statements {
        match statement {
            crate::data::AcmdStmt::Excute(commands) => {
                for command in commands {
                    match command {
                        crate::data::ExcuteStmt::GimmickEvent { name }
                            if options.include_legacy_main_events =>
                        {
                            validate_event_target(
                                fighter,
                                move_name,
                                track,
                                name,
                                crate::gimmicks::controller_project::MAIN_CHART,
                                receiver_tables,
                                errors,
                            );
                        }
                        crate::data::ExcuteStmt::GimmickEvent { .. } => {}
                        crate::data::ExcuteStmt::GimmickChartEvent { chart_id, name } => {
                            if !valid_controller_chart_id(chart_id) {
                                errors.push(gimmick_event_error(
                                    fighter, move_name, track, name,
                                    "target chart ID must be an exact non-main controller chart ID within the supported byte limit",
                                ));
                            } else {
                                validate_event_target(
                                    fighter,
                                    move_name,
                                    track,
                                    name,
                                    chart_id,
                                    receiver_tables,
                                    errors,
                                );
                            }
                        }
                        crate::data::ExcuteStmt::GimmickChartEventValues {
                            chart_id,
                            name,
                            arguments,
                        } => {
                            validate_typed_event_target(
                                TypedEventValidation {
                                    fighter,
                                    move_name,
                                    track,
                                    receiver_tables,
                                    require_canonical_arguments: options
                                        .require_canonical_typed_arguments,
                                },
                                name,
                                chart_id,
                                arguments,
                                errors,
                            );
                        }
                        _ => {}
                    }
                }
            }
            crate::data::AcmdStmt::Bare(command) => match command.as_ref() {
                crate::data::ExcuteStmt::GimmickEvent { name }
                    if options.include_legacy_main_events =>
                {
                    errors.push(gimmick_event_error(
                        fighter,
                        move_name,
                        track,
                        name,
                        "named event receivers must be inside a timed execute block",
                    ));
                }
                crate::data::ExcuteStmt::GimmickChartEvent { name, .. }
                | crate::data::ExcuteStmt::GimmickChartEventValues { name, .. } => {
                    errors.push(gimmick_event_error(
                        fighter,
                        move_name,
                        track,
                        name,
                        "named event receivers must be inside a timed execute block",
                    ));
                }
                _ => {}
            },
            crate::data::AcmdStmt::RawBlock { body, .. }
            | crate::data::AcmdStmt::Loop { body, .. } => collect_gimmick_event_receiver_errors(
                fighter,
                move_name,
                track,
                body,
                receiver_tables,
                options,
                errors,
            ),
            _ => {}
        }
    }
}

fn validate_event_target(
    fighter: &str,
    move_name: &str,
    track: Track,
    name: &str,
    chart_id: &str,
    receiver_tables: &ReceiverTables,
    errors: &mut Vec<NodeError>,
) {
    if track != Track::Game {
        errors.push(gimmick_event_error(
            fighter,
            move_name,
            track,
            name,
            "named event receivers are valid only on the Game track",
        ));
        return;
    }
    if !valid_gimmick_event_name(name) {
        errors.push(gimmick_event_error(
            fighter,
            move_name,
            track,
            name,
            "event name must be non-empty, NUL-free, and within the supported byte limit",
        ));
        return;
    }
    let Some(receivers) = receiver_tables.get(chart_id) else {
        errors.push(gimmick_event_error(
            fighter,
            move_name,
            track,
            name,
            &format!("target controller chart {chart_id:?} does not exist for this fighter"),
        ));
        return;
    };
    if receivers.get(name).is_some_and(|fields| !fields.is_empty()) {
        errors.push(gimmick_event_error(
            fighter,
            move_name,
            track,
            name,
            "zero-argument timed events cannot target a receiver that requires payload fields",
        ));
    } else if !receivers.contains_key(name) {
        let reason = if chart_id == crate::gimmicks::controller_project::MAIN_CHART {
            "no matching compiled event binding".to_owned()
        } else {
            format!("no matching compiled event binding exists in chart {chart_id:?}")
        };
        errors.push(gimmick_event_error(
            fighter, move_name, track, name, &reason,
        ));
    }
}

fn validate_typed_event_target(
    context: TypedEventValidation<'_>,
    name: &str,
    chart_id: &str,
    arguments: &[crate::gimmicks::event_payload::Argument],
    errors: &mut Vec<NodeError>,
) {
    let TypedEventValidation {
        fighter,
        move_name,
        track,
        receiver_tables,
        require_canonical_arguments,
    } = context;
    if track != Track::Game {
        errors.push(gimmick_event_error(
            fighter,
            move_name,
            track,
            name,
            "typed named event receivers are valid only on the Game track",
        ));
        return;
    }
    if !valid_controller_chart_id(chart_id) {
        errors.push(gimmick_event_error(fighter, move_name, track, name,
            "target chart ID must be an exact non-main controller chart ID within the supported byte limit"));
        return;
    }
    if !valid_gimmick_event_name(name) {
        errors.push(gimmick_event_error(
            fighter,
            move_name,
            track,
            name,
            "event name must be non-empty, NUL-free, and within the supported byte limit",
        ));
        return;
    }
    let Some(receivers) = receiver_tables.get(chart_id) else {
        errors.push(gimmick_event_error(
            fighter,
            move_name,
            track,
            name,
            &format!("target controller chart {chart_id:?} does not exist for this fighter"),
        ));
        return;
    };
    let Some(fields) = receivers.get(name) else {
        errors.push(gimmick_event_error(
            fighter,
            move_name,
            track,
            name,
            &format!("no matching compiled event binding exists in chart {chart_id:?}"),
        ));
        return;
    };
    if fields.is_empty() {
        errors.push(gimmick_event_error(
            fighter,
            move_name,
            track,
            name,
            "typed timed events require a receiver with one or more payload fields",
        ));
        return;
    }
    match crate::gimmicks::event_payload::canonicalize_arguments(fighter, name, fields, arguments) {
        Ok(_)
            if require_canonical_arguments
                && arguments
                    .iter()
                    .map(|argument| argument.field.as_str())
                    .ne(fields.iter().map(|field| field.name.as_str())) =>
        {
            errors.push(gimmick_event_error(
                fighter,
                move_name,
                track,
                name,
                "persisted typed event arguments must follow the receiver declaration order",
            ));
        }
        Ok(_) => {}
        Err(found) => {
            errors.extend(found.into_iter().map(|error| {
                gimmick_event_error(fighter, move_name, track, name, &error.to_string())
            }))
        }
    }
}

fn canonicalize_chart_event_scripts(
    fighter: &str,
    move_name: &str,
    track: Track,
    script: &mut AcmdScript,
    receiver_tables: &ReceiverTables,
) -> Result<(), Vec<NodeError>> {
    fn statements(
        fighter: &str,
        move_name: &str,
        track: Track,
        body: &mut [crate::data::AcmdStmt],
        receiver_tables: &ReceiverTables,
        errors: &mut Vec<NodeError>,
    ) {
        for statement in body {
            match statement {
                crate::data::AcmdStmt::Excute(commands) => {
                    for command in commands {
                        if let crate::data::ExcuteStmt::GimmickChartEventValues {
                            chart_id,
                            name,
                            arguments,
                        } = command
                        {
                            if let Some(fields) = receiver_tables
                                .get(chart_id)
                                .and_then(|table| table.get(name))
                            {
                                match crate::gimmicks::event_payload::canonicalize_arguments(
                                    fighter, name, fields, arguments,
                                ) {
                                    Ok(values) => {
                                        *arguments = fields
                                            .iter()
                                            .zip(values)
                                            .map(|(field, value)| {
                                                crate::gimmicks::event_payload::Argument {
                                                    field: field.name.clone(),
                                                    value,
                                                }
                                            })
                                            .collect();
                                    }
                                    Err(found) => errors.extend(found.into_iter().map(|error| {
                                        gimmick_event_error(
                                            fighter,
                                            move_name,
                                            track,
                                            name,
                                            &error.to_string(),
                                        )
                                    })),
                                }
                            }
                        }
                    }
                }
                crate::data::AcmdStmt::RawBlock { body, .. }
                | crate::data::AcmdStmt::Loop { body, .. } => {
                    statements(fighter, move_name, track, body, receiver_tables, errors)
                }
                _ => {}
            }
        }
    }
    let mut errors = Vec::new();
    statements(
        fighter,
        move_name,
        track,
        &mut script.stmts,
        receiver_tables,
        &mut errors,
    );
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn gimmick_event_error(
    fighter: &str,
    move_name: &str,
    track: Track,
    name: &str,
    reason: &str,
) -> NodeError {
    NodeError {
        subject: format!("fighter {fighter} move {move_name} {track:?}"),
        reason: format!(
            "gimmick event receiver {:?}: {reason}",
            name.chars()
                .take(crate::gimmicks::live_model::MAX_NAME_BYTES)
                .collect::<String>()
        ),
    }
}

fn valid_gimmick_event_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.len() <= crate::gimmicks::live_model::MAX_NAME_BYTES
        && !name.contains('\0')
}

/// Structured move/effect output is attached to roster identities, not display labels. Validate
/// those identities globally before compiling or merging anything so case-insensitive roster
/// lookup cannot make a recipe disappear or attach it to the wrong fighter.
fn validate_structured_fighter_identities(project: &ModProjectFile, errors: &mut Vec<NodeError>) {
    let mut folded_names: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut fighter_names = project
        .fighters
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    fighter_names.sort_unstable();

    for fighter_name in fighter_names {
        let fighter = &project.fighters[fighter_name];
        let has_structured_data = !fighter.move_graphs.is_empty()
            || !fighter.effect_scripts.is_empty()
            || !fighter.controller_charts.is_empty();
        if has_structured_data
            && (!fighter_name.is_ascii() || fighter_name != fighter_name.to_ascii_lowercase())
        {
            errors.push(NodeError {
                subject: format!("fighter {fighter_name}"),
                reason: "structured move/effect data requires the canonical lowercase ASCII engine fighter ID (for example `mario`), not a display name; rename the project fighter key".into(),
            });
        }
        if has_structured_data {
            validate_identifier(fighter_name, "fighter", errors);
        }

        folded_names
            .entry(fighter_name.to_ascii_lowercase())
            .or_default()
            .push(fighter_name);
    }

    for names in folded_names.values_mut() {
        if names.len() < 2 {
            continue;
        }
        names.sort_unstable();
        let contains_structured_data = names.iter().any(|name| {
            project.fighters.get(*name).is_some_and(|fighter| {
                !fighter.move_graphs.is_empty()
                    || !fighter.effect_scripts.is_empty()
                    || !fighter.controller_charts.is_empty()
            })
        });
        if contains_structured_data {
            errors.push(NodeError {
                subject: format!("fighters {}", names.join(", ")),
                reason: "fighter keys collide under case-insensitive engine roster lookup; keep one canonical lowercase ASCII engine fighter ID and move its structured data there".into(),
            });
        }
    }
}

fn validate_identifier(value: &str, subject: &str, errors: &mut Vec<NodeError>) {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        errors.push(NodeError {
            subject: subject.into(),
            reason: "name is empty".into(),
        });
        return;
    };
    let valid_start = first == '_' || first.is_ascii_alphabetic();
    let valid_rest = chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric());
    let reserved = matches!(
        value,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "try"
    );
    if !(valid_start && valid_rest) || reserved {
        errors.push(NodeError {
            subject: subject.into(),
            reason: format!("{value:?} must be a safe Rust identifier (paths are not allowed)"),
        });
    }
}

fn merge_track(
    fighter: &mut FighterMod,
    fighter_name: &str,
    move_name: &str,
    track: Track,
    script: AcmdScript,
    errors: &mut Vec<NodeError>,
) {
    match track {
        Track::Game => {
            if let Some(record) = fighter.acmd.get_mut(move_name) {
                if !structurally_equal(&record.script, &script) {
                    errors.push(collision(fighter_name, move_name, track));
                } else {
                    // Equal materializations are accepted. Refresh the preview rows from the
                    // compiled script while retaining pristine history and costume scopes.
                    record.script = script;
                    record.hitboxes = record.script.to_hitboxes();
                }
            } else {
                let hitboxes = script.to_hitboxes();
                fighter.acmd.insert(
                    move_name.to_owned(),
                    EditRecord {
                        fighter: fighter_name.to_owned(),
                        fighter_display: fighter.display.clone(),
                        move_name: move_name.to_owned(),
                        script,
                        hitboxes_pristine: Vec::new(),
                        hitboxes,
                        slot_scope: None,
                        slot_scopes: Vec::new(),
                    },
                );
            }
        }
        Track::Sound => {
            if let Some(existing) = fighter.sound_scripts.get_mut(move_name) {
                if !structurally_equal(existing, &script) {
                    errors.push(collision(fighter_name, move_name, track));
                } else {
                    *existing = script;
                }
            } else {
                fighter.sound_scripts.insert(move_name.to_owned(), script);
            }
        }
        Track::Expression => {
            if let Some(existing) = fighter.expression_scripts.get_mut(move_name) {
                if !structurally_equal(existing, &script) {
                    errors.push(collision(fighter_name, move_name, track));
                } else {
                    *existing = script;
                }
            } else {
                fighter
                    .expression_scripts
                    .insert(move_name.to_owned(), script);
            }
        }
        Track::Effect => {
            if has_active_legacy_effect_data(fighter, move_name) {
                errors.push(NodeError {
                    subject: format!("fighter {fighter_name} move {move_name} effect track"),
                    reason: "legacy effect-call edits also exist for this move; apply the effect graph to explicitly replace those copies, or remove the graph track before exporting the legacy edits".into(),
                });
                return;
            }
            if let Some(existing) = fighter.effect_scripts.get(move_name) {
                if !structurally_equal(existing, &script) {
                    errors.push(collision(fighter_name, move_name, track));
                }
                // Equal copies keep the persisted full source/baseline byte-for-byte. Unlike
                // derived preview fields, effect scripts carry unknown source structure.
            } else {
                // Presence matters even when the full script has no statements: an empty
                // replacement intentionally suppresses the stock effect script.
                fighter.effect_scripts.insert(move_name.to_owned(), script);
            }
        }
    }
}

fn has_active_legacy_effect_data(fighter: &FighterMod, move_name: &str) -> bool {
    fighter.effect_calls_full.contains_key(move_name)
        || fighter
            .effect_calls
            .get(move_name)
            .is_some_and(|edits| !edits.is_empty())
        || fighter
            .effect_frame_residue
            .get(move_name)
            .is_some_and(|residue| !residue.is_empty())
}

fn collision(fighter: &str, move_name: &str, track: Track) -> NodeError {
    NodeError {
        subject: format!("fighter {fighter} move {move_name} {track:?} track"),
        reason: "saved script differs from the compiled move graph; make the existing edit match the graph or remove that category edit before applying the recipe".into(),
    }
}

fn structurally_equal<T: Serialize>(left: &T, right: &T) -> bool {
    match (serde_json::to_value(left), serde_json::to_value(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::MoveGraph;
    use super::*;
    use crate::data::{AcmdStmt, EditRecord, EffectMacro, ExcuteStmt};
    use crate::gimmicks::behavior::BehaviorGraph;
    use crate::mod_project::ModProjectFile;
    use crate::move_nodes::test_support as fixture;
    use crate::move_nodes::test_support::assert_json_eq;

    fn project_with_graph(fighter: &str, move_name: &str) -> ModProjectFile {
        let mut project = ModProjectFile::default();
        let fighter_mod = project.fighters.entry(fighter.into()).or_default();
        fighter_mod
            .move_graphs
            .insert(move_name.into(), MoveGraph::default());
        project
    }

    fn project_with_gimmick_event(name: &str, nested: bool, with_receiver: bool) -> ModProjectFile {
        use super::super::model::{MoveCondition, NodeKind, Track};

        let mut project = project_with_graph("mario", "attack11");
        let fighter = project.fighters.get_mut("mario").unwrap();
        let event_command = || ExcuteStmt::GimmickEvent { name: name.into() };
        let execute_node = |id| fixture::execute(id, vec![event_command()]);
        let node = if nested {
            fixture::node(
                1,
                NodeKind::Branch {
                    condition: MoveCondition::Grounded,
                    then_nodes: vec![execute_node(2)],
                    else_nodes: Vec::new(),
                },
            )
        } else {
            execute_node(1)
        };
        fighter
            .move_graphs
            .get_mut("attack11")
            .unwrap()
            .tracks
            .push(fixture::track(Track::Game, vec![node]));
        if with_receiver {
            fighter.behavior = fixture::controller_event(name, vec![]);
        }
        project
    }
    fn behavior_with_event(
        name: &str,
        fields: Vec<crate::gimmicks::event_payload::FieldDef>,
    ) -> BehaviorGraph {
        let mut graph = crate::gimmicks::templates::Preset::Timer.graph();
        let receiver = graph.transitions.len();
        graph
            .transitions
            .push(crate::gimmicks::behavior::Transition {
                from: "waiting".into(),
                to: "waiting".into(),
                guard: crate::gimmicks::behavior::Predicate::Flag("ready".into()),
                on_frame: false,
                actions: Vec::new(),
            });
        graph
            .emission_plan
            .event_bindings
            .push(crate::gimmicks::codegen::EventBinding {
                name: name.into(),
                transitions: vec![receiver],
                fields,
            });
        graph
    }

    fn chart_with_event(
        name: &str,
        fields: Vec<crate::gimmicks::event_payload::FieldDef>,
    ) -> crate::mod_project::ControllerChart {
        crate::mod_project::ControllerChart {
            graph: behavior_with_event(name, fields),
            layout: crate::gimmicks::field_canvas::FieldLayout::default(),
        }
    }

    fn framed_graph() -> MoveGraph {
        let mut graph = MoveGraph::default();
        graph
            .tracks
            .push(fixture::track(Track::Game, vec![fixture::frame(1, 4.0)]));
        graph
    }

    fn effect_graph() -> MoveGraph {
        MoveGraph {
            tracks: vec![fixture::track(
                Track::Effect,
                vec![fixture::effect_execute(
                    3 << 29,
                    vec![EffectMacro::LastEffectSetRate { rate: 0.75 }],
                )],
            )],
            ..MoveGraph::default()
        }
    }

    fn record(script: AcmdScript) -> EditRecord {
        EditRecord {
            fighter: "mario".into(),
            fighter_display: "Mario".into(),
            move_name: "attack11".into(),
            script,
            hitboxes_pristine: Vec::new(),
            hitboxes: Vec::new(),
            slot_scope: Some(2),
            slot_scopes: vec![3],
        }
    }

    #[test]
    fn old_and_graph_projects_round_trip_through_json() {
        let old: ModProjectFile = serde_json::from_str(
            r#"{"version":1,"name":"old","fighters":{"mario":{"display":"Mario"}}}"#,
        )
        .unwrap();
        assert!(old.fighters["mario"].move_graphs.is_empty());
        let graph_project = project_with_graph("mario", "attack11");
        let encoded = serde_json::to_string(&graph_project).unwrap();
        let decoded: ModProjectFile = serde_json::from_str(&encoded).unwrap();
        assert!(decoded.fighters["mario"]
            .move_graphs
            .contains_key("attack11"));
    }

    #[test]
    fn invalid_name_fails_atomically_without_mutating_input() {
        let mut project = project_with_graph("mario", "../attack11");
        let before = serde_json::to_value(&project).unwrap();
        let errors = expand(&project).unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.reason.contains("safe Rust identifier")));
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .move_graphs
            .clear();
    }

    #[test]
    fn structured_graph_requires_canonical_lowercase_engine_id_atomically() {
        let project = project_with_graph("Mario", "attack11");
        let before = serde_json::to_value(&project).unwrap();

        let errors = expand(&project).unwrap_err();

        assert!(errors.iter().any(|error| {
            error.reason.contains("lowercase ASCII engine fighter ID")
                && error.reason.contains("not a display name")
        }));
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
    }

    #[test]
    fn structured_effect_script_requires_canonical_lowercase_engine_id() {
        let mut project = ModProjectFile::default();
        project
            .fighters
            .entry("MARIO".into())
            .or_default()
            .effect_scripts
            .insert("attack11".into(), AcmdScript::default());
        let before = serde_json::to_value(&project).unwrap();

        let errors = expand(&project).unwrap_err();

        assert!(errors
            .iter()
            .any(|error| { error.reason.contains("lowercase ASCII engine fighter ID") }));
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
    }

    #[test]
    fn malformed_effect_only_fighter_key_is_rejected_atomically() {
        let mut project = ModProjectFile::default();
        project
            .fighters
            .entry("".into())
            .or_default()
            .effect_scripts
            .insert("attack11".into(), AcmdScript::default());
        let before = serde_json::to_value(&project).unwrap();

        let errors = expand(&project).unwrap_err();

        assert!(errors
            .iter()
            .any(|error| { error.subject == "fighter" && error.reason.contains("name is empty") }));
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
    }

    #[test]
    fn structured_case_collisions_are_rejected_before_expansion_atomically() {
        let mut project = project_with_graph("mario", "attack11");
        project.fighters.entry("MARIO".into()).or_default();
        let before = serde_json::to_value(&project).unwrap();

        let errors = expand(&project).unwrap_err();

        assert!(errors.iter().any(|error| {
            error
                .reason
                .contains("collide under case-insensitive engine roster lookup")
        }));
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
    }

    #[test]
    fn canonical_structured_engine_id_expands_and_flat_legacy_uppercase_is_unchanged() {
        let canonical = project_with_graph("mario", "attack11");
        let expanded = expand(&canonical).expect("canonical engine ID should expand");
        assert!(expanded.fighters["mario"].move_graphs.is_empty());

        let mut legacy = ModProjectFile::default();
        let legacy_fighter = legacy.fighters.entry("MARIO".into()).or_default();
        legacy_fighter.acmd.insert(
            "attack11".into(),
            record(AcmdScript {
                stmts: vec![AcmdStmt::Frame(3.0)],
            }),
        );
        let before = serde_json::to_value(&legacy).unwrap();
        let unchanged = expand(&legacy).expect("flat-only legacy names remain supported");
        assert_eq!(serde_json::to_value(&unchanged).unwrap(), before);
    }

    #[test]
    fn a_graph_with_no_authored_tracks_expands_without_creating_a_record() {
        let project = project_with_graph("mario", "attack11");
        let expanded = expand(&project).unwrap();
        let fighter = &expanded.fighters["mario"];
        assert!(fighter.move_graphs.is_empty());
        assert!(!fighter.acmd.contains_key("attack11"));
    }

    #[test]
    fn unbound_gimmick_event_in_nested_game_block_rejects_expansion_atomically() {
        let project = project_with_gimmick_event("unbound", true, false);
        let before = serde_json::to_value(&project).unwrap();

        let errors = expand(&project).unwrap_err();

        assert!(errors.iter().any(|error| {
            error.reason.contains("gimmick event receiver \"unbound\"")
                && error.reason.contains("no matching compiled event binding")
        }));
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
    }

    #[test]
    fn compile_recipes_preserves_schemaful_named_receiver_without_mutation() {
        use crate::gimmicks::behavior::{CmpOp, ConstValue, Expr, Predicate, StateDef, Transition};
        use crate::gimmicks::codegen::EventBinding;
        use crate::gimmicks::event_payload::FieldDef;
        let mut behavior = BehaviorGraph {
            initial: "idle".into(),
            states: vec![StateDef {
                status: None,
                name: "idle".into(),
            }],
            transitions: vec![Transition {
                from: "idle".into(),
                to: "idle".into(),
                on_frame: true,
                guard: Predicate::Compare {
                    op: CmpOp::Eq,
                    left: Expr::Const(ConstValue::Int(1)),
                    right: Expr::Const(ConstValue::Int(1)),
                },
                actions: vec![],
            }],
            ..BehaviorGraph::default()
        };
        behavior.emission_plan.event_bindings.push(EventBinding {
            name: "ready".into(),
            transitions: vec![0],
            fields: vec![FieldDef {
                name: "count".into(),
                kind: crate::gimmicks::behavior::VarType::Int,
            }],
        });
        let before = serde_json::to_value(&behavior).unwrap();
        let compiled = compile_recipes("mario", &behavior, &BTreeMap::new()).unwrap();
        assert_eq!(
            compiled.behavior.emission_plan.event_bindings[0].fields,
            behavior.emission_plan.event_bindings[0].fields
        );
        assert_eq!(serde_json::to_value(&behavior).unwrap(), before);
    }

    #[test]
    fn named_gimmick_event_exports_only_with_existing_compiled_receiver() {
        let project = project_with_gimmick_event("ready", false, true);

        let expanded = expand(&project).expect("matching event trigger binds the source receiver");

        let fighter = &expanded.fighters["mario"];
        assert_eq!(fighter.behavior.emission_plan.event_bindings.len(), 1);
        assert_eq!(
            fighter.behavior.emission_plan.event_bindings[0].name,
            "ready"
        );
        let game = &fighter.acmd["attack11"].script;
        assert!(game.stmts.iter().any(|statement| matches!(
            statement,
            AcmdStmt::Excute(commands)
                if matches!(commands.as_slice(), [ExcuteStmt::GimmickEvent { name }] if name == "ready")
        )));
        let emitted = crate::acmd::preview_game_fn(game, "attack11");
        assert!(emitted.contains(
            "crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, \"ready\");"
        ));
    }

    #[test]
    fn nested_named_event_expansion_keeps_game_control_flow_and_receiver_checks() {
        let project = project_with_gimmick_event("ready", true, true);
        let expanded = expand(&project).expect("nested zero-field event has a compiled receiver");
        let game = &expanded.fighters["mario"].acmd["attack11"].script;
        assert!(matches!(
            game.stmts.as_slice(),
            [AcmdStmt::RawBlock { body, .. }]
                if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::GimmickEvent { name }] if name == "ready"))
        ));
        let emitted = crate::acmd::preview_game_fn(game, "attack11");
        assert!(emitted.contains(
            "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {"
        ));
        assert!(emitted.contains(
            "crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, \"ready\");"
        ));
    }

    #[test]
    fn public_gimmick_event_validator_checks_nested_statements_and_track_scope() {
        use crate::gimmicks::behavior::BehaviorGraph;
        use crate::gimmicks::codegen::{EmissionPlan, EventBinding};

        let behavior = BehaviorGraph {
            emission_plan: EmissionPlan {
                event_bindings: vec![EventBinding {
                    name: "bound".into(),
                    transitions: vec![0],
                    fields: Vec::new(),
                }],
                ..EmissionPlan::default()
            },
            ..BehaviorGraph::default()
        };
        let script = AcmdScript {
            stmts: vec![AcmdStmt::RawBlock {
                header: "if preserved_condition {".into(),
                body: vec![AcmdStmt::Loop {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 2,
                    body: vec![AcmdStmt::Excute(vec![
                        ExcuteStmt::GimmickEvent {
                            name: "bound".into(),
                        },
                        ExcuteStmt::GimmickEvent {
                            name: "missing".into(),
                        },
                    ])],
                }],
            }],
        };

        let errors =
            validate_gimmick_event_receivers("mario", "attack11", Track::Game, &script, &behavior)
                .unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].reason.contains("\"missing\"")
                && errors[0]
                    .reason
                    .contains("no matching compiled event binding")
        );
        assert!(validate_gimmick_event_receivers(
            "mario",
            "attack11",
            Track::Expression,
            &script,
            &behavior,
        )
        .unwrap_err()
        .iter()
        .all(|error| error.reason.contains("only on the Game track")));
    }

    #[test]
    fn chart_event_resolves_only_the_exact_same_fighter_chart() {
        use super::super::model::Track;
        use crate::data::{AcmdScript, AcmdStmt, ExcuteStmt};

        let main = behavior_with_event("shared", Vec::new());
        let charts = BTreeMap::from([
            ("charge".into(), chart_with_event("shared", Vec::new())),
            ("cooldown".into(), chart_with_event("shared", Vec::new())),
        ]);
        let script = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::GimmickChartEvent {
                chart_id: "charge".into(),
                name: "shared".into(),
            }])],
        };
        assert!(validate_gimmick_event_receivers_with_controllers(
            "mario",
            "attack11",
            Track::Game,
            &script,
            &main,
            &charts,
        )
        .is_ok());

        // A name present in main and another chart does not make a missing
        // receiver in the explicitly selected chart valid.
        let wrong_chart_event = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::GimmickChartEvent {
                chart_id: "charge".into(),
                name: "only_in_other_chart".into(),
            }])],
        };
        let mut charts_with_other_receiver = charts.clone();
        charts_with_other_receiver.insert(
            "cooldown".into(),
            chart_with_event("only_in_other_chart", Vec::new()),
        );
        let errors = validate_gimmick_event_receivers_with_controllers(
            "mario",
            "attack11",
            Track::Game,
            &wrong_chart_event,
            &main,
            &charts_with_other_receiver,
        )
        .unwrap_err();
        assert!(
            errors[0].reason.contains("chart \"charge\"")
                && errors[0]
                    .reason
                    .contains("no matching compiled event binding")
        );

        let main_only = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::GimmickEvent {
                name: "shared".into(),
            }])],
        };
        assert!(validate_gimmick_event_receivers(
            "mario",
            "attack11",
            Track::Game,
            &main_only,
            &main,
        )
        .is_ok());
        let main_does_not_fall_back_to_extra = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::GimmickEvent {
                name: "only_in_other_chart".into(),
            }])],
        };
        assert!(validate_gimmick_event_receivers_with_controllers(
            "mario",
            "attack11",
            Track::Game,
            &main_does_not_fall_back_to_extra,
            &main,
            &charts_with_other_receiver,
        )
        .unwrap_err()[0]
            .reason
            .contains("no matching compiled event binding"));
    }

    #[test]
    fn chart_event_rejects_missing_target_typed_receiver_and_bad_ids() {
        use super::super::model::Track;
        use crate::data::{AcmdScript, AcmdStmt, ExcuteStmt};

        let main = BehaviorGraph::default();
        let charts = BTreeMap::from([
            ("charge".into(), chart_with_event("plain", Vec::new())),
            (" charge ".into(), chart_with_event("padded", Vec::new())),
            (
                "typed".into(),
                chart_with_event(
                    "payload",
                    vec![crate::gimmicks::event_payload::FieldDef {
                        name: "amount".into(),
                        kind: crate::gimmicks::behavior::VarType::Int,
                    }],
                ),
            ),
        ]);
        let validate = |chart_id: &str, name: &str, track| {
            let script = AcmdScript {
                stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::GimmickChartEvent {
                    chart_id: chart_id.into(),
                    name: name.into(),
                }])],
            };
            validate_gimmick_event_receivers_with_controllers(
                "mario", "attack11", track, &script, &main, &charts,
            )
        };
        let too_long = "x".repeat(crate::gimmicks::controller_protocol::MAX_CHART_ID_BYTES + 1);
        for bad_id in ["unknown", "main", "bad\0id", too_long.as_str()] {
            assert!(
                validate(bad_id, "plain", Track::Game).is_err(),
                "accepted {bad_id:?}"
            );
        }
        assert!(validate(" charge ", "padded", Track::Game).is_ok());
        assert!(validate("charge", "missing", Track::Game).unwrap_err()[0]
            .reason
            .contains("chart \"charge\""));
        assert!(validate("typed", "payload", Track::Game).unwrap_err()[0]
            .reason
            .contains("requires payload fields"));
        assert!(
            validate("charge", "plain", Track::Expression).unwrap_err()[0]
                .reason
                .contains("only on the Game track")
        );

        let bare = AcmdScript {
            stmts: vec![AcmdStmt::Bare(Box::new(ExcuteStmt::GimmickChartEvent {
                chart_id: "charge".into(),
                name: "plain".into(),
            }))],
        };
        assert!(validate_gimmick_event_receivers_with_controllers(
            "mario",
            "attack11",
            Track::Game,
            &bare,
            &main,
            &charts,
        )
        .unwrap_err()[0]
            .reason
            .contains("inside a timed execute block"));

        let nested = AcmdScript {
            stmts: vec![AcmdStmt::RawBlock {
                header: "if preserved_condition {".into(),
                body: vec![AcmdStmt::Loop {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 2,
                    body: vec![AcmdStmt::Excute(vec![ExcuteStmt::GimmickChartEvent {
                        chart_id: "absent".into(),
                        name: "plain".into(),
                    }])],
                }],
            }],
        };
        assert!(validate_gimmick_event_receivers_with_controllers(
            "mario",
            "attack11",
            Track::Game,
            &nested,
            &main,
            &charts,
        )
        .unwrap_err()[0]
            .reason
            .contains("does not exist for this fighter"));
    }

    #[test]
    fn typed_chart_events_validate_exact_schema_and_canonicalize_compiled_copies() {
        use crate::data::{AcmdScript, AcmdStmt, ExcuteStmt};
        use crate::gimmicks::behavior::{ConstValue, VarType};
        use crate::gimmicks::event_payload::{Argument, FieldDef};

        let fields = vec![
            FieldDef {
                name: "count".into(),
                kind: VarType::Int,
            },
            FieldDef {
                name: "active".into(),
                kind: VarType::Flag,
            },
            FieldDef {
                name: "ratio".into(),
                kind: VarType::Float,
            },
        ];
        let receivers = BTreeMap::from([(
            "charge".into(),
            BTreeMap::from([("pulse".into(), fields.clone())]),
        )]);
        let authored = vec![
            Argument {
                field: "ratio".into(),
                value: ConstValue::Float(0.5),
            },
            Argument {
                field: "count".into(),
                value: ConstValue::Int(7),
            },
            Argument {
                field: "active".into(),
                value: ConstValue::Flag(true),
            },
        ];
        let original = authored.clone();
        let mut script = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![
                ExcuteStmt::GimmickChartEventValues {
                    chart_id: "charge".into(),
                    name: "pulse".into(),
                    arguments: authored,
                },
            ])],
        };
        assert!(validate_gimmick_event_receivers_with_tables(
            "mario",
            "attack11",
            Track::Game,
            &script,
            &receivers,
            ReceiverValidationOptions {
                include_legacy_main_events: true,
                require_canonical_typed_arguments: false
            },
        )
        .is_ok());
        canonicalize_chart_event_scripts("mario", "attack11", Track::Game, &mut script, &receivers)
            .unwrap();
        let commands = match &script.stmts[0] {
            AcmdStmt::Excute(commands) => commands,
            _ => unreachable!(),
        };
        let ExcuteStmt::GimmickChartEventValues { arguments, .. } = &commands[0] else {
            unreachable!()
        };
        assert_eq!(
            arguments
                .iter()
                .map(|argument| argument.field.as_str())
                .collect::<Vec<_>>(),
            ["count", "active", "ratio"]
        );
        assert_eq!(
            original
                .iter()
                .map(|argument| argument.field.as_str())
                .collect::<Vec<_>>(),
            ["ratio", "count", "active"]
        );
        assert!(validate_gimmick_event_receivers_with_tables(
            "mario",
            "attack11",
            Track::Game,
            &script,
            &receivers,
            ReceiverValidationOptions {
                include_legacy_main_events: true,
                require_canonical_typed_arguments: true
            },
        )
        .is_ok());

        let invalids = [
            vec![Argument {
                field: "count".into(),
                value: ConstValue::Int(1),
            }],
            vec![
                Argument {
                    field: "count".into(),
                    value: ConstValue::Int(1),
                },
                Argument {
                    field: "count".into(),
                    value: ConstValue::Int(2),
                },
                Argument {
                    field: "ratio".into(),
                    value: ConstValue::Float(0.5),
                },
            ],
            vec![
                Argument {
                    field: "count".into(),
                    value: ConstValue::Flag(false),
                },
                Argument {
                    field: "active".into(),
                    value: ConstValue::Flag(true),
                },
                Argument {
                    field: "ratio".into(),
                    value: ConstValue::Float(0.5),
                },
            ],
            vec![
                Argument {
                    field: "count".into(),
                    value: ConstValue::Int(1),
                },
                Argument {
                    field: "active".into(),
                    value: ConstValue::Flag(true),
                },
                Argument {
                    field: "ratio".into(),
                    value: ConstValue::Float(f32::NAN),
                },
            ],
        ];
        for arguments in invalids {
            let invalid = AcmdScript {
                stmts: vec![AcmdStmt::Excute(vec![
                    ExcuteStmt::GimmickChartEventValues {
                        chart_id: "charge".into(),
                        name: "pulse".into(),
                        arguments,
                    },
                ])],
            };
            assert!(validate_gimmick_event_receivers_with_tables(
                "mario",
                "attack11",
                Track::Game,
                &invalid,
                &receivers,
                ReceiverValidationOptions {
                    include_legacy_main_events: true,
                    require_canonical_typed_arguments: false
                },
            )
            .is_err());
        }

        let timer_receivers = BTreeMap::from([(
            "charge".into(),
            BTreeMap::from([(
                "pulse".into(),
                vec![FieldDef {
                    name: "timer".into(),
                    kind: VarType::Timer,
                }],
            )]),
        )]);
        let timer = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![
                ExcuteStmt::GimmickChartEventValues {
                    chart_id: "charge".into(),
                    name: "pulse".into(),
                    arguments: vec![Argument {
                        field: "timer".into(),
                        value: ConstValue::Int(1),
                    }],
                },
            ])],
        };
        assert!(validate_gimmick_event_receivers_with_tables(
            "mario",
            "attack11",
            Track::Game,
            &timer,
            &timer_receivers,
            ReceiverValidationOptions {
                include_legacy_main_events: true,
                require_canonical_typed_arguments: false
            },
        )
        .unwrap_err()[0]
            .reason
            .contains("Timer is not a supported event field kind"));

        let legacy = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::GimmickChartEvent {
                chart_id: "charge".into(),
                name: "pulse".into(),
            }])],
        };
        assert!(validate_gimmick_event_receivers_with_tables(
            "mario",
            "attack11",
            Track::Game,
            &legacy,
            &receivers,
            ReceiverValidationOptions {
                include_legacy_main_events: true,
                require_canonical_typed_arguments: false
            },
        )
        .unwrap_err()[0]
            .reason
            .contains("requires payload fields"));
        assert!(validate_gimmick_event_receivers_with_tables(
            "mario",
            "attack11",
            Track::Sound,
            &script,
            &receivers,
            ReceiverValidationOptions {
                include_legacy_main_events: true,
                require_canonical_typed_arguments: false
            },
        )
        .unwrap_err()[0]
            .reason
            .contains("only on the Game track"));
        let bare = AcmdScript {
            stmts: vec![AcmdStmt::Bare(Box::new(
                ExcuteStmt::GimmickChartEventValues {
                    chart_id: "charge".into(),
                    name: "pulse".into(),
                    arguments: vec![Argument {
                        field: "count".into(),
                        value: ConstValue::Int(1),
                    }],
                },
            ))],
        };
        assert!(validate_gimmick_event_receivers_with_tables(
            "mario",
            "attack11",
            Track::Game,
            &bare,
            &receivers,
            ReceiverValidationOptions {
                include_legacy_main_events: true,
                require_canonical_typed_arguments: false
            },
        )
        .unwrap_err()[0]
            .reason
            .contains("inside a timed execute block"));
    }

    #[test]
    fn persisted_typed_chart_events_require_canonical_order_and_references_are_retained() {
        use super::super::model::{MoveCommand, MoveTrack, NodeKind};
        use crate::data::{AcmdScript, AcmdStmt, EditRecord, ExcuteStmt};
        use crate::gimmicks::behavior::{ConstValue, VarType};
        use crate::gimmicks::event_payload::{Argument, FieldDef};

        let fields = vec![
            FieldDef {
                name: "first".into(),
                kind: VarType::Int,
            },
            FieldDef {
                name: "second".into(),
                kind: VarType::Flag,
            },
        ];
        let receivers =
            BTreeMap::from([("charge".into(), BTreeMap::from([("pulse".into(), fields)]))]);
        let mut data = FighterMod::default();
        data.acmd.insert(
            "attack11".into(),
            EditRecord {
                fighter: "mario".into(),
                fighter_display: "Mario".into(),
                move_name: "attack11".into(),
                script: AcmdScript {
                    stmts: vec![AcmdStmt::Excute(vec![
                        ExcuteStmt::GimmickChartEventValues {
                            chart_id: "charge".into(),
                            name: "pulse".into(),
                            arguments: vec![
                                Argument {
                                    field: "second".into(),
                                    value: ConstValue::Flag(true),
                                },
                                Argument {
                                    field: "first".into(),
                                    value: ConstValue::Int(1),
                                },
                            ],
                        },
                    ])],
                },
                hitboxes_pristine: Vec::new(),
                hitboxes: Vec::new(),
                slot_scope: None,
                slot_scopes: Vec::new(),
            },
        );
        let errors = validate_persisted_chart_event_scripts("mario", &data, &receivers);
        assert!(errors
            .iter()
            .any(|error| error.reason.contains("receiver declaration order")));

        let command = |chart_id: &str| ExcuteStmt::GimmickChartEventValues {
            chart_id: chart_id.into(),
            name: "pulse".into(),
            arguments: Vec::new(),
        };
        let graph = MoveGraph {
            tracks: vec![MoveTrack {
                track: Track::Game,
                effect_execute_baseline: Vec::new(),
                nodes: vec![
                    fixture::node(
                        1,
                        NodeKind::MixedExecute {
                            commands: vec![
                                MoveCommand::Editable(command("mixed_editable")),
                                MoveCommand::Preserved(command("mixed_preserved")),
                            ],
                        },
                    ),
                    fixture::node(
                        2,
                        NodeKind::Preserved {
                            statement: AcmdStmt::RawBlock {
                                header: "if preserved {".into(),
                                body: vec![AcmdStmt::Loop {
                                    motion_loop: crate::data::MotionLoop::No,
                                    count: 1,
                                    body: vec![AcmdStmt::Excute(vec![command("nested")])],
                                }],
                            },
                        },
                    ),
                ],
                baseline: vec![AcmdStmt::Loop {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 1,
                    body: vec![AcmdStmt::RawBlock {
                        header: "if baseline {".into(),
                        body: vec![AcmdStmt::Excute(vec![command("baseline")])],
                    }],
                }],
            }],
            ..MoveGraph::default()
        };
        assert_eq!(
            referenced_controller_charts(&graph).unwrap(),
            BTreeSet::from([
                "mixed_editable".into(),
                "mixed_preserved".into(),
                "nested".into(),
                "baseline".into(),
            ])
        );
    }

    #[test]
    fn chart_value_recipe_compilation_reorders_only_emitted_copies_per_exact_chart() {
        use super::super::model::Track;
        use crate::data::ExcuteStmt;
        use crate::gimmicks::behavior::{ConstValue, VarType};
        use crate::gimmicks::event_payload::{Argument, FieldDef};

        let charge_fields = vec![
            FieldDef {
                name: "count".into(),
                kind: VarType::Int,
            },
            FieldDef {
                name: "active".into(),
                kind: VarType::Flag,
            },
            FieldDef {
                name: "ratio".into(),
                kind: VarType::Float,
            },
        ];
        let recovery_fields = vec![
            FieldDef {
                name: "ratio".into(),
                kind: VarType::Float,
            },
            FieldDef {
                name: "active".into(),
                kind: VarType::Flag,
            },
            FieldDef {
                name: "count".into(),
                kind: VarType::Int,
            },
        ];
        let authored_commands = vec![
            ExcuteStmt::GimmickChartEventValues {
                chart_id: "charge".into(),
                name: "pulse".into(),
                arguments: vec![
                    Argument {
                        field: "ratio".into(),
                        value: ConstValue::Float(0.75),
                    },
                    Argument {
                        field: "active".into(),
                        value: ConstValue::Flag(true),
                    },
                    Argument {
                        field: "count".into(),
                        value: ConstValue::Int(12),
                    },
                ],
            },
            ExcuteStmt::GimmickChartEventValues {
                chart_id: "recovery".into(),
                name: "pulse".into(),
                arguments: vec![
                    Argument {
                        field: "count".into(),
                        value: ConstValue::Int(4),
                    },
                    Argument {
                        field: "ratio".into(),
                        value: ConstValue::Float(0.25),
                    },
                    Argument {
                        field: "active".into(),
                        value: ConstValue::Flag(false),
                    },
                ],
            },
        ];
        let mut graph = MoveGraph::default();
        graph.tracks.push(fixture::track(
            Track::Game,
            vec![fixture::execute(1, authored_commands)],
        ));
        let graphs = BTreeMap::from([("attack11".into(), graph)]);
        let original_graphs = serde_json::to_value(&graphs).unwrap();
        let charts = BTreeMap::from([
            ("charge".into(), chart_with_event("pulse", charge_fields)),
            (
                "recovery".into(),
                chart_with_event("pulse", recovery_fields),
            ),
        ]);

        let compiled =
            compile_recipes_with_controllers("mario", &BehaviorGraph::default(), &charts, &graphs)
                .expect("typed commands resolve against their exact same-fighter chart");
        assert_eq!(serde_json::to_value(&graphs).unwrap(), original_graphs);
        let script = &compiled.tracks["attack11"]
            .iter()
            .find(|(track, _)| *track == Track::Game)
            .unwrap()
            .1;
        let commands = match &script.stmts[0] {
            AcmdStmt::Excute(commands) => commands,
            _ => panic!("expected the source timed-execute block"),
        };
        let expected = [
            (
                "charge",
                vec![
                    ("count", ConstValue::Int(12)),
                    ("active", ConstValue::Flag(true)),
                    ("ratio", ConstValue::Float(0.75)),
                ],
            ),
            (
                "recovery",
                vec![
                    ("ratio", ConstValue::Float(0.25)),
                    ("active", ConstValue::Flag(false)),
                    ("count", ConstValue::Int(4)),
                ],
            ),
        ];
        for (command, (expected_chart, expected_arguments)) in commands.iter().zip(expected) {
            let ExcuteStmt::GimmickChartEventValues {
                chart_id,
                name,
                arguments,
            } = command
            else {
                panic!("expected a typed chart event")
            };
            assert_eq!(chart_id, expected_chart);
            assert_eq!(name, "pulse");
            assert_eq!(
                arguments
                    .iter()
                    .map(|argument| (argument.field.as_str(), argument.value))
                    .collect::<Vec<_>>(),
                expected_arguments
            );
        }
    }

    #[test]
    fn no_graph_expansion_rejects_invalid_persisted_typed_events_atomically() {
        use crate::data::{AcmdScript, AcmdStmt, ExcuteStmt};
        use crate::gimmicks::behavior::{ConstValue, VarType};
        use crate::gimmicks::event_payload::{Argument, FieldDef};

        let fields = vec![
            FieldDef {
                name: "count".into(),
                kind: VarType::Int,
            },
            FieldDef {
                name: "active".into(),
                kind: VarType::Flag,
            },
        ];
        let make_project = |script: AcmdScript, sound: bool| {
            let mut project = ModProjectFile::default();
            let fighter = project.fighters.entry("mario".into()).or_default();
            fighter
                .controller_charts
                .insert("charge".into(), chart_with_event("pulse", fields.clone()));
            if sound {
                fighter.sound_scripts.insert("attack11".into(), script);
            } else {
                fighter.acmd.insert("attack11".into(), record(script));
            }
            project
        };
        let script = |chart_id: &str, arguments: Vec<Argument>| AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![
                ExcuteStmt::GimmickChartEventValues {
                    chart_id: chart_id.into(),
                    name: "pulse".into(),
                    arguments,
                },
            ])],
        };
        let valid = vec![
            Argument {
                field: "count".into(),
                value: ConstValue::Int(3),
            },
            Argument {
                field: "active".into(),
                value: ConstValue::Flag(true),
            },
        ];
        let wrong_type = vec![
            Argument {
                field: "count".into(),
                value: ConstValue::Flag(true),
            },
            Argument {
                field: "active".into(),
                value: ConstValue::Flag(true),
            },
        ];
        let wrong_order = vec![valid[1].clone(), valid[0].clone()];
        let mut cases = vec![
            (
                "missing chart",
                make_project(script("absent", valid.clone()), false),
                "does not exist for this fighter",
            ),
            (
                "wrong type",
                make_project(script("charge", wrong_type), false),
                "expected Int, received Flag",
            ),
            (
                "wrong order",
                make_project(script("charge", wrong_order), false),
                "receiver declaration order",
            ),
            (
                "wrong track",
                make_project(script("charge", valid.clone()), true),
                "only on the Game track",
            ),
        ];
        let bare = AcmdScript {
            stmts: vec![AcmdStmt::Bare(Box::new(
                ExcuteStmt::GimmickChartEventValues {
                    chart_id: "charge".into(),
                    name: "pulse".into(),
                    arguments: valid,
                },
            ))],
        };
        cases.push((
            "bare",
            make_project(bare, false),
            "inside a timed execute block",
        ));
        for (label, project, expected_reason) in cases {
            assert!(
                project.fighters["mario"].move_graphs.is_empty(),
                "{label} must exercise persisted-source expansion"
            );
            let before = serde_json::to_value(&project).unwrap();
            let errors = expand(&project).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.reason.contains(expected_reason)),
                "{label}: {errors:?}"
            );
            assert_eq!(
                serde_json::to_value(&project).unwrap(),
                before,
                "{label} mutated the project"
            );
        }
    }

    #[test]
    fn chart_aware_recipe_compile_uses_only_passed_owner_charts_and_is_read_only() {
        use super::super::model::Track;
        use crate::data::ExcuteStmt;

        let mut move_graph = MoveGraph::default();
        move_graph.tracks.push(fixture::track(
            Track::Game,
            vec![fixture::execute(
                1,
                vec![ExcuteStmt::GimmickChartEvent {
                    chart_id: "charge".into(),
                    name: "shared".into(),
                }],
            )],
        ));
        let graphs = BTreeMap::from([("attack11".into(), move_graph)]);
        let charts = BTreeMap::from([("charge".into(), chart_with_event("shared", Vec::new()))]);
        let before_graphs = serde_json::to_value(&graphs).unwrap();
        let before_charts = serde_json::to_value(&charts).unwrap();
        assert!(compile_recipes_with_controllers(
            "mario",
            &BehaviorGraph::default(),
            &charts,
            &graphs,
        )
        .is_ok());
        assert!(compile_recipes("mario", &BehaviorGraph::default(), &graphs)
            .err()
            .expect("main-only entry point must refuse additional-chart sends")
            .iter()
            .any(|error| error.reason.contains("does not exist for this fighter")));
        assert_eq!(serde_json::to_value(&graphs).unwrap(), before_graphs);
        assert_eq!(serde_json::to_value(&charts).unwrap(), before_charts);
    }

    #[test]
    fn project_expansion_does_not_borrow_another_fighters_chart() {
        use super::super::model::Track;
        use crate::data::ExcuteStmt;
        use crate::mod_project::ControllerChart;

        let mut project = project_with_graph("mario", "attack11");
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .move_graphs
            .get_mut("attack11")
            .unwrap()
            .tracks
            .push(fixture::track(
                Track::Game,
                vec![fixture::execute(
                    1,
                    vec![ExcuteStmt::GimmickChartEvent {
                        chart_id: "charge".into(),
                        name: "shared".into(),
                    }],
                )],
            ));
        project
            .fighters
            .entry("luigi".into())
            .or_default()
            .controller_charts
            .insert(
                "charge".into(),
                ControllerChart {
                    graph: crate::gimmicks::templates::Preset::Timer.graph(),
                    layout: crate::gimmicks::field_canvas::FieldLayout::default(),
                },
            );
        let before = serde_json::to_value(&project).unwrap();
        let errors = expand(&project).unwrap_err();
        assert!(
            errors.iter().any(|error| {
                error.subject.contains("mario")
                    && error.reason.contains("target controller chart \"charge\"")
                    && error.reason.contains("does not exist for this fighter")
            }),
            "{errors:?}"
        );
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
    }

    #[test]
    fn no_move_graphs_persisted_chart_events_still_validate_owner_and_track() {
        use crate::data::{AcmdScript, AcmdStmt, EditRecord, ExcuteStmt};
        use crate::mod_project::ControllerChart;

        let mut project = project_with_graph("mario", "unused");
        let mario = project.fighters.get_mut("mario").unwrap();
        mario.move_graphs.clear();
        mario.acmd.insert(
            "attack11".into(),
            EditRecord {
                fighter: "mario".into(),
                fighter_display: "Mario".into(),
                move_name: "attack11".into(),
                script: AcmdScript {
                    stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::GimmickChartEvent {
                        chart_id: "charge".into(),
                        name: "pulse".into(),
                    }])],
                },
                hitboxes_pristine: Vec::new(),
                hitboxes: Vec::new(),
                slot_scope: None,
                slot_scopes: Vec::new(),
            },
        );
        mario.sound_scripts.insert(
            "attack11".into(),
            AcmdScript {
                stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::GimmickChartEvent {
                    chart_id: "charge".into(),
                    name: "pulse".into(),
                }])],
            },
        );
        project
            .fighters
            .entry("luigi".into())
            .or_default()
            .controller_charts
            .insert(
                "charge".into(),
                ControllerChart {
                    graph: crate::gimmicks::templates::Preset::Timer.graph(),
                    layout: crate::gimmicks::field_canvas::FieldLayout::default(),
                },
            );
        let before = serde_json::to_value(&project).unwrap();
        let errors = expand(&project).unwrap_err();
        assert!(
            errors.iter().any(|error| {
                error.subject.contains("mario")
                    && error.subject.contains("Game")
                    && error.reason.contains("target controller chart \"charge\"")
                    && error.reason.contains("does not exist for this fighter")
            }),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| {
                error.subject.contains("mario")
                    && error.subject.contains("Sound")
                    && error.reason.contains("only on the Game track")
            }),
            "{errors:?}"
        );
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
    }

    #[test]
    fn a_new_game_track_creates_an_unscoped_record_without_hitbox_slots() {
        let mut project = project_with_graph("mario", "attack11");
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .move_graphs
            .insert("attack11".into(), framed_graph());

        let expanded = expand(&project).unwrap();
        let record = &expanded.fighters["mario"].acmd["attack11"];
        assert!(record.hitboxes.is_empty());
        assert!(record.hitboxes_pristine.is_empty());
        assert_eq!(record.slot_scope, None);
        assert!(record.slot_scopes.is_empty());
    }

    #[test]
    fn a_graph_only_affects_its_own_move() {
        let mut project = project_with_graph("mario", "attack11");
        project.fighters.get_mut("mario").unwrap().acmd.insert(
            "attack12".into(),
            EditRecord {
                fighter: "mario".into(),
                fighter_display: "Mario".into(),
                move_name: "attack12".into(),
                script: AcmdScript::default(),
                hitboxes_pristine: Vec::new(),
                hitboxes: Vec::new(),
                slot_scope: Some(3),
                slot_scopes: vec![4],
            },
        );
        let expanded = expand(&project).unwrap();
        assert!(expanded.fighters["mario"].acmd.contains_key("attack12"));
        assert_eq!(
            expanded.fighters["mario"].acmd["attack12"].slot_scope,
            Some(3)
        );
        assert_eq!(
            expanded.fighters["mario"].acmd["attack12"].slot_scopes,
            vec![4]
        );
    }

    #[test]
    fn a_different_existing_game_script_is_a_collision_and_input_stays_unchanged() {
        let mut project = project_with_graph("mario", "attack11");
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .move_graphs
            .insert("attack11".into(), framed_graph());
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .acmd
            .insert("attack11".into(), record(AcmdScript::default()));
        let before = serde_json::to_value(&project).unwrap();

        let errors = expand(&project).unwrap_err();

        assert!(errors
            .iter()
            .any(|error| error.reason.contains("differs from the compiled")));
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
    }

    #[test]
    fn an_equal_existing_game_script_is_accepted_and_keeps_record_metadata() {
        let mut project = project_with_graph("mario", "attack11");
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .move_graphs
            .insert("attack11".into(), framed_graph());
        project.fighters.get_mut("mario").unwrap().acmd.insert(
            "attack11".into(),
            record(AcmdScript {
                stmts: vec![AcmdStmt::Frame(4.0)],
            }),
        );

        let expanded = expand(&project).unwrap();
        let materialized = &expanded.fighters["mario"].acmd["attack11"];
        assert_eq!(materialized.slot_scope, Some(2));
        assert_eq!(materialized.slot_scopes, vec![3]);
        assert!(materialized.hitboxes_pristine.is_empty());
        assert!(materialized.hitboxes.is_empty());
    }

    #[test]
    fn imported_track_compiles_back_to_its_source_statements() {
        let source = AcmdScript {
            stmts: vec![AcmdStmt::Frame(4.0), AcmdStmt::Wait(2.0)],
        };
        let track = super::super::timeline::import(Track::Game, &source);
        let graph = MoveGraph {
            version: super::super::model::SCHEMA_VERSION,
            tracks: vec![track],
            ..MoveGraph::default()
        };

        let compiled = super::super::timeline::compile("mario", "attack11", &graph).unwrap();

        assert_eq!(compiled.len(), 1);
        assert_eq!(compiled[0].0, Track::Game);
        assert_json_eq!(&compiled[0].1, source);
    }

    #[test]
    fn effect_track_materializes_a_full_script_and_accepts_equal_source() {
        let mut project = project_with_graph("mario", "attack11");
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .move_graphs
            .insert("attack11".into(), effect_graph());
        let same_graph = super::super::timeline::compile(
            "mario",
            "attack11",
            &project.fighters["mario"].move_graphs["attack11"],
        )
        .unwrap();
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .effect_scripts
            .insert("attack11".into(), same_graph[0].1.clone());
        let expected = project.fighters["mario"].effect_scripts["attack11"].clone();
        let expanded = expand(&project).unwrap();
        assert_json_eq!(
            &expanded.fighters["mario"].effect_scripts["attack11"],
            expected
        );

        let mut graph_only = project_with_graph("mario", "attack11");
        graph_only
            .fighters
            .get_mut("mario")
            .unwrap()
            .move_graphs
            .insert("attack11".into(), effect_graph());
        let expanded = expand(&graph_only).unwrap();
        assert!(expanded.fighters["mario"]
            .effect_scripts
            .contains_key("attack11"));
        assert!(expanded.fighters["mario"].effect_scripts["attack11"]
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, AcmdStmt::Excute(commands) if commands.iter().any(|command| matches!(command, crate::data::ExcuteStmt::Raw(line) if line.contains("LAST_EFFECT_SET_RATE"))))));
        assert!(expanded.fighters["mario"].move_graphs.is_empty());
    }

    #[test]
    fn effect_track_rejects_conflicting_materializations_and_legacy_copies() {
        let mut project = project_with_graph("mario", "attack11");
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .move_graphs
            .insert("attack11".into(), effect_graph());
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .effect_scripts
            .insert("attack11".into(), AcmdScript::default());
        assert!(expand(&project)
            .unwrap_err()
            .iter()
            .any(|error| error.reason.contains("differs from the compiled")));

        let legacy_copies = ["full", "calls", "residue"];
        for legacy in legacy_copies {
            let mut project = project_with_graph("mario", "attack11");
            project
                .fighters
                .get_mut("mario")
                .unwrap()
                .move_graphs
                .insert("attack11".into(), effect_graph());
            let fighter = project.fighters.get_mut("mario").unwrap();
            match legacy {
                "full" => {
                    fighter
                        .effect_calls_full
                        .insert("attack11".into(), Vec::new());
                }
                "calls" => {
                    fighter.effect_calls.insert(
                        "attack11".into(),
                        vec![crate::data::EffectCallEdit {
                            index: 0,
                            op: crate::data::EffectCallOp::Remove,
                            pristine: None,
                        }],
                    );
                }
                _ => {
                    fighter.effect_frame_residue.insert(
                        "attack11".into(),
                        BTreeMap::from([(1, vec!["unknown_effect(agent);".into()])]),
                    );
                }
            }
            let errors = expand(&project).unwrap_err();
            assert!(errors.iter().any(|error| {
                error.reason.contains("legacy effect-call edits")
                    && error.reason.contains("apply the effect graph")
            }));
        }
    }

    #[test]
    fn empty_effect_track_is_an_explicit_replacement() {
        let mut project = project_with_graph("mario", "attack11");
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .move_graphs
            .insert(
                "attack11".into(),
                MoveGraph {
                    tracks: vec![fixture::track(Track::Effect, Vec::new())],
                    ..MoveGraph::default()
                },
            );
        let expanded = expand(&project).unwrap();
        assert!(expanded.fighters["mario"]
            .effect_scripts
            .contains_key("attack11"));
        assert!(expanded.fighters["mario"].effect_scripts["attack11"]
            .stmts
            .is_empty());
    }
    #[test]
    fn main_payload_receivers_reject_flat_and_nested_name_only_sends_atomically() {
        for nested in [false, true] {
            let mut project = project_with_gimmick_event("ready", nested, true);
            project
                .fighters
                .get_mut("mario")
                .unwrap()
                .behavior
                .emission_plan
                .event_bindings[0]
                .fields
                .push(crate::gimmicks::event_payload::FieldDef {
                    name: "amount".into(),
                    kind: crate::gimmicks::behavior::VarType::Int,
                });
            let before = serde_json::to_value(&project).unwrap();
            let errors = expand(&project).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| e.reason.contains("requires payload fields")),
                "{errors:?}"
            );
            assert_eq!(serde_json::to_value(&project).unwrap(), before);
        }
    }
    #[test]
    fn changing_main_receiver_revalidates_sibling_move_emitters() {
        let mut project = project_with_gimmick_event("ready", true, true);
        let fighter = project.fighters.get_mut("mario").unwrap();
        fighter
            .move_graphs
            .insert("attack12".into(), fighter.move_graphs["attack11"].clone());
        assert!(expand(&project).is_ok());
        project
            .fighters
            .get_mut("mario")
            .unwrap()
            .behavior
            .emission_plan
            .event_bindings[0]
            .name = "renamed".into();
        let before = serde_json::to_value(&project).unwrap();
        let errors = expand(&project).unwrap_err();
        for name in ["attack11", "attack12"] {
            assert!(
                errors.iter().any(|e| e.subject.contains(name)),
                "{errors:?}"
            );
        }
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
    }
}
