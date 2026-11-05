//! Constrained live article/status operations (F1.5).
//!
//! TCP only supplies a symbolic registry name plus the ids of already-live
//! objects. Every engine call re-resolves the name and re-proves the object:
//! fighter category/name for article operations, and fighter ownership plus
//! generated concrete weapon kind for weapon-status requests. There is no
//! raw WorkModule write or numeric identifier input on this boundary.

use serde::Deserialize;
use smash::app::lua_bind::{Article, ArticleModule, PostureModule, StatusModule};
use smash::app::{self, sv_battle_object};
use smash::lib::lua_const::{ARTICLE_OPE_TARGET_ALL, ARTICLE_OPE_TARGET_LAST};

use super::registry::{self, IdentifierKind, ValidatedIdentifier};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireEnvelope {
    gimmick_operation: WireOperation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum WireOperation {
    GenerateArticle {
        version: u8,
        fighter: String,
        fighter_boid: u32,
        symbol: String,
    },
    RemoveArticle {
        version: u8,
        fighter: String,
        fighter_boid: u32,
        symbol: String,
    },
    RequestWeaponStatus {
        version: u8,
        fighter: String,
        fighter_boid: u32,
        weapon_boid: u32,
        symbol: String,
    },
}

/// Consume exactly one operation on the game thread. Errors are retained as a
/// diagnostic because the existing TCP protocol has no response frame; they
/// are intentionally precise enough for the desktop diagnostic stream.
pub fn handle(raw: &str) {
    let result = decode(raw).and_then(execute);
    match result {
        Ok(message) => crate::slight::diag::note(format!("gimmick operation accepted: {message}")),
        Err(error) => crate::slight::diag::note(format!("gimmick operation rejected: {error}")),
    }
}

/// Registration-time check for a generated behavior action. This deliberately
/// does not touch a live object or call the engine; execution resolves again
/// so a static artifact cannot bypass the operation boundary later.
pub fn validate_behavior_article(fighter: &str, symbol: &str) -> Result<(), String> {
    if fighter.is_empty() || symbol.is_empty() {
        return Err("behavior article action needs a non-empty fighter and symbol".to_string());
    }
    registry::resolve(fighter, symbol, IdentifierKind::Article)
        .map(|_| ())
        .map_err(|error| format!("behavior article action is not allowlisted: {error}"))
}

/// Registration-time check for a generated behavior weapon-status action.
/// This checks only persisted artifact data; the exact live object is checked
/// again by [`request_weapon_status_from_behavior`] through [`execute`].
pub fn validate_behavior_weapon_status(
    fighter: &str,
    weapon_boid: u32,
    symbol: &str,
) -> Result<(), String> {
    resolve_weapon_status_target(fighter, weapon_boid, symbol).map(|_| ())
}

/// Generate one allowlisted article for the fighter whose behavior transition
/// is currently executing. The fighter id is supplied by the dispatcher, not
/// persisted in the project, and [`execute`] reuses the live identity checks
/// used by TCP-originated operations.
pub fn generate_article_from_behavior(
    fighter: &str,
    fighter_boid: u32,
    symbol: &str,
) -> Result<(), String> {
    execute_behavior_article(fighter, fighter_boid, symbol, true)
}

/// Remove one allowlisted article for the fighter whose behavior transition
/// is currently executing. See [`generate_article_from_behavior`] for the
/// object-identity boundary.
pub fn remove_article_from_behavior(
    fighter: &str,
    fighter_boid: u32,
    symbol: &str,
) -> Result<(), String> {
    execute_behavior_article(fighter, fighter_boid, symbol, false)
}

pub fn validate_behavior_article_status(
    fighter: &str,
    symbol: &str,
    status: &str,
) -> Result<(), String> {
    validate_behavior_article(fighter, symbol)?;
    resolve_weapon_status_target(fighter, 1, status).map(|_| ())
}

pub fn remove_newest_article_from_behavior(
    fighter: &str,
    boid: u32,
    symbol: &str,
) -> Result<(), String> {
    newest_article_action(fighter, boid, symbol, None, false)
}

/// Apply speed/position to every live projectile of one owned article kind.
pub fn move_articles_from_behavior(
    fighter: &str,
    boid: u32,
    symbol: &str,
    op: super::native_inputs::MovementAction,
) -> Result<(), String> {
    use super::native_inputs::MovementAction;
    op.validate_for_article()?;
    registry::resolve(fighter, symbol, IdentifierKind::Article)?;
    live_fighter(&FighterTarget {
        name: fighter.into(),
        boid,
        symbol: symbol.into(),
    })?;
    for weapon in crate::slight::systems::article_notifier::owned_weapons(boid, fighter, symbol) {
        let Some(record) = crate::slight::agents::lookup(weapon) else {
            continue;
        };
        let module = record.module_accessor_addr as *mut app::BattleObjectModuleAccessor;
        if module.is_null() || record.category != 1 {
            continue;
        }
        if let MovementAction::MoveBy { dx, dy } = op {
            unsafe {
                PostureModule::add_pos(
                    module,
                    &smash::phx::Vector3f {
                        x: dx,
                        y: dy,
                        z: 0.0,
                    },
                )
            };
        }
    }
    Ok(())
}

pub fn request_article_status_from_behavior(
    fighter: &str,
    boid: u32,
    symbol: &str,
    status: &str,
) -> Result<(), String> {
    validate_behavior_article_status(fighter, symbol, status)?;
    newest_article_action(fighter, boid, symbol, Some(status), false)
}

fn newest_article_action(
    fighter: &str,
    boid: u32,
    symbol: &str,
    status: Option<&str>,
    preflight: bool,
) -> Result<(), String> {
    let article = registry::resolve(fighter, symbol, IdentifierKind::Article)?;
    let target = FighterTarget {
        name: fighter.into(),
        boid,
        symbol: symbol.into(),
    };
    let accessor = live_fighter(&target)?;
    unsafe {
        let count = ArticleModule::get_active_num(accessor, article.value as i32);
        if count == 0 {
            return Ok(());
        }
        if count < 0 {
            return Err("invalid owned article count".into());
        }
        let status = status
            .map(|symbol| resolve_weapon_status_target(fighter, 1, symbol))
            .transpose()?;
        // The engine chooses LAST. Prove every candidate belongs to this exact
        // fighter and has the requested native type before invoking that selector.
        for index in 0..count {
            let instance =
                ArticleModule::get_article_from_no(accessor, article.value as i32, index);
            if instance.is_null() {
                return Err("owned article is no longer live".into());
            }
            let weapon = u32::try_from(Article::get_battle_object_id(instance))
                .map_err(|_| "invalid article object id")?;
            if let Some(status) = &status {
                live_owned_weapon(&target, weapon, status)?;
            } else if sv_battle_object::is_null(weapon)
                || !sv_battle_object::is_active(weapon)
                || sv_battle_object::category(weapon) != 1
                || crate::slight::systems::article_notifier::weapon_owner_boid(weapon) != boid
            {
                return Err("article ownership or liveness changed before removal".into());
            }
        }
        if live_fighter(&target)? != accessor
            || ArticleModule::get_active_num(accessor, article.value as i32) != count
        {
            return Err("article identity changed before action".into());
        }
        if preflight {
            return Ok(());
        }
        if let Some(status) = status {
            ArticleModule::change_status(
                accessor,
                article.value as i32,
                status.value as i32,
                app::ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST),
            );
        } else {
            ArticleModule::remove_exist(
                accessor,
                article.value as i32,
                app::ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST),
            );
        }
    }
    Ok(())
}

/// Validate current target/resource requirements before accepting a
/// controller batch. This path must not mutate native state.
pub(crate) fn preflight_behavior_operation(
    fighter: &str,
    boid: u32,
    action: &super::live_model::Action,
) -> Result<(), String> {
    use super::live_model::Action;
    let (symbol, kind) = match action {
        Action::GenerateArticle { symbol } | Action::RemoveArticle { symbol } => {
            (symbol, IdentifierKind::Article)
        }
        Action::RequestFighterStatus { symbol } => (symbol, IdentifierKind::FighterStatus),
        Action::RequestWeaponStatus { target, symbol } => {
            let status = resolve_weapon_status_target(fighter, target.battle_object_id, symbol)?;
            let target_fighter = FighterTarget {
                name: fighter.into(),
                boid,
                symbol: symbol.clone(),
            };
            live_fighter(&target_fighter)?;
            live_owned_weapon(&target_fighter, target.battle_object_id, &status)?;
            return Ok(());
        }
        Action::RemoveNewestArticle { symbol } => {
            return newest_article_action(fighter, boid, symbol, None, true)
        }
        Action::RequestArticleStatus { symbol, status } => {
            validate_behavior_article_status(fighter, symbol, status)?;
            return newest_article_action(fighter, boid, symbol, Some(status), true);
        }
        _ => return Ok(()),
    };
    let identifier = registry::resolve(fighter, symbol, kind)?;
    let target = FighterTarget {
        name: fighter.into(),
        boid,
        symbol: symbol.clone(),
    };
    let accessor = live_fighter(&target)?;
    if matches!(action, Action::GenerateArticle { .. }) {
        unsafe {
            if !ArticleModule::is_exist(accessor, identifier.value as i32)
                && !ArticleModule::is_generatable(accessor, identifier.value as i32)
            {
                return Err("article is not generatable".into());
            }
        }
    }
    Ok(())
}

pub(crate) fn request_resolved_status(fighter: &str, boid: u32, status: u32) -> Result<(), String> {
    let identity = super::actor_identity::Identity::from_live_fighter(boid, fighter)
        .ok_or("fighter identity changed before queuing status")?;
    PENDING_FIGHTER_STATUS.lock().enqueue(
        identity.entry_id,
        PendingFighterStatus {
            boid,
            fighter: fighter.into(),
            identity,
            status: status as i32,
        },
    )
}

pub fn validate_behavior_fighter_status(fighter: &str, symbol: &str) -> Result<(), String> {
    registry::resolve(fighter, symbol, IdentifierKind::FighterStatus).map(|_| ())
}

pub fn request_fighter_status_from_behavior(
    fighter: &str,
    fighter_boid: u32,
    symbol: &str,
) -> Result<(), String> {
    let status = registry::resolve(fighter, symbol, IdentifierKind::FighterStatus)?;
    execute(Operation::RequestFighterStatus {
        fighter: FighterTarget {
            name: fighter.into(),
            boid: fighter_boid,
            symbol: symbol.into(),
        },
        status,
    })
    .map(|_| ())
}

/// Request an allowlisted status for the exact weapon object persisted in the
/// currently executing behavior transition. Unlike article actions, there is
/// no generated-object search or fallback: a zero target is rejected and the
/// selected object must still be a live weapon owned by this fighter with the
/// generated concrete kind paired to `symbol`.
pub fn request_weapon_status_from_behavior(
    fighter: &str,
    fighter_boid: u32,
    weapon_boid: u32,
    symbol: &str,
) -> Result<(), String> {
    let status = resolve_weapon_status_target(fighter, weapon_boid, symbol)?;
    execute(Operation::RequestWeaponStatus {
        fighter: FighterTarget {
            name: fighter.to_string(),
            boid: fighter_boid,
            symbol: symbol.to_string(),
        },
        weapon_boid,
        status,
    })
    .map(|_| ())
}

fn execute_behavior_article(
    fighter: &str,
    fighter_boid: u32,
    symbol: &str,
    generate: bool,
) -> Result<(), String> {
    validate_behavior_article(fighter, symbol)?;
    let article = registry::resolve(fighter, symbol, IdentifierKind::Article)?;
    let fighter = FighterTarget {
        name: fighter.to_string(),
        boid: fighter_boid,
        symbol: symbol.to_string(),
    };
    let operation = if generate {
        Operation::GenerateArticle { fighter, article }
    } else {
        Operation::RemoveArticle { fighter, article }
    };
    execute(operation).map(|_| ())
}

fn decode(raw: &str) -> Result<Operation, String> {
    let envelope: WireEnvelope = serde_json::from_str(raw)
        .map_err(|error| format!("malformed gimmick operation: {error}"))?;
    match envelope.gimmick_operation {
        WireOperation::GenerateArticle {
            version,
            fighter,
            fighter_boid,
            symbol,
        } => {
            let header = check_header(version, fighter, fighter_boid, symbol)?;
            let article = registry::resolve(&header.name, &header.symbol, IdentifierKind::Article)?;
            Ok(Operation::GenerateArticle {
                fighter: header,
                article,
            })
        }
        WireOperation::RemoveArticle {
            version,
            fighter,
            fighter_boid,
            symbol,
        } => {
            let header = check_header(version, fighter, fighter_boid, symbol)?;
            let article = registry::resolve(&header.name, &header.symbol, IdentifierKind::Article)?;
            Ok(Operation::RemoveArticle {
                fighter: header,
                article,
            })
        }
        WireOperation::RequestWeaponStatus {
            version,
            fighter,
            fighter_boid,
            weapon_boid,
            symbol,
        } => {
            let header = check_header(version, fighter, fighter_boid, symbol)?;
            let status = resolve_weapon_status_target(&header.name, weapon_boid, &header.symbol)?;
            Ok(Operation::RequestWeaponStatus {
                fighter: header,
                weapon_boid,
                status,
            })
        }
    }
}

fn resolve_weapon_status_target(
    fighter: &str,
    weapon_boid: u32,
    symbol: &str,
) -> Result<ValidatedIdentifier, String> {
    if fighter.is_empty() || symbol.is_empty() {
        return Err(
            "behavior weapon-status action needs a non-empty fighter and symbol".to_string(),
        );
    }
    if weapon_boid == 0 {
        return Err(
            "weapon-status action needs a nonzero explicit weapon battle-object id".to_string(),
        );
    }
    let status = registry::resolve(fighter, symbol, IdentifierKind::WeaponStatus)?;
    if registry::weapon_kind(status.fighter, status.group).is_none() {
        return Err(format!(
            "weapon status {} is unavailable: no generated weapon-kind guard for {}/{}",
            status.symbol, status.fighter, status.group
        ));
    }
    Ok(status)
}

fn check_header(
    version: u8,
    name: String,
    boid: u32,
    symbol: String,
) -> Result<FighterTarget, String> {
    if version != 1 {
        return Err(format!("unsupported gimmick operation version {version}"));
    }
    if name.is_empty() || symbol.is_empty() {
        return Err("fighter and symbolic identifier must be non-empty".to_string());
    }
    Ok(FighterTarget { name, boid, symbol })
}

#[derive(Debug)]
struct FighterTarget {
    name: String,
    boid: u32,
    symbol: String,
}

#[derive(Debug)]
enum Operation {
    RequestFighterStatus {
        fighter: FighterTarget,
        status: ValidatedIdentifier,
    },
    GenerateArticle {
        fighter: FighterTarget,
        article: ValidatedIdentifier,
    },
    RemoveArticle {
        fighter: FighterTarget,
        article: ValidatedIdentifier,
    },
    RequestWeaponStatus {
        fighter: FighterTarget,
        weapon_boid: u32,
        status: ValidatedIdentifier,
    },
}

type PendingFighterStatus = super::fighter_status_queue::Request<super::actor_identity::Identity>;
static PENDING_FIGHTER_STATUS: std::sync::LazyLock<
    parking_lot::Mutex<super::fighter_status_queue::Pending<super::actor_identity::Identity>>,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(Default::default()));

/// Called at the tail of this fighter's smashline Main callback. Remove the
/// request before entering the engine: status callbacks may re-enter here.
pub(crate) fn flush_fighter_status(boid: u32) {
    let request = PENDING_FIGHTER_STATUS.lock().take(boid);
    let Some(request) = request else {
        return;
    };
    if super::actor_identity::Identity::from_live_fighter(boid, &request.fighter)
        != Some(request.identity)
    {
        return;
    }
    let target = FighterTarget {
        name: request.fighter,
        boid,
        symbol: String::new(),
    };
    let Ok(accessor) = live_fighter(&target) else {
        return;
    };
    // Pinned skyline-smash 258a566 cpp.rs, StatusModule::prev_status_kind:
    // its fighter transition example uses change_status_request(..., false).
    // Main callbacks run after vanilla Main in smashline df194a0 callbacks.rs.
    // Use that native-status profile here; the script bool is documented as
    // unknown and change_status_force has no documented fighter contract.
    unsafe {
        StatusModule::change_status_request(accessor, request.status, false);
    }
}

pub(crate) fn clear_fighter_status(boid: u32) {
    PENDING_FIGHTER_STATUS.lock().take(boid);
}

pub(crate) fn clear_fighter_statuses() {
    *PENDING_FIGHTER_STATUS.lock() = Default::default();
}

fn execute(operation: Operation) -> Result<String, String> {
    match operation {
        Operation::RequestFighterStatus { fighter, status } => {
            live_fighter(&fighter)?;
            let identity =
                super::actor_identity::Identity::from_live_fighter(fighter.boid, &fighter.name)
                    .ok_or("fighter identity changed before queuing status")?;
            // The global pump can run on another fighter's Main line. Deliver
            // on this fighter's own Main boundary, after its vanilla status
            // logic, rather than writing the ACMD script-request queue here.
            PENDING_FIGHTER_STATUS.lock().enqueue(
                identity.entry_id,
                PendingFighterStatus {
                    boid: fighter.boid,
                    fighter: fighter.name.clone(),
                    identity,
                    status: status.value as i32,
                },
            )?;
            Ok(format!(
                "fighter status requested: {}/{} on 0x{:X}",
                status.fighter, status.symbol, fighter.boid
            ))
        }
        Operation::GenerateArticle { fighter, article } => {
            let accessor = live_fighter(&fighter)?;
            unsafe {
                if ArticleModule::is_exist(accessor, article.value as i32) {
                    return Ok(format!(
                        "generate skipped: {}/{} already exists on 0x{:X}",
                        article.fighter, article.symbol, fighter.boid
                    ));
                }
                if !ArticleModule::is_generatable(accessor, article.value as i32) {
                    return Err(format!(
                        "generate unavailable: {}/{} is not generatable on 0x{:X}",
                        article.fighter, article.symbol, fighter.boid
                    ));
                }
                // The signature is pinned by the checked-in skyline-smash
                // revision. F1.5 fixes its two non-identifier arguments to
                // this one non-configurable profile; the TCP request cannot
                // select any alternate engine behavior.
                ArticleModule::generate_article(accessor, article.value as i32, false, -1);
            }
            Ok(format!(
                "generate requested: {}/{} on 0x{:X}",
                article.fighter, article.symbol, fighter.boid
            ))
        }
        Operation::RemoveArticle { fighter, article } => {
            let accessor = live_fighter(&fighter)?;
            unsafe {
                if !ArticleModule::is_exist(accessor, article.value as i32) {
                    return Ok(format!(
                        "remove skipped: {}/{} is absent on 0x{:X}",
                        article.fighter, article.symbol, fighter.boid
                    ));
                }
                ArticleModule::remove_exist(
                    accessor,
                    article.value as i32,
                    app::ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL),
                );
            }
            Ok(format!(
                "remove requested: {}/{} on 0x{:X}",
                article.fighter, article.symbol, fighter.boid
            ))
        }
        Operation::RequestWeaponStatus {
            fighter,
            weapon_boid,
            status,
        } => {
            let _fighter_accessor = live_fighter(&fighter)?;
            let weapon_accessor = live_owned_weapon(&fighter, weapon_boid, &status)?;
            unsafe {
                StatusModule::change_status_request(weapon_accessor, status.value as i32, false);
            }
            Ok(format!(
                "weapon status requested: {}/{} on weapon 0x{:X} owned by 0x{:X}",
                status.fighter, status.symbol, weapon_boid, fighter.boid
            ))
        }
    }
}

fn live_fighter(target: &FighterTarget) -> Result<*mut app::BattleObjectModuleAccessor, String> {
    // The sampler alone cannot prove identity: an active BOID and even an
    // accessor address can be reused before its cached record is refreshed.
    // Recheck kind, player entry, accessor and the observed stock identity.
    if !super::live_behavior::actor_matches(target.boid, &target.name) {
        return Err(format!(
            "fighter 0x{:X} is not the exact current {} identity",
            target.boid, target.name
        ));
    }
    let record = crate::slight::agents::lookup(target.boid).ok_or_else(|| {
        format!(
            "fighter 0x{:X} is not a live registered object",
            target.boid
        )
    })?;
    unsafe {
        if sv_battle_object::is_null(target.boid) || !sv_battle_object::is_active(target.boid) {
            return Err(format!("fighter 0x{:X} is inactive", target.boid));
        }
        if sv_battle_object::category(target.boid) != 0 || record.category != 0 {
            return Err(format!("object 0x{:X} is not a fighter", target.boid));
        }
        let accessor = sv_battle_object::module_accessor(target.boid);
        if accessor.is_null() || accessor as u64 != record.module_accessor_addr {
            return Err(format!(
                "fighter 0x{:X} module accessor changed",
                target.boid
            ));
        }
        let actual =
            crate::slight::gimmicks::registry::fighter_name(record.kind).ok_or_else(|| {
                format!(
                    "fighter 0x{:X} has an unknown kind {}",
                    target.boid, record.kind
                )
            })?;
        if actual != target.name {
            return Err(format!(
                "fighter target mismatch: requested {}, live object 0x{:X} is {actual}",
                target.name, target.boid
            ));
        }
        Ok(accessor)
    }
}

fn live_owned_weapon(
    fighter: &FighterTarget,
    weapon_boid: u32,
    status: &ValidatedIdentifier,
) -> Result<*mut app::BattleObjectModuleAccessor, String> {
    let expected_kind = registry::weapon_kind(status.fighter, status.group).ok_or_else(|| {
        format!(
            "weapon status {} is unavailable: no generated weapon-kind guard for {}/{}",
            status.symbol, status.fighter, status.group
        )
    })?;
    let record = crate::slight::agents::lookup(weapon_boid)
        .ok_or_else(|| format!("weapon 0x{weapon_boid:X} is not a live registered object"))?;
    unsafe {
        if sv_battle_object::is_null(weapon_boid) || !sv_battle_object::is_active(weapon_boid) {
            return Err(format!("weapon 0x{weapon_boid:X} is inactive"));
        }
        if sv_battle_object::category(weapon_boid) != 1 || record.category != 1 {
            return Err(format!("object 0x{weapon_boid:X} is not a weapon"));
        }
        let accessor = sv_battle_object::module_accessor(weapon_boid);
        if accessor.is_null() || accessor as u64 != record.module_accessor_addr {
            return Err(format!("weapon 0x{weapon_boid:X} module accessor changed"));
        }
        let owner = crate::slight::systems::article_notifier::weapon_owner_boid(weapon_boid);
        if owner != fighter.boid {
            return Err(format!(
                "weapon ownership mismatch: weapon 0x{weapon_boid:X} resolves to 0x{owner:X}, not fighter 0x{:X}",
                fighter.boid
            ));
        }
        if sv_battle_object::kind(weapon_boid) != record.kind
            || record.kind != expected_kind.value as i32
        {
            return Err(format!(
                "weapon type mismatch: status {} requires {}/{} kind 0x{:X}, live weapon 0x{weapon_boid:X} is 0x{:X}",
                status.symbol, expected_kind.fighter, expected_kind.group, expected_kind.value, record.kind
            ));
        }
        Ok(accessor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(operation: &str, fields: &str) -> String {
        format!(
            r#"{{"gimmick_operation":{{"version":1,"fighter":"cloud","fighter_boid":1,"operation":"{operation}",{fields}}}}}"#
        )
    }

    #[test]
    fn decode_accepts_only_symbolic_typed_operations() {
        assert!(matches!(
            decode(&request(
                "generate_article",
                r#""symbol":"FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE""#
            )),
            Ok(Operation::GenerateArticle { .. })
        ));
        assert!(matches!(
            decode(&request(
                "remove_article",
                r#""symbol":"FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE""#
            )),
            Ok(Operation::RemoveArticle { .. })
        ));
        assert!(matches!(
            decode(&request(
                "request_weapon_status",
                r#""weapon_boid":2,"symbol":"WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR""#
            )),
            Ok(Operation::RequestWeaponStatus { .. })
        ));
    }

    #[test]
    fn decode_rejects_numeric_unknown_wrong_kind_wrong_fighter_and_extra_fields() {
        assert!(decode("not json").is_err());
        assert!(decode(&request("generate_article", r#""symbol":"0""#)).is_err());
        assert!(decode(&request(
            "generate_article",
            r#""symbol":"WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR""#
        ))
        .is_err());
        assert!(decode(
            r#"{"gimmick_operation":{"version":1,"fighter":"mario","fighter_boid":1,"operation":"generate_article","symbol":"FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE"}}"#
        )
        .is_err());
        assert!(decode(
            r#"{"gimmick_operation":{"version":1,"fighter":"cloud","fighter_boid":1,"operation":"generate_article","symbol":"FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE","value":0}}"#
        )
        .is_err());
        assert!(decode(&request(
            "request_weapon_status",
            r#""weapon_boid":0,"symbol":"WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR""#
        ))
        .is_err());
    }

    #[test]
    fn behavior_article_actions_accept_only_the_exact_article_namespace() {
        assert!(validate_behavior_article("cloud", "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE").is_ok());
        assert!(
            validate_behavior_article("cloud", "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR")
                .unwrap_err()
                .contains("not allowlisted")
        );
        assert!(
            validate_behavior_article("cloud", "FIGHTER_BRAVE_GENERATE_ARTICLE_FIREBALL")
                .unwrap_err()
                .contains("not allowlisted")
        );
        assert!(validate_behavior_article("cloud", "0").is_err());
    }

    #[test]
    fn behavior_weapon_status_actions_require_a_nonzero_exact_guarded_target() {
        assert!(validate_behavior_weapon_status(
            "cloud",
            0x2000_0001,
            "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR"
        )
        .is_ok());
        assert!(validate_behavior_weapon_status(
            "cloud",
            0,
            "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR"
        )
        .unwrap_err()
        .contains("nonzero explicit weapon battle-object id"));
        assert!(validate_behavior_weapon_status(
            "cloud",
            0x2000_0001,
            "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE"
        )
        .is_err());
        assert!(validate_behavior_weapon_status(
            "cloud",
            0x2000_0001,
            "WEAPON_MARIO_FIREBALL_STATUS_KIND_REGULAR"
        )
        .is_err());
    }
}
