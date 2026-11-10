//! Lets a move made from blocks drop a call the game's own script makes.
//!
//! The editor sends a removed call as a `CAT_DROP` rule keyed by the call's name. Each hook
//! here skips the game's call while such a rule matches the acting move and frame.
use super::{LuaArg, CAT_DROP};
use smash::app::BattleObjectModuleAccessor as Boma;

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_size)]
unsafe fn drop_attackmodule_set_size(boma: *mut Boma, arg2: i32, arg3: f32) {
    if super::dropped(boma, &["AttackModule::set_size"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_add_reaction_frame)]
unsafe fn drop_attackmodule_set_add_reaction_frame(
    boma: *mut Boma,
    arg2: i32,
    arg3: f32,
    arg4: bool,
) {
    if super::dropped(boma, &["AttackModule::set_add_reaction_frame"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3, arg4)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_add_reaction_frame_revised)]
unsafe fn drop_attackmodule_set_add_reaction_frame_revised(
    boma: *mut Boma,
    arg2: i32,
    arg3: f32,
    arg4: bool,
) {
    if super::dropped(boma, &["AttackModule::set_add_reaction_frame_revised"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3, arg4)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_damage_shake_scale)]
unsafe fn drop_attackmodule_set_damage_shake_scale(boma: *mut Boma, arg2: f32) {
    if super::dropped(boma, &["AttackModule::set_damage_shake_scale"]) {
        return Default::default();
    }
    original!()(boma, arg2)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_no_uniq_effect_all)]
unsafe fn drop_attackmodule_set_no_uniq_effect_all(boma: *mut Boma, arg2: bool, arg3: bool) {
    if super::dropped(boma, &["AttackModule::set_no_uniq_effect_all"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::init_attack_pos)]
unsafe fn drop_attackmodule_init_attack_pos(boma: *mut Boma, arg2: i32) -> u64 {
    if super::dropped(boma, &["AttackModule::init_attack_pos"]) {
        return Default::default();
    }
    original!()(boma, arg2)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_no_damage_fly_smoke_all)]
unsafe fn drop_attackmodule_set_no_damage_fly_smoke_all(boma: *mut Boma, arg2: bool, arg3: bool) {
    if super::dropped(boma, &["AttackModule::set_no_damage_fly_smoke_all"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::disable_tip)]
unsafe fn drop_attackmodule_disable_tip(boma: *mut Boma) -> u64 {
    if super::dropped(boma, &["AttackModule::disable_tip"]) {
        return Default::default();
    }
    original!()(boma)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::enable_safe_pos)]
unsafe fn drop_attackmodule_enable_safe_pos(boma: *mut Boma) -> u64 {
    if super::dropped(boma, &["AttackModule::enable_safe_pos"]) {
        return Default::default();
    }
    original!()(boma)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_attack_keep_rumble)]
unsafe fn drop_attackmodule_set_attack_keep_rumble(boma: *mut Boma, arg2: i32, arg3: bool) {
    if super::dropped(boma, &["AttackModule::set_attack_keep_rumble"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_force_reaction)]
unsafe fn drop_attackmodule_set_force_reaction(boma: *mut Boma, arg2: i32, arg3: bool, arg4: bool) {
    if super::dropped(boma, &["AttackModule::set_force_reaction"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3, arg4)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_attack_level)]
unsafe fn drop_attackmodule_set_attack_level(boma: *mut Boma, arg2: i32, arg3: u8) {
    if super::dropped(boma, &["AttackModule::set_attack_level"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_invalid_xlu)]
unsafe fn drop_attackmodule_set_invalid_xlu(boma: *mut Boma, arg2: i32, arg3: bool) {
    if super::dropped(boma, &["AttackModule::set_invalid_xlu"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_no_finish_camera_ex)]
unsafe fn drop_attackmodule_set_no_finish_camera_ex(
    boma: *mut Boma,
    arg2: i32,
    arg3: bool,
    arg4: bool,
) {
    if super::dropped(boma, &["AttackModule::set_no_finish_camera_ex"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3, arg4)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_final_finish_cut_in)]
unsafe fn drop_attackmodule_set_final_finish_cut_in(
    boma: *mut Boma,
    arg2: i32,
    arg3: bool,
    arg4: bool,
    arg5: f32,
    arg6: bool,
) {
    if super::dropped(boma, &["AttackModule::set_final_finish_cut_in"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3, arg4, arg5, arg6)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_attack_camera_quake_forced)]
unsafe fn drop_attackmodule_set_attack_camera_quake_forced(
    boma: *mut Boma,
    arg2: i32,
    arg3: i32,
    arg4: bool,
) {
    if super::dropped(boma, &["AttackModule::set_attack_camera_quake_forced"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3, arg4)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_ink_value)]
unsafe fn drop_attackmodule_set_ink_value(boma: *mut Boma, arg2: i32, arg3: f32) {
    if super::dropped(boma, &["AttackModule::set_ink_value"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_target_category)]
unsafe fn drop_attackmodule_set_target_category(boma: *mut Boma, arg2: i32, arg3: u32) {
    if super::dropped(boma, &["AttackModule::set_target_category"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_attack_height_all)]
unsafe fn drop_attackmodule_set_attack_height_all(
    boma: *mut Boma,
    arg2: smash::app::AttackHeight,
    arg3: bool,
) {
    if super::dropped(boma, &["AttackModule::set_attack_height_all"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_no_dead_all)]
unsafe fn drop_attackmodule_set_no_dead_all(boma: *mut Boma, arg2: bool, arg3: bool) {
    if super::dropped(boma, &["AttackModule::set_no_dead_all"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_catch_only_all)]
unsafe fn drop_attackmodule_set_catch_only_all(boma: *mut Boma, arg2: bool, arg3: bool) {
    if super::dropped(boma, &["AttackModule::set_catch_only_all"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_no_finish_camera)]
unsafe fn drop_attackmodule_set_no_finish_camera(
    boma: *mut Boma,
    arg2: i32,
    arg3: bool,
    arg4: bool,
) {
    if super::dropped(boma, &["AttackModule::set_no_finish_camera"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3, arg4)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_attack_reference_joint_id)]
unsafe fn drop_attackmodule_set_attack_reference_joint_id(
    boma: *mut Boma,
    arg2: smash::phx::Hash40,
    arg3: smash::app::AttackDirectionAxis,
    arg4: smash::app::AttackDirectionAxis,
    arg5: smash::app::AttackDirectionAxis,
) {
    if super::dropped(boma, &["AttackModule::set_attack_reference_joint_id"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3, arg4, arg5)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_vec_target_pos)]
unsafe fn drop_attackmodule_set_vec_target_pos(
    boma: *mut Boma,
    arg2: i32,
    arg3: smash::phx::Hash40,
    arg4: *const smash::phx::Vector2f,
    arg5: u32,
    arg6: bool,
) {
    if super::dropped(boma, &["AttackModule::set_vec_target_pos"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3, arg4, arg5, arg6)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_optional_hit_effect)]
unsafe fn drop_attackmodule_set_optional_hit_effect(
    boma: *mut Boma,
    arg2: i32,
    arg3: smash::phx::Hash40,
) {
    if super::dropped(boma, &["AttackModule::set_optional_hit_effect"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::AttackModule::set_optional_hit_sound)]
unsafe fn drop_attackmodule_set_optional_hit_sound(
    boma: *mut Boma,
    arg2: i32,
    arg3: smash::phx::Hash40,
) {
    if super::dropped(boma, &["AttackModule::set_optional_hit_sound"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::ItemModule::set_have_item_visibility)]
unsafe fn drop_itemmodule_set_have_item_visibility(boma: *mut Boma, arg2: bool, arg3: i32) {
    if super::dropped(boma, &["ItemModule::set_have_item_visibility"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::ItemModule::set_attach_item_visibility)]
unsafe fn drop_itemmodule_set_attach_item_visibility(boma: *mut Boma, arg2: bool, arg3: u8) {
    if super::dropped(boma, &["ItemModule::set_attach_item_visibility"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::FighterAreaModuleImpl::enable_fix_jostle_area)]
unsafe fn drop_fighterareamoduleimpl_enable_fix_jostle_area(
    boma: *mut Boma,
    arg2: f32,
    arg3: f32,
) -> u64 {
    if super::dropped(boma, &["FighterAreaModuleImpl::enable_fix_jostle_area"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::FighterAreaModuleImpl::enable_fix_jostle_area_xy)]
unsafe fn drop_fighterareamoduleimpl_enable_fix_jostle_area_xy(
    boma: *mut Boma,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
) -> u64 {
    if super::dropped(boma, &["FighterAreaModuleImpl::enable_fix_jostle_area_xy"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3, arg4, arg5)
}

#[skyline::hook(replace = smash::app::lua_bind::VisibilityModule::set_int64)]
unsafe fn drop_visibilitymodule_set_int64(boma: *mut Boma, arg2: i64, arg3: i64) {
    if super::dropped(boma, &["VisibilityModule::set_int64"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::VisibilityModule::set_default_int64)]
unsafe fn drop_visibilitymodule_set_default_int64(boma: *mut Boma, arg2: i64) {
    if super::dropped(boma, &["VisibilityModule::set_default_int64"]) {
        return Default::default();
    }
    original!()(boma, arg2)
}

#[skyline::hook(replace = smash::app::lua_bind::VisibilityModule::set_status_default_int64)]
unsafe fn drop_visibilitymodule_set_status_default_int64(boma: *mut Boma, arg2: i64, arg3: i64) {
    if super::dropped(boma, &["VisibilityModule::set_status_default_int64"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::VisibilityModule::set_whole)]
unsafe fn drop_visibilitymodule_set_whole(boma: *mut Boma, arg2: bool) {
    if super::dropped(boma, &["VisibilityModule::set_whole", "set_whole"]) {
        return Default::default();
    }
    original!()(boma, arg2)
}

#[skyline::hook(replace = smash::app::lua_bind::ArticleModule::change_status)]
unsafe fn drop_articlemodule_change_status(
    boma: *mut Boma,
    article: i32,
    status: i32,
    operation_target: smash::app::ArticleOperationTarget,
) -> u64 {
    if super::dropped(boma, &["ArticleModule::change_status"]) {
        return Default::default();
    }
    original!()(boma, article, status, operation_target)
}

#[skyline::hook(replace = smash::app::lua_bind::ArticleModule::change_status_exist)]
unsafe fn drop_articlemodule_change_status_exist(
    boma: *mut Boma,
    article: i32,
    status: i32,
) -> u64 {
    if super::dropped(boma, &["ArticleModule::change_status_exist"]) {
        return Default::default();
    }
    original!()(boma, article, status)
}

#[skyline::hook(replace = smash::app::lua_bind::ArticleModule::change_motion)]
unsafe fn drop_articlemodule_change_motion(
    boma: *mut Boma,
    article: i32,
    motion: smash::phx::Hash40,
    arg4: bool,
    frame: f32,
) -> u64 {
    if super::dropped(boma, &["ArticleModule::change_motion"]) {
        return Default::default();
    }
    original!()(boma, article, motion, arg4, frame)
}

#[skyline::hook(replace = smash::app::lua_bind::ArticleModule::shoot_exist)]
unsafe fn drop_articlemodule_shoot_exist(
    boma: *mut Boma,
    article: i32,
    operation_target: smash::app::ArticleOperationTarget,
    arg4: bool,
) -> u64 {
    if super::dropped(boma, &["ArticleModule::shoot_exist", "shoot_article"]) {
        return Default::default();
    }
    original!()(boma, article, operation_target, arg4)
}

#[skyline::hook(replace = smash::app::lua_bind::ArticleModule::shoot)]
unsafe fn drop_articlemodule_shoot(
    boma: *mut Boma,
    article: i32,
    operation_target: smash::app::ArticleOperationTarget,
    arg4: bool,
) -> u64 {
    if super::dropped(boma, &["ArticleModule::shoot", "shoot_article"]) {
        return Default::default();
    }
    original!()(boma, article, operation_target, arg4)
}

#[skyline::hook(replace = smash::app::lua_bind::ArticleModule::remove_exist)]
unsafe fn drop_articlemodule_remove_exist(
    boma: *mut Boma,
    arg2: i32,
    arg3: smash::app::ArticleOperationTarget,
) -> u64 {
    if super::dropped(boma, &["ArticleModule::remove_exist", "remove_article"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::ArticleModule::set_visibility)]
unsafe fn drop_articlemodule_set_visibility(
    boma: *mut Boma,
    article: i32,
    arg3: smash::phx::Hash40,
    arg4: smash::phx::Hash40,
    operation_target: smash::app::ArticleOperationTarget,
) {
    if super::dropped(boma, &["ArticleModule::set_visibility"]) {
        return Default::default();
    }
    original!()(boma, article, arg3, arg4, operation_target)
}

#[skyline::hook(replace = smash::app::lua_bind::ArticleModule::set_visibility_whole)]
unsafe fn drop_articlemodule_set_visibility_whole(
    boma: *mut Boma,
    article: i32,
    visibility: bool,
    operation_target: smash::app::ArticleOperationTarget,
) {
    if super::dropped(
        boma,
        &["ArticleModule::set_visibility_whole", "article_visibility"],
    ) {
        return Default::default();
    }
    original!()(boma, article, visibility, operation_target)
}

#[skyline::hook(replace = smash::app::lua_bind::ArticleModule::add_motion_partial)]
unsafe fn drop_articlemodule_add_motion_partial(
    boma: *mut Boma,
    arg2: i32,
    arg3: i32,
    arg4: smash::phx::Hash40,
    arg5: f32,
    arg6: f32,
    arg7: bool,
    arg8: bool,
    arg9: f32,
    arg10: bool,
    arg11: bool,
    arg12: bool,
) -> u64 {
    if super::dropped(boma, &["ArticleModule::add_motion_partial"]) {
        return Default::default();
    }
    original!()(
        boma, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12,
    )
}

#[skyline::hook(replace = smash::app::lua_bind::GrabModule::set_rebound)]
unsafe fn drop_grabmodule_set_rebound(boma: *mut Boma, arg2: bool) {
    if super::dropped(boma, &["GrabModule::set_rebound"]) {
        return Default::default();
    }
    original!()(boma, arg2)
}

#[skyline::hook(replace = smash::app::lua_bind::GrabModule::set_constraint)]
unsafe fn drop_grabmodule_set_constraint(boma: *mut Boma, arg2: i32, arg3: bool) {
    if super::dropped(boma, &["GrabModule::set_constraint"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::WorkModule::enable_transition_term_group)]
unsafe fn drop_workmodule_enable_transition_term_group(boma: *mut Boma, transition_term: i32) {
    if super::dropped(boma, &["WorkModule::enable_transition_term_group"]) {
        return Default::default();
    }
    original!()(boma, transition_term)
}

#[skyline::hook(replace = smash::app::lua_bind::WorkModule::unable_transition_term_group_ex)]
unsafe fn drop_workmodule_unable_transition_term_group_ex(
    boma: *mut Boma,
    transition_term_group: i32,
) {
    if super::dropped(boma, &["WorkModule::unable_transition_term_group_ex"]) {
        return Default::default();
    }
    original!()(boma, transition_term_group)
}

#[skyline::hook(replace = smash::app::lua_bind::ModelModule::disable_gold_eye)]
unsafe fn drop_modelmodule_disable_gold_eye(boma: *mut Boma) -> u64 {
    if super::dropped(boma, &["ModelModule::disable_gold_eye"]) {
        return Default::default();
    }
    original!()(boma)
}

#[skyline::hook(replace = smash::app::lua_bind::GroundModule::select_cliff_hangdata)]
unsafe fn drop_groundmodule_select_cliff_hangdata(boma: *mut Boma, arg2: u32) -> u64 {
    if super::dropped(boma, &["GroundModule::select_cliff_hangdata"]) {
        return Default::default();
    }
    original!()(boma, arg2)
}

#[skyline::hook(replace = smash::app::lua_bind::StatusModule::change_status_request_from_script)]
unsafe fn drop_statusmodule_change_status_request_from_script(
    boma: *mut Boma,
    status: i32,
    arg3: bool,
) -> u64 {
    if super::dropped(boma, &["StatusModule::change_status_request_from_script"]) {
        return Default::default();
    }
    original!()(boma, status, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::EffectModule::request_post_effect_line_circle)]
unsafe fn drop_effectmodule_request_post_effect_line_circle(
    boma: *mut Boma,
    effHash: smash::phx::Hash40,
    boneHash: smash::phx::Hash40,
    radius: smash::phx::simd::Vector2,
    offsets: smash::phx::simd::Vector3,
    arg6: bool,
    duration: f32,
    fade: f32,
) -> u64 {
    if super::dropped(boma, &["EffectModule::request_post_effect_line_circle"]) {
        return Default::default();
    }
    original!()(
        boma, effHash, boneHash, radius, offsets, arg6, duration, fade,
    )
}

#[skyline::hook(replace = smash::app::lua_bind::EffectModule::remove_post_effect_line)]
unsafe fn drop_effectmodule_remove_post_effect_line(boma: *mut Boma, arg2: i32, arg3: bool) -> u64 {
    if super::dropped(boma, &["EffectModule::remove_post_effect_line"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::HitModule::set_status_all)]
unsafe fn drop_hitmodule_set_status_all(boma: *mut Boma, arg2: smash::app::HitStatus, arg3: i32) {
    if super::dropped(boma, &["HitModule::set_status_all", "hit_all_normal"]) {
        return Default::default();
    }
    original!()(boma, arg2, arg3)
}

#[skyline::hook(replace = smash::app::lua_bind::JostleModule::set_status)]
unsafe fn drop_jostlemodule_set_status(boma: *mut Boma, arg2: bool) {
    if super::dropped(boma, &["JostleModule::set_status", "jostle"]) {
        return Default::default();
    }
    original!()(boma, arg2)
}

#[skyline::hook(replace = smash::app::lua_bind::SlowModule::set_whole)]
unsafe fn drop_slowmodule_set_whole(boma: *mut Boma, slow_mul: u8, frames: i32) {
    if super::dropped(boma, &["SlowModule::set_whole", "slow_whole"]) {
        return Default::default();
    }
    original!()(boma, slow_mul, frames)
}

#[skyline::hook(replace = smash::app::lua_bind::SlowModule::clear_whole)]
unsafe fn drop_slowmodule_clear_whole(boma: *mut Boma) -> u64 {
    if super::dropped(boma, &["SlowModule::clear_whole", "slow_clear"]) {
        return Default::default();
    }
    original!()(boma)
}

#[skyline::hook(replace = smash::app::sv_animcmd::FT_ADD_DAMAGE)]
unsafe fn drop_cmd_ft_add_damage(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::FT_ADD_DAMAGE"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::FT_LEAVE_NEAR_OTTOTTO)]
unsafe fn drop_cmd_ft_leave_near_ottotto(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::FT_LEAVE_NEAR_OTTOTTO"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::QUAKE)]
unsafe fn drop_cmd_quake(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::QUAKE"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::SLOW_OPPONENT)]
unsafe fn drop_cmd_slow_opponent(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::SLOW_OPPONENT"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::FILL_SCREEN_MODEL_COLOR)]
unsafe fn drop_cmd_fill_screen_model_color(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::FILL_SCREEN_MODEL_COLOR"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::CANCEL_FILL_SCREEN)]
unsafe fn drop_cmd_cancel_fill_screen(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::CANCEL_FILL_SCREEN"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::RUMBLE_HIT_STATUS)]
unsafe fn drop_cmd_rumble_hit_status(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::RUMBLE_HIT_STATUS"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::SHOOT_ITEM_BULLET)]
unsafe fn drop_cmd_shoot_item_bullet(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::SHOOT_ITEM_BULLET"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::ATK_SET_SHIELD_SETOFF_MUL2)]
unsafe fn drop_cmd_atk_set_shield_setoff_mul2(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::ATK_SET_SHIELD_SETOFF_MUL2"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::ATK_SET_SHIELD_SETOFF_MUL_arg3)]
unsafe fn drop_cmd_atk_set_shield_setoff_mul_arg3(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::ATK_SET_SHIELD_SETOFF_MUL_arg3"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::ATK_SET_SHIELD_SETOFF_MUL_arg4)]
unsafe fn drop_cmd_atk_set_shield_setoff_mul_arg4(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::ATK_SET_SHIELD_SETOFF_MUL_arg4"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::ATK_SET_SHIELD_SETOFF_MUL_arg5)]
unsafe fn drop_cmd_atk_set_shield_setoff_mul_arg5(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::ATK_SET_SHIELD_SETOFF_MUL_arg5"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::CHECK_FINISH_CAMERA)]
unsafe fn drop_cmd_check_finish_camera(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::CHECK_FINISH_CAMERA"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::CHECK_VALID_FINAL_START_CAMERA)]
unsafe fn drop_cmd_check_valid_final_start_camera(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::CHECK_VALID_FINAL_START_CAMERA"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::FT_SET_FINAL_FEAR_FACE)]
unsafe fn drop_cmd_ft_set_final_fear_face(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["macros::FT_SET_FINAL_FEAR_FACE"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::FT_START_CUTIN)]
unsafe fn drop_cmd_ft_start_cutin(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["start_cutin"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::START_INFO_FLASH_EYE)]
unsafe fn drop_cmd_start_info_flash_eye(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["flash_eye"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_animcmd::CAM_ZOOM_OUT)]
unsafe fn drop_cmd_cam_zoom_out(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["camera_zoom_out"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_module_access::attack)]
unsafe fn drop_access_attack(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["attack!"]) {
        return;
    }
    original!()(lua_state)
}

#[skyline::hook(replace = smash::app::sv_module_access::slope)]
unsafe fn drop_access_slope(lua_state: u64) {
    let boma = smash::app::sv_system::battle_object_module_accessor(lua_state) as *mut Boma;
    if super::dropped(boma, &["slope!"]) {
        return;
    }
    original!()(lua_state)
}

pub fn install() {
    skyline::install_hooks!(
        drop_attackmodule_set_size,
        drop_attackmodule_set_add_reaction_frame,
        drop_attackmodule_set_add_reaction_frame_revised,
        drop_attackmodule_set_damage_shake_scale,
        drop_attackmodule_set_no_uniq_effect_all,
        drop_attackmodule_init_attack_pos,
        drop_attackmodule_set_no_damage_fly_smoke_all,
        drop_attackmodule_disable_tip,
        drop_attackmodule_enable_safe_pos,
        drop_attackmodule_set_attack_keep_rumble,
        drop_attackmodule_set_force_reaction,
        drop_attackmodule_set_attack_level,
        drop_attackmodule_set_invalid_xlu,
        drop_attackmodule_set_no_finish_camera_ex,
        drop_attackmodule_set_final_finish_cut_in,
        drop_attackmodule_set_attack_camera_quake_forced,
        drop_attackmodule_set_ink_value,
        drop_attackmodule_set_target_category,
        drop_attackmodule_set_attack_height_all,
        drop_attackmodule_set_no_dead_all,
        drop_attackmodule_set_catch_only_all,
        drop_attackmodule_set_no_finish_camera,
        drop_attackmodule_set_attack_reference_joint_id,
        drop_attackmodule_set_vec_target_pos,
        drop_attackmodule_set_optional_hit_effect,
        drop_attackmodule_set_optional_hit_sound,
        drop_itemmodule_set_have_item_visibility,
        drop_itemmodule_set_attach_item_visibility,
        drop_fighterareamoduleimpl_enable_fix_jostle_area,
        drop_fighterareamoduleimpl_enable_fix_jostle_area_xy,
        drop_visibilitymodule_set_int64,
        drop_visibilitymodule_set_default_int64,
        drop_visibilitymodule_set_status_default_int64,
        drop_visibilitymodule_set_whole,
        drop_articlemodule_change_status,
        drop_articlemodule_change_status_exist,
        drop_articlemodule_change_motion,
        drop_articlemodule_shoot_exist,
        drop_articlemodule_shoot,
        drop_articlemodule_remove_exist,
        drop_articlemodule_set_visibility,
        drop_articlemodule_set_visibility_whole,
        drop_articlemodule_add_motion_partial,
        drop_grabmodule_set_rebound,
        drop_grabmodule_set_constraint,
        drop_workmodule_enable_transition_term_group,
        drop_workmodule_unable_transition_term_group_ex,
        drop_modelmodule_disable_gold_eye,
        drop_groundmodule_select_cliff_hangdata,
        drop_statusmodule_change_status_request_from_script,
        drop_effectmodule_request_post_effect_line_circle,
        drop_effectmodule_remove_post_effect_line,
        drop_hitmodule_set_status_all,
        drop_jostlemodule_set_status,
        drop_slowmodule_set_whole,
        drop_slowmodule_clear_whole,
        drop_cmd_ft_add_damage,
        drop_cmd_ft_leave_near_ottotto,
        drop_cmd_quake,
        drop_cmd_slow_opponent,
        drop_cmd_fill_screen_model_color,
        drop_cmd_cancel_fill_screen,
        drop_cmd_rumble_hit_status,
        drop_cmd_shoot_item_bullet,
        drop_cmd_atk_set_shield_setoff_mul2,
        drop_cmd_atk_set_shield_setoff_mul_arg3,
        drop_cmd_atk_set_shield_setoff_mul_arg4,
        drop_cmd_atk_set_shield_setoff_mul_arg5,
        drop_cmd_check_finish_camera,
        drop_cmd_check_valid_final_start_camera,
        drop_cmd_ft_set_final_fear_face,
        drop_cmd_ft_start_cutin,
        drop_cmd_start_info_flash_eye,
        drop_cmd_cam_zoom_out,
        drop_access_attack,
        drop_access_slope,
    );
}
