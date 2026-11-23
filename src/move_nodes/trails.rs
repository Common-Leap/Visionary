//! Bounded authoring support for the pinned `AFTER_IMAGE4_ON_arg29` source form.
//!
//! This module deliberately edits the imported source literal instead of assigning meanings
//! to undocumented trail parameters or inventing a default trail. Its timeline controls require
//! complete source export; this does not extend the flat live-effect dispatcher.

use crate::data::EffectMacro;

const MAX_NAME_BYTES: usize = 128;
const MAX_NUMBER: f32 = 10_000.0;
const ARG_COUNT: usize = 30; // includes `agent`
const MAX_SOURCE_BYTES: usize = 8192;
const TRAIL4_MACRO: &str = "AFTER_IMAGE4_ON_arg29";

// Independent literal from the pinned Shulk AttackAirF corpus, not emitter-produced source.
#[cfg(test)]
pub(crate) const TRAIL4_SOURCE_FIXTURE: &str = r#"macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new("tex_shulk_sword1"), Hash40::new("tex_shulk_sword2"), 7, Hash40::new("haver"), 0, 3, 0.9, Hash40::new("haver"), 0, 18, 0.9, true, Hash40::new("null"), Hash40::new("haver"), 0, 0, 0, 0, 0, 0, 1, 0, *EFFECT_AXIS_X, 0, *TRAIL_BLEND_ALPHA, 101, *TRAIL_CULL_NONE, 1.4, 0.2);"#;

/// The literal, editable subset of one exact `AFTER_IMAGE4_ON_arg29` call.
///
/// `native_values` retains the final two wrapper `f32` arguments without assigning them
/// gameplay meanings. The other opaque wrapper slots are required to match the observed closed
/// source form and are never user-editable here.
#[derive(Debug, Clone, PartialEq)]
pub struct Trail4Settings {
    pub texture1: String,
    pub texture2: String,
    pub length: u64,
    pub bone1: String,
    pub bone2: String,
    pub offset1: [f32; 3],
    pub offset2: [f32; 3],
    pub native_bool: bool,
    pub flare: String,
    pub flare_bone: String,
    pub flare_offset: [f32; 3],
    pub flare_rotation: [f32; 3],
    pub flare_scale: f32,
    pub axis: String,
    pub native_values: [f32; 2],
}

/// Read settings without modifying the stored source or its metadata.
pub fn settings(command: &EffectMacro) -> Result<Trail4Settings, String> {
    let EffectMacro::AfterImage {
        command: family,
        effect_name,
        bone_name,
        bone_name2,
        raw,
    } = command
    else {
        return Err("expected an imported after-image command".into());
    };
    if family != TRAIL4_MACRO {
        return Err(format!("unsupported after-image command {family:?}"));
    }

    // Validate the preserved source before asking the established source preview to apply any
    // separately-retargeted graphic/joint metadata. This keeps preview from blessing an
    // arbitrary or malformed raw expression.
    let raw_call = parse_source(raw)?;
    let raw_settings = decode_args(raw, &raw_call.args)?;
    if effect_name != &raw_settings.texture1
        || bone_name != &raw_settings.bone1
        || bone_name2.as_deref() != Some(raw_settings.bone2.as_str())
    {
        // The flat EffectCall owner can retarget these three slots independently. Derive that
        // effective source with the existing single-call preview, then validate its exact shape
        // and literal types again; do not recurse through timeline validation.
        let effective = crate::acmd::preview_effect_macro(command);
        let site = parse_source(&effective)?;
        decode_args(&effective, &site.args)
    } else {
        Ok(raw_settings)
    }
}

/// Validate settings and source first, then atomically update source and matching metadata.
pub fn update(command: &mut EffectMacro, value: &Trail4Settings) -> Result<(), String> {
    validate_settings(value)?;

    match command {
        EffectMacro::AfterImage {
            command: family, ..
        } if family == TRAIL4_MACRO => {}
        EffectMacro::AfterImage {
            command: family, ..
        } => {
            return Err(format!("unsupported after-image command {family:?}"));
        }
        _ => return Err("expected an imported after-image command".into()),
    }

    // Validate the stored raw call before applying any edits; separate metadata retargets are
    // reconciled into its original argument spans below.
    let raw = match &*command {
        EffectMacro::AfterImage { raw, .. } => raw,
        _ => unreachable!("variant checked above"),
    };
    let raw_site = parse_source(raw)?;
    let raw_settings = decode_args(raw, &raw_site.args)?;
    if let EffectMacro::AfterImage {
        effect_name,
        bone_name,
        bone_name2,
        ..
    } = &*command
    {
        if raw_settings == *value
            && effect_name == &raw_settings.texture1
            && bone_name == &raw_settings.bone1
            && bone_name2.as_deref() == Some(raw_settings.bone2.as_str())
        {
            return Ok(());
        }
    }
    // Validate the effective retargeted view too, but edit the original raw spans below. The
    // preview renderer is allowed to format source (including its trailing newline); using its
    // output as the replacement base would gradually rewrite surrounding source bytes.
    let effective = crate::acmd::preview_effect_macro(command);
    let effective_site = parse_source(&effective)?;
    decode_args(&effective, &effective_site.args)?;

    let mut replacements = Vec::new();
    let desired: [(usize, bool, String); 16] = [
        (
            1,
            raw_settings.texture1 != value.texture1,
            hash40(&value.texture1),
        ),
        (
            2,
            raw_settings.texture2 != value.texture2,
            hash40(&value.texture2),
        ),
        (
            3,
            raw_settings.length != value.length,
            value.length.to_string(),
        ),
        (4, raw_settings.bone1 != value.bone1, hash40(&value.bone1)),
        (
            5,
            raw_settings.offset1[0] != value.offset1[0],
            float_token(value.offset1[0], false),
        ),
        (
            6,
            raw_settings.offset1[1] != value.offset1[1],
            float_token(value.offset1[1], false),
        ),
        (
            7,
            raw_settings.offset1[2] != value.offset1[2],
            float_token(value.offset1[2], false),
        ),
        (8, raw_settings.bone2 != value.bone2, hash40(&value.bone2)),
        (
            9,
            raw_settings.offset2[0] != value.offset2[0],
            float_token(value.offset2[0], false),
        ),
        (
            10,
            raw_settings.offset2[1] != value.offset2[1],
            float_token(value.offset2[1], false),
        ),
        (
            11,
            raw_settings.offset2[2] != value.offset2[2],
            float_token(value.offset2[2], false),
        ),
        (
            12,
            raw_settings.native_bool != value.native_bool,
            value.native_bool.to_string(),
        ),
        (13, raw_settings.flare != value.flare, hash40(&value.flare)),
        (
            14,
            raw_settings.flare_bone != value.flare_bone,
            hash40(&value.flare_bone),
        ),
        (
            15,
            raw_settings.flare_offset[0] != value.flare_offset[0],
            float_token(value.flare_offset[0], false),
        ),
        (
            16,
            raw_settings.flare_offset[1] != value.flare_offset[1],
            float_token(value.flare_offset[1], false),
        ),
    ];
    for (slot, changed, replacement) in desired {
        if changed {
            replacements.push((raw_site.args[slot].clone(), replacement));
        }
    }
    for (slot, changed, replacement) in [
        (
            17,
            raw_settings.flare_offset[2] != value.flare_offset[2],
            float_token(value.flare_offset[2], false),
        ),
        (
            18,
            raw_settings.flare_rotation[0] != value.flare_rotation[0],
            float_token(value.flare_rotation[0], false),
        ),
        (
            19,
            raw_settings.flare_rotation[1] != value.flare_rotation[1],
            float_token(value.flare_rotation[1], false),
        ),
        (
            20,
            raw_settings.flare_rotation[2] != value.flare_rotation[2],
            float_token(value.flare_rotation[2], false),
        ),
        (
            21,
            raw_settings.flare_scale != value.flare_scale,
            float_token(value.flare_scale, false),
        ),
        (23, raw_settings.axis != value.axis, value.axis.clone()),
        (
            28,
            raw_settings.native_values[0] != value.native_values[0],
            float_token(value.native_values[0], true),
        ),
        (
            29,
            raw_settings.native_values[1] != value.native_values[1],
            float_token(value.native_values[1], true),
        ),
    ] {
        if changed {
            replacements.push((raw_site.args[slot].clone(), replacement));
        }
    }

    // All validation is complete before any field is changed. Descending spans within the
    // original raw source preserve its surrounding bytes, separators, spacing, and unedited
    // argument spellings, including when metadata had been retargeted independently.
    let mut updated_raw = raw.clone();
    replacements.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    for (span, replacement) in replacements {
        updated_raw.replace_range(span, &replacement);
    }
    let updated_site = parse_source(&updated_raw)?;
    let updated = decode_args(&updated_raw, &updated_site.args)?;
    if updated != *value {
        return Err("updated trail source does not match the requested settings".into());
    }

    let EffectMacro::AfterImage {
        raw,
        effect_name,
        bone_name,
        bone_name2,
        ..
    } = command
    else {
        unreachable!("variant checked above")
    };
    *raw = updated_raw;
    *effect_name = value.texture1.clone();
    *bone_name = value.bone1.clone();
    *bone_name2 = Some(value.bone2.clone());
    Ok(())
}

fn parse_source(source: &str) -> Result<crate::acmd_src::MacroSite, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err("after-image statement exceeds the bounded source size".into());
    }
    let block = syn::parse_str::<syn::Block>(&format!("{{ {source} }}"))
        .map_err(|_| "after-image source must be valid Rust".to_owned())?;
    if block.stmts.len() != 1 {
        return Err("expected exactly one after-image statement".into());
    }
    let syn::Stmt::Expr(syn::Expr::Call(call), Some(_)) = &block.stmts[0] else {
        return Err("expected one standalone after-image function call".into());
    };
    let syn::Expr::Path(callee) = call.func.as_ref() else {
        return Err("after-image callee must be the exact macros path".into());
    };
    let path = &callee.path;
    if !call.attrs.is_empty()
        || !callee.attrs.is_empty()
        || callee.qself.is_some()
        || path.leading_colon.is_some()
        || path.segments.len() != 2
        || path.segments[0].ident != "macros"
        || !matches!(&path.segments[0].arguments, syn::PathArguments::None)
        || path.segments[1].ident != TRAIL4_MACRO
        || !matches!(&path.segments[1].arguments, syn::PathArguments::None)
    {
        return Err(format!("expected macros::{TRAIL4_MACRO}(...)"));
    }
    let receiver_is_agent = matches!(
        call.args.first(),
        Some(syn::Expr::Path(receiver))
            if receiver.attrs.is_empty()
                && receiver.qself.is_none()
                && receiver.path.leading_colon.is_none()
                && receiver.path.is_ident("agent")
    );
    if call.args.len() != ARG_COUNT || !receiver_is_agent {
        return Err(format!(
            "expected {TRAIL4_MACRO} with {ARG_COUNT} arguments and agent receiver"
        ));
    }

    let sites = crate::acmd_src::scan_macro_sites(source, 0..source.len());
    if sites.len() != 1 {
        return Err("expected exactly one after-image macro call".into());
    }
    let site = sites.into_iter().next().unwrap();
    if site.name != TRAIL4_MACRO {
        return Err(format!("unsupported after-image macro {:?}", site.name));
    }
    if !source[..site.span.start].trim().is_empty() || source[site.span.end..].trim() != ";" {
        return Err("expected one standalone macro statement".into());
    }
    if site.args.len() != ARG_COUNT {
        return Err(format!(
            "{TRAIL4_MACRO} requires {ARG_COUNT} arguments including agent; got {}",
            site.args.len()
        ));
    }
    if site.arg(source, 0).unwrap_or_default().trim() != "agent" {
        return Err("after-image call must use the agent receiver".into());
    }
    Ok(site)
}

fn decode_args(source: &str, args: &[std::ops::Range<usize>]) -> Result<Trail4Settings, String> {
    if args.len() != ARG_COUNT {
        return Err(format!("expected {ARG_COUNT} after-image arguments"));
    }
    let token = |index: usize| source[args[index].clone()].trim();
    let texture1 = parse_hash40(token(1), "texture1")?;
    let texture2 = parse_hash40(token(2), "texture2")?;
    let length = parse_unsigned(token(3), "trail length")?;
    if length > MAX_NUMBER as u64 {
        return Err("trail length exceeds the supported numeric bound".into());
    }
    let bone1 = parse_hash40(token(4), "bone1")?;
    let offset1 = [
        parse_number(token(5), "offset1.x", false)?,
        parse_number(token(6), "offset1.y", false)?,
        parse_number(token(7), "offset1.z", false)?,
    ];
    let bone2 = parse_hash40(token(8), "bone2")?;
    let offset2 = [
        parse_number(token(9), "offset2.x", false)?,
        parse_number(token(10), "offset2.y", false)?,
        parse_number(token(11), "offset2.z", false)?,
    ];
    let native_bool = match token(12) {
        "true" => true,
        "false" => false,
        _ => return Err("native_bool must be a literal boolean".into()),
    };
    let flare = parse_hash40(token(13), "flare")?;
    let flare_bone = parse_hash40(token(14), "flare bone")?;
    let flare_offset = [
        parse_number(token(15), "flare offset x", false)?,
        parse_number(token(16), "flare offset y", false)?,
        parse_number(token(17), "flare offset z", false)?,
    ];
    let flare_rotation = [
        parse_number(token(18), "flare rotation x", false)?,
        parse_number(token(19), "flare rotation y", false)?,
        parse_number(token(20), "flare rotation z", false)?,
    ];
    let flare_scale = parse_number(token(21), "flare scale", false)?;
    if token(22) != "0" {
        return Err("unsupported native argument at slot 22".into());
    }
    let axis = token(23).to_owned();
    if !matches!(axis.as_str(), "*EFFECT_AXIS_X" | "*EFFECT_AXIS_Y") {
        return Err("axis must be *EFFECT_AXIS_X or *EFFECT_AXIS_Y".into());
    }
    if token(24) != "0" {
        return Err("unsupported native argument at slot 24".into());
    }
    if token(25) != "*TRAIL_BLEND_ALPHA" {
        return Err("unsupported trail-blend constant".into());
    }
    if token(26) != "101" {
        return Err("unsupported blend constant".into());
    }
    if token(27) != "*TRAIL_CULL_NONE" {
        return Err("unsupported trail-cull constant".into());
    }
    let native_values = [
        parse_number(token(28), "native value 1", true)?,
        parse_number(token(29), "native value 2", true)?,
    ];

    Ok(Trail4Settings {
        texture1,
        texture2,
        length,
        bone1,
        bone2,
        offset1,
        offset2,
        native_bool,
        flare,
        flare_bone,
        flare_offset,
        flare_rotation,
        flare_scale,
        axis,
        native_values,
    })
}

fn validate_settings(value: &Trail4Settings) -> Result<(), String> {
    for (name, field) in [
        ("texture1", &value.texture1),
        ("texture2", &value.texture2),
        ("bone1", &value.bone1),
        ("bone2", &value.bone2),
        ("flare", &value.flare),
        ("flare bone", &value.flare_bone),
    ] {
        validate_hash_name(field, name)?;
    }
    if value.length > MAX_NUMBER as u64 {
        return Err("trail length exceeds the supported numeric bound".into());
    }
    if !matches!(value.axis.as_str(), "*EFFECT_AXIS_X" | "*EFFECT_AXIS_Y") {
        return Err("axis must be *EFFECT_AXIS_X or *EFFECT_AXIS_Y".into());
    }
    for (name, fields) in [
        ("offset1", value.offset1.as_slice()),
        ("offset2", value.offset2.as_slice()),
        ("flare offset", value.flare_offset.as_slice()),
        ("flare rotation", value.flare_rotation.as_slice()),
        ("native values", value.native_values.as_slice()),
    ] {
        validate_numbers(fields, name)?;
    }
    validate_number(value.flare_scale, "flare scale")
}

fn parse_hash40(token: &str, field: &str) -> Result<String, String> {
    let inner = token
        .strip_prefix("Hash40::new(\"")
        .and_then(|value| value.strip_suffix("\")"))
        .ok_or_else(|| format!("{field} must be Hash40::new with a literal string"))?;
    validate_hash_name(inner, field)?;
    Ok(inner.to_owned())
}

fn validate_hash_name(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_NAME_BYTES
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte < 0x20 || byte == 0x7f || byte == b'"' || byte == b'\\')
    {
        return Err(format!(
            "{field} must be a bounded printable ASCII Hash40 literal"
        ));
    }
    Ok(())
}

fn parse_unsigned(token: &str, field: &str) -> Result<u64, String> {
    if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{field} must be an unsigned integer literal"));
    }
    token
        .parse::<u64>()
        .map_err(|_| format!("{field} is outside the supported integer range"))
}

fn parse_number(token: &str, field: &str, require_float: bool) -> Result<f32, String> {
    if !is_numeric_literal(token, require_float) {
        return Err(format!("{field} must be a numeric literal"));
    }
    let value = token
        .parse::<f32>()
        .map_err(|_| format!("{field} must be a numeric literal"))?;
    validate_number(value, field)?;
    Ok(value)
}

fn is_numeric_literal(token: &str, require_float: bool) -> bool {
    let unsigned = token
        .strip_prefix('-')
        .or_else(|| token.strip_prefix('+'))
        .unwrap_or(token);
    if unsigned.is_empty() || unsigned.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return false;
    }
    let mut exponent_parts = unsigned.split(['e', 'E']);
    let mantissa = exponent_parts.next().unwrap_or_default();
    let exponent = exponent_parts.next();
    if exponent_parts.next().is_some() {
        return false;
    }
    let exponent_ok = exponent.is_none_or(|part| {
        let digits = part
            .strip_prefix('-')
            .or_else(|| part.strip_prefix('+'))
            .unwrap_or(part);
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    });
    if !exponent_ok {
        return false;
    }
    let mut decimal_parts = mantissa.split('.');
    let whole = decimal_parts.next().unwrap_or_default();
    let fraction = decimal_parts.next();
    if decimal_parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    match fraction {
        Some(digits) => {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) && exponent_ok
        }
        None => !require_float || exponent.is_some(),
    }
}

fn validate_numbers(values: &[f32], field: &str) -> Result<(), String> {
    for value in values {
        validate_number(*value, field)?;
    }
    Ok(())
}

fn validate_number(value: f32, field: &str) -> Result<(), String> {
    if !value.is_finite() || value.abs() > MAX_NUMBER {
        return Err(format!(
            "{field} must be finite and within the supported bound"
        ));
    }
    Ok(())
}

fn hash40(value: &str) -> String {
    format!("Hash40::new(\"{value}\")")
}

fn float_token(value: f32, require_float: bool) -> String {
    let token = value.to_string();
    if require_float && !token.contains('.') && !token.contains('e') && !token.contains('E') {
        format!("{token}.0")
    } else {
        token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independent literal copied from the pinned Shulk AttackAirF corpus. The second joint is
    // deliberately changed below to make a slot-4/slot-8 swap observable.
    const SHULK: &str = TRAIL4_SOURCE_FIXTURE;

    fn imported(source: &str) -> EffectMacro {
        let source = source.replace(
            "Hash40::new(\"haver\"), 0, 18",
            "Hash40::new(\"haver_far\"), 0, 18",
        );
        EffectMacro::AfterImage {
            command: TRAIL4_MACRO.into(),
            effect_name: "tex_shulk_sword1".into(),
            bone_name: "haver".into(),
            bone_name2: Some("haver_far".into()),
            raw: source,
        }
    }

    #[test]
    fn shulk_literal_reads_and_edits_exact_source_slots() {
        let mut command = imported(SHULK);
        let mut value = settings(&command).unwrap();
        assert_eq!(value.bone1, "haver");
        assert_eq!(value.bone2, "haver_far");
        assert_eq!(value.length, 7);
        assert_eq!(value.offset1, [0.0, 3.0, 0.9]);
        assert_eq!(value.offset2, [0.0, 18.0, 0.9]);
        assert_eq!(value.native_values, [1.4, 0.2]);

        value.texture1 = "custom_trail_a".into();
        value.texture2 = "custom_trail_b".into();
        value.offset1 = [-1.25, 3.0, 0.9];
        value.axis = "*EFFECT_AXIS_Y".into();
        value.native_values = [1.25, -0.5];
        update(&mut command, &value).unwrap();

        let EffectMacro::AfterImage {
            raw,
            effect_name,
            bone_name,
            bone_name2,
            ..
        } = &command
        else {
            panic!("after-image expected")
        };
        assert_eq!(effect_name, "custom_trail_a");
        assert_eq!(bone_name, "haver");
        assert_eq!(bone_name2.as_deref(), Some("haver_far"));
        assert!(raw.contains("-1.25, 3, 0.9"));
        assert!(raw.contains("*EFFECT_AXIS_Y"));
        assert!(raw.contains("1.25, -0.5"));
        assert_eq!(settings(&command).unwrap(), value);

        let no_change = raw.clone();
        let current = settings(&command).unwrap();
        update(&mut command, &current).unwrap();
        let EffectMacro::AfterImage { raw, .. } = &command else {
            panic!("after-image expected")
        };
        assert_eq!(raw, &no_change);
    }

    #[test]
    fn repeated_edits_preserve_raw_suffix_and_do_not_grow_source() {
        let original = format!(" \t{SHULK}\r\n \t");
        let mut command = imported(&original);
        let original = match &command {
            EffectMacro::AfterImage { raw, .. } => raw.clone(),
            _ => unreachable!(),
        };
        let original_len = original.len();
        let unchanged = settings(&command).unwrap();
        update(&mut command, &unchanged).unwrap();
        assert!(matches!(&command, EffectMacro::AfterImage { raw, .. } if raw == &original));

        let mut previous_len = None;
        for index in 0..128 {
            let mut value = settings(&command).unwrap();
            value.texture1 = format!("trail_custom_{index:03}");
            value.offset1[0] = if index % 2 == 0 { -1.25 } else { -2.25 };
            value.axis = if index % 2 == 0 {
                "*EFFECT_AXIS_Y".into()
            } else {
                "*EFFECT_AXIS_X".into()
            };
            update(&mut command, &value).unwrap();
            let EffectMacro::AfterImage { raw, .. } = &command else {
                panic!("after-image expected")
            };
            assert!(raw.starts_with(" \t"));
            assert!(raw.ends_with("\r\n \t"));
            assert!(
                raw.len() <= original_len + 32,
                "raw grew to {} bytes",
                raw.len()
            );
            if let Some(expected_len) = previous_len {
                assert_eq!(raw.len(), expected_len);
            }
            previous_len = Some(raw.len());
            assert_eq!(settings(&command).unwrap(), value);
        }
    }

    #[test]
    fn malformed_source_shapes_and_tokens_are_rejected() {
        let valid = imported(SHULK);
        let EffectMacro::AfterImage { raw, .. } = &valid else {
            unreachable!()
        };
        let site = crate::acmd_src::scan_macro_sites(raw, 0..raw.len())
            .into_iter()
            .next()
            .unwrap();
        let short_22 = format!("{});", &raw[..site.args[21].end]);
        let cases = [
            format!("{}{raw}", " ".repeat(MAX_SOURCE_BYTES)),
            short_22, // the observed Elight-shaped 22-argument short form
            raw.replace(
                "macros::AFTER_IMAGE4_ON_arg29(agent",
                "macros::AFTER_IMAGE4_ON_arg29(other",
            ),
            raw.replace("Hash40::new(\"tex_shulk_sword1\")", "make_texture()"),
            raw.replace("Hash40::new(\"tex_shulk_sword1\")", "Hash40::new(texture)"),
            raw.replace("0, 3, 0.9", "0 + 1, 3, 0.9"),
            raw.replace("0, 3, 0.9", "+1, 3, 0.9"),
            raw.replace("*EFFECT_AXIS_X", "*TRAIL_BLEND_ALPHA"),
            raw.replace("*EFFECT_AXIS_X", "*EFFECT_AXIS_Z"),
            raw.replace("*TRAIL_BLEND_ALPHA", "*EFFECT_AXIS_X"),
            raw.replace("true, Hash40", "1, Hash40"),
            raw.replace("1.4, 0.2", "f32::INFINITY, 0.2"),
            raw.replace("1.4, 0.2", "1, 0.2"), // final wrapper args are declared f32
            format!("{raw} {raw}"),
            raw.replace(
                "macros::AFTER_IMAGE4_ON_arg29",
                "other::AFTER_IMAGE4_ON_arg29",
            ),
            raw.replace(
                "macros::AFTER_IMAGE4_ON_arg29(",
                "macros::AFTER_IMAGE4_ON_arg29!(",
            ),
            format!("#[allow(unused)] {raw}"),
            raw.replace(
                "macros::AFTER_IMAGE4_ON_arg29(",
                "::macros::AFTER_IMAGE4_ON_arg29(",
            ),
            raw.replace(
                "macros::AFTER_IMAGE4_ON_arg29(",
                "macros::<()>::AFTER_IMAGE4_ON_arg29(",
            ),
            format!("({raw})"),
        ];
        for malformed in cases {
            let command = match &valid {
                EffectMacro::AfterImage {
                    command,
                    effect_name,
                    bone_name,
                    bone_name2,
                    ..
                } => EffectMacro::AfterImage {
                    command: command.clone(),
                    effect_name: effect_name.clone(),
                    bone_name: bone_name.clone(),
                    bone_name2: bone_name2.clone(),
                    raw: malformed,
                },
                _ => unreachable!(),
            };
            assert!(settings(&command).is_err());
        }
    }

    #[test]
    fn invalid_settings_and_source_leave_command_unchanged() {
        let mut command = imported(SHULK);
        let before = command.clone();
        let mut value = settings(&command).unwrap();
        value.axis = "*TRAIL_BLEND_ALPHA".into();
        assert!(update(&mut command, &value).is_err());
        assert!(matches!((&command, &before), (
            EffectMacro::AfterImage { raw: a, effect_name: ae, bone_name: ab, bone_name2: aj, .. },
            EffectMacro::AfterImage { raw: b, effect_name: be, bone_name: bb, bone_name2: bj, .. }
        ) if a == b && ae == be && ab == bb && aj == bj));

        let mut bad_source = imported(SHULK);
        let EffectMacro::AfterImage { raw, .. } = &mut bad_source else {
            unreachable!()
        };
        *raw = raw.replace("1.4, 0.2", "1.4, f32::NAN");
        let before_bad = bad_source.clone();
        let mut valid_value = settings(&imported(SHULK)).unwrap();
        valid_value.length = 8;
        assert!(update(&mut bad_source, &valid_value).is_err());
        assert!(matches!((&bad_source, &before_bad), (
            EffectMacro::AfterImage { raw: a, .. },
            EffectMacro::AfterImage { raw: b, .. }
        ) if a == b));
    }

    #[test]
    fn effective_metadata_retarget_is_read_and_committed_as_source() {
        let source = format!(" \t{SHULK}\r\n \t");
        let mut command = imported(&source);
        let original_raw = match &command {
            EffectMacro::AfterImage { raw, .. } => raw.clone(),
            _ => unreachable!(),
        };
        let EffectMacro::AfterImage {
            effect_name,
            bone_name,
            bone_name2,
            ..
        } = &mut command
        else {
            unreachable!()
        };
        *effect_name = "retargeted_texture".into();
        *bone_name = "arm_joint".into();
        *bone_name2 = Some("hand_joint".into());

        let mut value = settings(&command).unwrap();
        assert_eq!(value.texture1, "retargeted_texture");
        assert_eq!(value.bone1, "arm_joint");
        assert_eq!(value.bone2, "hand_joint");
        assert!(matches!(&command, EffectMacro::AfterImage { raw, .. } if raw == &original_raw));

        value.texture2 = "retargeted_texture_2".into();
        update(&mut command, &value).unwrap();
        let EffectMacro::AfterImage {
            raw,
            effect_name,
            bone_name,
            bone_name2,
            ..
        } = &command
        else {
            unreachable!()
        };
        assert!(raw.starts_with(" \t"));
        assert!(raw.ends_with("\r\n \t"));
        assert!(raw.len() <= original_raw.len() + 64);
        assert!(raw.contains("Hash40::new(\"retargeted_texture\")"));
        assert!(raw.contains("Hash40::new(\"retargeted_texture_2\")"));
        assert!(raw.contains("Hash40::new(\"arm_joint\")"));
        assert!(raw.contains("Hash40::new(\"hand_joint\")"));
        assert_eq!(effect_name, "retargeted_texture");
        assert_eq!(bone_name, "arm_joint");
        assert_eq!(bone_name2.as_deref(), Some("hand_joint"));
        assert_eq!(settings(&command).unwrap(), value);
    }

    #[test]
    fn invalid_new_values_leave_raw_and_metadata_unchanged() {
        let invalid = [
            {
                let mut value = settings(&imported(SHULK)).unwrap();
                value.texture1 = "bad\"texture".into();
                value
            },
            {
                let mut value = settings(&imported(SHULK)).unwrap();
                value.length = 10_001;
                value
            },
            {
                let mut value = settings(&imported(SHULK)).unwrap();
                value.offset2[1] = f32::INFINITY;
                value
            },
        ];
        for value in invalid {
            let mut command = imported(SHULK);
            let before = match &command {
                EffectMacro::AfterImage {
                    raw,
                    effect_name,
                    bone_name,
                    bone_name2,
                    ..
                } => (
                    raw.clone(),
                    effect_name.clone(),
                    bone_name.clone(),
                    bone_name2.clone(),
                ),
                _ => unreachable!(),
            };
            assert!(update(&mut command, &value).is_err());
            assert!(matches!(&command, EffectMacro::AfterImage {
                raw,
                effect_name,
                bone_name,
                bone_name2,
                ..
            } if raw == &before.0
                && effect_name == &before.1
                && bone_name == &before.2
                && bone_name2 == &before.3));
        }
    }
}
