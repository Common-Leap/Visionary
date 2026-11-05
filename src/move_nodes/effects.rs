//! Typed effect timeline import and the bounded authored effect-command surface.
//!
//! Unsupported conditional blocks, trail forms, unknown source, and WorkModule/area identifiers
//! remain immutable `Preserved` baseline statements. Proven fighter-aware conditions retain
//! editable branch bodies. Authored commands are limited to the
//! existing measured effect-spawn layouts, effect-off, rate/offset/color/alpha/scale controls,
//! effect detach by name, closed one-argument controls for the last spawned visual effect,
//! measured `req_screen` presets and a literal camera-depth bool, and the measured literal
//! `AFTER_IMAGE4_ON_arg29` trail form.
//! Imported `EFFECT_FOLLOW_WORK` calls use a closed owner/slot table and transform-only edits;
//! arbitrary WorkModule slots and Raw Rust are not authoring inputs.

use crate::data::{AcmdScript, AcmdStmt, EffectMacro, EffectScript, EffectStmt};
use crate::move_nodes::model::{EffectCommandSlot, MoveNode, MoveTrack, NodeKind, Track};

const MAX_NESTING: usize = 8;
const MAX_REPEAT: usize = 100;
const MAX_NAME_BYTES: usize = 128;
const MAX_TAIL_NUMBER: f32 = 10_000.0;
const MAX_COMMANDS_PER_EXECUTE: usize = 256;

/// Import one effect script without fighter context. Conditional source remains immutable.
#[cfg(test)]
pub fn import(script: &EffectScript) -> MoveTrack {
    import_inner(None, script)
}

/// Import effect branches only when their condition passes the fighter-aware timeline gate.
/// Unsupported conditions and their nested source remain immutable baseline structure.
pub fn import_for_fighter(fighter: &str, script: &EffectScript) -> MoveTrack {
    import_inner(Some(fighter), script)
}

fn import_inner(fighter: Option<&str>, script: &EffectScript) -> MoveTrack {
    let mut next_id = 3 << 29;
    let mut baseline = Vec::new();
    let mut effect_execute_baseline = Vec::new();
    let nodes = import_stmts(
        fighter,
        &script.stmts,
        &mut next_id,
        0,
        &mut baseline,
        &mut effect_execute_baseline,
        false,
    );
    MoveTrack {
        track: Track::Effect,
        nodes,
        baseline,
        effect_execute_baseline,
    }
}

fn fresh_id(next_id: &mut u32) -> u32 {
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    id
}

fn import_stmts(
    fighter: Option<&str>,
    stmts: &[EffectStmt],
    next_id: &mut u32,
    depth: usize,
    baseline: &mut Vec<AcmdStmt>,
    effect_execute_baseline: &mut Vec<Vec<EffectMacro>>,
    execute_context: bool,
) -> Vec<MoveNode> {
    let mut clean_execute_heads = std::collections::BTreeSet::new();
    if stmts.iter().any(|stmt| matches!(stmt, EffectStmt::Cond { header, .. }
        if header.contains("is_excute") || super::timeline::source_condition(fighter, header).is_some())) {
        let mut lines = Vec::new();
        let mut starts = Vec::new();
        for statement in stmts {
            starts.push(lines.len());
            lines.extend(crate::acmd::emit_stmts(&[effect_stmt_to_acmd(statement)], "")
                .into_iter().flat_map(|line| line.lines().map(str::to_owned).collect::<Vec<_>>()));
        }
        let refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
        let heads = crate::acmd::unattributed_source_branch_lines(&refs);
        clean_execute_heads.extend(starts.iter().enumerate().filter_map(|(index, line)| heads.contains(line).then_some(index)));
    }
    let mut out = Vec::with_capacity(stmts.len());
    let mut index = 0;
    while index < stmts.len() {
        let stmt = &stmts[index];
        if depth < MAX_NESTING {
            if let EffectStmt::Cond { header, body } = stmt {
                if let Some(condition) = super::timeline::import_condition(fighter, header)
                    .filter(|condition| {
                        super::timeline::condition_allowed_on_track(Track::Effect, condition)
                    })
                    .filter(|condition| {
                        !super::timeline::contains_execute_condition(condition)
                            || clean_execute_heads.contains(&index)
                    })
                    .or_else(|| {
                        (super::timeline::import_condition(fighter, header).is_none()
                            && super::timeline::source_branch_allowed(
                                header,
                                &out,
                                clean_execute_heads.contains(&index),
                            ))
                        .then(|| super::timeline::source_condition(fighter, header))
                        .flatten()
                    })
                {
                    let source = matches!(condition, super::model::MoveCondition::Source { .. });
                    let paired_else = stmts.get(index + 1).and_then(|next| match next {
                        EffectStmt::Cond { header, body } if header.trim() == "else {" => {
                            Some(body.as_slice())
                        }
                        _ => None,
                    });
                    let followed_by_other_else = stmts.get(index + 1).is_some_and(|next| {
                        matches!(next, EffectStmt::Cond { header, .. } if header.trim() != "else {" && header.trim_start().starts_with("else"))
                    });
                    // Match the game-track importer: MoveCondition has no syntax node for an
                    // empty else or else-if chain, so retain the original statements. A source
                    // header needs no pair: the else stays its own source branch.
                    let paired_else = paired_else.filter(|body| !(source && body.is_empty()));
                    if source
                        || !followed_by_other_else
                            && !paired_else.is_some_and(|body| body.is_empty())
                    {
                        if let super::model::MoveCondition::Source { header } = &condition {
                            baseline.push(super::timeline::source_condition_witness(header));
                        }
                        let then_nodes = import_stmts(
                            fighter,
                            body,
                            next_id,
                            depth + 1,
                            baseline,
                            effect_execute_baseline,
                            execute_context
                                || matches!(condition, super::model::MoveCondition::IsExecute),
                        );
                        let else_nodes = paired_else
                            .map(|body| {
                                import_stmts(
                                    fighter,
                                    body,
                                    next_id,
                                    depth + 1,
                                    baseline,
                                    effect_execute_baseline,
                                    execute_context,
                                )
                            })
                            .unwrap_or_default();
                        out.push(MoveNode {
                            id: fresh_id(next_id),
                            kind: NodeKind::Branch {
                                condition,
                                then_nodes,
                                else_nodes,
                            },
                        });
                        index += if paired_else.is_some() { 2 } else { 1 };
                        continue;
                    }
                }
            }
        }

        let kind = match stmt {
            EffectStmt::BareEffect(command)
                if execute_context && command_lines_for_fighter(fighter, command).is_ok() =>
            {
                NodeKind::BareCommand {
                    command: Box::new(crate::data::ExcuteStmt::Effect(command.clone())),
                }
            }
            EffectStmt::BareNativeCall(call) if call.validate_for_fighter(fighter).is_ok() => {
                NodeKind::BareCommand {
                    command: Box::new(crate::data::ExcuteStmt::NativeCall(call.clone())),
                }
            }
            EffectStmt::Frame(frame) if frame.is_finite() => NodeKind::Frame { frame: *frame },
            EffectStmt::NativeMotionLoopSync if depth == 0 => NodeKind::NativeMotionLoopSync,
            EffectStmt::NativeExecuteFrame(frame)
                if depth == 0 && super::timeline::valid_native_execute_frame(*frame) =>
            {
                NodeKind::NativeExecuteFrame { frame: *frame }
            }
            EffectStmt::Wait(frames) if frames.is_finite() => NodeKind::Wait { frames: *frames },
            // The motion loop is the whole script: its body is imported as the root it replaces.
            EffectStmt::Loop {
                count: 1,
                body,
                motion_loop: crate::data::MotionLoop::SyncMotion,
            } if depth == 0
                && index + 1 == stmts.len()
                && !body.iter().any(|stmt| {
                    matches!(
                        stmt,
                        EffectStmt::NativeMotionLoopSync
                            | EffectStmt::Loop {
                                motion_loop: crate::data::MotionLoop::Clear
                                    | crate::data::MotionLoop::SyncMotion,
                                ..
                            }
                    )
                }) =>
            {
                NodeKind::Repeat {
                    motion_loop: crate::data::MotionLoop::SyncMotion,
                    count: 1,
                    body: import_stmts(
                        fighter,
                        body,
                        next_id,
                        depth,
                        baseline,
                        effect_execute_baseline,
                        execute_context,
                    ),
                }
            }
            EffectStmt::Loop {
                count,
                body,
                motion_loop: crate::data::MotionLoop::No,
            } if depth < MAX_NESTING && *count > 0 && *count <= MAX_REPEAT => NodeKind::Repeat {
                motion_loop: crate::data::MotionLoop::No,
                count: *count as u32,
                body: import_stmts(
                    fighter,
                    body,
                    next_id,
                    depth + 1,
                    baseline,
                    effect_execute_baseline,
                    execute_context,
                ),
            },
            EffectStmt::Excute(commands)
                if !commands.is_empty() && commands.len() <= MAX_COMMANDS_PER_EXECUTE =>
            {
                let classified = commands
                    .iter()
                    .map(|command| {
                        if command_lines_for_fighter(fighter, command).is_ok() {
                            EffectCommandSlot::Editable(command.clone())
                        } else {
                            EffectCommandSlot::Preserved(command.clone())
                        }
                    })
                    .collect::<Vec<_>>();
                let editable_count = classified
                    .iter()
                    .filter(|command| matches!(command, EffectCommandSlot::Editable(_)))
                    .count();
                let needs_follow_work_witness = commands
                    .iter()
                    .any(|command| matches!(command, EffectMacro::EffectFollowWork { .. }));
                if editable_count == commands.len() && !needs_follow_work_witness {
                    NodeKind::EffectExecute {
                        commands: commands.clone(),
                    }
                } else if editable_count > 0 {
                    let source_wrapper = effect_stmt_to_acmd(stmt);
                    if effect_wrapper_matches_source(commands, &source_wrapper) {
                        baseline.push(source_wrapper);
                        effect_execute_baseline.push(commands.clone());
                        NodeKind::MixedEffectExecute {
                            commands: classified,
                            source_commands: commands.clone(),
                        }
                    } else {
                        // Do not create an editable slot map unless the serialized wrapper
                        // reparses into the same typed/raw command sequence.
                        baseline.push(source_wrapper.clone());
                        NodeKind::Preserved {
                            statement: source_wrapper,
                        }
                    }
                } else {
                    baseline.push(effect_stmt_to_acmd(stmt));
                    NodeKind::Preserved {
                        statement: baseline.last().unwrap().clone(),
                    }
                }
            }
            _ => {
                baseline.push(effect_stmt_to_acmd(stmt));
                NodeKind::Preserved {
                    statement: baseline.last().unwrap().clone(),
                }
            }
        };
        out.push(MoveNode {
            id: fresh_id(next_id),
            kind,
        });
        index += 1;
    }
    out
}

/// Validate an authored typed effect macro and render it through the existing emitter.
pub fn command_lines(command: &EffectMacro) -> Result<Vec<String>, String> {
    validate_command(command)?;
    // New timeline commands may leave the ABI-specific tail blank. Fill only the defaults
    // published by the measured layout table; never guess an arity or accept source tokens.
    let mut rendered_command = command.clone();
    if let EffectMacro::Effect {
        spawn_func,
        extra_args,
        flip_axis,
        ..
    } = &mut rendered_command
    {
        let layout = crate::acmd::effect_spawn_layout(spawn_func)
            .ok_or_else(|| format!("unsupported effect spawn macro {spawn_func:?}"))?;
        if extra_args.is_empty() && !layout.tail.is_empty() {
            *extra_args = layout
                .tail
                .iter()
                .map(|(default, _)| (*default).to_owned())
                .collect();
        }
        if let (Some(axis), Some(index)) = (
            flip_axis.as_ref(),
            crate::acmd::effect_flip_axis_tail_index(spawn_func),
        ) {
            if let Some(token) = extra_args.get_mut(index) {
                *token = axis.clone();
            }
        }
    }
    let source = crate::acmd::preview_effect_macro(&rendered_command);
    let lines = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Err("effect command emitted no source".into());
    }
    let script = AcmdScript {
        stmts: vec![AcmdStmt::Excute(
            lines
                .iter()
                .cloned()
                .map(crate::data::ExcuteStmt::Raw)
                .collect(),
        )],
    };
    let wrapped = crate::acmd::preview_effect_structure_fn(&script, "validation");
    syn::parse_file(&wrapped)
        .map_err(|err| format!("generated effect command failed Rust parsing: {err}"))?;
    Ok(lines)
}

/// Render a command only when any owner-scoped source preset is valid for this exact fighter.
/// Generic `command_lines` remains useful for rendering a known tuple, but it must not authorize
/// ownerless import or a tuple borrowed from another fighter.
pub fn command_lines_for_fighter(
    fighter: Option<&str>,
    command: &EffectMacro,
) -> Result<Vec<String>, String> {
    if let EffectMacro::EffectFollowWork { work_symbol, .. } = command {
        let actual_owner = fighter.unwrap_or("<ownerless>");
        let preset =
            crate::data::EffectFollowWorkPreset::by_work_symbol(work_symbol).ok_or_else(|| {
                format!(
                    "fighter {actual_owner:?} uses unknown EFFECT_FOLLOW_WORK slot {work_symbol:?}"
                )
            })?;
        let expected = format!(
            "expected owner {:?} and instance WorkInt slot {}",
            preset.fighter, preset.work_symbol
        );
        if actual_owner != preset.fighter
            || crate::data::EffectFollowWorkPreset::from_source_parts(actual_owner, work_symbol)
                .is_none()
        {
            return Err(format!(
                "fighter {actual_owner:?} cannot use EFFECT_FOLLOW_WORK slot {}: {expected}",
                preset.id
            ));
        }
        let entry = crate::gimmicks::registry::resolve(
            actual_owner,
            preset.work_symbol,
            crate::gimmicks::registry::IdentifierKind::WorkInt,
        )
        .map_err(|err| {
            format!(
                "fighter {actual_owner:?} cannot use EFFECT_FOLLOW_WORK slot {}: {expected}; {err}",
                preset.id
            )
        })?;
        if !entry.group.is_empty() {
            return Err(format!(
                "fighter {actual_owner:?} cannot use EFFECT_FOLLOW_WORK slot {}: {expected}; slot has non-empty group {:?}",
                preset.id, entry.group
            ));
        }
    }
    if let EffectMacro::Control(crate::data::EffectControl::OffKindWork { preset_id }) = command {
        let preset = crate::data::EffectOffKindWorkPreset::by_id(preset_id).ok_or_else(|| {
            format!(
                "fighter {:?} uses unknown off-kind Work preset {preset_id:?}",
                fighter.unwrap_or("<ownerless>")
            )
        })?;
        let actual_owner = fighter.unwrap_or("<ownerless>");
        let expected = format!(
            "expected owner {:?} and instance WorkInt slot {}",
            preset.fighter, preset.work_symbol
        );
        if actual_owner != preset.fighter {
            return Err(format!(
                "fighter {actual_owner:?} cannot use off-kind Work preset {preset_id:?}: {expected}"
            ));
        }
        let entry = crate::gimmicks::registry::resolve(
            actual_owner,
            preset.work_symbol,
            crate::gimmicks::registry::IdentifierKind::WorkInt,
        )
        .map_err(|err| {
            format!(
                "fighter {actual_owner:?} cannot use off-kind Work preset {preset_id:?}: {expected}; {err}"
            )
        })?;
        if !entry.group.is_empty() {
            return Err(format!(
                "fighter {actual_owner:?} cannot use off-kind Work preset {preset_id:?}: {expected}; slot has non-empty group {:?}",
                entry.group
            ));
        }
    }
    if let EffectMacro::NativeCall(call) = command {
        call.validate_for_fighter(fighter)?;
    }
    command_lines(command)
}

fn validate_command(command: &EffectMacro) -> Result<(), String> {
    if let EffectMacro::NativeCall(call) = command {
        return call.validate();
    }
    // The imported trail retains a complete source statement, so it cannot pass the ordinary
    // no-source-text JSON gate. Admit only this exact, independently validated literal call;
    // every argument and its Rust statement shape are checked by the trail owner.
    if matches!(command, EffectMacro::AfterImage { .. }) {
        return crate::move_nodes::trails::settings(command).map(|_| ());
    }
    let value = serde_json::to_value(command).map_err(|err| err.to_string())?;
    validate_json(&value, "effect")?;
    match command {
        EffectMacro::EffectFollowWork {
            work_symbol,
            bone_name,
            offset,
            rotation,
            scale,
            ..
        } => {
            if crate::data::EffectFollowWorkPreset::by_work_symbol(work_symbol).is_none() {
                return Err("EFFECT_FOLLOW_WORK slot is not in the measured source table".into());
            }
            validate_name(bone_name, "EFFECT_FOLLOW_WORK bone")?;
            validate_floats(offset, "EFFECT_FOLLOW_WORK position")?;
            validate_floats(rotation, "EFFECT_FOLLOW_WORK rotation")?;
            validate_number(*scale, "EFFECT_FOLLOW_WORK scale")?;
        }
        EffectMacro::Effect {
            effect_name,
            effect_name_alt,
            spawn_func,
            bone_name,
            offset,
            rotation,
            scale,
            follows_bone,
            extra_args,
            flip_axis,
        } => {
            let layout = crate::acmd::effect_spawn_layout(spawn_func)
                .ok_or_else(|| format!("unsupported effect spawn macro {spawn_func:?}"))?;
            validate_name(effect_name, "effect name")?;
            validate_name(bone_name, "bone name")?;
            if let Some(alt) = effect_name_alt {
                validate_name(alt, "alternate effect name")?;
            }
            if layout.flip != effect_name_alt.is_some() {
                return Err("alternate effect name does not match the spawn macro layout".into());
            }
            if layout.follows_bone != *follows_bone {
                return Err("follows_bone does not match the spawn macro layout".into());
            }
            validate_floats(offset, "effect offset")?;
            validate_floats(rotation, "effect rotation")?;
            validate_number(*scale, "effect scale")?;
            if !extra_args.is_empty() && extra_args.len() != layout.tail.len() {
                return Err(format!(
                    "{} requires {} trailing arguments; got {}",
                    layout.name,
                    layout.tail.len(),
                    extra_args.len()
                ));
            }
            if let Some(axis) = flip_axis {
                if !layout.flip {
                    return Err("flip_axis is only valid for a flipped spawn macro".into());
                }
                validate_spawn_integer_token(spawn_func, axis)?;
            }
            if !extra_args.is_empty() {
                for (index, ((_, kind), token)) in layout.tail.iter().zip(extra_args).enumerate() {
                    validate_tail_token(token, *kind, spawn_func)
                        .map_err(|reason| format!("tail[{index}]: {reason}"))?;
                }
            }
        }
        EffectMacro::EffectOffKind { effect_name, .. } => {
            validate_name(effect_name, "effect name")?;
        }
        EffectMacro::LastEffectSetRate { rate } => validate_number(*rate, "effect rate")?,
        EffectMacro::LastEffectSetOffsetToCameraFlat { offset } => {
            validate_number(*offset, "camera offset")?;
        }
        EffectMacro::LastEffectSetColor { rgb } | EffectMacro::LastParticleSetColor { rgb } => {
            validate_floats(rgb, "color")?;
        }
        EffectMacro::LastEffectSetAlpha { alpha } => validate_number(*alpha, "alpha")?,
        EffectMacro::LastEffectSetScaleW { values } => {
            if !(1..=3).contains(&values.len()) {
                return Err("scale_w requires one to three values".into());
            }
            validate_floats(values, "scale_w")?;
        }
        EffectMacro::Control(crate::data::EffectControl::DetachKind { effect_name, unk }) => {
            validate_name(effect_name, "effect name")?;
            if !(-1024..=1024).contains(unk) {
                return Err("detach control value is outside the supported range".into());
            }
        }
        EffectMacro::Control(
            crate::data::EffectControl::SetDisableRenderOffsetLast
            | crate::data::EffectControl::EnableSyncInitPosLast,
        ) => {}
        EffectMacro::Control(crate::data::EffectControl::ReqScreen { preset_id }) => {
            if crate::data::EffectReqScreenPreset::by_id(preset_id).is_none() {
                return Err("req_screen preset is not in the measured source table".into());
            }
        }
        EffectMacro::Control(crate::data::EffectControl::OffKindWork { preset_id }) => {
            if crate::data::EffectOffKindWorkPreset::by_id(preset_id).is_none() {
                return Err("off-kind Work preset is not in the measured source table".into());
            }
        }
        EffectMacro::Control(crate::data::EffectControl::SetDisableCameraDepthInfluence {
            ..
        }) => {}
        EffectMacro::Color { command, color } => {
            let (needs_transition, needs_rgba) = crate::data::color_command_layout(command)
                .ok_or_else(|| format!("unsupported color command {command:?}"))?;
            if color.transition.is_some() != needs_transition || color.rgba.is_some() != needs_rgba
            {
                return Err("color payload does not match the command layout".into());
            }
            if let Some(transition) = color.transition {
                validate_number(transition, "color transition")?;
            }
            if let Some(rgba) = color.rgba {
                validate_floats(&rgba, "color component")?;
            }
        }
        EffectMacro::AfterImage { .. } | EffectMacro::NativeCall(_) => {
            unreachable!("validated above")
        }
        EffectMacro::LastEffectSetWorkInt { .. }
        | EffectMacro::Control(
            crate::data::EffectControl::DetachKindWork { .. }
            | crate::data::EffectControl::EnableArea { .. }
            | crate::data::EffectControl::UnableArea { .. },
        )
        | EffectMacro::Raw(_) => {
            return Err(
                "effect command is immutable imported source and cannot be authored".into(),
            );
        }
        EffectMacro::AfterImageOff { arg } => validate_number(*arg, "after-image off argument")?,
    }
    Ok(())
}

fn validate_json(value: &serde_json::Value, path: &str) -> Result<(), String> {
    match value {
        serde_json::Value::Null
            if matches!(
                path.rsplit('.').next(),
                Some("effect_name_alt" | "flip_axis" | "transition" | "rgba")
            ) =>
        {
            Ok(())
        }
        serde_json::Value::Null => {
            Err(format!("{path} contains a non-finite or unsupported value"))
        }
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => Ok(()),
        serde_json::Value::String(text) => {
            if text.len() > 4096 || text.contains(['\n', '\r', ';', '{', '}']) {
                Err(format!("{path} contains unsupported source text"))
            } else {
                Ok(())
            }
        }
        serde_json::Value::Array(values) => values
            .iter()
            .enumerate()
            .try_for_each(|(index, value)| validate_json(value, &format!("{path}[{index}]"))),
        serde_json::Value::Object(fields) => fields
            .iter()
            .try_for_each(|(key, value)| validate_json(value, &format!("{path}.{key}"))),
    }
}

fn validate_name(name: &str, field: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Err(format!("{field} must be a bounded ASCII resource name"))
    } else {
        Ok(())
    }
}

fn validate_number(value: f32, field: &str) -> Result<(), String> {
    if value.is_finite() && value.abs() <= MAX_TAIL_NUMBER {
        Ok(())
    } else {
        Err(format!(
            "{field} must be finite and within ±{MAX_TAIL_NUMBER}"
        ))
    }
}

fn validate_floats(values: &[f32], field: &str) -> Result<(), String> {
    for value in values {
        validate_number(*value, field)?;
    }
    Ok(())
}

fn validate_tail_token(
    token: &str,
    kind: crate::acmd::EffectSpawnTailKind,
    spawn_func: &str,
) -> Result<(), String> {
    match kind {
        crate::acmd::EffectSpawnTailKind::Number => {
            let value = token
                .parse::<f32>()
                .map_err(|_| "expected a numeric literal")?;
            validate_number(value, "effect spawn tail")
        }
        crate::acmd::EffectSpawnTailKind::Bool => match token {
            "true" | "false" => Ok(()),
            _ => Err("expected `true` or `false`".into()),
        },
        crate::acmd::EffectSpawnTailKind::Integer => {
            validate_spawn_integer_token(spawn_func, token)
        }
    }
}

fn validate_spawn_integer_token(spawn_func: &str, token: &str) -> Result<(), String> {
    let allowed = crate::acmd::effect_spawn_integer_tail_tokens(spawn_func);
    if allowed.contains(&token) {
        Ok(())
    } else {
        Err(format!(
            "integer tail for {spawn_func} must be one of its supported literal or named constants"
        ))
    }
}

pub(crate) fn effect_stmt_to_acmd(stmt: &EffectStmt) -> AcmdStmt {
    match stmt {
        EffectStmt::BareEffect(command) => {
            AcmdStmt::Bare(Box::new(crate::data::ExcuteStmt::Effect(command.clone())))
        }
        EffectStmt::Frame(frame) => AcmdStmt::Frame(*frame),
        EffectStmt::NativeExecuteFrame(frame) => AcmdStmt::NativeExecuteFrame(*frame),
        EffectStmt::NativeMotionLoopSync => AcmdStmt::NativeMotionLoopSync,
        EffectStmt::Wait(frames) => AcmdStmt::Wait(*frames),
        EffectStmt::Loop {
            count,
            body,
            motion_loop,
        } => AcmdStmt::Loop {
            motion_loop: *motion_loop,
            count: *count,
            body: body.iter().map(effect_stmt_to_acmd).collect(),
        },
        EffectStmt::Cond { header, body } => AcmdStmt::RawBlock {
            header: header.clone(),
            body: body.iter().map(effect_stmt_to_acmd).collect(),
        },
        EffectStmt::Excute(commands) => AcmdStmt::RawBlock {
            header: "if macros::is_excute(agent) {".into(),
            body: commands
                .iter()
                .map(|command| {
                    AcmdStmt::Raw(crate::acmd::preview_effect_macro(command).trim().to_owned())
                })
                .collect(),
        },
        EffectStmt::BareNativeCall(call) => {
            AcmdStmt::Bare(Box::new(crate::data::ExcuteStmt::NativeCall(call.clone())))
        }
        EffectStmt::Raw(line) => AcmdStmt::Raw(line.clone()),
    }
}

/// Prove that a mixed block's typed/raw slot classification is what the baseline source itself
/// reparses as. Text equality alone is insufficient: a Raw line could render as a call that the
/// current parser would accept as a typed `EffectMacro`, and a mutable source witness must not
/// promote that imported opaque slot to editable.
pub(crate) fn effect_wrapper_matches_source(commands: &[EffectMacro], wrapper: &AcmdStmt) -> bool {
    let expected_wrapper = effect_stmt_to_acmd(&EffectStmt::Excute(commands.to_vec()));
    if !matches!(
        (serde_json::to_value(wrapper), serde_json::to_value(expected_wrapper)),
        (Ok(actual), Ok(expected)) if actual == expected
    ) {
        return false;
    }

    let emitted = crate::acmd::preview_effect_structure_fn(
        &AcmdScript {
            stmts: vec![wrapper.clone()],
        },
        "mixed_effect_source_witness",
    );
    let reparsed = crate::acmd::parse_effect_script(&emitted);
    matches!(
        reparsed.stmts.as_slice(),
        [EffectStmt::Excute(reparsed_commands)]
            if matches!(
                (serde_json::to_value(reparsed_commands), serde_json::to_value(commands)),
                (Ok(actual), Ok(expected)) if actual == expected
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acmd::preview_effect_macro;
    use crate::data::EffectControl;
    use crate::move_nodes::model::{MoveCondition, MoveGraph};
    use crate::move_nodes::test_support as fixture;
    use crate::move_nodes::test_support::assert_json_eq;

    fn simple_effect() -> EffectMacro {
        EffectMacro::Effect {
            effect_name: "sys_hit_smoke".into(),
            effect_name_alt: None,
            spawn_func: "EFFECT".into(),
            bone_name: "top".into(),
            offset: [0.0, 1.0, 0.0],
            rotation: [0.0; 3],
            scale: 1.0,
            follows_bone: false,
            extra_args: vec![],
            flip_axis: None,
        }
    }

    fn graph(track: MoveTrack) -> MoveGraph {
        MoveGraph {
            tracks: vec![track],
            ..MoveGraph::default()
        }
    }

    fn follow_work_source(symbol: &str) -> String {
        format!(
            "unsafe extern \"C\" fn effect_work(agent: &mut L2CAgentBase) {{\nif macros::is_excute(agent) {{\nmacros::EFFECT_FOLLOW_WORK(agent, *{symbol}, Hash40::new(\"top\"), -1, 2.5, 3, -90, 180, 0, 1.2, true);\n}}\n}}"
        )
    }

    #[test]
    fn effect_follow_work_closed_parser_registry_and_mixed_witness_roundtrip() {
        use crate::data::{EffectFollowWorkPreset, EffectStmt};
        use crate::gimmicks::registry::{self, IdentifierKind};

        assert_eq!(EffectFollowWorkPreset::all().len(), 17);
        for preset in EffectFollowWorkPreset::all() {
            let entry =
                registry::resolve(preset.fighter, preset.work_symbol, IdentifierKind::WorkInt)
                    .expect("every closed slot must be registry-backed");
            assert_eq!(entry.fighter, preset.fighter);
            assert_eq!(entry.kind, IdentifierKind::WorkInt);
            assert!(entry.group.is_empty());

            let script = crate::acmd::parse_effect_script(&follow_work_source(preset.work_symbol));
            let [EffectStmt::Excute(commands)] = script.stmts.as_slice() else {
                panic!("exact direct execute fixture must parse as one wrapper");
            };
            let [EffectMacro::EffectFollowWork {
                work_symbol,
                bone_name,
                offset,
                rotation,
                scale,
                unk8,
            }] = commands.as_slice()
            else {
                panic!("closed Work slot must parse as FollowWork: {}", preset.id);
            };
            assert_eq!(work_symbol, preset.work_symbol);
            assert_eq!(bone_name, "top");
            assert_eq!(*offset, [-1.0, 2.5, 3.0]);
            // The native x/y/z order maps into the existing Effect rotation IR convention.
            assert_eq!(*rotation, [0.0, 180.0, -90.0]);
            assert_eq!(*scale, 1.2);
            assert!(*unk8);
            let emitted = preview_effect_macro(&commands[0]);
            assert!(emitted.contains(&format!("*{}", preset.work_symbol)));
            assert!(emitted.contains("EFFECT_FOLLOW_WORK"));
            let reparse = crate::acmd::parse_effect_script(&format!(
                "unsafe extern \"C\" fn effect_work(agent: &mut L2CAgentBase) {{\nif macros::is_excute(agent) {{\n{emitted}\n}}\n}}"
            ));
            assert_json_eq!(
                &reparse,
                &script,
                "full effect tree must survive source parse/emit/reparse for {}",
                preset.id
            );
            assert!(
                matches!(reparse.stmts.as_slice(), [EffectStmt::Excute(parsed)] if parsed.len() == 1 && matches!(&parsed[0], EffectMacro::EffectFollowWork { work_symbol, .. } if work_symbol == preset.work_symbol))
            );

            let track = import_for_fighter(preset.fighter, &script);
            assert_eq!(track.effect_execute_baseline.len(), 1);
            assert!(
                matches!(track.nodes.as_slice(), [MoveNode { kind: NodeKind::MixedEffectExecute { commands, source_commands }, .. }] if commands.len() == 1 && source_commands.len() == 1 && matches!(&commands[0], EffectCommandSlot::Editable(EffectMacro::EffectFollowWork { .. })))
            );
            let compiled =
                crate::move_nodes::timeline::compile(preset.fighter, "effect_work", &graph(track))
                    .expect("source-anchored native effect compiles for its exact owner");
            let output = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "effect_work");
            assert!(output.contains(&format!("*{}", preset.work_symbol)));
            assert!(!output.contains("macros::EFFECT_FOLLOW(agent"));
        }
    }

    #[test]
    fn effect_follow_work_all_known_witnesses_reject_reuse_delete_reorder_and_wrong_track() {
        use crate::data::EffectFollowWorkPreset;

        let cloud_flare = EffectFollowWorkPreset::from_source_parts(
            "cloud",
            "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE",
        )
        .unwrap();
        let cloud_lb = EffectFollowWorkPreset::from_source_parts(
            "cloud",
            "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE_LB",
        )
        .unwrap();
        let source = format!(
            "unsafe extern \"C\" fn effect_work(agent: &mut L2CAgentBase) {{\nif macros::is_excute(agent) {{\nmacros::EFFECT_FOLLOW_WORK(agent, *{}, Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1, true);\nmacros::EFFECT_FOLLOW_WORK(agent, *{}, Hash40::new(\"haver\"), 1, 0, 0, 0, 0, 0, 1, true);\n}}\n}}",
            cloud_flare.work_symbol, cloud_lb.work_symbol
        );
        let script = crate::acmd::parse_effect_script(&source);
        let track = import_for_fighter("cloud", &script);
        assert!(matches!(
            track.nodes.as_slice(),
            [MoveNode {
                kind: NodeKind::MixedEffectExecute { commands, source_commands },
                ..
            }] if commands.len() == 2 && source_commands.len() == 2
        ));
        assert_eq!(track.effect_execute_baseline.len(), 1);
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "all_known_follow_work",
            &graph(track.clone()),
        )
        .is_ok());

        let mut deleted_node = track.clone();
        deleted_node.nodes.clear();
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "deleted_follow_work_wrapper",
            &graph(deleted_node),
        )
        .is_err());

        let mut duplicated_node = track.clone();
        let mut duplicate = duplicated_node.nodes[0].clone();
        duplicate.id = duplicate.id.saturating_add(1);
        duplicated_node.nodes.push(duplicate);
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "duplicated_follow_work_wrapper",
            &graph(duplicated_node),
        )
        .is_err());

        let mut reused_witness = track.clone();
        reused_witness
            .effect_execute_baseline
            .push(reused_witness.effect_execute_baseline[0].clone());
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "reused_follow_work_witness",
            &graph(reused_witness),
        )
        .is_err());

        let mut reordered = track.clone();
        let NodeKind::MixedEffectExecute { commands, .. } = &mut reordered.nodes[0].kind else {
            unreachable!()
        };
        commands.swap(0, 1);
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "reordered_follow_work_slots",
            &graph(reordered),
        )
        .is_err());

        let mut wrong_cardinality = track.clone();
        let NodeKind::MixedEffectExecute { commands, .. } = &mut wrong_cardinality.nodes[0].kind
        else {
            unreachable!()
        };
        commands.pop();
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "wrong_follow_work_cardinality",
            &graph(wrong_cardinality),
        )
        .is_err());

        let game_source = source.replacen("effect_work", "game_work", 1);
        let parsed_game = crate::acmd::parse_acmd_script(&game_source);
        assert!(
            matches!(
                parsed_game.stmts.as_slice(),
                [AcmdStmt::Excute(commands)]
                    if commands.iter().any(|command| matches!(command, crate::data::ExcuteStmt::Raw(line) if line.contains("EFFECT_FOLLOW_WORK")))
            ),
            "the GAME parser must conserve this effect-family call as Raw"
        );
        let emitted_game = crate::acmd::preview_game_fn(&parsed_game, "follow_work_in_game");
        assert!(emitted_game.contains("EFFECT_FOLLOW_WORK"));
    }

    #[test]
    fn effect_follow_work_fails_closed_and_legacy_effect_call_keeps_source_residue() {
        let preset = crate::data::EffectFollowWorkPreset::from_source_parts(
            "cloud",
            "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE",
        )
        .expect("Cloud slot");
        let valid = follow_work_source(preset.work_symbol);
        for (name, source) in [
            (
                "unknown slot",
                valid.replace(preset.work_symbol, "UNKNOWN_WORK_SLOT"),
            ),
            (
                "wrong receiver",
                valid.replace("agent, *FIGHTER_", "other, *FIGHTER_"),
            ),
            ("expression", valid.replace("-1, 2.5", "x + 1, 2.5")),
            ("bad bool", valid.replace("1.2, true", "1.2, FLAG")),
            ("arity", valid.replace(", 1.2, true", ", 1.2")),
            ("exponent", valid.replace("2.5", "2.5e1")),
            ("too large", valid.replace("2.5", "10001")),
            (
                "nested",
                valid
                    .replace(
                        "macros::EFFECT_FOLLOW_WORK",
                        "if true { macros::EFFECT_FOLLOW_WORK",
                    )
                    .replace("1.2, true);", "1.2, true); }"),
            ),
            (
                "multiline",
                valid.replace(
                    "macros::EFFECT_FOLLOW_WORK(agent,",
                    "macros::EFFECT_FOLLOW_WORK(\nagent,",
                ),
            ),
            (
                "attributed",
                valid.replace(
                    "macros::EFFECT_FOLLOW_WORK(agent,",
                    "#[inline]\nmacros::EFFECT_FOLLOW_WORK(agent,",
                ),
            ),
            (
                "malformed neighbor",
                valid.replace(
                    "macros::EFFECT_FOLLOW_WORK(agent,",
                    "let broken = ;\nmacros::EFFECT_FOLLOW_WORK(agent,",
                ),
            ),
        ] {
            let parsed = crate::acmd::parse_effect_script(&source);
            assert!(
                !parsed.stmts.iter().any(|stmt| matches!(stmt, crate::data::EffectStmt::Excute(commands) if commands.iter().any(|command| matches!(command, EffectMacro::EffectFollowWork { .. })))),
                "{name} should not be admitted as a typed command"
            );
        }
        let bare = format!(
            "unsafe extern \"C\" fn effect_work(agent: &mut L2CAgentBase) {{\nmacros::EFFECT_FOLLOW_WORK(agent, *{}, Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1, true);\n}}",
            preset.work_symbol
        );
        let bare_parsed = crate::acmd::parse_effect_script(&bare);
        assert!(!bare_parsed.stmts.iter().any(|stmt| matches!(
            stmt,
            crate::data::EffectStmt::Excute(commands)
                if commands.iter().any(|command| matches!(command, EffectMacro::EffectFollowWork { .. }))
        )));
        let unsupported_guard = valid
            .replace(
                "if macros::is_excute(agent) {",
                "if custom_condition(agent) {\nif macros::is_excute(agent) {",
            )
            .replace("\n}\n}", "\n}\n}\n}");
        let guarded = crate::acmd::parse_effect_script(&unsupported_guard);
        let guarded_track = import_for_fighter("cloud", &guarded);
        // The unknown guard stays source text; the FollowWork under it keeps its witness.
        assert!(
            matches!(
                guarded_track.nodes.first().map(|node| &node.kind),
                Some(NodeKind::Branch { condition: MoveCondition::Source { header }, then_nodes, .. })
                    if header == "if custom_condition(agent) {"
                        && matches!(then_nodes.as_slice(), [MoveNode { kind: NodeKind::MixedEffectExecute { .. }, .. }])
            ),
            "unsupported enclosing condition stays source and FollowWork keeps its witness"
        );
        let wrong_owner = crate::acmd::parse_effect_script(&valid);
        let wrong_owner_track = import_for_fighter("brave", &wrong_owner);
        assert!(matches!(
            wrong_owner_track.nodes.first().map(|node| &node.kind),
            Some(NodeKind::Preserved { .. })
        ));
        let wrong_owner_mixed_script = crate::data::EffectScript {
            stmts: vec![crate::data::EffectStmt::Excute(vec![
                match &wrong_owner.stmts[0] {
                    crate::data::EffectStmt::Excute(commands) => commands[0].clone(),
                    _ => unreachable!(),
                },
                EffectMacro::Control(EffectControl::SetDisableRenderOffsetLast),
                EffectMacro::Raw("unknown_native_effect(agent);".into()),
            ])],
        };
        let wrong_owner_mixed = import_for_fighter("brave", &wrong_owner_mixed_script);
        assert!(
            matches!(wrong_owner_mixed.nodes.first().map(|node| &node.kind),
            Some(NodeKind::MixedEffectExecute { commands, .. }) if matches!(commands.as_slice(), [
                EffectCommandSlot::Preserved(EffectMacro::EffectFollowWork { .. }),
                EffectCommandSlot::Editable(EffectMacro::Control(EffectControl::SetDisableRenderOffsetLast)),
                EffectCommandSlot::Preserved(EffectMacro::Raw(_)),
            ]))
        );
        assert!(
            crate::move_nodes::timeline::compile(
                "brave",
                "wrong_owner_mixed",
                &graph(wrong_owner_mixed.clone())
            )
            .is_ok(),
            "wrong-owner imported command remains immutable inside a mixed source block"
        );
        let mut wrong_owner_promoted = wrong_owner_mixed;
        let NodeKind::MixedEffectExecute { commands, .. } = &mut wrong_owner_promoted.nodes[0].kind
        else {
            unreachable!()
        };
        let command = match &commands[0] {
            EffectCommandSlot::Preserved(command) => command.clone(),
            _ => panic!("wrong-owner source must remain preserved"),
        };
        commands[0] = EffectCommandSlot::Editable(command);
        assert!(crate::move_nodes::timeline::compile(
            "brave",
            "wrong_owner_promoted",
            &graph(wrong_owner_promoted),
        )
        .is_err());

        let source = format!(
            "unsafe extern \"C\" fn effect_work(agent: &mut L2CAgentBase) {{\nif macros::is_excute(agent) {{\nmacros::EFFECT(agent, Hash40::new(\"sys_first\"), Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1);\nmacros::EFFECT_FOLLOW_WORK(agent, *{}, Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1, false);\nmacros::LAST_EFFECT_SET_RATE(agent, 0.5);\nmacros::EFFECT(agent, Hash40::new(\"sys_hit\"), Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1);\n}}\n}}",
            preset.work_symbol,
        );
        let script = crate::acmd::parse_effect_script(&source);
        assert_eq!(
            script.to_effect_calls().len(),
            2,
            "Work-follow is not flattened to EffectCall"
        );
        assert_eq!(script.call_macro_ordinals(), vec![0, 1]);
        let (calls, residue) = script.to_effect_calls_and_residue();
        let legacy = crate::acmd::preview_effect_fn(&calls, "effect_work", &[], &residue);
        let work_pos = legacy
            .find("EFFECT_FOLLOW_WORK")
            .expect("Work-follow is retained");
        let rate_pos = legacy
            .find("LAST_EFFECT_SET_RATE")
            .expect("rate stays residue, not attached to Work as an EffectCall");
        let next_spawn_pos = legacy
            .find("sys_hit")
            .expect("neighbor ordinary spawn remains");
        assert!(work_pos < rate_pos && rate_pos < next_spawn_pos);
        assert!(legacy.contains(preset.work_symbol));
        assert!(legacy.contains("EFFECT(agent, Hash40::new(\"sys_hit\")"));
    }

    #[test]
    fn effect_follow_work_witness_locks_identity_but_allows_bounded_transform_edits() {
        let preset = crate::data::EffectFollowWorkPreset::from_source_parts(
            "cloud",
            "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE",
        )
        .expect("Cloud slot");
        let script = crate::acmd::parse_effect_script(&follow_work_source(preset.work_symbol));
        let imported = import_for_fighter("cloud", &script);
        let json = serde_json::to_value(&imported).unwrap();
        let restored: MoveTrack = serde_json::from_value(json).unwrap();
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "roundtrip",
            &graph(restored.clone())
        )
        .is_ok());

        let mut transformed = restored.clone();
        let NodeKind::MixedEffectExecute { commands, .. } = &mut transformed.nodes[0].kind else {
            panic!("FollowWork import is witness-anchored mixed Execute even without Raw")
        };
        let EffectCommandSlot::Editable(EffectMacro::EffectFollowWork {
            offset,
            rotation,
            scale,
            ..
        }) = &mut commands[0]
        else {
            panic!("expected editable Work-follow")
        };
        *offset = [4.0, -2.5, 0.75];
        *rotation = [45.0, 90.0, -30.0];
        *scale = 0.85;
        let compiled = crate::move_nodes::timeline::compile(
            "cloud",
            "transformed",
            &graph(transformed.clone()),
        )
        .expect("only transform values are editable")
        .remove(0)
        .1;
        let output = crate::acmd::preview_effect_structure_fn(&compiled, "transformed");
        assert!(output.contains("4, -2.5, 0.75, -30, 90, 45, 0.85, true"));
        assert!(output.contains(preset.work_symbol));

        let mutate = |field: &str| {
            let mut changed = restored.clone();
            let NodeKind::MixedEffectExecute { commands, .. } = &mut changed.nodes[0].kind else {
                unreachable!()
            };
            let EffectCommandSlot::Editable(EffectMacro::EffectFollowWork {
                work_symbol,
                bone_name,
                unk8,
                ..
            }) = &mut commands[0]
            else {
                unreachable!()
            };
            match field {
                "slot" => {
                    *work_symbol =
                        "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_CLIMHAZZARD_SWORD".into()
                }
                "bone" => *bone_name = "haver".into(),
                "bool" => *unk8 = false,
                _ => unreachable!(),
            }
            changed
        };
        for field in ["slot", "bone", "bool"] {
            assert!(
                crate::move_nodes::timeline::compile("cloud", field, &graph(mutate(field)))
                    .is_err(),
                "{field} is locked to the source witness"
            );
        }
        let mut family_changed = restored.clone();
        if let NodeKind::MixedEffectExecute { commands, .. } = &mut family_changed.nodes[0].kind {
            commands[0] = EffectCommandSlot::Editable(simple_effect());
        }
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "family_changed",
            &graph(family_changed)
        )
        .is_err());
        let mut witness_changed = restored.clone();
        if let EffectMacro::EffectFollowWork { bone_name, .. } =
            &mut witness_changed.effect_execute_baseline[0][0]
        {
            *bone_name = "haver".into();
        }
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "witness_changed",
            &graph(witness_changed),
        )
        .is_err());
        let mut nonfinite = restored.clone();
        if let NodeKind::MixedEffectExecute { commands, .. } = &mut nonfinite.nodes[0].kind {
            if let EffectCommandSlot::Editable(EffectMacro::EffectFollowWork { offset, .. }) =
                &mut commands[0]
            {
                offset[0] = f32::NAN;
            }
        }
        assert!(
            crate::move_nodes::timeline::compile("cloud", "nonfinite", &graph(nonfinite)).is_err()
        );
        let mut out_of_bounds = restored.clone();
        if let NodeKind::MixedEffectExecute { commands, .. } = &mut out_of_bounds.nodes[0].kind {
            if let EffectCommandSlot::Editable(EffectMacro::EffectFollowWork { scale, .. }) =
                &mut commands[0]
            {
                *scale = 10_001.0;
            }
        }
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "out_of_bounds",
            &graph(out_of_bounds)
        )
        .is_err());

        let mut wrong_track = restored.clone();
        wrong_track.track = Track::Game;
        assert!(
            crate::move_nodes::timeline::compile("cloud", "wrong_track", &graph(wrong_track))
                .is_err()
        );

        let mut pure_bypass = restored.clone();
        let pure_commands = match &pure_bypass.nodes[0].kind {
            NodeKind::MixedEffectExecute {
                source_commands, ..
            } => source_commands.clone(),
            _ => unreachable!(),
        };
        pure_bypass.nodes[0].kind = NodeKind::EffectExecute {
            commands: pure_commands,
        };
        pure_bypass.effect_execute_baseline.clear();
        assert!(
            crate::move_nodes::timeline::compile("cloud", "pure_bypass", &graph(pure_bypass))
                .is_err()
        );

        let mut witness_tamper = restored.clone();
        if let EffectMacro::EffectFollowWork { work_symbol, .. } =
            &mut witness_tamper.effect_execute_baseline[0][0]
        {
            *work_symbol =
                "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_CLIMHAZZARD_SWORD".into();
        } else {
            unreachable!()
        }
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "witness_tamper",
            &graph(witness_tamper)
        )
        .is_err());

        let with_raw = crate::data::EffectScript {
            stmts: vec![crate::data::EffectStmt::Excute(vec![
                match &restored.nodes[0].kind {
                    NodeKind::MixedEffectExecute {
                        source_commands, ..
                    } => source_commands[0].clone(),
                    _ => unreachable!(),
                },
                EffectMacro::Raw("unknown_native_effect(agent);".into()),
            ])],
        };
        let mut promotion = import_for_fighter("cloud", &with_raw);
        let NodeKind::MixedEffectExecute {
            commands,
            source_commands,
        } = &mut promotion.nodes[0].kind
        else {
            unreachable!()
        };
        let original = source_commands[0].clone();
        commands[1] = EffectCommandSlot::Editable(original.clone());
        source_commands[1] = original;
        assert!(
            crate::move_nodes::timeline::compile("cloud", "raw_promotion", &graph(promotion))
                .is_err(),
            "a formerly preserved source slot cannot be promoted into FollowWork"
        );
    }

    #[test]
    fn off_kind_work_import_and_pure_mixed_compiles_require_owner_context() {
        let preset = crate::data::EffectOffKindWorkPreset::by_id("cloud_sword_flare_ft")
            .expect("measured Cloud tuple");
        let off_kind = EffectMacro::Control(EffectControl::OffKindWork {
            preset_id: preset.id.to_owned(),
        });
        let source_only = EffectScript {
            stmts: vec![EffectStmt::Excute(vec![off_kind.clone()])],
        };

        let ownerless = import(&source_only);
        assert!(matches!(
            ownerless.nodes.first().map(|node| &node.kind),
            Some(NodeKind::Preserved { .. })
        ));
        assert_eq!(ownerless.baseline.len(), 1);
        assert!(
            crate::move_nodes::timeline::compile("cloud", "off_kind_ownerless", &graph(ownerless))
                .is_ok(),
            "ownerless source remains preserved, not newly authorized"
        );

        let pure = import_for_fighter("cloud", &source_only);
        assert!(matches!(
            pure.nodes.first().map(|node| &node.kind),
            Some(NodeKind::EffectExecute { .. })
        ));
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "off_kind_pure",
            &graph(pure.clone())
        )
        .is_ok());
        assert!(
            crate::move_nodes::timeline::compile("brave", "off_kind_pure", &graph(pure)).is_err()
        );

        let mixed_source = EffectScript {
            stmts: vec![EffectStmt::Excute(vec![
                off_kind,
                EffectMacro::Control(EffectControl::SetDisableRenderOffsetLast),
                EffectMacro::Raw("EffectModule::is_exist_effect(agent.module_accessor, 0);".into()),
            ])],
        };
        let ownerless_mixed = import(&mixed_source);
        assert!(matches!(
            ownerless_mixed.nodes.first().map(|node| &node.kind),
            Some(NodeKind::MixedEffectExecute { commands, .. })
                if matches!(commands.as_slice(), [
                    EffectCommandSlot::Preserved(EffectMacro::Control(
                        EffectControl::OffKindWork { .. }
                    )),
                    EffectCommandSlot::Editable(EffectMacro::Control(
                        EffectControl::SetDisableRenderOffsetLast
                    )),
                    EffectCommandSlot::Preserved(EffectMacro::Raw(_)),
                ])
        ));
        // Restoring owner context cannot silently reclassify a preserved mixed slot as editable.
        let reclassified = crate::move_nodes::timeline::compile(
            "cloud",
            "off_kind_ownerless_mixed",
            &graph(ownerless_mixed.clone()),
        )
        .expect_err("ownerless mixed slots cannot be promoted after import");
        assert!(reclassified.iter().any(|error| {
            error
                .reason
                .contains("preserved effect command must remain unchanged in its imported slot")
        }));
        assert!(
            crate::move_nodes::timeline::compile(
                "mario",
                "off_kind_ownerless_mixed",
                &graph(ownerless_mixed)
            )
            .is_ok(),
            "unsupported native source stays immutable under the same classification"
        );

        let mixed = import_for_fighter("cloud", &mixed_source);
        assert!(matches!(
            mixed.nodes.first().map(|node| &node.kind),
            Some(NodeKind::MixedEffectExecute { .. })
        ));
        assert!(crate::move_nodes::timeline::compile(
            "cloud",
            "off_kind_mixed",
            &graph(mixed.clone())
        )
        .is_ok());
        assert!(
            crate::move_nodes::timeline::compile("brave", "off_kind_mixed", &graph(mixed)).is_err()
        );
    }

    fn round_trip(fighter: &str, script: &EffectScript) -> EffectScript {
        let track = import_for_fighter(fighter, script);
        let compiled = crate::move_nodes::timeline::compile(fighter, "attack11", &graph(track))
            .unwrap()
            .remove(0)
            .1;
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled, "attack11");
        syn::parse_file(&emitted).unwrap();
        crate::acmd::parse_effect_script(&emitted)
    }

    #[test]
    fn mixed_effect_execute_edits_valid_slots_and_anchors_opaque_neighbors() {
        let script = EffectScript {
            stmts: vec![EffectStmt::Excute(vec![
                simple_effect(),
                EffectMacro::Raw("mystery_effect(agent);".into()),
            ])],
        };
        let imported = import_for_fighter("mario", &script);
        let [node] = imported.nodes.as_slice() else {
            panic!("one effect execute wrapper should import as one node");
        };
        let NodeKind::MixedEffectExecute {
            commands,
            source_commands,
        } = &node.kind
        else {
            panic!("a supported call beside opaque source should be mixed");
        };
        assert!(matches!(
            commands.as_slice(),
            [
                EffectCommandSlot::Editable(EffectMacro::Effect { .. }),
                EffectCommandSlot::Preserved(EffectMacro::Raw(_)),
            ]
        ));
        assert_eq!(source_commands.len(), 2);
        assert_eq!(imported.baseline.len(), 1);
        let accounting = crate::move_nodes::coverage::track_summary(&imported);
        assert_eq!(accounting.node_counts.mixed_effect_execute, 1);
        assert_eq!(accounting.editable_command_count, 1);
        assert_eq!(accounting.preserved_command_count, 1);
        assert_eq!(
            crate::move_nodes::live_support::source_only_tracks(&graph(imported.clone()))
                .collect::<Vec<_>>(),
            vec![Track::Effect]
        );

        let mut edited = imported.clone();
        let NodeKind::MixedEffectExecute { commands, .. } = &mut edited.nodes[0].kind else {
            unreachable!()
        };
        let EffectCommandSlot::Editable(EffectMacro::Effect { effect_name, .. }) = &mut commands[0]
        else {
            unreachable!()
        };
        *effect_name = "sys_hit_spark".into();
        let compiled = crate::move_nodes::timeline::compile("mario", "attack11", &graph(edited))
            .expect("typed effect fields can change without replacing opaque slots")
            .remove(0)
            .1;
        let reparsed = crate::acmd::parse_effect_script(&crate::acmd::preview_effect_structure_fn(
            &compiled, "attack11",
        ));
        let EffectStmt::Excute(commands) = &reparsed.stmts[0] else {
            panic!("mixed block should remain an execute wrapper");
        };
        assert!(matches!(commands.as_slice(), [
            EffectMacro::Effect { effect_name, .. },
            EffectMacro::Raw(source),
        ] if effect_name == "sys_hit_spark" && source == "mystery_effect(agent);"));
    }

    #[test]
    fn duplicate_source_mixed_wrappers_consume_distinct_witness_entries() {
        let wrapper = EffectStmt::Excute(vec![
            simple_effect(),
            EffectMacro::Raw("mystery_effect(agent);".into()),
        ]);
        let imported = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![wrapper.clone(), wrapper],
            },
        );
        assert_eq!(imported.nodes.len(), 2);
        assert_eq!(imported.baseline.len(), 2);
        assert_eq!(imported.effect_execute_baseline.len(), 2);
        assert!(crate::move_nodes::timeline::compile(
            "mario",
            "attack11",
            &graph(imported.clone()),
        )
        .is_ok());

        let mut missing_claim = imported;
        missing_claim.effect_execute_baseline.pop();
        assert!(
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(missing_claim),)
                .is_err()
        );
    }

    #[test]
    fn generated_skyline_control_and_read_only_sdk_call_form_a_mixed_block() {
        // Source-derived fixture: the native control is present in Ken/FinalHit.txt and the
        // read-only SDK query in Reflet/CatchWait.txt in the pinned public ACMD cache.
        let source = r#"unsafe extern "C" fn effect_mixed_fixture(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        EffectModule::set_disable_render_offset_last(agent.module_accessor);
        EffectModule::is_exist_effect(agent.module_accessor, 0);
    }
}"#;
        let parsed = crate::acmd::parse_effect_script(source);
        let imported = import_for_fighter("mario", &parsed);
        let NodeKind::MixedEffectExecute { commands, .. } = &imported.nodes[0].kind else {
            panic!("source-derived native control and read-only query should be mixed");
        };
        assert!(matches!(commands.as_slice(), [
            EffectCommandSlot::Editable(EffectMacro::Control(
                crate::data::EffectControl::SetDisableRenderOffsetLast
            )),
            EffectCommandSlot::Preserved(EffectMacro::Raw(line)),
        ] if line == "EffectModule::is_exist_effect(agent.module_accessor, 0);"));
        let compiled =
            crate::move_nodes::timeline::compile("mario", "mixed_fixture", &graph(imported))
                .expect("read-only SDK source must remain in the compiling source wrapper");
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "mixed_fixture");
        assert!(emitted
            .contains("EffectModule::set_disable_render_offset_last(agent.module_accessor);"));
        assert!(emitted.contains("EffectModule::is_exist_effect(agent.module_accessor, 0);"));
    }

    #[test]
    fn effect_import_preserves_homogeneous_empty_oversized_and_invalid_blocks() {
        let all_known = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Excute(vec![
                    simple_effect(),
                    EffectMacro::Control(EffectControl::SetDisableRenderOffsetLast),
                ])],
            },
        );
        assert!(matches!(
            all_known.nodes[0].kind,
            NodeKind::EffectExecute { .. }
        ));
        assert!(all_known.baseline.is_empty());

        let all_unknown = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Excute(vec![
                    EffectMacro::Raw(
                        "EffectModule::is_exist_effect(agent.module_accessor, 0);".into(),
                    ),
                    EffectMacro::LastEffectSetWorkInt {
                        work: "opaque_slot".into(),
                    },
                ])],
            },
        );
        assert!(matches!(
            all_unknown.nodes[0].kind,
            NodeKind::Preserved { .. }
        ));
        assert_eq!(all_unknown.baseline.len(), 1);

        let empty = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Excute(Vec::new())],
            },
        );
        assert!(matches!(empty.nodes[0].kind, NodeKind::Preserved { .. }));

        let oversized = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Excute(vec![
                    simple_effect();
                    MAX_COMMANDS_PER_EXECUTE + 1
                ])],
            },
        );
        assert!(matches!(
            oversized.nodes[0].kind,
            NodeKind::Preserved { .. }
        ));

        let mut invalid = simple_effect();
        let EffectMacro::Effect { scale, .. } = &mut invalid else {
            unreachable!()
        };
        *scale = 10_001.0;
        let typed_but_invalid = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Excute(vec![simple_effect(), invalid])],
            },
        );
        assert!(matches!(
            &typed_but_invalid.nodes[0].kind,
            NodeKind::MixedEffectExecute { commands, .. }
                if matches!(commands.as_slice(), [
                    EffectCommandSlot::Editable(_),
                    EffectCommandSlot::Preserved(EffectMacro::Effect { .. }),
                ])
        ));
    }

    #[test]
    fn nested_mixed_effect_import_counts_slots_without_flattening_control_flow() {
        let imported = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Loop {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 2,
                    body: vec![EffectStmt::Cond {
                        header: "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {".into(),
                        body: vec![EffectStmt::Excute(vec![
                            simple_effect(),
                            EffectMacro::Raw("EffectModule::is_exist_effect(agent.module_accessor, 0);".into()),
                        ])],
                    }],
                }],
            },
        );
        assert!(matches!(
            imported.nodes.as_slice(),
            [MoveNode {
                kind: NodeKind::Repeat {
                    body,
                    ..
                },
                ..
            }] if matches!(body.as_slice(), [MoveNode {
                kind: NodeKind::Branch { then_nodes, .. },
                ..
            }] if matches!(then_nodes.as_slice(), [MoveNode {
                kind: NodeKind::MixedEffectExecute { .. },
                ..
            }]))
        ));
        let summary = crate::move_nodes::coverage::track_summary(&imported);
        assert_eq!(summary.node_counts.repeat, 1);
        assert_eq!(summary.node_counts.branch, 1);
        assert_eq!(summary.node_counts.mixed_effect_execute, 1);
        assert_eq!(summary.editable_command_count, 1);
        assert_eq!(summary.preserved_command_count, 1);
    }

    #[test]
    fn mixed_effect_execute_rejects_slot_and_witness_tampering() {
        let script = EffectScript {
            stmts: vec![EffectStmt::Excute(vec![
                simple_effect(),
                EffectMacro::Raw("mystery_effect(agent);".into()),
            ])],
        };
        let imported = import_for_fighter("mario", &script);

        let mut preserved_changed = imported.clone();
        let NodeKind::MixedEffectExecute { commands, .. } = &mut preserved_changed.nodes[0].kind
        else {
            unreachable!()
        };
        commands.swap(0, 1);
        assert!(crate::move_nodes::timeline::compile(
            "mario",
            "attack11",
            &graph(preserved_changed),
        )
        .is_err());

        let mut preserved_payload_changed = imported.clone();
        let NodeKind::MixedEffectExecute { commands, .. } =
            &mut preserved_payload_changed.nodes[0].kind
        else {
            unreachable!()
        };
        commands[1] = EffectCommandSlot::Preserved(EffectMacro::Raw(
            "EffectModule::is_exist_effect(agent.module_accessor, 0);".into(),
        ));
        assert!(crate::move_nodes::timeline::compile(
            "mario",
            "attack11",
            &graph(preserved_payload_changed),
        )
        .is_err());

        let mut raw_as_editable = imported.clone();
        let NodeKind::MixedEffectExecute { commands, .. } = &mut raw_as_editable.nodes[0].kind
        else {
            unreachable!()
        };
        commands[1] = EffectCommandSlot::Editable(EffectMacro::Raw(
            "EffectModule::is_exist_effect(agent.module_accessor, 0);".into(),
        ));
        assert!(
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(raw_as_editable),)
                .is_err()
        );

        let mut preserved_to_editable = imported.clone();
        let NodeKind::MixedEffectExecute { commands, .. } =
            &mut preserved_to_editable.nodes[0].kind
        else {
            unreachable!()
        };
        commands[1] = EffectCommandSlot::Editable(EffectMacro::Control(
            EffectControl::SetDisableRenderOffsetLast,
        ));
        assert!(crate::move_nodes::timeline::compile(
            "mario",
            "attack11",
            &graph(preserved_to_editable),
        )
        .is_err());

        let mut deleted = imported.clone();
        let NodeKind::MixedEffectExecute { commands, .. } = &mut deleted.nodes[0].kind else {
            unreachable!()
        };
        commands.pop();
        assert!(
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(deleted)).is_err()
        );

        let mut duplicated = imported.clone();
        let NodeKind::MixedEffectExecute { commands, .. } = &mut duplicated.nodes[0].kind else {
            unreachable!()
        };
        let duplicate = commands[0].clone();
        commands.push(duplicate);
        assert!(
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(duplicated)).is_err()
        );

        let mut family_changed = imported.clone();
        let NodeKind::MixedEffectExecute { commands, .. } = &mut family_changed.nodes[0].kind
        else {
            unreachable!()
        };
        let EffectCommandSlot::Editable(command) = &mut commands[0] else {
            unreachable!()
        };
        *command = EffectMacro::EffectOffKind {
            effect_name: "sys_hit_smoke".into(),
            fade: false,
            detach: false,
        };
        assert!(
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(family_changed),)
                .is_err()
        );

        let mut witness_changed = imported.clone();
        if let EffectMacro::Effect { effect_name, .. } =
            &mut witness_changed.effect_execute_baseline[0][0]
        {
            *effect_name = "forged_effect_name".into();
        } else {
            panic!("first typed witness must be an effect spawn");
        }
        assert!(
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(witness_changed),)
                .is_err()
        );

        let mut witness_removed = imported.clone();
        witness_removed.effect_execute_baseline.clear();
        assert!(
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(witness_removed),)
                .is_err()
        );

        let mut source_changed = imported.clone();
        let NodeKind::MixedEffectExecute {
            source_commands, ..
        } = &mut source_changed.nodes[0].kind
        else {
            unreachable!()
        };
        source_commands[1] = EffectMacro::Raw("forged_effect(agent);".into());
        assert!(
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(source_changed),)
                .is_err()
        );

        let mut wrong_source_count = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Excute(vec![
                    simple_effect(),
                    EffectMacro::Raw(
                        "EffectModule::is_exist_effect(agent.module_accessor, 0);".into(),
                    ),
                ])],
            },
        );
        let NodeKind::MixedEffectExecute {
            source_commands, ..
        } = &mut wrong_source_count.nodes[0].kind
        else {
            unreachable!()
        };
        source_commands.pop();
        assert!(crate::move_nodes::timeline::compile(
            "mario",
            "attack11",
            &graph(wrong_source_count),
        )
        .is_err());

        let mut duplicate_witness = imported.clone();
        duplicate_witness
            .effect_execute_baseline
            .push(duplicate_witness.effect_execute_baseline[0].clone());
        assert!(crate::move_nodes::timeline::compile(
            "mario",
            "attack11",
            &graph(duplicate_witness),
        )
        .is_err());

        for scale in [f32::NAN, 10_001.0] {
            let mut invalid_value = import_for_fighter(
                "mario",
                &EffectScript {
                    stmts: vec![EffectStmt::Excute(vec![
                        simple_effect(),
                        EffectMacro::Raw(
                            "EffectModule::is_exist_effect(agent.module_accessor, 0);".into(),
                        ),
                    ])],
                },
            );
            let NodeKind::MixedEffectExecute { commands, .. } = &mut invalid_value.nodes[0].kind
            else {
                unreachable!()
            };
            let EffectCommandSlot::Editable(EffectMacro::Effect {
                scale: current_scale,
                ..
            }) = &mut commands[0]
            else {
                unreachable!()
            };
            *current_scale = scale;
            assert!(crate::move_nodes::timeline::compile(
                "mario",
                "attack11",
                &graph(invalid_value),
            )
            .is_err());
        }

        let mut wrong_track = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Excute(vec![
                    simple_effect(),
                    EffectMacro::Raw(
                        "EffectModule::is_exist_effect(agent.module_accessor, 0);".into(),
                    ),
                ])],
            },
        );
        wrong_track.track = Track::Game;
        assert!(
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(wrong_track)).is_err()
        );

        let mut duplicate_wrapper = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Excute(vec![
                    simple_effect(),
                    EffectMacro::Raw(
                        "EffectModule::is_exist_effect(agent.module_accessor, 0);".into(),
                    ),
                ])],
            },
        );
        let mut duplicate_node = duplicate_wrapper.nodes[0].clone();
        duplicate_node.id += 1;
        duplicate_wrapper.nodes.push(duplicate_node);
        assert!(crate::move_nodes::timeline::compile(
            "mario",
            "attack11",
            &graph(duplicate_wrapper),
        )
        .is_err());

        let mut reused_as_whole_preserved = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Excute(vec![
                    simple_effect(),
                    EffectMacro::Raw(
                        "EffectModule::is_exist_effect(agent.module_accessor, 0);".into(),
                    ),
                ])],
            },
        );
        let baseline = reused_as_whole_preserved.baseline[0].clone();
        reused_as_whole_preserved.nodes.push(fixture::node(
            reused_as_whole_preserved.nodes[0].id + 1,
            NodeKind::Preserved {
                statement: baseline,
            },
        ));
        assert!(crate::move_nodes::timeline::compile(
            "mario",
            "attack11",
            &graph(reused_as_whole_preserved),
        )
        .is_err());
    }

    #[test]
    fn mixed_effect_slots_and_node_source_witness_are_bounded_before_rendering() {
        let imported = import_for_fighter(
            "mario",
            &EffectScript {
                stmts: vec![EffectStmt::Excute(vec![
                    EffectMacro::LastEffectSetRate { rate: 0.5 },
                    EffectMacro::Raw(
                        "EffectModule::is_exist_effect(agent.module_accessor, 0);".into(),
                    ),
                ])],
            },
        );
        for oversized_source in [false, true] {
            let mut track = imported.clone();
            let NodeKind::MixedEffectExecute {
                commands,
                source_commands,
            } = &mut track.nodes[0].kind
            else {
                panic!("expected canonical mixed block")
            };
            if oversized_source {
                *source_commands = vec![
                    EffectMacro::LastEffectSetRate { rate: 0.5 };
                    MAX_COMMANDS_PER_EXECUTE + 1
                ];
            } else {
                *commands =
                    vec![
                        EffectCommandSlot::Editable(EffectMacro::LastEffectSetRate { rate: 0.5 });
                        MAX_COMMANDS_PER_EXECUTE + 1
                    ];
            }
            let errors =
                crate::move_nodes::timeline::compile("mario", "bounded_effect", &graph(track))
                    .unwrap_err();
            assert!(errors
                .iter()
                .any(|error| error.reason.contains("exceed 256 commands")));
        }
    }

    #[test]
    fn baseline_reparse_prevents_raw_source_from_becoming_an_editable_slot() {
        let raw_control = "EffectModule::set_disable_render_offset_last(agent.module_accessor);";
        let manually_raw = EffectScript {
            stmts: vec![EffectStmt::Excute(vec![
                simple_effect(),
                EffectMacro::Raw(raw_control.into()),
            ])],
        };
        let imported = import_for_fighter("mario", &manually_raw);
        assert!(matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }));
        let parsed_from_source = crate::acmd::parse_effect_script(
            r#"unsafe extern "C" fn effect_witness(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_hit_smoke"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        EffectModule::set_disable_render_offset_last(agent.module_accessor);
    }
}"#,
        );
        assert!(matches!(
            &parsed_from_source.stmts[0],
            EffectStmt::Excute(commands)
                if matches!(commands.get(1), Some(EffectMacro::Control(
                    EffectControl::SetDisableRenderOffsetLast
                )))
        ));
    }

    #[test]
    fn typed_track_witness_rejects_raw_to_typed_slot_promotion() {
        let opaque_alpha = EffectMacro::Raw("macros::LAST_EFFECT_SET_ALPHA(agent, 0.5);".into());
        let unknown = EffectMacro::Raw("unknown(agent);".into());
        let original = vec![
            EffectMacro::LastEffectSetRate { rate: 1.0 },
            opaque_alpha.clone(),
            unknown,
        ];
        let source_wrapper = effect_stmt_to_acmd(&EffectStmt::Excute(original.clone()));
        let track = MoveTrack {
            track: Track::Effect,
            nodes: vec![fixture::node(
                3 << 29,
                NodeKind::MixedEffectExecute {
                    commands: vec![
                        EffectCommandSlot::Editable(EffectMacro::LastEffectSetRate { rate: 1.0 }),
                        EffectCommandSlot::Preserved(opaque_alpha.clone()),
                        EffectCommandSlot::Preserved(EffectMacro::Raw("unknown(agent);".into())),
                    ],
                    source_commands: original.clone(),
                },
            )],
            baseline: vec![source_wrapper],
            effect_execute_baseline: vec![original],
        };

        let mut promoted = track;
        let NodeKind::MixedEffectExecute {
            commands,
            source_commands,
        } = &mut promoted.nodes[0].kind
        else {
            unreachable!()
        };
        let typed_alpha = EffectMacro::LastEffectSetAlpha { alpha: 0.5 };
        assert_eq!(
            preview_effect_macro(&opaque_alpha),
            preview_effect_macro(&typed_alpha)
        );
        source_commands[1] = typed_alpha.clone();
        commands[1] = EffectCommandSlot::Editable(typed_alpha);

        // These two commands render to the same source bytes. Only the independent typed
        // track baseline can detect promotion of the originally Raw source command.
        assert!(
            crate::move_nodes::timeline::compile("mario", "effect_witness", &graph(promoted),)
                .is_err()
        );
    }

    const JACK_DOYLE: &str = r#"unsafe extern "C" fn effect_attacks3lw(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 7.0);
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_JACK_INSTANCE_WORK_ID_FLAG_DOYLE) {
        if macros::is_excute(agent) {
            macros::EFFECT(agent, Hash40::new("jack_doyle_magic_line"), Hash40::new("top"), 0, 13, -8, 22, 0, 0, 0.85, 0, 0, 0, 0, 0, 0, true);
        }
    }
}"#;

    const DEMON_FLAG_ELSE: &str = r#"unsafe extern "C" fn effect_attackragedrivefall(agent: &mut L2CAgentBase) {
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_DEMON_INSTANCE_WORK_ID_FLAG_ATTACK_RAGE_CAPTURE) {
        if macros::is_excute(agent) {
            macros::EFFECT_FOLLOW(agent, Hash40::new("demon_ragedrive_speedline"), Hash40::new("rot"), 0, -5, -3, 0, 0, 0, 1, true);
        }
    }
    else {
        if macros::is_excute(agent) {
            macros::EFFECT_FOLLOW(agent, Hash40::new("demon_ragedrive_speedline"), Hash40::new("rot"), 0, -5, -3, 0, 0, 0, 1, true);
        }
    }
}"#;

    #[test]
    fn fighter_aware_effect_import_reuses_negated_flag_with_nonempty_else() {
        let script = crate::acmd::parse_effect_script(DEMON_FLAG_ELSE);
        let imported = import_for_fighter("demon", &script);
        assert!(imported.baseline.is_empty());
        assert_eq!(imported.nodes.len(), 1);
        assert!(matches!(
            &imported.nodes[0].kind,
            NodeKind::Branch {
                condition: MoveCondition::WorkFlag { symbol, expected: false },
                then_nodes,
                else_nodes,
            } if symbol == "FIGHTER_DEMON_INSTANCE_WORK_ID_FLAG_ATTACK_RAGE_CAPTURE"
                && then_nodes.len() == 1
                && else_nodes.len() == 1
        ));
        assert_json_eq!(round_trip("demon", &script), script);
    }

    #[test]
    fn unsupported_effect_conditions_and_their_nested_source_remain_whole() {
        let wrong_fighter = crate::acmd::parse_effect_script(JACK_DOYLE);
        for imported in [
            import(&wrong_fighter),
            import_for_fighter("mario", &wrong_fighter),
        ] {
            assert_eq!(imported.nodes.len(), 2);
            assert!(fixture::untyped_condition(&imported.nodes[1].kind));
            assert_eq!(imported.baseline.len(), 1);
            assert_json_eq!(round_trip("mario", &wrong_fighter), &wrong_fighter);
        }

        let nested = crate::acmd::parse_effect_script(
            r#"unsafe extern "C" fn effect_nested(agent: &mut L2CAgentBase) {
    if(0x2508e0(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 3)){
        if WorkModule::is_flag(agent.module_accessor, *FIGHTER_JACK_INSTANCE_WORK_ID_FLAG_DOYLE) {
            if macros::is_excute(agent) {
                macros::EFFECT(agent, Hash40::new("jack_doyle_magic_line"), Hash40::new("top"), 0, 13, -8, 22, 0, 0, 0.85, 0, 0, 0, 0, 0, 0, true);
            }
        }
    }
}"#,
        );
        // The address-call header stays source text; the flag branch under it is typed.
        let imported = import_for_fighter("jack", &nested);
        assert!(matches!(
            imported.nodes.as_slice(),
            [MoveNode { kind: NodeKind::Branch { condition: MoveCondition::Source { header }, then_nodes, else_nodes }, .. }]
                if header.starts_with("if(0x2508e0") && else_nodes.is_empty()
                    && matches!(then_nodes.as_slice(), [MoveNode { kind: NodeKind::Branch { condition: MoveCondition::WorkFlag { .. }, .. }, .. }])
        ));
        // The only baseline entry is the header's witness, with its body lifted out.
        assert!(matches!(
            imported.baseline.as_slice(),
            [AcmdStmt::RawBlock { header, body }] if header.starts_with("if(0x2508e0") && body.is_empty()
        ));
        assert_json_eq!(round_trip("jack", &nested), nested);
    }

    #[test]
    fn fighter_aware_effect_import_preserves_empty_else_and_else_if_chains() {
        let sources = [
            r#"unsafe extern "C" fn effect_empty(agent: &mut L2CAgentBase) {
    if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {
        if macros::is_excute(agent) {
            macros::EFFECT(agent, Hash40::new("sys_hit_smoke"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        }
    }
    else {
    }
}"#,
            r#"unsafe extern "C" fn effect_else_if(agent: &mut L2CAgentBase) {
    if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {
        if macros::is_excute(agent) {
            macros::EFFECT(agent, Hash40::new("sys_hit_smoke"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        }
    }
    else if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_AIR {
        if macros::is_excute(agent) {
            macros::EFFECT(agent, Hash40::new("sys_hit_elec"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        }
    }
    else {
        if macros::is_excute(agent) {
            macros::EFFECT(agent, Hash40::new("sys_hit_fire"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        }
    }
}"#,
        ];
        for source in sources {
            let script = crate::acmd::parse_effect_script(source);
            let imported = import_for_fighter("mario", &script);
            assert!(imported
                .nodes
                .iter()
                .all(|node| matches!(node.kind, NodeKind::Preserved { .. })));
            assert_eq!(imported.baseline.len(), script.stmts.len());
            assert_json_eq!(round_trip("mario", &script), script);
        }
    }

    #[test]
    fn effect_branch_import_obeys_timeline_nesting_limit() {
        fn nested(depth: usize) -> EffectStmt {
            if depth == 0 {
                EffectStmt::Excute(vec![simple_effect()])
            } else {
                EffectStmt::Cond {
                    header: "if WorkModule::is_flag(agent.module_accessor, *FIGHTER_JACK_INSTANCE_WORK_ID_FLAG_DOYLE) {".into(),
                    body: vec![nested(depth - 1)],
                }
            }
        }
        let script = EffectScript {
            stmts: vec![nested(9)],
        };
        let imported = import_for_fighter("jack", &script);
        let mut current = &imported.nodes[0];
        for level in 0..8 {
            let NodeKind::Branch { then_nodes, .. } = &current.kind else {
                panic!("branch level {level} should be within the shared nesting limit")
            };
            assert_eq!(then_nodes.len(), 1);
            current = &then_nodes[0];
        }
        assert!(matches!(current.kind, NodeKind::Preserved { .. }));
        assert_eq!(imported.baseline.len(), 1);
        assert_json_eq!(round_trip("jack", &script), script);
    }

    #[test]
    fn measured_trail_import_edit_save_and_source_roundtrip_stay_typed() {
        let source = crate::acmd::parse_effect_script(&format!(
            "unsafe extern \"C\" fn effect_attackairf(agent: &mut L2CAgentBase) {{\nif macros::is_excute(agent) {{\n{}\nmacros::AFTER_IMAGE_OFF(agent, 1);\n}}\n}}",
            crate::move_nodes::trails::TRAIL4_SOURCE_FIXTURE
        ));
        let mut imported = import(&source);
        assert!(imported.baseline.is_empty());
        let NodeKind::EffectExecute { commands } = &mut imported.nodes[0].kind else {
            panic!("trail group must be editable")
        };
        let mut settings = crate::move_nodes::trails::settings(&commands[0]).unwrap();
        settings.length = 9;
        settings.bone2 = "swordr".into();
        settings.offset2[1] = 22.5;
        settings.native_values = [1.0, -0.5];
        crate::move_nodes::trails::update(&mut commands[0], &settings).unwrap();
        commands[1] = EffectMacro::AfterImageOff { arg: 3.0 };
        let authored = graph(imported);
        let stored = serde_json::to_string(&authored).unwrap();
        let reloaded: MoveGraph = serde_json::from_str(&stored).unwrap();
        let compiled =
            crate::move_nodes::timeline::compile("shulk", "attackairf", &reloaded).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "attackairf");
        syn::parse_file(&emitted).unwrap();
        let reparsed = crate::acmd::parse_effect_script(&emitted);
        let EffectStmt::Excute(commands) = &reparsed.stmts[0] else {
            panic!()
        };
        assert_eq!(
            crate::move_nodes::trails::settings(&commands[0]).unwrap(),
            settings
        );
        assert!(matches!(
            commands[1],
            EffectMacro::AfterImageOff { arg: 3.0 }
        ));
        assert!(
            crate::move_nodes::live_support::source_only_tracks(&reloaded)
                .any(|track| track == Track::Effect)
        );
    }

    #[test]
    fn trail_source_exception_is_closed_and_does_not_authorize_raw_commands() {
        let fixture = crate::move_nodes::trails::TRAIL4_SOURCE_FIXTURE;
        for source in [
            fixture.replace("1.4, 0.2", "1, 0.2"),
            fixture.replace("*EFFECT_AXIS_X", "*TRAIL_BLEND_ALPHA"),
            format!("{fixture}\ncustom(agent);"),
            fixture.replace("Hash40::new(\"tex_shulk_sword1\")", "make_texture()"),
        ] {
            let command = EffectMacro::AfterImage {
                command: "AFTER_IMAGE4_ON_arg29".into(),
                effect_name: "tex_shulk_sword1".into(),
                bone_name: "haver".into(),
                bone_name2: Some("haver".into()),
                raw: source,
            };
            assert!(command_lines(&command).is_err());
        }
        assert!(command_lines(&EffectMacro::Raw(fixture.into())).is_err());
    }

    #[test]
    fn effect_import_keeps_unknown_and_conditional_blocks_immutable() {
        let source = EffectScript {
            stmts: vec![
                EffectStmt::Frame(2.0),
                EffectStmt::Cond {
                    header: "if WorkModule::is_flag(agent.module_accessor, *SOME_FLAG) {".into(),
                    body: vec![
                        EffectStmt::Wait(1.0),
                        EffectStmt::Raw("custom(agent);".into()),
                    ],
                },
                EffectStmt::Excute(vec![EffectMacro::Raw("mystery(agent);".into())]),
            ],
        };
        let imported = import(&source);
        assert_eq!(imported.track, Track::Effect);
        assert_eq!(imported.baseline.len(), 2);
        assert!(matches!(imported.nodes[1].kind, NodeKind::Preserved { .. }));
        assert!(matches!(imported.nodes[2].kind, NodeKind::Preserved { .. }));
        let compiled =
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(imported)).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "attack11");
        assert!(syn::parse_file(&emitted).is_ok(), "{emitted}");
        assert!(emitted.contains("custom(agent);"));
        assert!(emitted.contains("mystery(agent);"));
        assert!(emitted.contains("SOME_FLAG"));
    }

    #[test]
    fn exact_last_effect_controls_are_closed_authorable_calls_in_source_order() {
        use crate::data::EffectControl;

        let controls = [
            EffectMacro::Control(EffectControl::SetDisableRenderOffsetLast),
            EffectMacro::Control(EffectControl::EnableSyncInitPosLast),
        ];
        assert_eq!(
            command_lines(&controls[0]).unwrap(),
            vec![
                "EffectModule::set_disable_render_offset_last(agent.module_accessor);".to_string()
            ],
        );
        assert_eq!(
            command_lines(&controls[1]).unwrap(),
            vec!["EffectModule::enable_sync_init_pos_last(agent.module_accessor);".to_string()],
        );

        let source = r#"unsafe extern "C" fn effect_probe(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_hit_smoke"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, true);
        EffectModule::set_disable_render_offset_last(agent.module_accessor);
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_hit_elec"), Hash40::new("top"), 0, 2, 0, 0, 0, 0, 1, true);
        EffectModule::enable_sync_init_pos_last(agent.module_accessor);
    }
}
"#;
        let parsed = crate::acmd::parse_effect_script(source);
        let imported = import(&parsed);
        assert!(imported.baseline.is_empty());
        let NodeKind::EffectExecute { commands } = &imported.nodes[1].kind else {
            panic!("exact effect controls should stay in one authorable execute block");
        };
        assert!(matches!(
            commands.as_slice(),
            [
                EffectMacro::Effect { effect_name, .. },
                EffectMacro::Control(EffectControl::SetDisableRenderOffsetLast),
                EffectMacro::Effect { effect_name: next, .. },
                EffectMacro::Control(EffectControl::EnableSyncInitPosLast),
            ] if effect_name == "sys_hit_smoke" && next == "sys_hit_elec"
        ));

        let compiled =
            crate::move_nodes::timeline::compile("mario", "probe", &graph(imported)).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "probe");
        assert!(syn::parse_file(&emitted).is_ok(), "{emitted}");
        let first_spawn = emitted.find("sys_hit_smoke").unwrap();
        let render_control = emitted
            .find("EffectModule::set_disable_render_offset_last(agent.module_accessor);")
            .unwrap();
        let second_spawn = emitted.find("sys_hit_elec").unwrap();
        let sync_control = emitted
            .find("EffectModule::enable_sync_init_pos_last(agent.module_accessor);")
            .unwrap();
        assert!(first_spawn < render_control && render_control < second_spawn);
        assert!(second_spawn < sync_control);
        let reparsed = crate::acmd::parse_effect_script(&emitted);
        assert!(matches!(
            reparsed.stmts.get(1),
            Some(EffectStmt::Excute(commands))
                if matches!(commands.get(1), Some(EffectMacro::Control(EffectControl::SetDisableRenderOffsetLast)))
                    && matches!(commands.get(3), Some(EffectMacro::Control(EffectControl::EnableSyncInitPosLast)))
        ));

        let unsupported = crate::acmd::parse_effect_script(
            r#"unsafe extern "C" fn effect_unsupported_control(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        EffectModule::enable_sync_init_pos_last(agent.module_accessor, 0);
        EffectModule::set_disable_render_offset_last(agent.module_accessor);
    }
}
"#,
        );
        let preserved = import(&unsupported);
        assert!(matches!(
            preserved.nodes.first().map(|node| &node.kind),
            Some(NodeKind::MixedEffectExecute { commands, .. })
                if matches!(commands.as_slice(), [
                    EffectCommandSlot::Preserved(EffectMacro::Raw(_)),
                    EffectCommandSlot::Editable(EffectMacro::Control(
                        EffectControl::SetDisableRenderOffsetLast
                    )),
                ])
        ));
        let compiled =
            crate::move_nodes::timeline::compile("mario", "unsupported_control", &graph(preserved))
                .unwrap();
        let emitted =
            crate::acmd::preview_effect_structure_fn(&compiled[0].1, "unsupported_control");
        assert!(syn::parse_file(&emitted).is_ok(), "{emitted}");
        assert!(
            emitted.contains("EffectModule::enable_sync_init_pos_last(agent.module_accessor, 0);"),
            "unsupported call spelling must remain untouched: {emitted}"
        );
        assert!(
            emitted
                .contains("EffectModule::set_disable_render_offset_last(agent.module_accessor);"),
            "editable sibling must stay ordered beside the preserved call: {emitted}"
        );
    }

    #[test]
    fn measured_screen_and_camera_controls_validate_and_serialize() {
        use crate::data::EffectControl;
        let req = EffectMacro::Control(EffectControl::ReqScreen {
            preset_id: "bg_bayonetta_final".into(),
        });
        let camera =
            EffectMacro::Control(EffectControl::SetDisableCameraDepthInfluence { value: true });
        assert_eq!(
            command_lines(&req).unwrap(),
            vec!["EffectModule::req_screen(agent.module_accessor, Hash40::new(\"bg_bayonetta_final\"), false, false, false);".to_string()]
        );
        assert_eq!(
            command_lines(&camera).unwrap(),
            vec!["ColorBlendModule::set_disable_camera_depth_influence(agent.module_accessor, true);".to_string()]
        );
        assert!(
            command_lines(&EffectMacro::Control(EffectControl::ReqScreen {
                preset_id: "not_measured".into(),
            }))
            .is_err()
        );
        assert!(command_lines(&EffectMacro::Raw(
            "EffectModule::req_screen(agent.module_accessor, Hash40::new(\"arbitrary\"), false, false, false);".into()
        ))
        .is_err());
        let encoded = serde_json::to_value(&req).unwrap();
        let decoded: EffectMacro = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
        let encoded = serde_json::to_value(&camera).unwrap();
        let decoded: EffectMacro = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }

    #[test]
    fn measured_controls_compile_in_the_effect_track_and_round_trip_the_tree() {
        let source = r#"unsafe extern "C" fn effect_native_roundtrip(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        EffectModule::req_screen(agent.module_accessor, Hash40::new("bg_bayonetta_final"), false, false, false);
        ColorBlendModule::set_disable_camera_depth_influence(agent.module_accessor, false);
    }
}
"#;
        let script = crate::acmd::parse_effect_script(source);
        let imported = import(&script);
        assert!(imported.baseline.is_empty());
        let compiled =
            crate::move_nodes::timeline::compile("mario", "native_roundtrip", &graph(imported))
                .unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "native_roundtrip");
        syn::parse_file(&emitted).unwrap();
        let reparsed = crate::acmd::parse_effect_script(&emitted);
        assert_json_eq!(reparsed, script);
        let reimported = import(&crate::acmd::parse_effect_script(&emitted));
        let compiled_again =
            crate::move_nodes::timeline::compile("mario", "native_roundtrip", &graph(reimported))
                .unwrap();
        assert_eq!(
            crate::acmd::preview_effect_structure_fn(&compiled_again[0].1, "native_roundtrip"),
            emitted
        );
    }

    #[test]
    fn preserved_effect_raw_trails_and_work_slots_keep_source_tokens() {
        let trail_source = "effect(*MA_MSC_CMD_EFFECT_AFTER_IMAGE3_ON, Hash40::new(\"texture\"), 9, Hash40::new(\"sword\"), 0.0, 8.75, 0.0, Hash40::new(\"texture\"), 9, Hash40::new(\"sword_tip\"), 0.0, 6.5, 0.0, 0.0, 0, 0.0, 0.0, 0.0, 0.0, 0.0, false, 0, false);";
        let raw_source = "effect(*MA_MSC_CMD_EFFECT_AFTER_IMAGE3_ON, Hash40::new(\"raw_texture\"), 17, 1.25, 2.5, 3.75, 24680.125);";
        let source = EffectScript {
            stmts: vec![
                EffectStmt::Excute(vec![
                    EffectMacro::Raw(raw_source.into()),
                    EffectMacro::LastEffectSetWorkInt {
                        work: "FIGHTER_INSTANCE_WORK_ID_INT_LAST_EFFECT_HANDLE".into(),
                    },
                ]),
                EffectStmt::Excute(vec![EffectMacro::AfterImage {
                    command: "AFTER_IMAGE3_ON".into(),
                    effect_name: "texture".into(),
                    bone_name: "sword".into(),
                    bone_name2: Some("sword_tip".into()),
                    raw: trail_source.into(),
                }]),
            ],
        };
        let imported = import(&source);
        assert!(imported
            .nodes
            .iter()
            .all(|node| matches!(node.kind, NodeKind::Preserved { .. })));
        let compiled =
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(imported)).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "attack11");
        assert!(syn::parse_file(&emitted).is_ok(), "{emitted}");
        assert!(emitted.contains(raw_source), "{emitted}");
        assert!(
            emitted.contains("FIGHTER_INSTANCE_WORK_ID_INT_LAST_EFFECT_HANDLE"),
            "{emitted}"
        );
        assert!(
            emitted.contains("8.75") && emitted.contains("6.5"),
            "{emitted}"
        );
        assert!(emitted.contains("24680.125"), "{emitted}");
    }

    #[test]
    fn authored_effects_keep_branch_loop_order_and_emit_through_effect_renderer() {
        let imported = fixture::track(
            Track::Effect,
            vec![fixture::node(
                3 << 29,
                NodeKind::Branch {
                    condition: MoveCondition::Grounded,
                    then_nodes: vec![fixture::effect_execute(
                        (3 << 29) + 1,
                        vec![simple_effect()],
                    )],
                    else_nodes: vec![fixture::node(
                        (3 << 29) + 2,
                        NodeKind::Repeat {
                            motion_loop: crate::data::MotionLoop::No,
                            count: 2,
                            body: vec![fixture::effect_execute(
                                (3 << 29) + 3,
                                vec![EffectMacro::LastEffectSetRate { rate: 0.5 }],
                            )],
                        },
                    )],
                },
            )],
        );
        let compiled =
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(imported)).unwrap();
        let source = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "attack11");
        assert!(syn::parse_file(&source).is_ok(), "{source}");
        assert!(source.contains("SITUATION_KIND_GROUND"));
        assert!(source.contains("EFFECT(agent"));
        assert!(source.contains("LAST_EFFECT_SET_RATE"));
        assert!(source.contains("if macros::is_excute(agent)"));
        let file = syn::parse_file(&source).unwrap();
        let function = file
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Fn(function) => Some(function),
                _ => None,
            })
            .unwrap();
        let syn::Stmt::Expr(syn::Expr::If(branch), _) = &function.block.stmts[0] else {
            panic!("expected generated outer if branch: {source}");
        };
        assert!(
            branch.else_branch.is_some(),
            "else arm must bind to the outer if: {source}"
        );
    }

    #[test]
    fn scale_helper_and_color_controls_survive_structural_preview() {
        let track = fixture::track(
            Track::Effect,
            vec![fixture::effect_execute(
                3 << 29,
                vec![
                    EffectMacro::LastEffectSetScaleW {
                        values: vec![1.0, 0.5],
                    },
                    EffectMacro::Color {
                        command: "BURN_COLOR".into(),
                        color: crate::data::ColorCall {
                            transition: None,
                            rgba: Some([1.0, 0.25, 0.1, 0.75]),
                        },
                    },
                ],
            )],
        );
        let compiled =
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(track)).unwrap();
        let source = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "attack11");
        assert!(
            source.contains("unsafe fn visionary_last_effect_set_scale_w"),
            "{source}"
        );
        assert!(
            source.contains("visionary_last_effect_set_scale_w(agent, &[1.0, 0.5])"),
            "{source}"
        );
        assert!(source.contains("macros::BURN_COLOR(agent"), "{source}");
        assert!(syn::parse_file(&source).is_ok(), "{source}");
    }

    #[test]
    fn preserved_scale_w_block_does_not_reimport_generated_helper_attributes() {
        // Authored analogue of an imported execute block containing a typed spawn, an opaque
        // WorkModule read, and the linked LAST_EFFECT_SET_SCALE_W primitive.
        let source = r#"unsafe extern "C" fn effect_scale(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_aura"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, true);
        WorkModule::get_float(agent.module_accessor, *FIGHTER_STATUS_WORK_ID_FLOAT_SPECIAL_LW);
        macros::LAST_EFFECT_SET_SCALE_W(agent, 1821741189);
    }
}
"#;
        let parsed = crate::acmd::parse_effect_script(source);
        let imported = import(&parsed);
        let compiled =
            crate::move_nodes::timeline::compile("mario", "speciallw", &graph(imported)).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "speciallw");
        assert!(
            emitted.contains("visionary_last_effect_set_scale_w(agent, &["),
            "{emitted}"
        );
        let reparsed = crate::acmd::parse_effect_script(&emitted);
        assert_json_eq!(
            reparsed,
            parsed,
            "generated helper attributes or opaque commands changed the imported statement tree"
        );
    }

    #[test]
    fn effect_authoring_rejects_raw_injection_and_nonfinite_values() {
        assert!(command_lines(&EffectMacro::Raw("agent.clear_lua_stack();".into())).is_err());
        let mut malicious = simple_effect();
        if let EffectMacro::Effect { effect_name, .. } = &mut malicious {
            *effect_name = "smoke\"); panic!(); (\"".into();
        }
        assert!(command_lines(&malicious).is_err());
        let mut nonfinite = simple_effect();
        if let EffectMacro::Effect { offset, .. } = &mut nonfinite {
            offset[0] = f32::NAN;
        }
        assert!(command_lines(&nonfinite).is_err());
        for field in 0..6 {
            let mut extreme = simple_effect();
            if let EffectMacro::Effect {
                offset, rotation, ..
            } = &mut extreme
            {
                if field < 3 {
                    offset[field] = f32::MAX;
                } else {
                    rotation[field - 3] = f32::MAX;
                }
            }
            assert!(
                command_lines(&extreme).is_err(),
                "accepted transform field {field}"
            );
        }
        let mut extreme_scale = simple_effect();
        if let EffectMacro::Effect { scale, .. } = &mut extreme_scale {
            *scale = f32::MAX;
        }
        assert!(command_lines(&extreme_scale).is_err());
        assert!(command_lines(&EffectMacro::LastEffectSetScaleW {
            values: vec![f32::INFINITY]
        })
        .is_err());
    }

    #[test]
    fn observed_ness_effect_attr_round_trips_and_keeps_integer_domains_closed() {
        const SOURCE: &str = r#"unsafe extern "C" fn effect_attackairhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::EFFECT_ATTR(agent, Hash40::new("ness_psi_rush"), Hash40::new("haver"), -0.5, 2.6, 2.9, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true, *EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE);
    }
}
"#;
        const ATTRIBUTE: &str = "*EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE";
        const FLIP_AXIS: &str = "*EF_FLIP_YZ";

        let parsed = crate::acmd::parse_effect_script(SOURCE);
        let imported = import(&parsed);
        assert!(imported.baseline.is_empty());
        assert!(matches!(
            &imported.nodes[0].kind,
            NodeKind::Frame { frame } if *frame == 8.0
        ));
        let NodeKind::EffectExecute { commands } = &imported.nodes[1].kind else {
            panic!("the observed EFFECT_ATTR call must be editable as typed source");
        };
        assert_eq!(commands.len(), 1);
        let command = commands[0].clone();
        let EffectMacro::Effect {
            effect_name,
            effect_name_alt,
            spawn_func,
            bone_name,
            offset,
            rotation,
            scale,
            follows_bone,
            extra_args,
            flip_axis,
        } = &command
        else {
            panic!("the observed EFFECT_ATTR call must import as a spawn");
        };
        assert_eq!(effect_name, "ness_psi_rush");
        assert!(effect_name_alt.is_none());
        assert_eq!(spawn_func, "EFFECT_ATTR");
        assert_eq!(bone_name, "haver");
        assert_eq!(*offset, [-0.5, 2.6, 2.9]);
        assert_eq!(*rotation, [0.0, 0.0, 0.0]);
        assert_eq!(*scale, 1.0);
        assert!(!*follows_bone);
        assert_eq!(
            extra_args.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["0", "0", "0", "0", "0", "0", "true", ATTRIBUTE]
        );
        assert!(flip_axis.is_none());

        let exact_command = "macros::EFFECT_ATTR(agent, Hash40::new(\"ness_psi_rush\"), Hash40::new(\"haver\"), -0.5, 2.6, 2.9, 0.0, 0.0, 0.0, 1.0, 0, 0, 0, 0, 0, 0, true, *EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE);";
        assert_eq!(
            command_lines(&command).unwrap(),
            vec![exact_command.to_owned()]
        );

        let editable_graph = graph(imported);
        assert!(
            crate::move_nodes::live_support::source_only_tracks(&editable_graph)
                .any(|track| track == Track::Effect)
        );
        let compiled =
            crate::move_nodes::timeline::compile("ness", "attackairhi", &editable_graph).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "attackairhi");
        assert!(emitted.contains(exact_command), "{emitted}");
        let reparsed = crate::acmd::parse_effect_script(&emitted);
        assert_json_eq!(
            reparsed,
            parsed,
            "the observed source must round-trip through the editable typed tree"
        );

        let set_integer_tail = |command: &mut EffectMacro, value: &str| {
            let EffectMacro::Effect { extra_args, .. } = command else {
                panic!("expected effect spawn")
            };
            *extra_args.last_mut().unwrap() = value.to_owned();
        };
        let invalid_attributes = [
            "*EFFECT_SUB_ATTRIBUTE_UNKNOWN",
            "0x1000000",
            "*EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE | 0x1",
            "1 + 2",
            FLIP_AXIS,
        ];
        for token in invalid_attributes {
            let mut invalid = command.clone();
            set_integer_tail(&mut invalid, token);
            assert!(
                command_lines(&invalid).is_err(),
                "EFFECT_ATTR accepted cross-domain or unmeasured integer token {token}"
            );
        }

        let mut zero_default = command.clone();
        if let EffectMacro::Effect {
            extra_args,
            flip_axis,
            ..
        } = &mut zero_default
        {
            extra_args.clear();
            *flip_axis = None;
        }
        assert!(command_lines(&zero_default)
            .unwrap()
            .last()
            .unwrap()
            .ends_with(", 0);"));

        let mut wrong_boolean = command.clone();
        if let EffectMacro::Effect { extra_args, .. } = &mut wrong_boolean {
            extra_args[6] = "0".into();
        }
        assert!(command_lines(&wrong_boolean).is_err());

        let mut wrong_arity = command.clone();
        if let EffectMacro::Effect { extra_args, .. } = &mut wrong_arity {
            extra_args.pop();
        }
        assert!(command_lines(&wrong_arity).is_err());

        let mut nonfinite_tail = command.clone();
        if let EffectMacro::Effect { extra_args, .. } = &mut nonfinite_tail {
            extra_args[0] = "NaN".into();
        }
        assert!(command_lines(&nonfinite_tail).is_err());

        let mut nonfinite_transform = command.clone();
        if let EffectMacro::Effect { offset, .. } = &mut nonfinite_transform {
            offset[0] = f32::INFINITY;
        }
        assert!(command_lines(&nonfinite_transform).is_err());

        let mut nonflip_axis_field = command.clone();
        if let EffectMacro::Effect { flip_axis, .. } = &mut nonflip_axis_field {
            *flip_axis = Some(FLIP_AXIS.into());
        }
        assert!(command_lines(&nonflip_axis_field).is_err());

        let make_flip = |integer_token: &str| {
            let mut flipped = command.clone();
            let EffectMacro::Effect {
                effect_name,
                effect_name_alt,
                spawn_func,
                follows_bone,
                extra_args,
                flip_axis,
                ..
            } = &mut flipped
            else {
                unreachable!()
            };
            *effect_name_alt = Some(effect_name.clone());
            *spawn_func = "FOOT_EFFECT_FLIP".into();
            *follows_bone = false;
            *extra_args = vec!["0".into(); 6];
            extra_args.push("false".into());
            extra_args.push(integer_token.into());
            *flip_axis = Some(integer_token.into());
            flipped
        };
        assert!(command_lines(&make_flip(FLIP_AXIS)).is_ok());
        assert!(command_lines(&make_flip(ATTRIBUTE)).is_err());

        let malformed_source = SOURCE.replace(
            "true, *EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE",
            "0, *EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE",
        );
        let malformed_parsed = crate::acmd::parse_effect_script(&malformed_source);
        let malformed_imported = import(&malformed_parsed);
        assert_eq!(malformed_imported.baseline.len(), 1);
        assert!(matches!(
            &malformed_imported.nodes[1].kind,
            NodeKind::Preserved { .. }
        ));
        let preserved =
            crate::move_nodes::timeline::compile("ness", "attackairhi", &graph(malformed_imported))
                .unwrap();
        let preserved_source =
            crate::acmd::preview_effect_structure_fn(&preserved[0].1, "attackairhi");
        assert!(preserved_source.contains("0, *EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE"));
    }

    #[test]
    fn authored_effect_command_count_is_bounded() {
        let track = fixture::track(
            Track::Effect,
            vec![fixture::effect_execute(
                3 << 29,
                vec![EffectMacro::LastEffectSetRate { rate: 0.5 }; 257],
            )],
        );
        let errors =
            crate::move_nodes::timeline::compile("mario", "attack11", &graph(track)).unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.reason.contains("exceeds 256 commands")));
    }
}
