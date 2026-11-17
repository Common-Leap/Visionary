//! Reflector / absorber volumes created by owned statuses.
//!
//! There is no generic absorber shape: AbsorberModule only toggles shapes a
//! fighter's own param data defines (Ness, Lucas, Game & Watch, Mii Gunner).
//! An absorber is therefore the same Home-Run Bat reflector volume
//! (`MA_MSC_CMD_REFLECTOR`, valid for every fighter) with its contacts
//! re-routed: the receiver gets OnAbsorb instead of OnReflect, and the
//! projectile removes itself through the vanilla weapon remove event
//! (`notify_event_msc_cmd 0x199c462b5d`, 62 vanilla weapon scripts) on its
//! own next Main line, where its lua agent is current.
//!
//! While any volume is on, and for a few seconds after, every live weapon's
//! facing, speed, team and owner are sampled once per frame and logged only
//! when they change, so a reflection or removal is visible in diag.txt.
use parking_lot::Mutex;
use smash::app::{lua_bind::*, sv_battle_object};
use std::sync::atomic::{AtomicU32, Ordering};

/// (fighter boid, absorbs) for every volume an owned status turned on.
static VOLUMES: Mutex<Vec<(u32, bool)>> = Mutex::new(Vec::new());
/// What a weapon does on its own next Main line.
#[derive(Clone, Copy, PartialEq)]
enum Pending {
    /// Remove through the vanilla weapon remove event; the text names the cause in diag.txt.
    Remove(&'static str),
    /// `set_speed` on the weapon's normal kinetic energy.
    Speed(f32, f32),
}
/// (weapon boid, module accessor, frames left, op) queued by absorbers and controllers.
static ABSORBED: Mutex<Vec<(u32, u64, u8, Pending)>> = Mutex::new(Vec::new());
/// Last speed logged per weapon, so steering logs changes rather than every frame.
static SPEED_LOG: Mutex<Vec<(u32, f32, f32)>> = Mutex::new(Vec::new());
static TRACE_FRAMES: AtomicU32 = AtomicU32::new(0);
const TRACE_TAIL: u32 = 240;

#[derive(Clone, Copy, PartialEq)]
struct Sample {
    lr: f32,
    speed_x: f32,
    team: u64,
    hit_team: u64,
    owner: u64,
}
static SAMPLES: Mutex<Vec<(u32, u64, Sample)>> = Mutex::new(Vec::new());

/// `Some(absorb)` turns the fighter's volume on; `None` records it off.
pub fn set(boid: u32, volume: Option<bool>) {
    let mut volumes = VOLUMES.lock();
    volumes.retain(|(b, _)| *b != boid);
    if let Some(absorb) = volume {
        volumes.push((boid, absorb));
    }
    TRACE_FRAMES.store(TRACE_TAIL, Ordering::Relaxed);
    crate::slight::diag::note(format!(
        "deflect volume object={boid} state={}",
        match volume {
            Some(true) => "absorber on",
            Some(false) => "reflector on",
            None => "off",
        }
    ));
}

pub fn is_absorber(boid: u32) -> bool {
    VOLUMES
        .try_lock()
        .is_some_and(|v| v.iter().any(|(b, absorb)| *b == boid && *absorb))
}

/// Queue the contacted weapon for removal on its own Main line.
pub fn absorb(weapon: u32, accessor: u64) {
    queue(weapon, accessor, Pending::Remove("deflect absorbed"));
}

/// Controller "Remove object" / "Set object speed" on the newest live owned weapon.
pub fn object_op(
    owner: u32,
    fighter: &str,
    symbol: &str,
    speed: Option<(f32, f32)>,
) -> Result<(), String> {
    let Some(weapon) = super::article_context::newest_owned_article(owner, fighter, symbol) else {
        return Ok(());
    };
    let accessor = unsafe {
        if sv_battle_object::is_null(weapon)
            || !sv_battle_object::is_active(weapon)
            || sv_battle_object::category(weapon) != 1
        {
            return Ok(());
        }
        sv_battle_object::module_accessor(weapon) as u64
    };
    if accessor == 0 {
        return Ok(());
    }
    queue(
        weapon,
        accessor,
        match speed {
            Some((x, y))
                if x.is_finite() && y.is_finite() && x.abs() <= 10.0 && y.abs() <= 10.0 =>
            {
                Pending::Speed(x, y)
            }
            Some(_) => return Err("object speed must be finite and within ±10".into()),
            None => Pending::Remove("object remove"),
        },
    );
    Ok(())
}

/// Removes every live object of this kind the fighter owns, including ones it already let
/// go of and the pieces it turns into (Sephiroth's Flare 1 becomes Flare 2), which the
/// game's own article remove no longer reaches.
pub fn remove_all(owner: u32, fighter: &str, symbol: &str) {
    let Some(group) = symbol
        .rsplit_once("_GENERATE_ARTICLE_")
        .map(|(_, g)| g.to_ascii_lowercase())
    else {
        return;
    };
    let family = group.trim_end_matches(|c: char| c.is_ascii_digit());
    let kinds: Vec<i32> = super::registry::weapon_kinds(fighter)
        .filter(|entry| entry.group.starts_with(family))
        .map(|entry| entry.value as i32)
        .collect();
    for rec in crate::slight::agents::all_records() {
        if rec.category != 1
            || !kinds.contains(&rec.kind)
            || crate::slight::systems::article_notifier::weapon_owner_boid(rec.boid) != owner
        {
            continue;
        }
        let accessor = unsafe {
            if sv_battle_object::is_null(rec.boid) || !sv_battle_object::is_active(rec.boid) {
                continue;
            }
            sv_battle_object::module_accessor(rec.boid) as u64
        };
        if accessor != 0 {
            queue(rec.boid, accessor, Pending::Remove("object remove"));
        }
    }
}

fn queue(weapon: u32, accessor: u64, op: Pending) {
    if let Some(mut queue) = ABSORBED.try_lock() {
        // A removal wins; a newer speed replaces an older one for the same weapon.
        if queue
            .iter()
            .any(|(b, a, _, p)| *b == weapon && *a == accessor && matches!(p, Pending::Remove(_)))
        {
            return;
        }
        queue.retain(|(b, a, _, _)| !(*b == weapon && *a == accessor));
        queue.push((weapon, accessor, 3, op));
    }
}

/// Weapon `StatusLine::Main`: run the op an absorber or controller queued for it.
pub unsafe fn on_weapon_main(agent: &mut smash::lua2cpp::L2CFighterBase) {
    let module = agent.module_accessor;
    if module.is_null() {
        return;
    }
    let boid = (*module).battle_object_id;
    let op = ABSORBED.try_lock().and_then(|mut queue| {
        let index = queue
            .iter()
            .position(|(b, a, _, _)| *b == boid && *a == module as u64)?;
        Some(queue.remove(index).3)
    });
    let Some(op) = op else { return };
    if sv_battle_object::category(boid) != 1 {
        return;
    }
    let kind = smash::app::utility::get_kind(&mut *module);
    let weapon = &mut agent.agent;
    match op {
        Pending::Remove(cause) => {
            crate::slight::diag::note(format!("{cause} weapon={boid} kind={kind} removed"));
            use smash_script::{lua_args, notify_event_msc_cmd};
            notify_event_msc_cmd!(weapon, smash::phx::Hash40::new_raw(0x199c462b5d));
        }
        Pending::Speed(x, y) => {
            use smash_script::{lua_args, sv_kinetic_energy};
            sv_kinetic_energy!(
                set_speed,
                weapon,
                *smash::lib::lua_const::WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL,
                x,
                y
            );
            if let Some(mut log) = SPEED_LOG.try_lock() {
                let changed = log
                    .iter()
                    .find(|(b, _, _)| *b == boid)
                    .is_none_or(|(_, lx, ly)| (lx - x).abs() > 0.25 || (ly - y).abs() > 0.25);
                if changed {
                    log.retain(|(b, _, _)| *b != boid);
                    log.push((boid, x, y));
                    log.truncate(16);
                    let main = *smash::lib::lua_const::KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN;
                    crate::slight::diag::note(format!(
                        "object speed weapon={boid} kind={kind} set=({x:.2},{y:.2}) sum=({:.2},{:.2})",
                        KineticModule::get_sum_speed_x(module, main), KineticModule::get_sum_speed_y(module, main)
                    ));
                }
            }
        }
    }
}

/// Once per game frame.
pub fn tick() {
    if let Some(mut queue) = ABSORBED.try_lock() {
        queue.retain_mut(|(_, _, left, _)| {
            *left = left.saturating_sub(1);
            *left > 0
        });
    }
    let active = VOLUMES.try_lock().is_some_and(|v| !v.is_empty());
    if active {
        TRACE_FRAMES.store(TRACE_TAIL, Ordering::Relaxed);
    } else if TRACE_FRAMES.load(Ordering::Relaxed) == 0 {
        return;
    } else {
        TRACE_FRAMES.fetch_sub(1, Ordering::Relaxed);
    }
    let Some(mut samples) = SAMPLES.try_lock() else {
        return;
    };
    let mut seen = Vec::new();
    for record in crate::slight::agents::all_records()
        .into_iter()
        .filter(|r| r.category == 1)
    {
        unsafe {
            if sv_battle_object::is_null(record.boid)
                || !sv_battle_object::is_active(record.boid)
                || sv_battle_object::category(record.boid) != 1
            {
                continue;
            }
            let module = sv_battle_object::module_accessor(record.boid);
            if module.is_null() || module as u64 != record.module_accessor_addr {
                continue;
            }
            let sample = Sample {
                lr: PostureModule::lr(module),
                speed_x: KineticModule::get_sum_speed_x(
                    module,
                    *smash::lib::lua_const::KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN,
                ),
                team: TeamModule::team_no(module),
                hit_team: TeamModule::hit_team_no(module),
                owner: TeamModule::team_owner_id(module) as u64,
            };
            seen.push((record.boid, record.module_accessor_addr));
            let previous = samples
                .iter()
                .position(|(b, a, _)| *b == record.boid && *a == record.module_accessor_addr);
            let changed = previous.is_none_or(|i| {
                let old = samples[i].2;
                old.lr != sample.lr
                    || old.team != sample.team
                    || old.hit_team != sample.hit_team
                    || old.owner != sample.owner
                    || (old.speed_x < 0.0) != (sample.speed_x < 0.0)
            });
            if changed {
                crate::slight::diag::note(format!(
                    "deflect trace weapon={} kind={} lr={} speed_x={:.2} team={} hit_team={} team_owner={:#x} {}",
                    record.boid, record.kind, sample.lr, sample.speed_x, sample.team, sample.hit_team, sample.owner,
                    if previous.is_some() { "changed" } else { "first" }
                ));
            }
            match previous {
                Some(i) => samples[i].2 = sample,
                None => samples.push((record.boid, record.module_accessor_addr, sample)),
            }
        }
    }
    samples.retain(|(b, a, _)| {
        let live = seen.contains(&(*b, *a));
        if !live {
            crate::slight::diag::note(format!("deflect trace weapon={b} gone"));
        }
        live
    });
}
