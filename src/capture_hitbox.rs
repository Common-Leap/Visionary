//! Convert captured Lua calls into editor hitboxes using each family's argument layout.
use crate::game_link::LuaArgWire;
use std::collections::HashMap;

struct CaptureArgs<'a>(&'a [LuaArgWire]);
impl CaptureArgs<'_> {
    fn int(&self, i: usize) -> Option<i64> {
        self.0.get(i)?.as_i64()
    }
    fn float(&self, i: usize) -> Option<f32> {
        self.0.get(i)?.as_f32()
    }
    fn flag(&self, i: usize) -> bool {
        self.int(i).is_some_and(|v| v != 0)
    }
    fn konst(&self, i: usize, table: crate::param_labels::ConstTable) -> String {
        self.int(i)
            .map(|v| {
                crate::param_labels::const_name(table, v)
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default()
    }
    fn bone(&self, i: usize, names: &HashMap<u64, String>) -> Option<String> {
        let hash = self.0.get(i)?.as_hash()?;
        Some(
            names
                .get(&hash)
                .cloned()
                .unwrap_or_else(|| format!("{hash:#x}")),
        )
    }
    fn capsule(&self, i: usize) -> Option<[f32; 3]> {
        Some([self.float(i)?, self.float(i + 1)?, self.float(i + 2)?])
    }
}

pub(crate) fn hitbox_from_capture(
    func: &str,
    args: &[crate::game_link::LuaArgWire],
    frame: f32,
    bone_rev: &HashMap<u64, String>,
    labels: &HashMap<u64, String>,
) -> Option<crate::data::Hitbox> {
    let r = CaptureArgs(args);
    if args.len() < 17 {
        return None;
    }
    let bone_name = r.bone(2, bone_rev)?;
    let capsule_end = r.capsule(12);
    let start = crate::data::motion_to_script_frame(frame);
    Some(crate::data::Hitbox {
        func: func.to_string(),
        id: r.int(0)? as u32,
        part: r.int(1).unwrap_or(0) as u32,
        bone_name,
        damage: r.float(3).unwrap_or(0.0),
        angle: r.int(4).unwrap_or(0) as i32,
        kb_scaling: r.int(5).unwrap_or(0) as i32,
        fkb: r.int(6).unwrap_or(0) as i32,
        kb_base: r.int(7).unwrap_or(0) as i32,
        size: r.float(8).unwrap_or(1.0),
        offset_x: r.float(9).unwrap_or(0.0),
        offset_y: r.float(10).unwrap_or(0.0),
        offset_z: r.float(11).unwrap_or(0.0),
        capsule_end,
        hitlag_mult: r.float(15).unwrap_or(1.0),
        sdi_mult: r.float(16).unwrap_or(1.0),
        setoff_kind: r.konst(17, crate::param_labels::SETOFF_KIND),
        lr_check: r.konst(18, crate::param_labels::LR_CHECK),
        is_clang: r.flag(19),
        is_add_attack: r.int(20).unwrap_or(0) as i32,
        hitbox_attr: r.float(21).unwrap_or(0.0),
        ground_or_air: r.int(22).unwrap_or(0) as i32,
        is_mtk: r.flag(23),
        is_shield_disable: r.flag(24),
        is_reflectable: r.flag(25),
        is_absorbable: r.flag(26),
        is_landing_attack: r.flag(27),
        situation_mask: r.konst(28, crate::param_labels::SITUATION_MASK),
        category_mask: r.konst(29, crate::param_labels::CATEGORY_MASK),
        part_mask: r.konst(30, crate::param_labels::PART_MASK),
        no_finish_camera: r.flag(31),
        collision_attr: args
            .get(32)
            .and_then(|a| a.as_hash())
            .map(|h| crate::param_labels::decode_collision_attr(h, labels))
            .unwrap_or_default(),
        sound_level: r.konst(33, crate::param_labels::SOUND_LEVEL),
        sound_attr: r.konst(34, crate::param_labels::SOUND_ATTR),
        attack_region: r.konst(35, crate::param_labels::ATTACK_REGION),
        active_start: start,
        active_end: u32::MAX,
        hitbox_type: 0,
        category: 0,
        wind: None,
        catch: None,
        abs: None,
        search: None,
        fp: None,
    })
}
pub(crate) fn hitbox_from_capture_fp(
    args: &[crate::game_link::LuaArgWire],
    frame: f32,
    bone_rev: &HashMap<u64, String>,
    labels: &HashMap<u64, String>,
) -> Option<crate::data::Hitbox> {
    let r = CaptureArgs(args);
    use crate::data::{AttackFpArg, AttackFpCall};
    if args.len() < crate::data::ATTACK_FP_ARGC {
        return None;
    }
    let fp_args = args
        .iter()
        .take(crate::data::ATTACK_FP_ARGC)
        .map(|arg| match arg {
            crate::game_link::LuaArgWire::Hash(value) => AttackFpArg::Hash(*value),
            crate::game_link::LuaArgWire::Num(value) => AttackFpArg::Num(*value),
            crate::game_link::LuaArgWire::Int(value) => AttackFpArg::Int(*value),
            crate::game_link::LuaArgWire::Bool(value) => AttackFpArg::Bool(*value),
            crate::game_link::LuaArgWire::Nil => AttackFpArg::Nil,
        })
        .collect();
    let call = AttackFpCall { args: fp_args };
    let mut hitbox = call.to_hitbox(crate::data::motion_to_script_frame(frame));

    if let Some(hash) = args.get(2).and_then(|arg| arg.as_hash()) {
        hitbox.bone_name = bone_rev
            .get(&hash)
            .cloned()
            .unwrap_or_else(|| format!("{hash:#x}"));
    }
    hitbox.collision_attr = args
        .get(12)
        .and_then(|arg| arg.as_hash())
        .and_then(|hash| labels.get(&hash).cloned())
        .or_else(|| {
            args.get(12)
                .and_then(|arg| arg.as_hash())
                .map(|hash| format!("{hash:#x}"))
        })
        .unwrap_or_default();
    hitbox.lr_check = r.konst(34, crate::param_labels::LR_CHECK);
    hitbox.sound_level = r.konst(19, crate::param_labels::SOUND_LEVEL);
    hitbox.sound_attr = r.konst(20, crate::param_labels::SOUND_ATTR);
    hitbox.attack_region = r.konst(23, crate::param_labels::ATTACK_REGION);
    hitbox.is_clang = r.flag(16);
    hitbox.ground_or_air = r.int(21).unwrap_or(0) as i32;
    hitbox.is_reflectable = r.flag(29);
    hitbox.is_absorbable = r.flag(30);
    hitbox.hitlag_mult = r.float(14).unwrap_or(1.0);
    hitbox.sdi_mult = r.float(15).unwrap_or(1.0);
    Some(hitbox)
}
pub(crate) fn hitbox_from_capture_abs(
    args: &[crate::game_link::LuaArgWire],
    frame: f32,
) -> Option<crate::data::Hitbox> {
    if args.len() < 16 {
        return None;
    }
    let int = |i: usize| args.get(i).and_then(|a| a.as_i64()).unwrap_or(0);
    let float = |i: usize| args.get(i).and_then(|a| a.as_f32()).unwrap_or(0.0);
    let konst = |i: usize, table: crate::param_labels::ConstTable| {
        crate::param_labels::const_name(table, int(i))
            .map(str::to_string)
            .unwrap_or_else(|| int(i).to_string())
    };
    let call = crate::data::AttackAbsCall {
        kind: int(0).to_string(),
        id: int(1).max(0) as u32,
        damage: float(2),
        angle: int(3) as i32,
        kb_scaling: int(4) as i32,
        fkb: int(5) as i32,
        kb_base: int(6) as i32,
        hitlag_mult: float(7),
        lr_check: konst(9, crate::param_labels::LR_CHECK),
        collision_attr: args
            .get(12)
            .and_then(|a| a.as_hash())
            .map(|h| format!("{h:#x}"))
            .unwrap_or_default(),
        sound_level: konst(13, crate::param_labels::SOUND_LEVEL),
        sound_attr: konst(14, crate::param_labels::SOUND_ATTR),
        attack_region: konst(15, crate::param_labels::ATTACK_REGION),
        unknowns: (
            float(8),
            float(10),
            args.get(11).and_then(|a| a.as_i64()).unwrap_or(1) != 0,
        ),
    };
    Some(call.to_hitbox(crate::data::motion_to_script_frame(frame)))
}

pub(crate) fn hitbox_from_capture_grab(
    args: &[crate::game_link::LuaArgWire],
    frame: f32,
    bone_rev: &HashMap<u64, String>,
) -> Option<crate::data::Hitbox> {
    let r = CaptureArgs(args);
    if args.len() < 6 {
        return None;
    }
    let bone_name = r.bone(1, bone_rev)?;
    let capsule_end = r.capsule(6);
    let start = crate::data::motion_to_script_frame(frame);
    let catch = crate::data::CatchExtras {
        status: r
            .int(9)
            .map(|v| v.to_string())
            .unwrap_or_else(|| crate::data::CATCH_DEFAULT_STATUS.to_string()),
        situation: r
            .int(10)
            .map(|v| v.to_string())
            .unwrap_or_else(|| crate::data::CATCH_DEFAULT_SITUATION.to_string()),
    };
    Some(crate::data::Hitbox {
        id: r.int(0).unwrap_or(0) as u32,
        bone_name,
        size: r.float(2).unwrap_or(2.0),
        offset_x: r.float(3).unwrap_or(0.0),
        offset_y: r.float(4).unwrap_or(0.0),
        offset_z: r.float(5).unwrap_or(0.0),
        capsule_end,
        catch: Some(catch),
        damage: 0.0,
        angle: 0,
        kb_scaling: 0,
        fkb: 0,
        kb_base: 0,
        active_start: start,
        active_end: u32::MAX,
        category: 1,
        ..Default::default()
    })
}
pub(crate) fn hitbox_from_capture_search(
    args: &[crate::game_link::LuaArgWire],
    frame: f32,
    bone_rev: &HashMap<u64, String>,
) -> Option<crate::data::Hitbox> {
    let r = CaptureArgs(args);
    use crate::param_labels as pl;
    if args.len() < 8 {
        return None;
    }
    let bone_name = r.bone(2, bone_rev)?;
    let has_capsule_slots = args.len() >= 17;
    let capsule_end = match (r.float(7), r.float(8), r.float(9)) {
        (Some(x), Some(y), Some(z)) if has_capsule_slots => Some([x, y, z]),
        _ => None,
    };
    let tail = if has_capsule_slots { 10 } else { 7 };
    let konst = |i: usize, table: pl::ConstTable, default: &str| {
        r.int(i)
            .and_then(|v| pl::const_name(table, v))
            .map(|name| name.to_string())
            .unwrap_or_else(|| default.to_string())
    };
    Some(crate::data::Hitbox {
        id: r.int(0).unwrap_or(0) as u32,
        part: r.int(1).unwrap_or(0) as u32,
        bone_name,
        size: r.float(3).unwrap_or(2.0),
        offset_x: r.float(4).unwrap_or(0.0),
        offset_y: r.float(5).unwrap_or(0.0),
        offset_z: r.float(6).unwrap_or(0.0),
        capsule_end,
        situation_mask: konst(tail + 3, pl::SITUATION_MASK, "COLLISION_SITUATION_MASK_GA"),
        category_mask: konst(tail + 4, pl::CATEGORY_MASK, "COLLISION_CATEGORY_MASK_ALL"),
        part_mask: konst(tail + 5, pl::PART_MASK, "COLLISION_PART_MASK_ALL"),
        search: Some(crate::data::SearchExtras {
            collision_kind: konst(
                tail,
                pl::COLLISION_KIND_MASK,
                crate::data::SEARCH_DEFAULT_COLLISION_KIND,
            ),
            hit_status: konst(
                tail + 1,
                pl::HIT_STATUS_MASK,
                crate::data::SEARCH_DEFAULT_HIT_STATUS,
            ),
            unk: r.int(tail + 2).unwrap_or(0),
            unk2: r.int(tail + 6).is_some_and(|v| v != 0),
        }),
        damage: 0.0,
        angle: 0,
        kb_scaling: 0,
        fkb: 0,
        kb_base: 0,
        active_start: crate::data::motion_to_script_frame(frame),
        active_end: u32::MAX,
        category: crate::data::CAT_SEARCH,
        ..Default::default()
    })
}
pub(crate) fn hitbox_from_capture_wind(
    command: &str,
    args: &[crate::game_link::LuaArgWire],
    frame: f32,
) -> Option<crate::data::Hitbox> {
    let expected = match command {
        "AREA_WIND_2ND_RAD" => 8,
        "AREA_WIND_2ND_RAD_arg9" => 9,
        "AREA_WIND_2ND" => 9,
        "AREA_WIND_2ND_arg10" => 10,
        _ => return None,
    };
    if args.len() != expected {
        return None;
    }
    let values: Option<Vec<f32>> = args.iter().map(|arg| arg.as_f32()).collect();
    let wind = crate::data::WindboxData {
        command: command.into(),
        args: values?,
    };
    Some(wind.to_hitbox(crate::data::motion_to_script_frame(frame)))
}
