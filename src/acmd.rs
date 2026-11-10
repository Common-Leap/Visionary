/// Parse ACMD scripts into a structured IR that preserves loops and can be re-exported.
use crate::data::{
    AcmdScript, AcmdStmt, AttackCall, EffectMacro, EffectScript, EffectStmt, ExcuteStmt,
};

/// Convert snake_case motion name to PascalCase filename.
pub fn move_name_to_pascal(name: &str) -> String {
    name.split('_')
        .map(|part| {
            let mut c = part.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect()
}

/// Motion names whose ACMD scripts do not share the motion's spelling.
///
/// The motion list labels the mid forward tilt/smash `attack_s3_s` / `attack_s4_s`,
/// but the game's scripts (and the dumped-script archive) spell the mid variants
/// without the side indicator: `attacks3` / `attacks4` (`AttackS3.txt`,
/// `game_attacks3`). The angled variants keep theirs (`attack_s3_hi` →
/// `attacks3hi`), so only the `_s` forms need an alias. Ryu and Ken are the
/// exception that proves it: their `attack_s3_s` really is `attacks3s`
/// (`AttackS3S.txt`), so callers must try the motion's own spelling first and
/// fall back to the alias rather than remapping unconditionally.
///
/// The numbered follow-ups behave the same way: `attack_s3_s2` / `attack_s3_s3`
/// are `attacks32` / `attacks33` (`AttackS32.txt`), while Link's `attack_s4_s2`
/// keeps its `s` (`AttackS4S2.txt`) and is listed for completeness.
pub fn move_script_aliases(move_name: &str) -> Vec<String> {
    let mut out = vec![move_name.to_string()];
    let lower = move_name.to_ascii_lowercase();
    let alias: Option<&str> = match lower.as_str() {
        "attack_s3_s" => Some("attack_s3"),
        "attack_s4_s" => Some("attack_s4"),
        "attack_s3_s2" => Some("attack_s3_2"),
        "attack_s3_s3" => Some("attack_s3_3"),
        "attack_s4_s2" => Some("attack_s4_2"),
        "attack_s4_s3" => Some("attack_s4_3"),
        _ => None,
    };
    if let Some(alias) = alias {
        if alias != lower.as_str() {
            out.push(alias.to_string());
        }
    }
    out
}

/// ACMD script names to try for a motion + category prefix, in order.
pub fn acmd_script_candidates(prefix: &str, move_name: &str) -> Vec<String> {
    move_script_aliases(move_name)
        .iter()
        .map(|alias| acmd_script_name(prefix, alias))
        .collect()
}

/// Script-name spellings to try for one ACMD function, in order.
///
/// This is the [`move_script_aliases`] mapping in script-name space (`game_…`,
/// no underscores): `game_attacks3s` ↔ `game_attacks3`, `game_attacks4s` ↔
/// `game_attacks4`, `game_attacks3s2` ↔ `game_attacks32`, and so on. Both
/// directions are listed because both spellings are real scripts on different
/// fighters — Ryu/Ken really do install `game_attacks3s` — so a lookup for
/// either spelling must find the other as a fallback.
pub fn script_name_aliases(script_name: &str) -> Vec<String> {
    let mut out = vec![script_name.to_string()];
    const PREFIXES: &[&str] = &["game_", "effect_", "sound_", "expression_"];
    let lower = script_name.to_ascii_lowercase();
    for prefix in PREFIXES {
        if let Some(suffix) = lower.strip_prefix(prefix) {
            let counterpart: Option<&str> = match suffix {
                "attacks3s" => Some("attacks3"),
                "attacks3" => Some("attacks3s"),
                "attacks4s" => Some("attacks4"),
                "attacks4" => Some("attacks4s"),
                "attacks3s2" => Some("attacks32"),
                "attacks32" => Some("attacks3s2"),
                "attacks3s3" => Some("attacks33"),
                "attacks33" => Some("attacks3s3"),
                "attacks4s2" => Some("attacks42"),
                "attacks42" => Some("attacks4s2"),
                "attacks4s3" => Some("attacks43"),
                "attacks43" => Some("attacks4s3"),
                _ => None,
            };
            if let Some(other) = counterpart {
                // Keep the caller's prefix spelling; only the suffix varies.
                let prefix_len = prefix.len();
                let original_prefix = &script_name[..prefix_len.min(script_name.len())];
                out.push(format!("{original_prefix}{other}"));
            }
            break;
        }
    }
    out
}

/// The move spelling to use when *writing* a script name for a motion.
///
/// Fetching and linked-source reads try every spelling and take what exists,
/// but an export has to pick one name without a network round trip. This is
/// that pick: the cached hit when the move was viewed, else the game's own
/// spelling — the de-`_s` alias for everyone except Ryu/Ken's real `attacks3s`,
/// which is why the fighter matters here and nowhere else.
///
/// `attack_s4_s` always maps to `attack_s4` (no fighter ships `AttackS4S.txt`);
/// `attack_s3_s2`/`s3` map to `attack_s3_2`/`_3` (`AttackS32/33.txt`); Link's
/// `attack_s4_s2` keeps its `s` and needs no resolution.
pub fn resolve_script_move_name(fighter: &str, move_name: &str) -> String {
    let aliases = move_script_aliases(move_name);
    if aliases.len() <= 1 {
        return move_name.to_string();
    }
    // A viewed move already proved which spelling the archive carries.
    for alias in &aliases {
        if let Some(body) = cached_script_body_at(&script_cache_path(fighter, alias)) {
            if !body.is_empty() {
                return alias.clone();
            }
        }
    }
    let lower_fighter = fighter
        .strip_prefix("fighter_")
        .unwrap_or(fighter)
        .to_ascii_lowercase();
    let lower_move = move_name.to_ascii_lowercase();
    match lower_move.as_str() {
        "attack_s3_s" if lower_fighter == "ryu" || lower_fighter == "ken" => move_name.to_string(),
        "attack_s3_s" => "attack_s3".to_string(),
        "attack_s4_s" => "attack_s4".to_string(),
        "attack_s3_s2" => "attack_s3_2".to_string(),
        "attack_s3_s3" => "attack_s3_3".to_string(),
        // `attack_s4_s2` is `AttackS4S2.txt` upstream; the alias never exists.
        _ => move_name.to_string(),
    }
}

/// Worker threads used for fighter-wide script scans (and the connection-pool size).
/// GitHub's raw host serves this happily; more stops helping once the pipe is saturated.
pub const SCRIPT_FETCH_THREADS: usize = 8;

/// Shared blocking HTTP client for every GitHub script/index fetch.
///
/// `reqwest::blocking::get` builds a THROWAWAY client per call, so a fighter-wide scan paid a
/// fresh DNS + TCP + TLS handshake for all ~450 moves (measured ~236 ms per request, against
/// ~32 ms on a kept-alive connection). One shared client pools connections across every
/// request and is safe to use from several threads at once.
static HTTP: std::sync::LazyLock<reqwest::blocking::Client> = std::sync::LazyLock::new(|| {
    reqwest::blocking::Client::builder()
        .user_agent("visionary")
        .pool_max_idle_per_host(SCRIPT_FETCH_THREADS)
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
});

/// The shared pooled HTTP client (see [`HTTP`]).
pub fn http_client() -> &'static reqwest::blocking::Client {
    &HTTP
}

/// Disk path a fighter+move's cached script body lives at.
fn script_cache_path(fighter: &str, move_name: &str) -> std::path::PathBuf {
    crate::scratch_dirs::app_storage_root()
        .join("script-cache")
        .join(fighter)
        .join(format!("{}.txt", move_name_to_pascal(move_name)))
}

/// Remove Visionary's cached ACMD scripts for one fighter.
///
/// This only touches the app-owned `script-cache/<fighter>/` directory. The dumped game files,
/// linked source project, and any exported mod remain untouched. An absent cache is a successful
/// no-op so forgetting a fighter that was only indexed locally does not surface a false error.
pub fn forget_fighter_cache(fighter: &str) -> std::io::Result<bool> {
    forget_fighter_cache_at(&crate::scratch_dirs::app_storage_root(), fighter)
}

/// [`forget_fighter_cache`] against an explicit cache root, kept separate so the deletion scope
/// can be tested without redirecting the process-wide user cache used by the corpus tests.
pub(crate) fn forget_fighter_cache_at(
    storage_root: &std::path::Path,
    fighter: &str,
) -> std::io::Result<bool> {
    if fighter.is_empty() || fighter == "." || fighter == ".." || fighter.contains(['/', '\\']) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "fighter name is not a single cache path component",
        ));
    }
    let dir = storage_root.join("script-cache").join(fighter);
    if !dir.is_dir() {
        return Ok(false);
    }
    std::fs::remove_dir_all(dir)?;
    Ok(true)
}

/// A cached body that is really an HTTP error page rather than a script.
///
/// Until 2026-08-06 the fetch below wrote whatever came back to the cache, and a 404 from
/// `raw.githubusercontent.com` is a *successful* request whose body is the eight-word error
/// page. Those files reached the parser as script source: the move opened as a one-line script
/// reading `404: Not Found`, and because that line is kept as [`AcmdStmt::Raw`] the export wrote
/// it verbatim into a generated `.rs` file.
///
/// The fetch now checks the status, so no new file can look like this. This exists for the ones
/// already on disk — normalising them rather than deleting them, so the fix takes effect without
/// anyone clearing their cache.
fn is_cached_error_page(body: &str) -> bool {
    body.trim().starts_with("404: Not Found")
}

/// The script source a body represents: the body itself, or **empty** if it is a miss.
///
/// A miss is empty rather than [`None`] on purpose. To the fighter-wide scan `None` means "not
/// cached", so reporting a known-missing move that way would send it back to the network on
/// every scan and undo the caching this whole path exists for. Upstream never serves an empty
/// script, so an empty body is an unambiguous "this move has none".
///
/// **Every body reaching a caller goes through here, fresh off the wire or off the disk.** The
/// status check in [`fetch_script_body`] cannot be reached by a test without a network, so it is
/// deliberately not the only guard: if it regressed, this would still catch the page before it
/// reached a parser.
pub(crate) fn script_source_from_body(raw: &str) -> String {
    if is_cached_error_page(raw) {
        String::new()
    } else {
        raw.to_string()
    }
}

/// Cached script body, if this move was fetched before. Never touches the network — lets a
/// scan resolve every already-known move up front and spend threads only on the rest.
///
/// [`Some`] means "this move has been fetched", **not** "this move has a script": a cached miss
/// comes back as `Some("")`. See [`script_source_from_body`].
///
/// Motions whose scripts drop the `_s` side indicator (`attack_s3_s` →
/// `AttackS3.txt`) resolve through [`move_script_aliases`]: the first cached
/// candidate with a body wins, so Ryu/Ken's `AttackS3S.txt` still wins for them
/// while everyone else falls through to `AttackS3.txt`. When every candidate is
/// cached but all are empty the move is a known miss (`Some("")`); when any
/// candidate is uncached the answer needs the network (`None`).
pub fn cached_script_body(fighter: &str, move_name: &str) -> Option<String> {
    let mut seen_uncached = false;
    let mut seen_cached = false;
    for alias in move_script_aliases(move_name) {
        match cached_script_body_at(&script_cache_path(fighter, &alias)) {
            Some(body) if !body.is_empty() => return Some(body),
            Some(_) => seen_cached = true,
            None => seen_uncached = true,
        }
    }
    if seen_cached && !seen_uncached {
        Some(String::new())
    } else if seen_cached {
        // The exact spelling missed but an alias was never fetched: the network
        // still has the answer, so this is not a definitive miss.
        None
    } else {
        None
    }
}

/// [`cached_script_body`] against an explicit path, so a test can exercise the real read and
/// normalise without redirecting the process-wide cache directory out from under the corpus
/// tests (which would make them silently skip rather than fail).
pub(crate) fn cached_script_body_at(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|raw| script_source_from_body(&raw))
}

/// The dump commit the move names and corpus were built against. Upstream reorganised its
/// folders (and renamed `*Cancel` scripts) afterwards, so `main` no longer has these paths.
pub const DUMP_REV: &str = "59004685238a3f6f93bae905d3a4e079701c40b4";

/// Disk-cached [`fetch_script_body`]: bodies (**including misses**, stored as an empty file) are
/// kept under `{app_storage_root}/script-cache/{fighter}/`, so fighter-wide scans (the
/// transplant studio's full-use discovery) only hit the network once per move ever.
pub fn fetch_script_body_cached(fighter: &str, move_name: &str) -> anyhow::Result<String> {
    if let Some(body) = cached_script_body(fighter, move_name) {
        return Ok(body);
    }
    // Fetch each candidate in order, caching every result (including misses) under
    // its own file, and return the first hit. Caching per candidate keeps the warm
    // path consistent: what goes on disk is exactly what a later read gives back,
    // and a miss for the exact spelling never masks a hit for its alias.
    for alias in move_script_aliases(move_name) {
        if let Some(body) = cached_script_body_at(&script_cache_path(fighter, &alias)) {
            if !body.is_empty() {
                return Ok(body);
            }
            continue;
        }
        // Normalised before it is stored *and* before it is returned, so the cold path and the warm
        // path cannot disagree: what goes on disk is exactly what a later read gives back.
        let pascal = move_name_to_pascal(&alias);
        let url = format!(
            "https://raw.githubusercontent.com/WuBoytH/SSBU-Dumped-Scripts/{DUMP_REV}/smashline/lua2cpp_{fighter}/{fighter}/{pascal}.txt"
        );
        let response = HTTP.get(&url).send()?;
        let body = if !response.status().is_success() {
            String::new()
        } else {
            script_source_from_body(&response.text()?)
        };
        let path = script_cache_path(fighter, &alias);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, &body);
        if !body.is_empty() {
            return Ok(body);
        }
    }
    Ok(String::new())
}

pub fn parse_acmd_script(source: &str) -> AcmdScript {
    let game_fn = extract_function(source, "game_");
    // A body with no function header at all is a live capture or a paste, and is read whole —
    // that is the only reason this falls back. A source that *does* hold ACMD functions but no
    // `game_` is a different thing entirely: reading it whole pulls another category's lines in
    // as `Raw`, and `emit_stmts` writes those straight back out into the generated `game_`
    // function. A sound-only or effect-only move would export its sounds twice.
    let source = match game_fn {
        Some(ref extracted) => extracted.as_str(),
        None if holds_acmd_function(source) => return AcmdScript::default(),
        None => source,
    };
    let lines: Vec<&str> = source.lines().collect();
    // Skip the function signature line and closing brace
    let body_lines = if lines.len() >= 2 {
        &lines[1..lines.len() - 1]
    } else {
        &lines[..]
    };
    if let Some(preserved) = preserve_combined_execute_else(source, body_lines) {
        return preserved;
    }
    let (stmts, _) = parse_stmts(body_lines, 0, ParseDomain::Game, true);
    AcmdScript {
        stmts: recover_dumped_motion_loop(stmts),
    }
}

/// Re-roll a motion loop the script dump flattened.
///
/// The game runs looped scripts as `loop { body; wait }`, where the wait is `wait_loop_clear`
/// or `wait_loop_sync_mot` (Lua: stafy's `sound_Walk`, kireihana's `game` script). The dump
/// drops the `loop` and starts the body at the wait: Steve's run effect reads
/// `wait_loop_sync_mot; frame(14) … frame(29) …`, which HDR writes back as
/// `loop { frame(14) … frame(29) …; wait_loop_sync_mot }`, and a rapid jab reads
/// `hit; wait_loop_clear; frame(2) hit … frame(10)`, whose leading hit is the body's tail (Fox,
/// Buddy, Zelda… end on a bare `frame(N)` that only makes sense with it after; HDR's Buddy jab is
/// exactly the re-rolled order). Left flat, the script runs once and the move stops hitting, or
/// the run stops kicking up dust, after the first cycle. Applied only to a single root wait of
/// either kind whose re-rolled body still has non-decreasing `frame` calls; anything else stays
/// as written.
struct Reroll<T> {
    /// Which wait a root statement is, if any.
    wait: fn(&T) -> Option<crate::data::MotionLoop>,
    /// A statement that would make the rotation unsafe: a block holding a wait, an existing
    /// motion loop, or an unparsed line that may carry timing.
    blocks: fn(&T) -> bool,
    frame: fn(&T) -> Option<f32>,
}

impl<T> Reroll<T> {
    fn apply(&self, stmts: Vec<T>) -> Result<(Vec<T>, crate::data::MotionLoop), Vec<T>> {
        let waits: Vec<(usize, crate::data::MotionLoop)> = stmts
            .iter()
            .enumerate()
            .filter_map(|(index, stmt)| (self.wait)(stmt).map(|kind| (index, kind)))
            .collect();
        let [(at, kind)] = waits.as_slice() else {
            return Err(stmts);
        };
        let (at, kind) = (*at, *kind);
        if stmts.iter().any(self.blocks) {
            return Err(stmts);
        }
        let mut rest = stmts;
        let tail = rest.split_off(at + 1);
        let wait = rest.pop().expect("the wait is at `at`");
        let mut body = tail;
        body.extend(rest);
        let mut last = f32::NEG_INFINITY;
        for stmt in &body {
            if let Some(frame) = (self.frame)(stmt) {
                if frame < last {
                    // Not a rotation of one ordered body; restore the source order.
                    let split = body.len() - at;
                    let mut original = body.split_off(split);
                    original.push(wait);
                    original.extend(body);
                    return Err(original);
                }
                last = frame;
            }
        }
        Ok((body, kind))
    }
}

fn recover_dumped_motion_loop(stmts: Vec<AcmdStmt>) -> Vec<AcmdStmt> {
    fn holds_wait(stmts: &[AcmdStmt]) -> bool {
        stmts.iter().any(|stmt| match stmt {
            AcmdStmt::WaitLoopClear | AcmdStmt::NativeMotionLoopSync => true,
            AcmdStmt::Loop { body, .. } | AcmdStmt::RawBlock { body, .. } => holds_wait(body),
            _ => false,
        })
    }
    let reroll = Reroll {
        wait: |stmt: &AcmdStmt| match stmt {
            AcmdStmt::WaitLoopClear => Some(crate::data::MotionLoop::Clear),
            AcmdStmt::NativeMotionLoopSync => Some(crate::data::MotionLoop::SyncMotion),
            _ => None,
        },
        blocks: |stmt: &AcmdStmt| match stmt {
            AcmdStmt::Loop {
                body, motion_loop, ..
            } => motion_loop.is_loop() || holds_wait(body),
            AcmdStmt::RawBlock { body, .. } => holds_wait(body),
            AcmdStmt::Raw(line) => line.contains("frame(") || line.contains("wait_loop"),
            _ => false,
        },
        frame: |stmt: &AcmdStmt| match stmt {
            AcmdStmt::Frame(frame) | AcmdStmt::NativeExecuteFrame(frame) => Some(*frame),
            _ => None,
        },
    };
    match reroll.apply(stmts) {
        Ok((body, motion_loop)) => vec![AcmdStmt::Loop {
            count: 1,
            body,
            motion_loop,
        }],
        Err(stmts) => stmts,
    }
}

fn recover_dumped_effect_motion_loop(stmts: Vec<EffectStmt>) -> Vec<EffectStmt> {
    fn holds_wait(stmts: &[EffectStmt]) -> bool {
        stmts.iter().any(|stmt| match stmt {
            EffectStmt::NativeMotionLoopSync => true,
            EffectStmt::Loop { body, .. } | EffectStmt::Cond { body, .. } => holds_wait(body),
            EffectStmt::Raw(line) => line.contains("wait_loop"),
            _ => false,
        })
    }
    let reroll = Reroll {
        wait: |stmt: &EffectStmt| {
            matches!(stmt, EffectStmt::NativeMotionLoopSync)
                .then_some(crate::data::MotionLoop::SyncMotion)
        },
        blocks: |stmt: &EffectStmt| match stmt {
            EffectStmt::Loop {
                body, motion_loop, ..
            } => motion_loop.is_loop() || holds_wait(body),
            EffectStmt::Cond { body, .. } => holds_wait(body),
            EffectStmt::Raw(line) => line.contains("frame(") || line.contains("wait_loop"),
            _ => false,
        },
        frame: |stmt: &EffectStmt| match stmt {
            EffectStmt::Frame(frame) | EffectStmt::NativeExecuteFrame(frame) => Some(*frame),
            _ => None,
        },
    };
    match reroll.apply(stmts) {
        Ok((body, motion_loop)) => vec![EffectStmt::Loop {
            count: 1,
            body,
            motion_loop,
        }],
        Err(stmts) => stmts,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParseDomain {
    Game,
    Sound,
    Expression,
}

/// Parse statements from a slice of lines starting at `pos`.
/// Returns (statements, lines_consumed).
fn parse_stmts(
    lines: &[&str],
    pos: usize,
    domain: ParseDomain,
    native_attack_all_allowed: bool,
) -> (Vec<AcmdStmt>, usize) {
    parse_stmts_with_fear_face_guard(
        lines,
        pos,
        domain,
        native_attack_all_allowed,
        domain == ParseDomain::Game,
        false,
        RootStatementPermissions {
            visibility: matches!(domain, ParseDomain::Game | ParseDomain::Expression),
            motion_loop_sync: true,
        },
    )
}

#[derive(Default)]
struct RootStatementPermissions {
    visibility: bool,
    motion_loop_sync: bool,
}

const FINAL_FEAR_FACE_READ_ONLY_GUARD: &str =
    "if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {";

/// Parse the one measured read-only camera condition as a narrow exception for its child command.
/// This permission is not inherited by further runtime blocks, loops, or sibling/else bodies.
fn parse_stmts_with_fear_face_guard(
    lines: &[&str],
    mut pos: usize,
    domain: ParseDomain,
    native_attack_all_allowed: bool,
    named_event_scope_allowed: bool,
    fear_face_read_only_guard: bool,
    root_permissions: RootStatementPermissions,
) -> (Vec<AcmdStmt>, usize) {
    let mut stmts = Vec::new();
    let root_visibility_allowed = root_permissions.visibility;
    let root_motion_loop_sync = root_permissions.motion_loop_sync;
    let native_motion_sync_heads = if root_permissions.motion_loop_sync
        && lines.iter().any(|line| line.contains("wait_loop_sync_mot"))
    {
        native_timing_source_lines(lines, "wait_loop_sync_mot")
    } else {
        Default::default()
    };
    let visibility_execute_heads = if root_visibility_allowed
        && lines
            .iter()
            .any(|line| line.contains("VisibilityModule::set_int64("))
    {
        unattributed_source_branch_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    let closed_root_execute_heads = if root_visibility_allowed
        && lines.iter().any(|line| {
            line.contains("VisibilityModule::set_whole(")
                || line.contains("macros::SHOOT_ITEM_BULLET(")
                || line.contains("macros::START_INFO_FLASH_EYE(")
                || line.contains("macros::CAM_ZOOM_OUT(")
                || line.contains("macros::FT_START_CUTIN(")
                || line.contains("AttackModule::set_attack_reference_joint_id(")
                || line.contains("VisibilityModule::set_default_int64(")
                || line.contains("ArticleModule::generate_article_enable(")
                || line.contains("ArticleModule::change_status(")
                || line.contains("ArticleModule::change_status_exist(")
                || line.contains("ArticleModule::change_motion(")
        }) {
        unattributed_plain_execute_without_else_heads(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    let capture_cut_common_bare_heads = if domain == ParseDomain::Game
        && root_visibility_allowed
        && lines
            .iter()
            .any(|line| line.contains("macros::game_CaptureCutCommon("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    let native_execute_frame_heads = if root_visibility_allowed
        && matches!(domain, ParseDomain::Game | ParseDomain::Expression)
        && lines.iter().any(|line| line.contains("execute"))
    {
        native_execute_frame_source_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // Resolve attributes and comments with Rust's statement boundaries, not proximity to
    // the previous physical line. A blank/comment/multiline attribute cannot grant authority.
    let fear_face_guard_heads = if domain == ParseDomain::Game
        && lines
            .iter()
            .any(|line| line.contains("FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA"))
    {
        unattributed_source_branch_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // Named-event parsing has its own source permission. Unlike the broad legacy native family
    // flag, this is restricted to an unattached, plain Game Execute at this scope; recursive
    // condition/loop walks receive `native_attack_all_allowed = false` and leave the call Raw for
    // the fighter-aware timeline importer to consider separately.
    let has_named_event_source = lines.iter().any(|line| {
        line.contains("dispatch_named_from_module")
            || line.contains("dispatch_named_chart_from_module")
            || line.contains("dispatch_named_chart_values_from_module")
    });
    let named_event_execute_heads =
        if domain == ParseDomain::Game && named_event_scope_allowed && has_named_event_source {
            unattributed_plain_execute_heads(lines)
        } else {
            std::collections::BTreeSet::new()
        };
    // Loop attributes change whether the statement runs. Keep them attached to the loop as
    // immutable source on every track, not only when a named-event source happens to be present.
    let clean_loop_heads = if lines
        .iter()
        .any(|line| line.trim_start().starts_with("for "))
    {
        unattributed_source_loop_heads(lines)
    } else {
        std::collections::BTreeSet::new()
    };

    let native_call_heads = if lines
        .iter()
        .any(|line| crate::move_nodes::native_calls::contains_supported_call(line))
    {
        direct_native_statement_lines(lines)
    } else {
        Default::default()
    };

    while pos < lines.len() {
        let (logical_line, lines_consumed) = logical_line_at(lines, pos);
        let line = logical_line.trim();

        // Skip empty lines and closing braces handled by caller
        if line.is_empty() || line == "}" {
            pos += 1;
            continue;
        }

        // for _ in 0..N { ... }
        if clean_loop_heads.contains(&pos) {
            if let Some(count) = parse_for_loop_header(line) {
                // Find the matching closing brace
                let body_start = pos + lines_consumed;
                let Some((body_start, body_lines_end)) =
                    complete_block_body(lines, pos, body_start)
                else {
                    stmts.push(AcmdStmt::Raw(raw_block_tail(lines, pos)));
                    pos = lines.len();
                    break;
                };
                let body_slice = &lines[body_start..body_lines_end];
                let (body, _) = parse_stmts_with_fear_face_guard(
                    body_slice,
                    0,
                    domain,
                    false,
                    false,
                    false,
                    RootStatementPermissions::default(),
                );
                let event_repeat_is_clean = clean_loop_heads.contains(&pos)
                    && line == format!("for _ in 0..{count} {{").as_str();
                if body_slice.iter().any(|source_line| {
                    source_line.contains("dispatch_named_from_module")
                        || source_line.contains("dispatch_named_chart_from_module")
                        || source_line.contains("dispatch_named_chart_values_from_module")
                }) && !event_repeat_is_clean
                {
                    // Only an exact, clean canonical event-bearing loop is editable here.
                    // V88 aligns its numeric ceiling with the existing node Repeat limit of
                    // 100; this does not grant helper/call/native authority or relax the
                    // donor/Game/whole-parent/Execute/call source gates.
                    stmts.push(AcmdStmt::RawBlock {
                        header: line.to_owned(),
                        body,
                    });
                } else {
                    stmts.push(AcmdStmt::Loop {
                        motion_loop: crate::data::MotionLoop::No,
                        count,
                        body,
                    });
                }
                pos = body_lines_end + 1;
                continue;
            }
        }

        // if macros::is_excute(agent) { ... }
        if is_plain_excute_header(line) {
            let body_start = pos + lines_consumed;
            let Some((body_start, body_end)) = complete_block_body(lines, pos, body_start) else {
                stmts.push(AcmdStmt::Raw(raw_block_tail(lines, pos)));
                pos = lines.len();
                break;
            };
            let excute_stmts = parse_excute_block_with_fear_face_guard(
                &lines[body_start..body_end],
                domain,
                native_attack_all_allowed,
                fear_face_read_only_guard,
                visibility_execute_heads.contains(&pos),
                closed_root_execute_heads.contains(&pos),
                named_event_execute_heads.contains(&pos),
            );
            // A separate-line alternate belongs to this Execute, even in decompiler dumps
            // where indentation suggests an outer WorkModule guard. Keep that ownership.
            if separate_else_follows(lines, body_end)
                && plain_command_body(&lines[body_start..body_end])
                && !excute_stmts
                    .iter()
                    .any(|command| matches!(command, ExcuteStmt::Raw(_)))
            {
                stmts.push(AcmdStmt::RawBlock {
                    header: "if macros::is_excute(agent) {".into(),
                    body: excute_stmts
                        .into_iter()
                        .map(|command| AcmdStmt::Bare(Box::new(command)))
                        .collect(),
                });
            } else {
                stmts.push(AcmdStmt::Excute(excute_stmts));
            }
            pos = body_end + 1;
            continue;
        }

        if native_motion_sync_heads.contains(&pos) {
            if domain != ParseDomain::Game && parse_native_motion_loop_sync_call(line) {
                stmts.push(AcmdStmt::NativeMotionLoopSync);
            } else {
                // Keep unsupported/adjacent calls intact before the legacy frame scanner.
                stmts.push(AcmdStmt::Raw(line.to_owned()));
            }
            pos += lines_consumed;
            continue;
        }

        if native_execute_frame_heads.contains(&pos) {
            if let Some(frame) = parse_native_execute_frame_call(line) {
                stmts.push(AcmdStmt::NativeExecuteFrame(frame));
            } else {
                // Do not let another call on the same physical line enter the legacy
                // frame/wait scanner and erase this unrecognized native statement.
                stmts.push(AcmdStmt::Raw(line.to_owned()));
            }
            pos += lines_consumed;
            continue;
        }

        // Unsupported, shadowed or nested sync calls must not be consumed by a
        // frame/wait found later on the same physical line. Preserve their line
        // without granting any native parsing authority.
        if line.contains("wait_loop_sync_mot") && !opens_block(line) {
            stmts.push(AcmdStmt::Raw(line.to_owned()));
            pos += lines_consumed;
            continue;
        }

        // frame(lua_state, N)
        if line.contains("frame(") && !line.contains("is_excute") {
            if let Some(f) = parse_frame_call(line) {
                stmts.push(AcmdStmt::Frame(f));
                pos += lines_consumed;
                continue;
            }
        }

        // wait_loop_clear, in one of its exact spellings. Anything else that mentions it is
        // kept verbatim rather than retyped.
        if line.contains("wait_loop_clear") {
            stmts.push(if is_wait_loop_clear_statement(line) {
                AcmdStmt::WaitLoopClear
            } else {
                AcmdStmt::Raw(line.to_owned())
            });
            pos += lines_consumed;
            continue;
        }

        // macros::FT_MOTION_RATE(agent, r) — the animation playback rate from here on. Read
        // before the block and `Raw` fallthroughs below, which is where it used to end up.
        if let Some(rate) = parse_motion_rate_call(line) {
            stmts.push(AcmdStmt::MotionRate(rate));
            pos += lines_consumed;
            continue;
        }

        // The measured motion-frame adjustment primitive is also emitted bare by captured
        // scripts. Preserve that source shape; wrapping it in `is_excute` would change the
        // capture's structure and make source write-back move the call into a different block.
        if let Some(stmt) = parse_ft_start_adjust_motion_frame_call(line) {
            stmts.push(AcmdStmt::Bare(Box::new(stmt)));
            pos += lines_consumed;
            continue;
        }

        // A measured expression script has one bare `slope!` presentation call. Type its
        // closed syntax for faithful inspection/round-trip, but timeline validation still keeps
        // this bare placement preserved rather than making it an editable command.
        if let Some(stmt @ ExcuteStmt::Slope(_)) = parse_slope_call(line) {
            stmts.push(AcmdStmt::Bare(Box::new(stmt)));
            pos += lines_consumed;
            continue;
        }

        // This helper carries its own Execute guard. Keep only its direct root Game bare form;
        // recognizing it in an Execute or nested block would change its source semantics.
        if domain == ParseDomain::Game && capture_cut_common_bare_heads.contains(&pos) {
            if let Some(stmt) = parse_capture_cut_common_call(line) {
                stmts.push(AcmdStmt::Bare(Box::new(stmt)));
                pos += lines_consumed;
                continue;
            }
        }

        // wait(lua_state, N)
        if line.contains("wait(") {
            if let Some(w) = parse_wait_call(line) {
                stmts.push(AcmdStmt::Wait(w));
                pos += lines_consumed;
                continue;
            }
        }

        // Generated movement helpers are nested in the ACMD function. Skip their exact
        // emitted attributes and bodies as one emitter detail; a same-named user helper or
        // unrelated attribute is retained unless the entire known helper body matches.
        if let Some(body_end) = generated_movement_helper_end(lines, pos) {
            pos = body_end + 1;
            continue;
        }

        if native_call_heads.contains(&pos) {
            if let Some(call) = crate::move_nodes::native_calls::parse(line)
                .or_else(|| crate::move_nodes::native_calls::parse_source_line(line))
            {
                stmts.push(AcmdStmt::Bare(Box::new(ExcuteStmt::NativeCall(call))));
                pos += lines_consumed;
                continue;
            }
        }

        // `loop { ...; wait_loop_clear }` — the native scheduler loop rapid jabs run under
        // (HDR writes Attack100 this way). Typed only when the body ends with the yield; any
        // other `loop` falls through and stays a preserved block.
        // The loop is the whole script, so its body is read with the function root's
        // permissions; only a root `loop` qualifies.
        if line == "loop {" && root_motion_loop_sync {
            let body_start = pos + lines_consumed;
            if let Some((body_start, body_end)) = complete_block_body(lines, pos, body_start) {
                if combined_else_header(lines[body_end]).is_none() {
                    let all_lines = &lines[body_start..body_end];
                    let sync_body = sync_motion_loop_body(all_lines);
                    let (mut body, _) = parse_stmts_with_fear_face_guard(
                        sync_body.unwrap_or(all_lines),
                        0,
                        domain,
                        native_attack_all_allowed,
                        named_event_scope_allowed,
                        false,
                        RootStatementPermissions {
                            visibility: root_visibility_allowed,
                            motion_loop_sync: true,
                        },
                    );
                    let waits = body
                        .iter()
                        .filter(|stmt| {
                            matches!(
                                stmt,
                                AcmdStmt::WaitLoopClear | AcmdStmt::NativeMotionLoopSync
                            )
                        })
                        .count();
                    let kind = match (sync_body, body.last()) {
                        (Some(_), _) if waits == 0 => Some(crate::data::MotionLoop::SyncMotion),
                        (None, Some(AcmdStmt::WaitLoopClear)) if waits == 1 => {
                            Some(crate::data::MotionLoop::Clear)
                        }
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        if kind == crate::data::MotionLoop::Clear {
                            body.pop();
                        }
                        stmts.push(AcmdStmt::Loop {
                            count: 1,
                            body,
                            motion_loop: kind,
                        });
                        pos = body_end + 1;
                        continue;
                    }
                }
            }
        }

        // Any other line that opens a block — a runtime branch, its `else`, or a `for` whose
        // exact count could not be proven. Detect the brace outside trailing comments/literals
        // so an unsupported loop header plus comment still owns its complete body.
        if opens_block(line) {
            let body_start = pos + lines_consumed;
            let Some((body_start, body_end)) = complete_block_body(lines, pos, body_start) else {
                stmts.push(AcmdStmt::Raw(raw_block_tail(lines, pos)));
                pos = lines.len();
                break;
            };
            let else_body_range = if let Some(else_header) = combined_else_header(lines[body_end]) {
                let else_body_start = body_end + 1;
                match complete_combined_else_body(lines, body_end, else_body_start) {
                    Some(else_body_end) => Some((else_header, else_body_start, else_body_end)),
                    None => {
                        // Do not emit the completed first arm and then flatten an incomplete
                        // sibling. Retain the complete combined construct as source instead.
                        stmts.push(AcmdStmt::Raw(raw_block_tail(lines, pos)));
                        pos = lines.len();
                        break;
                    }
                }
            } else {
                None
            };
            // A Rust attribute immediately attached to the branch can affect whether the
            // guarded statement exists at all. Do not split that attributed source construct
            // into an editable condition plus typed native child.
            let is_measured_fear_face_guard = domain == ParseDomain::Game
                && fear_face_guard_heads.contains(&pos)
                && line == FINAL_FEAR_FACE_READ_ONLY_GUARD;
            let (body, _) = parse_stmts_with_fear_face_guard(
                &lines[body_start..body_end],
                0,
                domain,
                false,
                false,
                is_measured_fear_face_guard,
                RootStatementPermissions::default(),
            );
            stmts.push(AcmdStmt::RawBlock {
                header: line.to_string(),
                body,
            });
            // Dumped scripts often put the closing brace and the following branch opener on
            // one physical line (`} else {`). `find_block_end` stops at the first closing brace,
            // so parse that sibling block here rather than feeding the combined line back into
            // the recursive walker (which would otherwise produce an inverted slice).
            if let Some((else_header, else_body_start, else_body_end)) = else_body_range {
                let (else_body, _) = parse_stmts_with_fear_face_guard(
                    &lines[else_body_start..else_body_end],
                    0,
                    domain,
                    false,
                    false,
                    false,
                    RootStatementPermissions::default(),
                );
                stmts.push(AcmdStmt::RawBlock {
                    header: else_header,
                    body: else_body,
                });
                pos = else_body_end + 1;
            } else {
                pos = body_end + 1;
            }
            continue;
        }

        // A direct expression primitive played outside every `is_excute` block. The source
        // corpus contains this native call both wrapped and bare; keep its authored structure
        // while giving the measured five-argument shape the same event walk as the macros.
        if let Some(expression) = parse_direct_expression_call(line) {
            stmts.push(AcmdStmt::Bare(Box::new(expression)));
            pos += lines_consumed;
            continue;
        }

        // A damage-reaction mode command may also be captured outside `is_excute`. Keep its
        // authored structure while feeding it into the shared frame evaluator.
        if let Some(damage) = parse_damage_no_reaction_call(line) {
            stmts.push(AcmdStmt::Bare(Box::new(damage)));
            pos += lines_consumed;
            continue;
        }

        // A sound played outside every `is_excute` block. Fifteen corpus `sound_` scripts end
        // this way, and the D1c oracle found them: they parsed as `Raw`, so a move whose last
        // footstep is written bare showed one fewer sound than it plays. Only this family is
        // routed here — a bare collision would need the timeline to decide what an unwrapped
        // hitbox means, which nothing has measured.
        if let Some(sound) = parse_sound_call(line) {
            stmts.push(AcmdStmt::Bare(Box::new(sound)));
            pos += lines_consumed;
            continue;
        }

        // Everything else — preserve verbatim
        if !line.is_empty() {
            stmts.push(AcmdStmt::Raw(line.to_string()));
        }
        pos += lines_consumed;
    }

    (stmts, pos)
}

fn is_plain_excute_header(line: &str) -> bool {
    matches!(
        line,
        "if macros::is_excute(agent) {" | "if is_excute(agent) {"
    )
}

/// Only direct, unattributed command statements may be lifted out of the command-list
/// parser. Nested control flow and malformed dump syntax retain their original witness.
fn plain_command_body(lines: &[&str]) -> bool {
    let Ok(block) =
        syn::parse_str::<syn::Block>(&format!("{{\n{}\n}}", native_statement_syntax(lines)))
    else {
        return false;
    };
    let direct = direct_native_statement_lines(lines);
    // Two statements on one physical line cannot share a command-parser slot.
    block.stmts.len() == direct.len()
        && logical_lines_with_origins(lines)
            .iter()
            .all(|(index, line)| line.trim().is_empty() || direct.contains(index))
}

fn separate_else_follows(lines: &[&str], close: usize) -> bool {
    lines
        .get(close + 1..)
        .and_then(|tail| tail.iter().find(|line| !line.trim().is_empty()))
        .is_some_and(|line| line.trim() == "else {")
}

/// The line walker cannot split `} else {` after a plain Execute guard. Preserve the whole
/// function body as one immutable witness rather than dropping or flattening its alternate arm.
/// Separate-line Else blocks retain their established structured import path.
fn preserve_combined_execute_else(source: &str, body_lines: &[&str]) -> Option<AcmdScript> {
    combined_execute_else_body(source, body_lines).map(|body| AcmdScript {
        stmts: vec![AcmdStmt::Raw(body)],
    })
}

fn combined_execute_else_body(source: &str, body_lines: &[&str]) -> Option<String> {
    use syn::visit::Visit;
    if !body_lines
        .iter()
        .any(|line| line.contains('}') && line.contains("else"))
    {
        return None;
    }
    let function = syn::parse_str::<syn::ItemFn>(source).ok()?;
    struct Finder<'a> {
        lines: Vec<&'a str>,
        found: bool,
    }
    impl<'ast> Visit<'ast> for Finder<'_> {
        fn visit_expr_if(&mut self, branch: &'ast syn::ExprIf) {
            if let Some((alternate, _)) = &branch.else_branch {
                let plain = self
                    .lines
                    .get(branch.if_token.span.start().line.saturating_sub(1))
                    .is_some_and(|line| is_plain_excute_header(line.trim()));
                if plain
                    && alternate.span.start().line
                        == branch.then_branch.brace_token.span.close().end().line
                {
                    self.found = true;
                }
            }
            syn::visit::visit_expr_if(self, branch);
        }
    }
    let mut finder = Finder {
        lines: source.lines().collect(),
        found: false,
    };
    finder.visit_block(&function.block);
    finder
        .found
        .then(|| body_lines.join("\n").trim().to_owned())
}

/// Direct, unattached `if macros::is_excute(agent)` / `if is_excute(agent)` statements in a
/// source block. This permission is intentionally separate from native write and visibility
/// permissions.
pub(crate) fn unattributed_plain_execute_heads(
    lines: &[&str],
) -> std::collections::BTreeSet<usize> {
    unattributed_source_branch_lines(lines)
        .into_iter()
        .filter(|index| {
            lines
                .get(*index)
                .is_some_and(|line| is_plain_excute_header(line.trim()))
        })
        .collect()
}

/// Direct, unattributed root Execute branches with no alternate arm. Visibility whole-body
/// authoring uses this stricter permission so `else`, expression attributes, or a lookalike
/// condition cannot authorize a source-only native setter.
fn unattributed_plain_execute_without_else_heads(
    lines: &[&str],
) -> std::collections::BTreeSet<usize> {
    use syn::spanned::Spanned;
    let Ok(block) = syn::parse_str::<syn::Block>(&format!("{{\n{}\n}}", lines.join("\n"))) else {
        return std::collections::BTreeSet::new();
    };
    block
        .stmts
        .iter()
        .filter_map(|statement| {
            let syn::Stmt::Expr(syn::Expr::If(branch), _) = statement else {
                return None;
            };
            if !branch.attrs.is_empty() || branch.else_branch.is_some() {
                return None;
            }
            let syn::Expr::Call(call) = branch.cond.as_ref() else {
                return None;
            };
            let syn::Expr::Path(path) = call.func.as_ref() else {
                return None;
            };
            let function = path.qself.is_none()
                && path.attrs.is_empty()
                && ((path.path.is_ident("is_excute"))
                    || path.path.segments.len() == 2
                        && path.path.segments[0].ident == "macros"
                        && path.path.segments[1].ident == "is_excute");
            let agent = call.args.len() == 1
                && matches!(call.args.first(), Some(syn::Expr::Path(agent))
                    if agent.attrs.is_empty() && agent.path.is_ident("agent"));
            if call.attrs.is_empty() && function && agent {
                let index = branch.span().start().line.checked_sub(2)?;
                lines
                    .get(index)
                    .is_some_and(|line| is_plain_excute_header(line.trim()))
                    .then_some(index)
            } else {
                None
            }
        })
        .collect()
}

/// Direct, unattached source `for` statements. The timeline uses these heads only when deciding
/// whether to promote a nested named-event receiver; it does not grant permission to any native
/// call family.
pub(crate) fn unattributed_source_loop_heads(lines: &[&str]) -> std::collections::BTreeSet<usize> {
    use syn::spanned::Spanned;
    let Ok(block) = syn::parse_str::<syn::Block>(&format!("{{\n{}\n}}", lines.join("\n"))) else {
        return std::collections::BTreeSet::new();
    };
    block
        .stmts
        .iter()
        .filter_map(|statement| {
            if let syn::Stmt::Expr(syn::Expr::ForLoop(loop_expr), _) = statement {
                if loop_expr.attrs.is_empty() {
                    return loop_expr.span().start().line.checked_sub(2);
                }
            }
            None
        })
        .collect()
}

const ATTACK_ABS_APPLY_THROW_SOURCE: [&str; 4] = [
    "let target = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_OBJECT);",
    "let target_group = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_GROUP);",
    "let target_no = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_NO);",
    "macros::ATK_HIT_ABS(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, Hash40::new(\"throw\"), target, target_group, target_no);",
];

/// Parse the contents of an is_excute block.
#[cfg(test)]
fn parse_excute_block(
    lines: &[&str],
    domain: ParseDomain,
    native_attack_all_allowed: bool,
) -> Vec<ExcuteStmt> {
    parse_excute_block_with_fear_face_guard(
        lines,
        domain,
        native_attack_all_allowed,
        false,
        matches!(domain, ParseDomain::Game | ParseDomain::Expression),
        matches!(domain, ParseDomain::Game | ParseDomain::Expression),
        domain == ParseDomain::Game,
    )
}

fn parse_excute_block_with_fear_face_guard(
    lines: &[&str],
    domain: ParseDomain,
    native_attack_all_allowed: bool,
    fear_face_read_only_guard: bool,
    root_visibility_allowed: bool,
    closed_root_native_allowed: bool,
    named_event_allowed: bool,
) -> Vec<ExcuteStmt> {
    let mut stmts = Vec::new();
    let visibility_heads = if root_visibility_allowed {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    let closed_root_statement_heads = if closed_root_native_allowed {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // CHECK_FINISH_CAMERA is independent of native hitbox support. Admit only exact root
    // statements from the direct Game Execute context and only literal tuples in the corpus
    // catalog; nested/attributed/Sound/Expression forms remain source text.
    let finish_camera_heads = if domain == ParseDomain::Game
        && native_attack_all_allowed
        && lines
            .iter()
            .any(|line| line.contains("macros::CHECK_FINISH_CAMERA("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // CHECK_VALID_FINAL_START_CAMERA is admitted only at the same root-direct Game Execute
    // boundary as CHECK_FINISH_CAMERA. Its six payload tokens are an indivisible measured tuple.
    let final_start_camera_heads = if domain == ParseDomain::Game
        && native_attack_all_allowed
        && lines
            .iter()
            .any(|line| line.contains("macros::CHECK_VALID_FINAL_START_CAMERA("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // Fear-face may occur directly inside the exact read-only camera-flag branch. Unlike the
    // broader root permission, this does not admit commands under any other nested control.
    let final_fear_face_heads = if domain == ParseDomain::Game
        && (native_attack_all_allowed || fear_face_read_only_guard)
        && lines
            .iter()
            .any(|line| line.contains("macros::FT_SET_FINAL_FEAR_FACE("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    let native_heads = if domain == ParseDomain::Game
        && native_attack_all_allowed
        && lines.iter().any(|line| {
            line.contains("AttackModule::set_add_reaction_frame(")
                || line.contains("AttackModule::set_add_reaction_frame_revised(")
                || line.contains("macros::ATK_HIT_ABS(")
        }) {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // Height selection has no root-only authority: the same complete SDK call appears in
    // measured bounded loops. Admit direct Game Execute statements independently of the
    // legacy root permission, retaining the enclosing branch/repeat in the timeline.
    let attack_height_heads = if domain == ParseDomain::Game
        && lines
            .iter()
            .any(|line| line.contains("AttackModule::set_attack_height_all("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // Article partial motion is admitted independently of the legacy native-attack context
    // flag, but only for direct Game statements whose complete line matches one of the twelve
    // closed owner/clip/timing tuples. Other native families retain their existing permission.
    let article_partial_motion_heads = if domain == ParseDomain::Game
        && lines
            .iter()
            .any(|line| line.contains("ArticleModule::add_motion_partial("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // These two measured AttackModule calls have their own closed source permissions. They do
    // not depend on `native_attack_all_allowed`, which belongs to unrelated legacy families.
    let damage_shake_scale_heads = if domain == ParseDomain::Game
        && lines
            .iter()
            .any(|line| line.contains("AttackModule::set_damage_shake_scale("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    let no_uniq_effect_heads = if domain == ParseDomain::Game
        && lines
            .iter()
            .any(|line| line.contains("AttackModule::set_no_uniq_effect_all("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // Target-category calls have an independent, closed source permission. Only direct Game
    // execute statements are considered; this does not enable other AttackModule families.
    let target_category_heads = if domain == ParseDomain::Game
        && lines
            .iter()
            .any(|line| line.contains("AttackModule::set_target_category("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // These exact source-only families have independent permissions: admitting either one does
    // not turn on the unrelated native AttackModule parser families.
    let set_size_heads = if domain == ParseDomain::Game
        && lines
            .iter()
            .any(|line| line.contains("AttackModule::set_size("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // Vector-target positions are admitted only from direct Game Execute statements and from
    // the exact closed Vector2f source table. This does not enable either Edge's different API
    // or unrelated AttackModule calls.
    let vec_target_pos_heads = if domain == ParseDomain::Game
        && lines
            .iter()
            .any(|line| line.contains("AttackModule::set_vec_target_pos("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    let hit_status_all_heads = if domain == ParseDomain::Game
        && lines
            .iter()
            .any(|line| line.contains("HitModule::set_status_all("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    // SLOW_OPPONENT is an independent source-only Game command. It is admitted only when
    // syn identifies the complete call as a direct statement in this Execute body.
    let slow_opponent_heads = if domain == ParseDomain::Game
        && lines
            .iter()
            .any(|line| line.contains("macros::SLOW_OPPONENT("))
    {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    let named_event_calls = if domain == ParseDomain::Game && named_event_allowed {
        unattributed_gimmick_event_calls(lines)
    } else {
        std::collections::BTreeMap::new()
    };
    // Optional hit feedback has its own independent native permission. Only an unattributed,
    // direct source statement in the top-level Game Execute context is eligible; recursive
    // branch/loop parsing passes `native_attack_all_allowed = false` and therefore stays Raw.
    let optional_hit_feedback_heads = if domain == ParseDomain::Game
        && native_attack_all_allowed
        && lines.iter().any(|line| {
            line.contains("AttackModule::set_optional_hit_effect(")
                || line.contains("AttackModule::set_optional_hit_sound(")
        }) {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    let native_call_heads = if lines
        .iter()
        .any(|line| crate::move_nodes::native_calls::contains_supported_call(line))
    {
        direct_native_statement_lines(lines)
    } else {
        Default::default()
    };
    let mut logical_lines = logical_lines_with_origins(lines).into_iter();
    while let Some((start_line, logical_line)) = logical_lines.next() {
        let line = logical_line.as_str();
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let direct_native_allowed = native_heads.contains(&start_line);
        if domain == ParseDomain::Game && slow_opponent_heads.contains(&start_line) {
            if let Some(stmt) = parse_slow_opponent_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game && optional_hit_feedback_heads.contains(&start_line) {
            if let Some(stmt) = parse_optional_hit_feedback_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game && finish_camera_heads.contains(&start_line) {
            if let Some(stmt) = parse_finish_camera_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game && final_start_camera_heads.contains(&start_line) {
            if let Some(stmt) = parse_final_start_camera_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game && final_fear_face_heads.contains(&start_line) {
            if let Some(stmt) = parse_final_fear_face_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if direct_native_allowed && line == ATTACK_ABS_APPLY_THROW_SOURCE[0] {
            let matches_bundle = logical_lines.as_slice().get(..3).is_some_and(|next_lines| {
                ATTACK_ABS_APPLY_THROW_SOURCE[1..]
                    .iter()
                    .zip(next_lines)
                    .all(|(expected, (_, actual))| actual.trim() == *expected)
            });
            if matches_bundle {
                logical_lines.nth(2);
                stmts.push(ExcuteStmt::AttackAbsApplyThrow);
                continue;
            }
        }
        if domain == ParseDomain::Game && damage_shake_scale_heads.contains(&start_line) {
            if let Some(stmt) = parse_attack_module_set_damage_shake_scale(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game && no_uniq_effect_heads.contains(&start_line) {
            if let Some(stmt) = parse_attack_module_set_no_uniq_effect_all(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game && target_category_heads.contains(&start_line) {
            if let Some(stmt) = parse_attack_module_set_target_category(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game && set_size_heads.contains(&start_line) {
            if let Some(stmt) = parse_attack_module_set_size(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game && vec_target_pos_heads.contains(&start_line) {
            if let Some(stmt) = parse_attack_module_set_vec_target_pos(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game && hit_status_all_heads.contains(&start_line) {
            if let Some(stmt) = parse_hit_module_set_status_all(line) {
                stmts.push(stmt);
                continue;
            }
        }
        // `ATTACK_FP` starts with `ATTACK` but has a different fixed 41-slot layout. Keep its
        // complete-name dispatch ahead of the ordinary family so a future addition to
        // `ATTACK_FUNCS` cannot route it through the 36-slot parser.
        if line.contains("macros::ATTACK_FP(") {
            if let Some(call) = parse_attack_fp_call(line) {
                stmts.push(ExcuteStmt::AttackFp(call));
                continue;
            }
        }
        if ATTACK_FUNCS
            .iter()
            .any(|name| line.contains(&format!("macros::{name}(")))
        {
            if let Some(call) = parse_attack_call(line) {
                stmts.push(ExcuteStmt::Attack(call));
                continue;
            }
        }
        if line.contains("macros::CATCH(") {
            if let Some(call) = parse_catch_call(line) {
                stmts.push(ExcuteStmt::Catch(call));
                continue;
            }
        }
        // The `macros::NAME(` form with the paren is doing real work here too:
        // `macros::SET_SEARCH_SIZE_EXIST(` does not contain `macros::SEARCH(`, so the
        // size-modifier macro cannot be read through the 17-slot box layout.
        if line.contains("macros::SEARCH(") {
            if let Some(call) = parse_search_call(line) {
                stmts.push(ExcuteStmt::Search(call));
                continue;
            }
        }
        // Safe below the `ATTACK_FUNCS` test above only because that one matches on
        // `macros::NAME(` with the paren: `macros::ATTACK_ABS(` does not contain
        // `macros::ATTACK(`. Without the paren this call would be read through `ATTACK`'s
        // 36-slot layout, which is the cross-family corruption this file keeps warning about.
        if line.contains("macros::ATTACK_ABS(") {
            if let Some(call) = parse_attack_abs_call(line) {
                stmts.push(ExcuteStmt::AttackAbs(call));
                continue;
            }
        }
        if line.contains("AREA_WIND_2ND") {
            if let Some(call) = parse_wind_call(line) {
                stmts.push(ExcuteStmt::Wind(call));
                continue;
            }
        }
        if line.contains("erase_wind") {
            if let Some(id) = parse_erase_wind(line) {
                stmts.push(ExcuteStmt::EraseWind(id));
                continue;
            }
        }
        if line.contains("AttackModule::clear(") {
            if let Some(id) = parse_attack_clear(line) {
                stmts.push(ExcuteStmt::Clear(id));
                continue;
            }
        }
        let attack_all = (domain == ParseDomain::Game && native_attack_all_allowed)
            .then(|| parse_attack_module_set_all_call(line))
            .flatten();
        if let Some(stmt) = attack_all {
            stmts.push(stmt);
            continue;
        }
        let indexed_attack = (domain == ParseDomain::Game
            && native_attack_all_allowed
            && (line.contains("AttackModule::set_no_finish_camera(")
                || line.contains("AttackModule::set_force_reaction(")))
        .then(|| parse_attack_module_set_indexed_call(line))
        .flatten();
        if let Some(stmt) = indexed_attack {
            stmts.push(stmt);
            continue;
        }
        let attack_height = attack_height_heads
            .contains(&start_line)
            .then(|| parse_attack_module_set_height_all_call(line))
            .flatten();
        if let Some(stmt) = attack_height {
            stmts.push(stmt);
            continue;
        }
        let final_finish_cut_in = (domain == ParseDomain::Game
            && native_attack_all_allowed
            && line.contains("AttackModule::set_final_finish_cut_in("))
        .then(|| parse_attack_module_set_final_finish_cut_in_call(line))
        .flatten();
        if let Some(stmt) = final_finish_cut_in {
            stmts.push(stmt);
            continue;
        }
        let init_attack_pos = (domain == ParseDomain::Game
            && native_attack_all_allowed
            && line.contains("AttackModule::init_attack_pos("))
        .then(|| parse_attack_module_init_attack_pos_call(line))
        .flatten();
        if let Some(stmt) = init_attack_pos {
            stmts.push(stmt);
            continue;
        }
        let attack_reference_choices = if line
            .contains("AttackModule::set_attack_reference_joint_id(")
        {
            match domain {
                ParseDomain::Game if native_attack_all_allowed => {
                    Some(&crate::data::AttackReferenceJointKind::GAME_CHOICES[..])
                }
                ParseDomain::Expression if closed_root_statement_heads.contains(&start_line) => {
                    Some(&crate::data::AttackReferenceJointKind::EXPRESSION_CHOICES[..])
                }
                _ => None,
            }
        } else {
            None
        };
        let attack_reference = attack_reference_choices.and_then(|choices| {
            parse_attack_module_set_attack_reference_joint_id_call(line, choices)
        });
        if let Some(stmt) = attack_reference {
            stmts.push(stmt);
            continue;
        }
        let no_damage_fly_smoke = (domain == ParseDomain::Game
            && native_attack_all_allowed
            && line.contains("AttackModule::set_no_damage_fly_smoke_all("))
        .then(|| parse_attack_module_set_no_damage_fly_smoke_all_call(line))
        .flatten();
        if let Some(stmt) = no_damage_fly_smoke {
            stmts.push(stmt);
            continue;
        }
        let add_reaction_frame = (domain == ParseDomain::Game
            && direct_native_allowed
            && (line.contains("AttackModule::set_add_reaction_frame(")
                || line.contains("AttackModule::set_add_reaction_frame_revised(")))
        .then(|| parse_attack_module_add_reaction_frame_call(line))
        .flatten();
        if let Some(stmt) = add_reaction_frame {
            stmts.push(stmt);
            continue;
        }
        let grab_rebound = (domain == ParseDomain::Game
            && native_attack_all_allowed
            && line.contains("GrabModule::set_rebound("))
        .then(|| parse_grab_rebound_call(line))
        .flatten();
        if let Some(stmt) = grab_rebound {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_hurtbox_call(line) {
            stmts.push(stmt);
            continue;
        }
        if line.contains("ItemModule::set_have_item_visibility(") {
            if let Some(stmt) = parse_item_visibility_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        // An item-fire action is admitted only as a direct root Game Execute statement.
        if domain == ParseDomain::Game
            && closed_root_statement_heads.contains(&start_line)
            && line.contains("macros::SHOOT_ITEM_BULLET(")
        {
            if let Some(stmt) = parse_shoot_item_bullet_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        // This pinned no-payload macro is source-only and admitted only on a direct statement
        // in an unattributed root Game or Expression Execute block. It is not a color value or
        // a general permission for other macros in either track.
        if matches!(domain, ParseDomain::Game | ParseDomain::Expression)
            && closed_root_statement_heads.contains(&start_line)
            && line.contains("macros::START_INFO_FLASH_EYE(")
        {
            if let Some(stmt) = parse_start_info_flash_eye_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        // FT_START_CUTIN has no payload and is source-only at a direct root Game Execute site.
        if domain == ParseDomain::Game
            && closed_root_statement_heads.contains(&start_line)
            && line.contains("macros::FT_START_CUTIN(")
        {
            if let Some(stmt) = parse_start_cut_in_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        // Camera zoom-out is admitted only as a direct, unattributed root Game Execute call.
        if domain == ParseDomain::Game
            && closed_root_statement_heads.contains(&start_line)
            && line.contains("macros::CAM_ZOOM_OUT(")
        {
            if let Some(stmt) = parse_camera_zoom_out_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if line.contains("JostleModule::set_status(") {
            if let Some(stmt) = parse_jostle_set_status_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game
            && line.contains("FighterAreaModuleImpl::enable_fix_jostle_area")
        {
            if let Some(stmt) = parse_fixed_jostle_area_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if line.contains("VisibilityModule::set_int64(") {
            if let Some(stmt @ ExcuteStmt::VisibilitySetInt64 { .. }) =
                parse_visibility_set_int64_call(line).filter(|command| match command {
                    ExcuteStmt::VisibilitySetInt64 { group, status } => {
                        crate::data::body_visibility_catalog::is_legacy_pair(group, status)
                            || visibility_heads.contains(&start_line)
                    }
                    _ => false,
                })
            {
                stmts.push(stmt);
                continue;
            }
        }
        if line.contains("VisibilityModule::set_whole(")
            && closed_root_statement_heads.contains(&start_line)
        {
            if let Some(stmt) = parse_visibility_set_whole_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if line.contains("VisibilityModule::set_default_int64(")
            && closed_root_statement_heads.contains(&start_line)
        {
            if let Some(stmt) = parse_visibility_set_default_int64_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if line.contains("ArticleModule::generate_article_enable(")
            && closed_root_statement_heads.contains(&start_line)
        {
            if let Some(stmt) = parse_enabled_article_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if matches!(domain, ParseDomain::Game | ParseDomain::Expression)
            && closed_root_statement_heads.contains(&start_line)
        {
            if let Some(stmt) = parse_article_status_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if line.contains("ArticleModule::generate_article(")
            || line.contains("ArticleModule::remove_exist(")
        {
            if let Some(stmt) = parse_article_lifecycle_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if domain == ParseDomain::Game && article_partial_motion_heads.contains(&start_line) {
            if let Some(preset) = crate::data::ArticlePartialMotionPreset::from_source_line(line) {
                stmts.push(ExcuteStmt::ArticleLifecycle(
                    crate::data::ArticleLifecycleCall::AddMotionPartial { preset },
                ));
                continue;
            }
        }
        let article_motion_allowed = match domain {
            ParseDomain::Game => native_attack_all_allowed,
            ParseDomain::Expression => closed_root_statement_heads.contains(&start_line),
            ParseDomain::Sound => false,
        };
        if article_motion_allowed && line.contains("ArticleModule::change_motion(") {
            if let Some(stmt) = parse_article_change_motion_call(line, domain) {
                stmts.push(stmt);
                continue;
            }
        }
        let article_native_allowed = match domain {
            ParseDomain::Game => native_attack_all_allowed,
            ParseDomain::Expression => true,
            ParseDomain::Sound => false,
        };
        if article_native_allowed
            && (line.contains("ArticleModule::set_visibility_whole(")
                || line.contains("ArticleModule::shoot(")
                || line.contains("ArticleModule::shoot_exist("))
        {
            if let Some(stmt) = parse_article_native_call(line) {
                let expression_shoot = domain == ParseDomain::Expression
                    && matches!(
                        &stmt,
                        ExcuteStmt::ArticleLifecycle(
                            crate::data::ArticleLifecycleCall::Shoot { .. }
                        )
                    );
                if !expression_shoot {
                    stmts.push(stmt);
                    continue;
                }
            }
        }
        if let Some(stmt) = named_event_calls.get(&start_line) {
            stmts.push(stmt.clone());
            continue;
        }
        if line.contains("slope!(") {
            if let Some(stmt) = parse_slope_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if let Some(stmt) = parse_damage_no_reaction_call(line) {
            stmts.push(stmt);
            continue;
        }
        if !line.starts_with("ATK_SET_SHIELD_SETOFF_MUL2(")
            || native_call_heads.contains(&start_line)
        {
            if let Some(stmt) = parse_attack_mod_call(line) {
                stmts.push(stmt);
                continue;
            }
        }
        if let Some(stmt) = parse_set_speed_ex_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_set_speed_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_add_speed_no_limit_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_ft_catch_stop_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_ft_start_adjust_motion_frame_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_motion_module_set_rate_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_motion_module_set_helper_calculation_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_motion_module_set_rate_partial_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_motion_module_set_frame_partial_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_clr_speed_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_set_air_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_kinetic_clear_speed_all_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_kinetic_set_consider_ground_friction_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_change_kinetic_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_kinetic_energy_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_kinetic_add_speed_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_work_flag_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_work_transition_term_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_work_module_inc_int_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_work_module_set_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_slow_module_whole_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_correct_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_reverse_lr_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_expression_call(line) {
            stmts.push(stmt);
            continue;
        }
        if let Some(stmt) = parse_sound_call(line) {
            stmts.push(stmt);
            continue;
        }
        // GrabModule and AttackModule clear different things, so they are different
        // statements. Matching `clear_all` alone re-emitted a grab clear as an attack clear.
        if line.contains("GrabModule::clear_all") {
            stmts.push(ExcuteStmt::GrabClearAll);
            continue;
        }
        if line.contains("clear_all") {
            stmts.push(ExcuteStmt::ClearAll);
            continue;
        }
        if native_call_heads.contains(&start_line) {
            if let Some(call) = crate::move_nodes::native_calls::parse(line)
                .or_else(|| crate::move_nodes::native_calls::parse_source_line(line))
            {
                stmts.push(ExcuteStmt::NativeCall(call));
                continue;
            }
        }
        stmts.push(ExcuteStmt::Raw(line.to_string()));
    }
    stmts
}

/// Find the line index of the closing `}` that matches the opening `{` on `lines[start]`.
/// Comments and Rust literals do not affect depth. Returns (closing_line_index, depth_at_end).
pub(crate) fn find_block_end(lines: &[&str], start: usize) -> (usize, i32) {
    find_lexical_block_end(lines, start, 0)
}

/// Return the body range only when its opening brace has a matching close and the close does
/// not precede the first body line. A truncated corpus fragment is kept as raw source by each
/// parser instead of being sliced, recursively interpreted, or given a synthetic closing brace.
fn complete_block_body(lines: &[&str], opener: usize, body_start: usize) -> Option<(usize, usize)> {
    if opener >= lines.len() || body_start > lines.len() {
        return None;
    }
    let (body_end, depth_at_end) = find_block_end(lines, opener);
    (depth_at_end == 0 && body_end < lines.len() && body_end >= body_start)
        .then_some((body_start, body_end))
}

fn raw_block_tail(lines: &[&str], start: usize) -> String {
    lines.get(start..).unwrap_or_default().join("\n")
}

fn complete_combined_else_body(
    lines: &[&str],
    combined_line: usize,
    body_start: usize,
) -> Option<usize> {
    if combined_line >= lines.len() || body_start > lines.len() {
        return None;
    }
    let (body_end, depth_at_end) = find_else_block_end(lines, combined_line);
    (depth_at_end == 0 && body_end < lines.len() && body_end >= body_start).then_some(body_end)
}

fn find_lexical_block_end(lines: &[&str], start: usize, mut depth: i32) -> (usize, i32) {
    let source = lines[start..].join("\n");
    let bytes = source.as_bytes();
    let mut line_index = start;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if let Some(next) = crate::acmd_src::skip_rust_literal_or_comment(&source, cursor) {
            line_index += bytes[cursor..next]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count();
            cursor = next;
            continue;
        }
        match bytes[cursor] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return (line_index, 0);
                }
            }
            b'\n' => line_index += 1,
            _ => {}
        }
        cursor += 1;
    }
    (lines.len().saturating_sub(1), depth)
}

/// Return the closing brace for one of the exact movement-helper definitions emitted above.
///
/// The generated `#[inline]` and `#[allow(dead_code)]` attributes precede the function header,
/// so testing only the header leaves two `Raw` statements in the imported script. We consume
/// those attributes only when they are immediately followed by a recognized helper whose full
/// body matches our emitter. This keeps unrelated attributes and user-written opaque code in
/// the ACMD function instead of broadly discarding anything near a helper-like name.
fn generated_movement_helper_end(lines: &[&str], start: usize) -> Option<usize> {
    let mut signature_index = start;
    if lines.get(start)?.trim() == "#[inline]" {
        if lines.get(start + 1)?.trim() != "#[allow(dead_code)]" {
            return None;
        }
        signature_index += 2;
    }

    let signature = lines.get(signature_index)?.trim();
    let expected_body: &[&str] = if signature
        == "unsafe fn visionary_set_speed(agent: &mut L2CAgentBase, speed_x: f32, speed_y: f32) {"
    {
        &[
            "agent.clear_lua_stack();",
            "lua_args!(agent, speed_x, speed_y);",
            "smash::app::sv_animcmd::SET_SPEED(agent.lua_state_agent);",
            "agent.clear_lua_stack();",
        ]
    } else if signature
        == "unsafe fn visionary_clr_speed(agent: &mut L2CAgentBase, kinetic_kind: i32) {"
    {
        &[
            "agent.clear_lua_stack();",
            "lua_args!(agent, kinetic_kind);",
            "smash::app::sv_kinetic_energy::clear_speed(agent.lua_state_agent);",
            "agent.clear_lua_stack();",
        ]
    } else if signature == "unsafe fn visionary_set_air(agent: &mut L2CAgentBase) {" {
        &[
            "agent.clear_lua_stack();",
            "smash::app::sv_animcmd::SET_AIR(agent.lua_state_agent);",
            "agent.clear_lua_stack();",
        ]
    } else {
        return None;
    };

    let (body_end, depth_at_end) = find_block_end(lines, signature_index);
    if depth_at_end != 0 || body_end <= signature_index {
        return None;
    }
    let body = lines[signature_index + 1..body_end]
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>();
    (body.as_slice() == expected_body).then_some(body_end)
}

/// Return the end of an exact generated effect helper definition, including its attributes.
fn generated_effect_helper_end(lines: &[&str], start: usize) -> Option<usize> {
    let mut signature_index = start;
    if lines.get(start)?.trim() == "#[inline]" {
        if lines.get(start + 1)?.trim() != "#[allow(dead_code)]" {
            return None;
        }
        signature_index += 2;
    }
    let signature = lines.get(signature_index)?.trim();
    let expected = match signature {
        "unsafe fn visionary_last_effect_set_work_int(agent: &mut L2CAgentBase, work: i32) {" => {
            emit_last_effect_set_work_int_helper("")
        }
        "unsafe fn visionary_last_effect_set_scale_w(agent: &mut L2CAgentBase, values: &[f32]) {" => {
            emit_last_effect_set_scale_w_helper("")
        }
        _ => return None,
    };
    let (body_end, depth_at_end) = find_block_end(lines, signature_index);
    if depth_at_end != 0 || body_end <= signature_index {
        return None;
    }
    let actual = lines[start..=body_end]
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>();
    let expected = expected
        .iter()
        .skip(2usize.saturating_sub(signature_index - start))
        .map(|line| line.trim())
        .collect::<Vec<_>>();
    (actual == expected).then_some(body_end)
}

/// Return the branch opener after a closing brace on the same physical line.
fn combined_else_header(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let suffix = trimmed.strip_prefix('}')?.trim_start();
    suffix
        .strip_prefix("else")
        .filter(|rest| rest.trim_start().starts_with('{') || rest.trim_start().starts_with("if "))
        .map(|_| suffix.to_string())
}

/// Find the closing brace for an `else` whose opening `{` shares a line with the prior `}`.
fn find_else_block_end(lines: &[&str], combined_line: usize) -> (usize, i32) {
    find_lexical_block_end(lines, combined_line + 1, 1)
}

/// Return the source line at `start`, joined with following lines while its parentheses are
/// still open.
///
/// Rustfmt commonly expands a long ACMD macro into one argument per line. The parser below is
/// intentionally small and dispatches one statement at a time, so feeding those physical lines
/// to it separately makes the first `macros::NAME(` line look like a malformed call and leaves
/// every argument as unrelated raw text. Joining only parenthesized continuations keeps the
/// existing block walker line-based while letting every call parser see the same token stream it
/// sees for a one-line call.
fn logical_line_at(lines: &[&str], start: usize) -> (String, usize) {
    let mut logical = lines[start].to_string();
    let mut depth = parenthesis_delta(lines[start]);
    let mut consumed = 1;
    while depth > 0 && start + consumed < lines.len() {
        logical.push('\n');
        logical.push_str(lines[start + consumed]);
        depth += parenthesis_delta(lines[start + consumed]);
        consumed += 1;
    }
    (logical, consumed)
}

/// Original physical line indices keep context checks separate from joined call text.
fn logical_lines_with_origins(lines: &[&str]) -> Vec<(usize, String)> {
    let mut result = Vec::new();
    let mut pos = 0;
    while pos < lines.len() {
        let (line, consumed) = logical_line_at(lines, pos);
        result.push((pos, line));
        pos += consumed;
    }
    result
}

/// Join every parenthesized continuation in a block, preserving statement order.
/// Only independently parsed, unattributed root statements can introduce the new native
/// commands. Strings, comments, nested bodies and malformed syntax never become command
/// context merely because their physical lines resemble a call or contain closing braces.
// Normalize only SDK-reviewed screen-fill statements for syntax classification.
// The original source lines remain available to every preservation pass.
fn native_statement_syntax(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|line| {
            if line
                .trim_start()
                .starts_with("macros::FILL_SCREEN_MODEL_COLOR(")
            {
                if let Some(call) = crate::move_nodes::native_calls::parse(line) {
                    return call.source();
                }
            }
            (*line).to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn direct_native_statement_lines(lines: &[&str]) -> std::collections::BTreeSet<usize> {
    use syn::spanned::Spanned;
    let Ok(block) =
        syn::parse_str::<syn::Block>(&format!("{{\n{}\n}}", native_statement_syntax(lines)))
    else {
        return std::collections::BTreeSet::new();
    };
    block
        .stmts
        .iter()
        .filter_map(|statement| {
            let eligible = match statement {
                syn::Stmt::Local(local) => local.attrs.is_empty(),
                syn::Stmt::Expr(syn::Expr::Call(call), _) => call.attrs.is_empty(),
                syn::Stmt::Macro(call) => call.attrs.is_empty(),
                _ => false,
            };
            // The synthetic opening brace occupies line1; body line0 begins on line2.
            if eligible {
                statement.span().start().line.checked_sub(2)
            } else {
                None
            }
        })
        .collect()
}

pub(crate) fn unattributed_source_branch_lines(
    lines: &[&str],
) -> std::collections::BTreeSet<usize> {
    use syn::spanned::Spanned;
    let Ok(block) = syn::parse_str::<syn::Block>(&format!("{{\n{}\n}}", lines.join("\n"))) else {
        return std::collections::BTreeSet::new();
    };
    block
        .stmts
        .iter()
        .filter_map(|statement| {
            if let syn::Stmt::Expr(syn::Expr::If(branch), _) = statement {
                if branch.attrs.is_empty() {
                    return branch.span().start().line.checked_sub(2);
                }
            }
            None
        })
        .collect()
}

/// Count parentheses that participate in Rust syntax, ignoring strings and comments.
fn parenthesis_delta(line: &str) -> i32 {
    let bytes = line.as_bytes();
    let mut delta = 0;
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;
    let mut block_comment = false;
    let mut i = 0;

    while i < bytes.len() {
        let byte = bytes[i];
        if block_comment {
            if byte == b'*' && bytes.get(i + 1) == Some(&b'/') {
                block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if in_string || in_char {
            if escaped {
                escaped = false;
                i += 1;
            } else if byte == b'\\' {
                escaped = true;
                i += 1;
            } else if (in_string && byte == b'"') || (in_char && byte == b'\'') {
                in_string = false;
                in_char = false;
                i += 1;
            } else {
                i += 1;
            }
            continue;
        }

        match byte {
            b'/' if bytes.get(i + 1) == Some(&b'/') => break,
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                block_comment = true;
                i += 2;
            }
            b'"' => {
                in_string = true;
                i += 1;
            }
            b'\'' => {
                // A lifetime has no closing quote, while a character literal does. Treating
                // both as quoted text is safe for the ACMD source forms this helper sees: a
                // lifetime never contains parentheses, and a character literal may.
                in_char = true;
                i += 1;
            }
            b'(' => {
                delta += 1;
                i += 1;
            }
            b')' => {
                delta -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    delta
}

/// The four ACMD categories, in the order a dumped script file writes them.
///
/// Used as the set a function header is matched *against*, not just for: a header names
/// exactly one of these, so the other three appearing in the same line is what rules it out.
pub(crate) const SCRIPT_PREFIXES: [&str; 4] = ["game_", "effect_", "sound_", "expression_"];

/// The categories the editor actually puts on screen, and so the only ones worth *waiting* on
/// the mirror for.
///
/// [`merge_project_over_mirror`] fills all four when a mirror body is already in hand — that is
/// free. This shorter list decides the other question: whether a partial project override is
/// worth a network round trip. Pulling vanilla's `sound_` for text nothing displays would make
/// an offline user with a perfectly good `game_`+`effect_` project sit out the HTTP timeout
/// before seeing their own move. D2 gives `expression_` the same lane now that its measured
/// macro slice is being modelled.
///
/// `sound_` joined it at D1d, which is where sounds became editable rather than merely drawn.
/// The cost the paragraph above describes is now the right trade: a project that overrides only
/// `game_` needs the mirror's `sound_` to show what the move plays, and without it the sound
/// section is empty for exactly the projects most likely to want it.
pub(crate) const DISPLAYED_PREFIXES: [&str; 4] = ["game_", "effect_", "sound_", "expression_"];

/// Fill the categories a project does not define with the mirror's, and return one body.
///
/// `project` is carried verbatim instead of being re-extracted category by category, and that
/// asymmetry is deliberate. A project's function can be called anything and bound to a script
/// by attribute — `#[acmd_script(script = "game_attacks4")] fn my_custom_name` — so pulling it
/// apart by prefix would silently drop it on the floor. Only the mirror is safe to take apart
/// that way, because it is always dumped vanilla and always plainly named.
/// Fill categories a project does not define, while leaving categories that were found more than
/// once untouched. The latter are blocked rather than replaced by vanilla: choosing one duplicate
/// source function would make the linked project appear correct while editing the wrong file.
pub fn merge_project_over_mirror_blocked(
    project: &str,
    covered: &[&str],
    blocked: &[&str],
    mirror: &str,
) -> String {
    let mut body = project.to_string();
    for prefix in SCRIPT_PREFIXES {
        if covered.contains(&prefix) || blocked.contains(&prefix) {
            continue;
        }
        if let Some(function) = extract_function(mirror, prefix) {
            body.push_str(&function);
            body.push_str("\n\n");
        }
    }
    body
}

/// Whether `trimmed` opens the function for `prefix` and no other category.
fn is_function_header(trimmed: &str, prefix: &str) -> bool {
    SCRIPT_PREFIXES
        .iter()
        .all(|other| (*other == prefix) == trimmed.contains(other))
        && (trimmed.contains(&format!("fn {prefix}")) || trimmed.starts_with("unsafe extern"))
}

/// Whether `source` holds any ACMD function at all, of any category.
///
/// The question [`parse_acmd_script`] asks to tell a headerless body — a live capture, or text
/// the user pasted — apart from a real file that simply has no `game_` in it.
fn holds_acmd_function(source: &str) -> bool {
    source.lines().any(|line| {
        let trimmed = line.trim();
        SCRIPT_PREFIXES
            .iter()
            .any(|prefix| is_function_header(trimmed, prefix))
    })
}

/// Extract one category's function body from a source that may hold all four.
pub(crate) fn extract_function(source: &str, prefix: &str) -> Option<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| is_function_header(line.trim(), prefix))?;
    let (end, _) = find_block_end(&lines, start);
    Some(format!("{}\n", lines[start..=end].join("\n")))
}

/// Parse the contents of an is_excute block from an effect_ script.
fn parse_work_follow_number(token: &str) -> Option<f32> {
    let token = token.trim();
    let digits = token
        .strip_prefix('-')
        .or_else(|| token.strip_prefix('+'))
        .unwrap_or(token);
    if digits.is_empty()
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        || digits.bytes().filter(|byte| *byte == b'.').count() > 1
        || !digits.bytes().any(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let value = token.parse::<f32>().ok()?;
    (value.is_finite() && value.abs() <= 10_000.0).then_some(value)
}

fn parse_excute_block_effects(lines: &[&str], allow_native_controls: bool) -> Vec<EffectMacro> {
    let mut macros = Vec::new();
    let has_native_effect_control = lines.iter().any(|line| {
        line.contains("EffectModule::set_disable_render_offset_last(")
            || line.contains("EffectModule::enable_sync_init_pos_last(")
            || line.contains("EffectModule::req_screen(")
            || line.contains("ColorBlendModule::set_disable_camera_depth_influence(")
            || line.contains("macros::EFFECT_OFF_KIND_WORK(")
            || line.contains("macros::EFFECT_FOLLOW_WORK(")
    });
    let native_heads = if allow_native_controls && has_native_effect_control {
        direct_native_statement_lines(lines)
    } else {
        std::collections::BTreeSet::new()
    };
    let native_call_heads = if allow_native_controls
        && lines
            .iter()
            .any(|line| crate::move_nodes::native_calls::contains_supported_call(line))
    {
        direct_native_statement_lines(lines)
    } else {
        Default::default()
    };
    for (start_line, logical_line) in logical_lines_with_origins(lines) {
        let line = logical_line.as_str();
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Helper: extract args string from a macro call like `macros::FOO(...)`
        let try_extract = |prefix: &str| -> Option<Vec<String>> {
            let start = line.find(prefix)?;
            let after = &line[start + prefix.len()..];
            // find matching closing paren
            let end = after.rfind(')')?;
            Some(tokenize_args(&after[..end]))
        };

        // Native controls have per-family closed argument contracts below. The two-argument
        // sync spelling in some dumps is intentionally not typed; the pinned binding accepts
        // only the one-argument form. Extra arguments, wrappers, alternate receivers, and
        // attributed/nested statements remain Raw. Bare calls are not routed here by the outer
        // parser and remain bare.
        if native_heads.contains(&start_line)
            && lines
                .get(start_line)
                .is_some_and(|source_line| source_line.trim() == line)
        {
            let native_args = |name: &str| -> Option<Vec<String>> {
                let prefix = format!("EffectModule::{name}(");
                let args = line.strip_prefix(&prefix)?.strip_suffix(");")?;
                Some(tokenize_args(args))
            };
            if let Some(args) = native_args("set_disable_render_offset_last") {
                if line == "EffectModule::set_disable_render_offset_last(agent.module_accessor);"
                    && args.len() == 1
                    && args[0] == "agent.module_accessor"
                {
                    macros.push(EffectMacro::Control(
                        crate::data::EffectControl::SetDisableRenderOffsetLast,
                    ));
                    continue;
                }
            }
            if let Some(args) = native_args("enable_sync_init_pos_last") {
                if line == "EffectModule::enable_sync_init_pos_last(agent.module_accessor);"
                    && args.len() == 1
                    && args[0] == "agent.module_accessor"
                {
                    macros.push(EffectMacro::Control(
                        crate::data::EffectControl::EnableSyncInitPosLast,
                    ));
                    continue;
                }
            }
            if let Some(args) = native_args("req_screen") {
                if args.len() == 5 && args[0] == "agent.module_accessor" {
                    let name = extract_hash40_string(&args[1]);
                    let flags = [args[2] == "true", args[3] == "true", args[4] == "true"];
                    let bools_are_literal = args[2..]
                        .iter()
                        .all(|flag| flag == "true" || flag == "false");
                    if bools_are_literal {
                        if let Some(preset) = name.as_deref().and_then(|name| {
                            crate::data::EffectReqScreenPreset::from_source_parts(name, flags)
                        }) {
                            macros.push(EffectMacro::Control(
                                crate::data::EffectControl::ReqScreen {
                                    preset_id: preset.id.to_string(),
                                },
                            ));
                            continue;
                        }
                    }
                }
            }
            if let Some(args) = line
                .strip_prefix("ColorBlendModule::set_disable_camera_depth_influence(")
                .and_then(|args| args.strip_suffix(");"))
                .map(tokenize_args)
            {
                if args.len() == 2
                    && args[0] == "agent.module_accessor"
                    && (args[1] == "true" || args[1] == "false")
                {
                    macros.push(EffectMacro::Control(
                        crate::data::EffectControl::SetDisableCameraDepthInfluence {
                            value: args[1] == "true",
                        },
                    ));
                    continue;
                }
            }
        }

        // All of these spawn families begin with the same editable payload: agent,
        // graphic[, flipped graphic], joint, position xyz, rotation xyz, scale. Their
        // remaining alpha/attribute/random/contact arguments do not alter the timeline
        // transform, so they ride along as text in `extra_args` for the export to replay.
        if let Some((name, flip, follows_bone)) = effect_spawn_macro_layout(line) {
            let prefix = format!("macros::{name}(");
            if let Some(t) = try_extract(&prefix) {
                let off = usize::from(flip);
                if t.len() > 9 + off {
                    let effect_name = extract_effect_hash40_name(&t[1])
                        .unwrap_or_else(|| t[1].trim().to_string());
                    let effect_name_alt = flip.then(|| {
                        extract_effect_hash40_name(&t[2]).unwrap_or_else(|| t[2].trim().to_string())
                    });
                    let bone_name = extract_effect_hash40_name(&t[2 + off])
                        .unwrap_or_else(|| t[2 + off].trim().to_string());
                    let num = |i: usize, default: f32| {
                        t.get(i)
                            .and_then(|value| value.trim().parse::<f32>().ok())
                            .unwrap_or(default)
                    };
                    let flip_axis = crate::acmd::effect_flip_axis_tail_index(name)
                        .and_then(|index| t.get(10 + off + index).cloned());
                    macros.push(EffectMacro::Effect {
                        effect_name,
                        effect_name_alt,
                        spawn_func: name.to_string(),
                        bone_name,
                        offset: [num(3 + off, 0.0), num(4 + off, 0.0), num(5 + off, 0.0)],
                        // The three rotation slots run zr, yr, xr — reversed from the
                        // [x, y, z] the rest of the editor uses. This used to read them left
                        // to right, silently swapping the X and Z angles of every script
                        // loaded from source and disagreeing with the live-capture and
                        // live-pin paths (app.rs) and the plugin (acmd_hooks.rs parse_args)
                        // about the very same call.
                        rotation: [num(8 + off, 0.0), num(7 + off, 0.0), num(6 + off, 0.0)],
                        scale: num(9 + off, 1.0),
                        follows_bone,
                        extra_args: t[10 + off..].to_vec(),
                        flip_axis,
                    });
                    continue;
                }
            }
        }

        // Effect lifetime/area controls are point events, not spawns. The wrapper signatures
        // are fixed and the external corpus supplies real calls for all four members, so accept
        // only the measured arities. A malformed call remains Raw and is carried rather than
        // being padded into a command with different semantics.
        if line.contains("macros::EFFECT_DETACH_KIND(") {
            if let Some(t) = try_extract("macros::EFFECT_DETACH_KIND(") {
                if t.len() == 3 {
                    if let Ok(unk) = t[2].trim().parse::<i64>() {
                        let effect_name = extract_effect_hash40_name(&t[1])
                            .unwrap_or_else(|| t[1].trim().to_string());
                        macros.push(EffectMacro::Control(
                            crate::data::EffectControl::DetachKind { effect_name, unk },
                        ));
                        continue;
                    }
                }
            }
        }

        if line.contains("macros::EFFECT_DETACH_KIND_WORK(") {
            if let Some(t) = try_extract("macros::EFFECT_DETACH_KIND_WORK(") {
                if t.len() == 3 {
                    if let Ok(unk) = t[2].trim().parse::<i64>() {
                        macros.push(EffectMacro::Control(
                            crate::data::EffectControl::DetachKindWork {
                                work: strip_deref(&t[1]),
                                unk,
                            },
                        ));
                        continue;
                    }
                }
            }
        }

        // Unlike the general Work-control family, off-kind admission is closed to exact
        // owner/slot/flag tuples observed in native fighter sources. Keep malformed spellings
        // and expressions as Raw so no values are guessed or normalized.
        if native_heads.contains(&start_line)
            && lines
                .get(start_line)
                .is_some_and(|source_line| source_line.trim() == line)
        {
            if let Some(args) = line
                .strip_prefix("macros::EFFECT_FOLLOW_WORK(")
                .and_then(|args| args.strip_suffix(");"))
                .map(tokenize_args)
            {
                if args.len() == 11 && args[0] == "agent" {
                    let symbol = args[1].strip_prefix('*').unwrap_or("");
                    let bone = extract_hash40_string(&args[2]);
                    let nums = args[3..10]
                        .iter()
                        .map(|token| parse_work_follow_number(token))
                        .collect::<Option<Vec<_>>>();
                    let literal_bool = matches!(args[10].as_str(), "true" | "false");
                    if args[1] == format!("*{symbol}")
                        && crate::data::EffectFollowWorkPreset::by_work_symbol(symbol).is_some()
                        && bone.as_ref().is_some_and(|bone| {
                            !bone.is_empty()
                                && bone.len() <= 128
                                && bone
                                    .bytes()
                                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                        })
                        && nums.as_ref().is_some_and(|values| {
                            values.iter().all(|value| value.abs() <= 10_000.0)
                        })
                        && literal_bool
                    {
                        let values = nums.unwrap();
                        macros.push(EffectMacro::EffectFollowWork {
                            work_symbol: symbol.to_owned(),
                            bone_name: bone.unwrap(),
                            offset: [values[0], values[1], values[2]],
                            rotation: [values[5], values[4], values[3]],
                            scale: values[6],
                            unk8: args[10] == "true",
                        });
                        continue;
                    }
                }
            }

            if let Some(args) = line
                .strip_prefix("macros::EFFECT_OFF_KIND_WORK(")
                .and_then(|args| args.strip_suffix(");"))
                .map(tokenize_args)
            {
                if args.len() == 4 && args[0] == "agent" {
                    let symbol = args[1].strip_prefix('*').unwrap_or("");
                    let flags_are_literal = args[2..]
                        .iter()
                        .all(|flag| flag == "true" || flag == "false");
                    if !symbol.is_empty() && flags_are_literal {
                        let flags = [args[2] == "true", args[3] == "true"];
                        if let Some(preset) = crate::data::EffectOffKindWorkPreset::all()
                            .iter()
                            .find(|preset| preset.work_symbol == symbol)
                            .and_then(|preset| {
                                crate::data::EffectOffKindWorkPreset::from_source_parts(
                                    preset.fighter,
                                    symbol,
                                    flags,
                                )
                            })
                        {
                            macros.push(EffectMacro::Control(
                                crate::data::EffectControl::OffKindWork {
                                    preset_id: preset.id.to_string(),
                                },
                            ));
                            continue;
                        }
                    }
                }
            }
        }

        if line.contains("macros::ENABLE_AREA(") {
            if let Some(t) = try_extract("macros::ENABLE_AREA(") {
                if t.len() == 2 {
                    macros.push(EffectMacro::Control(
                        crate::data::EffectControl::EnableArea {
                            kind: strip_deref(&t[1]),
                        },
                    ));
                    continue;
                }
            }
        }
        if line.contains("macros::UNABLE_AREA(") {
            if let Some(t) = try_extract("macros::UNABLE_AREA(") {
                if t.len() == 2 {
                    macros.push(EffectMacro::Control(
                        crate::data::EffectControl::UnableArea {
                            kind: strip_deref(&t[1]),
                        },
                    ));
                    continue;
                }
            }
        }

        if line.contains("macros::EFFECT_OFF_KIND(") {
            if let Some(t) = try_extract("macros::EFFECT_OFF_KIND(") {
                if t.len() > 1 {
                    let effect_name = extract_effect_hash40_name(&t[1])
                        .unwrap_or_else(|| t[1].trim().to_string());
                    // Modelled only when BOTH booleans are literals. Reading one and defaulting
                    // the other would write a call the script does not contain while looking
                    // like it preserved the author's value; a call that is not modelled falls
                    // through to `EffectMacro::Raw` below and is kept exactly as written.
                    if let (Some(fade), Some(detach)) = (bool_token_at(&t, 2), bool_token_at(&t, 3))
                    {
                        macros.push(EffectMacro::EffectOffKind {
                            effect_name,
                            fade,
                            detach,
                        });
                        continue;
                    }
                }
            }
        }

        // AFTER_IMAGE4_ON / AFTER_IMAGE4_ON_arg29 / AFTER_IMAGE_ON — sword/weapon trail effects.
        // Signature: AFTER_IMAGE4_ON_arg29(agent, tex1, tex2, count, bone, ...)
        // We extract the bone name (arg[4]) and use tex1 (arg[1]) as the effect name.
        if line.contains("macros::AFTER_IMAGE4_ON") || line.contains("macros::AFTER_IMAGE_ON") {
            let prefix = if line.contains("macros::AFTER_IMAGE4_ON_arg29(") {
                "macros::AFTER_IMAGE4_ON_arg29("
            } else if line.contains("macros::AFTER_IMAGE4_ON(") {
                "macros::AFTER_IMAGE4_ON("
            } else {
                "macros::AFTER_IMAGE_ON("
            };
            if let Some(t) = try_extract(prefix) {
                let read = |i: usize| {
                    extract_effect_hash40_name(t.get(i).map(|s| s.as_str()).unwrap_or(""))
                        .unwrap_or_else(|| {
                            t.get(i).map(|s| s.trim().to_string()).unwrap_or_default()
                        })
                };
                let effect_name = read(TRAIL_GRAPHIC_SLOT);
                let bone_name = read(TRAIL_JOINT_SLOT);
                // The second joint is read only from `_arg29`, the one spelling here that
                // `smash-script` actually declares. `AFTER_IMAGE4_ON` and `AFTER_IMAGE_ON` are
                // names no wrapper defines and no corpus script calls, so there is no signature
                // saying slot 8 is a joint in them — this branch parses them at all only so a
                // source file containing one survives a round trip. Claiming a joint there would
                // put an editable field on a call whose layout nothing establishes.
                let bone_name2 = if prefix.contains("_arg29") {
                    let name = read(TRAIL_JOINT2_SLOT);
                    (!name.is_empty()).then_some(name)
                } else {
                    None
                };
                if !effect_name.is_empty() {
                    macros.push(EffectMacro::AfterImage {
                        command: prefix
                            .trim_end_matches('(')
                            .rsplit("::")
                            .next()
                            .unwrap_or(prefix)
                            .to_string(),
                        effect_name,
                        bone_name,
                        bone_name2,
                        raw: line.to_string(),
                    });
                    continue;
                }
            }
        }

        // The same trail, written as a raw command instead of a wrapper: there is no
        // `macros::AFTER_IMAGE3_ON` for the caller to have used. All four trail-ON calls in the
        // corpus take this form, so this branch — not the one above it — is the one that fires
        // on real vanilla code. The command id occupies slot 0, where `agent` sits in a wrapper
        // call, so the graphic and joint are read from slots 1 and 4 exactly as above.
        // Read the slots off the site the rewriter would find, not off a second, looser scan of
        // the same text — that is what keeps "the parser made a call here" and "the rewriter can
        // edit a call here" the same statement.
        if let Some(site) = crate::acmd_src::raw_trail_line(line) {
            let slot = |i: usize| {
                site.arg(line, i)
                    .map(|value| {
                        extract_effect_hash40_name(value)
                            .unwrap_or_else(|| value.trim().to_string())
                    })
                    .unwrap_or_default()
            };
            let effect_name = slot(TRAIL_GRAPHIC_SLOT);
            if !effect_name.is_empty() {
                let bone_name2 = slot(TRAIL_JOINT2_SLOT);
                macros.push(EffectMacro::AfterImage {
                    command: site.name.clone(),
                    effect_name,
                    bone_name: slot(TRAIL_JOINT_SLOT),
                    // Absent rather than empty when the call is too short to reach slot 8, so
                    // the panel offers a second joint only where there is one to rewrite.
                    bone_name2: (!bone_name2.is_empty()).then_some(bone_name2),
                    raw: line.to_string(),
                });
                continue;
            }
        }

        // AFTER_IMAGE_OFF — turns off a sword trail. Its one argument is undocumented but
        // required by the macro, so it is read and carried rather than discarded.
        if line.contains("macros::AFTER_IMAGE_OFF(") {
            let arg = try_extract("macros::AFTER_IMAGE_OFF(")
                .and_then(|t| t.get(1).and_then(|v| v.trim().parse::<f32>().ok()))
                .unwrap_or(crate::data::TRAIL_OFF_DEFAULT);
            macros.push(EffectMacro::AfterImageOff { arg });
            continue;
        }

        if line.contains("macros::LAST_EFFECT_SET_RATE(") {
            if let Some(t) = try_extract("macros::LAST_EFFECT_SET_RATE(") {
                if t.len() > 1 {
                    let rate = t[1].trim().parse::<f32>().unwrap_or(0.0);
                    macros.push(EffectMacro::LastEffectSetRate { rate });
                    continue;
                }
            }
        }

        // `smash-script` has no wrapper for this linked primitive. The generated effect export
        // uses a local helper with the same Lua-stack contract as the crate's wrappers, so the
        // helper's own definition must not become an effect-timeline conditional on read-back.
        if line.contains("macros::LAST_EFFECT_SET_WORK_INT(")
            || line.contains("visionary_last_effect_set_work_int(")
        {
            let prefix = if line.contains("macros::LAST_EFFECT_SET_WORK_INT(") {
                "macros::LAST_EFFECT_SET_WORK_INT("
            } else {
                "visionary_last_effect_set_work_int("
            };
            if let Some(t) = try_extract(prefix) {
                if t.len() == 2 && t[0].trim() == "agent" && !t[1].trim().is_empty() {
                    macros.push(EffectMacro::LastEffectSetWorkInt {
                        work: strip_deref(&t[1]),
                    });
                    continue;
                }
            }
        }

        // The camera-flat offset has one numeric argument after `agent`, just like the rate.
        // Keep malformed or non-numeric calls raw rather than inventing a default that would
        // move an effect the source never moved.
        if line.contains("macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(") {
            if let Some(t) = try_extract("macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(") {
                if t.len() > 1 {
                    if let Ok(offset) = t[1].trim().parse::<f32>() {
                        macros.push(EffectMacro::LastEffectSetOffsetToCameraFlat { offset });
                        continue;
                    }
                }
            }
        }

        // Tint and opacity for the last spawned effect. Both arities are uniform across the
        // whole corpus — 65 `(agent, r, g, b)` and 4 `(agent, a)` — so a call of the wrong
        // length is not a variant to interpret but a line this parser does not understand, and
        // falls through to `Raw` rather than being padded into shape. A component that will not
        // parse does the same: these arguments are the entire content of the call, so defaulting
        // one means recolouring an effect the script never recoloured.
        if line.contains("macros::LAST_EFFECT_SET_COLOR(") {
            if let Some(t) = try_extract("macros::LAST_EFFECT_SET_COLOR(") {
                if t.len() > 3 {
                    let component = |i: usize| t[i].trim().parse::<f32>().ok();
                    if let (Some(r), Some(g), Some(b)) = (component(1), component(2), component(3))
                    {
                        macros.push(EffectMacro::LastEffectSetColor { rgb: [r, g, b] });
                        continue;
                    }
                }
            }
        }

        // `LAST_PARTICLE_SET_COLOR` has the same three numeric slots as the effect colour
        // modifier, but it targets the last particle rather than the last effect. Keep that
        // distinction in the IR so export, source sync, and live rules cannot silently retint
        // the wrong renderer object. A malformed arity stays raw and is carried by the residue
        // path instead of being padded with a guessed component.
        if line.contains("macros::LAST_PARTICLE_SET_COLOR(") {
            if let Some(t) = try_extract("macros::LAST_PARTICLE_SET_COLOR(") {
                if t.len() == 4 {
                    let component = |i: usize| t[i].trim().parse::<f32>().ok();
                    if let (Some(r), Some(g), Some(b)) = (component(1), component(2), component(3))
                    {
                        macros.push(EffectMacro::LastParticleSetColor { rgb: [r, g, b] });
                        continue;
                    }
                }
            }
        }

        if line.contains("macros::LAST_EFFECT_SET_ALPHA(") {
            if let Some(t) = try_extract("macros::LAST_EFFECT_SET_ALPHA(") {
                if t.len() > 1 {
                    if let Ok(alpha) = t[1].trim().parse::<f32>() {
                        macros.push(EffectMacro::LastEffectSetAlpha { alpha });
                        continue;
                    }
                }
            }
        }

        // The linked primitive accepts one through three values from the Lua stack, while the
        // vendored smash-script wrapper only exposes the three-value spelling. Keep the exact
        // dynamic arity in the IR; generated exports call a local helper that pushes exactly
        // this many values before dispatching the primitive.
        let scale_w_prefix = if line.contains("macros::LAST_EFFECT_SET_SCALE_W(") {
            Some("macros::LAST_EFFECT_SET_SCALE_W(")
        } else if line.contains("visionary_last_effect_set_scale_w(") {
            Some("visionary_last_effect_set_scale_w(")
        } else {
            None
        };
        if let Some(prefix) = scale_w_prefix {
            if let Some(t) = try_extract(prefix) {
                if (2..=4).contains(&t.len()) && t[0].trim() == "agent" {
                    let values = t[1..]
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            let mut value = value.trim();
                            if index == 0 {
                                value = value.strip_prefix("&[").unwrap_or(value);
                            }
                            if index + 1 == t.len() - 1 {
                                value = value.strip_suffix(']').unwrap_or(value);
                            }
                            value.parse::<f32>().ok()
                        })
                        .collect::<Option<Vec<_>>>();
                    if let Some(values) = values {
                        macros.push(EffectMacro::LastEffectSetScaleW { values });
                        continue;
                    }
                }
            }
        }

        // FLASH / BURN_COLOR and friends: not spawns, but they belong to the effect timeline
        // and the export regenerates that timeline from scratch, so a line left unmodelled
        // here is a line the export drops.
        if let Some((command, has_transition, has_rgba)) = color_macro_layout(line) {
            if let Some(t) = try_extract(&format!("macros::{command}(")) {
                let (transition_slot, rgba_slots) = crate::data::color_slots(has_transition);
                let num = |i: usize| -> Option<f32> { t.get(i)?.trim().parse::<f32>().ok() };
                // A short call is left as a `Raw` line rather than defaulted into shape: these
                // arguments are the whole content of the command, so guessing one means
                // shipping a colour the script never asked for.
                let transition = transition_slot.map(num);
                let rgba = has_rgba.then(|| {
                    let mut out = [0.0; 4];
                    for (dst, slot) in out.iter_mut().zip(rgba_slots) {
                        *dst = num(slot)?;
                    }
                    Some(out)
                });
                if !matches!(transition, Some(None)) && !matches!(rgba, Some(None)) {
                    macros.push(EffectMacro::Color {
                        command: command.to_string(),
                        color: crate::data::ColorCall {
                            transition: transition.flatten(),
                            rgba: rgba.flatten(),
                        },
                    });
                    continue;
                }
            }
        }

        if native_call_heads.contains(&start_line) {
            if let Some(call) = crate::move_nodes::native_calls::parse(line)
                .or_else(|| crate::move_nodes::native_calls::parse_source_line(line))
            {
                macros.push(EffectMacro::NativeCall(call));
                continue;
            }
        }
        macros.push(EffectMacro::Raw(line.to_string()));
    }
    macros
}

/// The colour command this line calls, with its argument layout.
///
/// Matched on the trailing `(`, which is what makes the table's order irrelevant here:
/// `macros::BURN_COLOR_FRAME(` does not contain `macros::BURN_COLOR(`, so the longer name
/// cannot be read as the shorter one with a stray argument. `effect_spawn_macro_layout` orders
/// its table longest-first for the same hazard; this one does not have to.
fn color_macro_layout(line: &str) -> Option<(&'static str, bool, bool)> {
    crate::data::COLOR_COMMANDS
        .iter()
        .copied()
        .find(|(command, _, _)| line.contains(&format!("macros::{command}(")))
}

/// Whether this line opens a brace block it does not also close.
///
/// Counted rather than tested with `ends_with('{')` because the dumps write costume checks as
/// `if(0x2508e0(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 3)){` — no space before the brace, and a
/// closing paren in between. Only lexical braces participate; comments and literals may contain
/// arbitrary brace text without introducing a control-flow region.
/// The source line every exported `wait_loop_clear` is written as: smash-script's macro, which
/// clears the Lua stack, calls `sv_animcmd::wait_loop_clear` and pops its result.
pub(crate) const WAIT_LOOP_CLEAR_SOURCE: &str = "macros::wait_loop_clear(agent);";

/// The exact statement spellings of the native `wait_loop_clear`: the dump's macro and its
/// argument-less decompiled form, HDR's imported macro, and the raw `sv_animcmd` call.
fn is_wait_loop_clear_statement(line: &str) -> bool {
    matches!(
        line.trim(),
        "macros::wait_loop_clear(agent);"
            | "wait_loop_clear(agent);"
            | "wait_loop_clear();"
            | "wait_loop_clear(agent.lua_state_agent);"
            | "sv_animcmd::wait_loop_clear(agent.lua_state_agent);"
            | "smash::app::sv_animcmd::wait_loop_clear(agent.lua_state_agent);"
    )
}

fn opens_block(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut cursor = 0;
    let mut depth = 0i32;
    while cursor < bytes.len() {
        if let Some(next) = crate::acmd_src::skip_rust_literal_or_comment(line, cursor) {
            cursor = next;
            continue;
        }
        match bytes[cursor] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        cursor += 1;
    }
    depth > 0
}

/// Parse statements from an effect_ function body, producing `EffectStmt`.
///
/// **On `else`.** These files are decompiler output, and the decompiler mis-scopes `else`: in
/// [kirby/CapturePulledHi.txt]() the `else {` sits inside the `if macros::is_excute(agent) {`
/// block it should be a sibling of, and the function only balances by accident. So an `else`
/// header is captured as an opaque [`EffectStmt::Cond`] header exactly like an `if` — no attempt
/// is made to pair it with anything. Anything that tried would be reasoning about a structure
/// the input does not actually have, and would be wrong on real fighters.
fn parse_effect_stmts(
    lines: &[&str],
    mut pos: usize,
    at_function_root: bool,
) -> (Vec<EffectStmt>, usize) {
    let mut stmts = Vec::new();
    let mut generated_helper_prolog = at_function_root;
    let native_motion_sync_heads =
        if at_function_root && lines.iter().any(|line| line.contains("wait_loop_sync_mot")) {
            native_timing_source_lines(lines, "wait_loop_sync_mot")
        } else {
            Default::default()
        };
    let native_execute_frame_heads =
        if at_function_root && lines.iter().any(|line| line.contains("execute")) {
            native_execute_frame_source_lines(lines)
        } else {
            std::collections::BTreeSet::new()
        };
    let clean_loop_heads = if lines
        .iter()
        .any(|line| line.trim_start().starts_with("for "))
    {
        unattributed_source_loop_heads(lines)
    } else {
        std::collections::BTreeSet::new()
    };

    let native_call_heads = if lines
        .iter()
        .any(|line| crate::move_nodes::native_calls::contains_supported_call(line))
    {
        direct_native_statement_lines(lines)
    } else {
        Default::default()
    };
    while pos < lines.len() {
        let (logical_line, lines_consumed) = logical_line_at(lines, pos);
        let line = logical_line.trim();

        if line.is_empty() {
            pos += 1;
            continue;
        }

        // Exported linked-primitive helpers form a small prolog immediately after the effect
        // function header. Keep eligibility across blank lines and exact generated helpers so
        // both helpers can be consumed, then close it permanently on any other source content.
        // This keeps comments and even multiline attributes attached to user-written items.
        if generated_helper_prolog {
            if let Some(body_end) = generated_effect_helper_end(lines, pos) {
                pos = body_end + 1;
                continue;
            }
        }
        generated_helper_prolog = false;

        if line == "}" {
            pos += 1;
            continue;
        }

        // `loop { ...; wait_loop_sync_mot }`, the effect side of the motion loops read in
        // `parse_stmts_with_fear_face_guard`. Root only; the body is read as the root it replaces.
        if line == "loop {" && at_function_root {
            let body_start = pos + lines_consumed;
            if let Some((body_start, body_end)) = complete_block_body(lines, pos, body_start) {
                if let Some(body_lines) = sync_motion_loop_body(&lines[body_start..body_end]) {
                    let (body, _) = parse_effect_stmts(body_lines, 0, true);
                    if !body
                        .iter()
                        .any(|stmt| matches!(stmt, EffectStmt::NativeMotionLoopSync))
                    {
                        stmts.push(EffectStmt::Loop {
                            count: 1,
                            body,
                            motion_loop: crate::data::MotionLoop::SyncMotion,
                        });
                        pos = body_end + 1;
                        continue;
                    }
                }
            }
        }

        if clean_loop_heads.contains(&pos) {
            if let Some(count) = parse_for_loop_header(line) {
                let body_start = pos + lines_consumed;
                let Some((body_start, body_lines_end)) =
                    complete_block_body(lines, pos, body_start)
                else {
                    stmts.push(EffectStmt::Raw(raw_block_tail(lines, pos)));
                    pos = lines.len();
                    break;
                };
                let body_slice = &lines[body_start..body_lines_end];
                let (body, _) = parse_effect_stmts(body_slice, 0, false);
                stmts.push(EffectStmt::Loop {
                    motion_loop: crate::data::MotionLoop::No,
                    count,
                    body,
                });
                pos = body_lines_end + 1;
                continue;
            }
        }

        if is_plain_excute_header(line) {
            let body_start = pos + lines_consumed;
            let Some((body_start, body_end)) = complete_block_body(lines, pos, body_start) else {
                stmts.push(EffectStmt::Raw(raw_block_tail(lines, pos)));
                pos = lines.len();
                break;
            };
            let effect_macros = parse_excute_block_effects(
                &lines[body_start..body_end],
                line.trim() == "if macros::is_excute(agent) {",
            );
            if is_plain_excute_header(line)
                && separate_else_follows(lines, body_end)
                && plain_command_body(&lines[body_start..body_end])
                // Work-selected effects retain their existing transform-only mixed witness.
                && !effect_macros.iter().any(|command| matches!(command, EffectMacro::EffectFollowWork { .. } | EffectMacro::Raw(_)))
            {
                stmts.push(EffectStmt::Cond {
                    header: "if macros::is_excute(agent) {".into(),
                    body: effect_macros
                        .into_iter()
                        .map(|command| EffectStmt::BareEffect(Box::new(command)))
                        .collect(),
                });
            } else {
                stmts.push(EffectStmt::Excute(effect_macros));
            }
            pos = body_end + 1;
            continue;
        }

        // Any other block-opening line: `if <costume check> {`, `if !WorkModule::is_flag(…) {`,
        // `else {`. Tested after `is_excute` and after the `for` header, both of which also open
        // a block and both of which have typed forms above.
        //
        // This used to fall through to `EffectStmt::Raw` below, which kept the header as a
        // dropped line and — worse — parsed the body as siblings of the conditional. Both arms
        // of an `if`/`else` then resolved as unconditional spawns at the same frame.
        if opens_block(line) {
            let body_start = pos + lines_consumed;
            let Some((body_start, body_end)) = complete_block_body(lines, pos, body_start) else {
                stmts.push(EffectStmt::Raw(raw_block_tail(lines, pos)));
                pos = lines.len();
                break;
            };
            let else_body_range = if let Some(else_header) = combined_else_header(lines[body_end]) {
                let else_body_start = body_end + 1;
                match complete_combined_else_body(lines, body_end, else_body_start) {
                    Some(else_body_end) => Some((else_header, else_body_start, else_body_end)),
                    None => {
                        stmts.push(EffectStmt::Raw(raw_block_tail(lines, pos)));
                        pos = lines.len();
                        break;
                    }
                }
            } else {
                None
            };
            let (body, _) = parse_effect_stmts(&lines[body_start..body_end], 0, false);
            stmts.push(EffectStmt::Cond {
                header: line.to_string(),
                body,
            });
            if let Some((else_header, else_body_start, else_body_end)) = else_body_range {
                let (else_body, _) =
                    parse_effect_stmts(&lines[else_body_start..else_body_end], 0, false);
                stmts.push(EffectStmt::Cond {
                    header: else_header,
                    body: else_body,
                });
                pos = else_body_end + 1;
            } else {
                pos = body_end + 1;
            }
            continue;
        }

        if native_motion_sync_heads.contains(&pos) {
            if parse_native_motion_loop_sync_call(line) {
                stmts.push(EffectStmt::NativeMotionLoopSync);
            } else {
                stmts.push(EffectStmt::Raw(line.to_owned()));
            }
            pos += lines_consumed;
            continue;
        }

        if native_execute_frame_heads.contains(&pos) {
            if let Some(frame) = parse_native_execute_frame_call(line) {
                stmts.push(EffectStmt::NativeExecuteFrame(frame));
            } else {
                stmts.push(EffectStmt::Raw(line.to_owned()));
            }
            pos += lines_consumed;
            continue;
        }

        if line.contains("wait_loop_sync_mot") && !opens_block(line) {
            stmts.push(EffectStmt::Raw(line.to_owned()));
            pos += lines_consumed;
            continue;
        }

        if line.contains("frame(") && !line.contains("is_excute") && !opens_block(line) {
            if let Some(f) = parse_frame_call(line) {
                stmts.push(EffectStmt::Frame(f));
                pos += lines_consumed;
                continue;
            }
        }

        if line.contains("wait(") {
            if let Some(w) = parse_wait_call(line) {
                stmts.push(EffectStmt::Wait(w));
                pos += lines_consumed;
                continue;
            }
        }

        // Check for bare EFFECT macro calls (outside is_excute blocks).
        // Some effect functions place EFFECT macros directly in the function body
        // without an is_excute wrapper — route them through parse_excute_block_effects.
        let is_effect_macro = effect_spawn_macro_layout(line).is_some()
            || line.contains("macros::EFFECT_OFF_KIND(")
            || line.contains("macros::AFTER_IMAGE4_ON")
            || line.contains("macros::AFTER_IMAGE_ON")
            || line.contains("macros::AFTER_IMAGE_OFF(")
            || crate::acmd_src::raw_trail_line(line).is_some()
            || line.contains("macros::LAST_EFFECT_SET_RATE(")
            || line.contains("macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(")
            || line.contains("macros::LAST_EFFECT_SET_COLOR(")
            || line.contains("macros::LAST_EFFECT_SET_ALPHA(")
            // The remaining evidence-bounded C7 members, plus the typed modifiers when they have no
            // spawn to bind to, belong to the effect timeline when written bare. Route them
            // through the same residue path as an unknown line inside `is_excute` instead of
            // dropping the whole statement.
            || line.contains("macros::LAST_PARTICLE_SET_COLOR(")
            || line.contains("macros::LAST_EFFECT_SET_WORK_INT(")
            || line.contains("macros::LAST_EFFECT_SET_SCALE_W(")
            || line.contains("visionary_last_effect_set_scale_w(")
            // C4's detach/area controls are typed point events. Keep the names in this routing
            // table because the parser is intentionally line-oriented: the typed parser below
            // will claim measured forms, while malformed forms remain carried residue.
            || line.contains("macros::EFFECT_DETACH_KIND(")
            || line.contains("macros::EFFECT_DETACH_KIND_WORK(")
            || line.contains("macros::ENABLE_AREA(")
            || line.contains("macros::UNABLE_AREA(")
            || color_macro_layout(line).is_some();
        if is_effect_macro {
            let effect_macros = parse_excute_block_effects(&[line], false);
            if !effect_macros.is_empty() {
                stmts.push(EffectStmt::Excute(effect_macros));
            }
            pos += lines_consumed;
            continue;
        }

        if native_call_heads.contains(&pos) {
            if let Some(call) = crate::move_nodes::native_calls::parse(line)
                .or_else(|| crate::move_nodes::native_calls::parse_source_line(line))
            {
                stmts.push(EffectStmt::BareNativeCall(call));
                pos += lines_consumed;
                continue;
            }
        }
        if !line.is_empty() {
            stmts.push(EffectStmt::Raw(line.to_string()));
        }
        pos += lines_consumed;
    }

    (stmts, pos)
}

/// The type of a default argument in an effect spawn's trailing native ABI.
///
/// The source spelling is kept beside the kind because export needs the former while live
/// injection needs the latter. In particular, an axis/attribute token is an integer on the
/// Lua stack, while the six ordinary `EFFECT` tail values are numbers even when their source
/// spelling is the same `0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectSpawnTailKind {
    Number,
    Bool,
    Integer,
}

/// One supported effect spawn family and the safe tail used when an editor command swap changes
/// its ABI. The common prefix is `agent, graphic[, graphic_flip], bone, position, rotation,
/// scale`; `tail` contains everything after `scale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectSpawnLayout {
    pub name: &'static str,
    pub flip: bool,
    pub follows_bone: bool,
    pub tail: &'static [(&'static str, EffectSpawnTailKind)],
}

const EFFECT_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("false", EffectSpawnTailKind::Bool),
];
const EFFECT_FLIP_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("false", EffectSpawnTailKind::Bool),
    ("0", EffectSpawnTailKind::Integer),
];
const EFFECT_ALPHA_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("false", EffectSpawnTailKind::Bool),
    ("1.0", EffectSpawnTailKind::Number),
];
const EFFECT_FLIP_ALPHA_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("false", EffectSpawnTailKind::Bool),
    ("0", EffectSpawnTailKind::Integer),
    ("1.0", EffectSpawnTailKind::Number),
];
const EFFECT_FOLLOW_TAIL: &[(&str, EffectSpawnTailKind)] = &[("false", EffectSpawnTailKind::Bool)];
const EFFECT_FOLLOW_ALPHA_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("false", EffectSpawnTailKind::Bool),
    ("1.0", EffectSpawnTailKind::Number),
];
const EFFECT_FOLLOW_FLIP_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("false", EffectSpawnTailKind::Bool),
    ("0", EffectSpawnTailKind::Integer),
];
const EFFECT_FOLLOW_FLIP_ALPHA_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("false", EffectSpawnTailKind::Bool),
    ("0", EffectSpawnTailKind::Integer),
    ("1.0", EffectSpawnTailKind::Number),
];
const EFFECT_FOLLOW_COLOR_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("false", EffectSpawnTailKind::Bool),
    ("1.0", EffectSpawnTailKind::Number),
    ("1.0", EffectSpawnTailKind::Number),
    ("1.0", EffectSpawnTailKind::Number),
];
const EFFECT_FOLLOW_FLIP_COLOR_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("false", EffectSpawnTailKind::Bool),
    ("0", EffectSpawnTailKind::Integer),
    ("1.0", EffectSpawnTailKind::Number),
    ("1.0", EffectSpawnTailKind::Number),
    ("1.0", EffectSpawnTailKind::Number),
];
const EFFECT_FOLLOW_FLIP_RND_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("false", EffectSpawnTailKind::Bool),
    ("0", EffectSpawnTailKind::Integer),
];
const EFFECT_ATTR_TAIL: &[(&str, EffectSpawnTailKind)] = &[
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("0", EffectSpawnTailKind::Number),
    ("false", EffectSpawnTailKind::Bool),
    ("0", EffectSpawnTailKind::Integer),
];

/// All effect spawn commands exposed by the parser, editor, exporter, and live replay path.
/// Keep this list synchronized with the plugin's `dispatch_effect` arms and the parser's
/// measured corpus scope. It is intentionally not a list of every native function: only
/// families with a verified common editable prefix belong here.
pub const EFFECT_SPAWN_LAYOUTS: &[EffectSpawnLayout] = &[
    EffectSpawnLayout {
        name: "EFFECT_FOLLOW_NO_STOP_FLIP",
        flip: true,
        follows_bone: true,
        tail: EFFECT_FOLLOW_FLIP_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FOLLOW_FLIP_ALPHA",
        flip: true,
        follows_bone: true,
        tail: EFFECT_FOLLOW_FLIP_ALPHA_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FOLLOW_FLIP_COLOR",
        flip: true,
        follows_bone: true,
        tail: EFFECT_FOLLOW_FLIP_COLOR_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FOLLOW_FLIP_RND",
        flip: true,
        follows_bone: true,
        tail: EFFECT_FOLLOW_FLIP_RND_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FOLLOW_FLIP",
        flip: true,
        follows_bone: true,
        tail: EFFECT_FOLLOW_FLIP_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FOLLOW_NO_SCALE",
        flip: false,
        follows_bone: true,
        tail: EFFECT_FOLLOW_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FOLLOW_NO_STOP",
        flip: false,
        follows_bone: true,
        tail: EFFECT_FOLLOW_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FOLLOW_ALPHA",
        flip: false,
        follows_bone: true,
        tail: EFFECT_FOLLOW_ALPHA_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FOLLOW_COLOR",
        flip: false,
        follows_bone: true,
        tail: EFFECT_FOLLOW_COLOR_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FLW_POS_UNSYNC_VIS",
        flip: false,
        follows_bone: true,
        tail: EFFECT_FOLLOW_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FLW_POS_NO_STOP",
        flip: false,
        follows_bone: true,
        tail: EFFECT_FOLLOW_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FLW_UNSYNC_VIS",
        flip: false,
        follows_bone: true,
        tail: EFFECT_FOLLOW_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FLW_POS",
        flip: false,
        follows_bone: true,
        tail: EFFECT_FOLLOW_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FOLLOW",
        flip: false,
        follows_bone: true,
        tail: EFFECT_FOLLOW_TAIL,
    },
    EffectSpawnLayout {
        name: "LANDING_EFFECT_FLIP",
        flip: true,
        follows_bone: false,
        tail: EFFECT_FLIP_TAIL,
    },
    EffectSpawnLayout {
        name: "FOOT_EFFECT_FLIP",
        flip: true,
        follows_bone: false,
        tail: EFFECT_FLIP_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FLIP_ALPHA",
        flip: true,
        follows_bone: false,
        tail: EFFECT_FLIP_ALPHA_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_FLIP",
        flip: true,
        follows_bone: false,
        tail: EFFECT_FLIP_TAIL,
    },
    EffectSpawnLayout {
        name: "LANDING_EFFECT",
        flip: false,
        follows_bone: false,
        tail: EFFECT_TAIL,
    },
    EffectSpawnLayout {
        name: "FOOT_EFFECT",
        flip: false,
        follows_bone: false,
        tail: EFFECT_TAIL,
    },
    EffectSpawnLayout {
        name: "DOWN_EFFECT",
        flip: false,
        follows_bone: false,
        tail: EFFECT_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_ALPHA",
        flip: false,
        follows_bone: false,
        tail: EFFECT_ALPHA_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT_ATTR",
        flip: false,
        follows_bone: false,
        tail: EFFECT_ATTR_TAIL,
    },
    EffectSpawnLayout {
        name: "EFFECT",
        flip: false,
        follows_bone: false,
        tail: EFFECT_TAIL,
    },
];

/// Look up a supported effect spawn command by its exact native/macro name.
pub fn effect_spawn_layout(name: &str) -> Option<&'static EffectSpawnLayout> {
    EFFECT_SPAWN_LAYOUTS
        .iter()
        .find(|layout| layout.name == name)
}

/// Return the tail-relative slot of the measured flip-axis/control argument.
///
/// The native `EFFECT_FOLLOW_FLIP` binding calls this final value an `i32 axis`; it is not a
/// rotation angle. Every supported flipped layout has exactly one integer tail slot, while the
/// surrounding numeric and boolean tail slots vary by macro family. Keeping the lookup beside
/// the layout table prevents the panel, source writer, and parser from assuming that the axis is
/// always the last argument or always at the same absolute position.
pub fn effect_flip_axis_tail_index(name: &str) -> Option<usize> {
    let layout = effect_spawn_layout(name)?;
    layout.flip.then(|| {
        layout
            .tail
            .iter()
            .position(|(_, kind)| *kind == EffectSpawnTailKind::Integer)
    })?
}

/// Closed native integer choices for one spawn layout. Attribute bits are not flip axes.
pub fn effect_spawn_integer_tail_tokens(name: &str) -> &'static [&'static str] {
    if name == "EFFECT_ATTR" {
        &["0", "*EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE"]
    } else if effect_spawn_layout(name).is_some_and(|layout| layout.flip) {
        &[
            "0",
            "*EF_FLIP_NONE",
            "*EF_FLIP_XY",
            "*EF_FLIP_YZ",
            "*EF_FLIP_ZX",
            "*EF_FLIP_AXIS_XY",
            "*EF_FLIP_AXIS_YZ",
            "*EF_FLIP_AXIS_ZX",
        ]
    } else {
        &[]
    }
}

/// Dumped effect spawn macros that share the common graphic/joint/transform prefix.
/// Returns `(macro name, has second flipped graphic, follows bone)`.
fn effect_spawn_macro_layout(line: &str) -> Option<(&'static str, bool, bool)> {
    EFFECT_SPAWN_LAYOUTS
        .iter()
        .find(|layout| line.contains(&format!("macros::{}(", layout.name)))
        .map(|layout| (layout.name, layout.flip, layout.follows_bone))
}

// Independent literal shapes from the pinned fighter dump, not generated from the layout table.
// These cross-fighter assets belong only to the synthetic source/build fixture, never gameplay.
#[cfg(test)]
pub(crate) const EFFECT_COMPATIBILITY_CORPUS_FIXTURE: &str = r#"
unsafe extern "C" fn effect_compatibility(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_FLIP_COLOR(agent, Hash40::new("sys_spin_wind"), Hash40::new("sys_spin_wind"), Hash40::new("rot"), 0, -3, 0, 180, -90, 0, 1.6, true, *EF_FLIP_YZ, 1, 1, 1);
        macros::EFFECT_FOLLOW_FLIP_RND(agent, Hash40::new("sys_spin_wind"), Hash40::new("sys_spin_wind"), Hash40::new("rot"), 0, -8, -1.5, 0, 0, 0, 0.4, 1, 1, 1, 0, 90, 0, true, *EF_FLIP_YZ);
        macros::EFFECT_FOLLOW_NO_SCALE(agent, Hash40::new("sys_catch"), Hash40::new("throw"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::EFFECT_FOLLOW_COLOR(agent, Hash40::new("sys_spin_wind"), Hash40::new("top"), -1.5, 18.5, 1.5, -20, 180, 20, 0.7, true, 0.3, 0.6, 0.2);
        macros::EFFECT_FLW_POS_UNSYNC_VIS(agent, Hash40::new("eflame_change_end"), Hash40::new("waist"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::EFFECT_FLW_POS_NO_STOP(agent, Hash40::new("edge_attack_speedline"), Hash40::new("swordl2"), -15, 0, 1.5, 90, 0, 180, 0.9, true);
        macros::FOOT_EFFECT_FLIP(agent, Hash40::new("sys_whirlwind_l"), Hash40::new("sys_whirlwind_r"), Hash40::new("top"), 1, 0, 0, 0, 0, 0, 0.8, 0, 0, 0, 0, 0, 0, false, *EF_FLIP_NONE);
        macros::DOWN_EFFECT(agent, Hash40::new("sys_down_smoke"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        macros::EFFECT_ATTR(agent, Hash40::new("ness_psi_rush"), Hash40::new("haver"), -0.5, 2.6, 2.9, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true, *EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE);
    }
}
"#;

/// Supply measured wrappers missing from the pinned script library, including the
/// no-payload item-fire command and signed position/no-stop transforms. Keep the
/// native source spelling and forward the reviewed arguments through the Lua stack.
/// Only actually used wrappers are emitted; all other script macros retain their library owner.
fn effect_source_compatibility_macros(source: &str) -> String {
    const COMPAT_NAMES: &[&str] = &[
        "EFFECT_FOLLOW_FLIP_COLOR",
        "EFFECT_FOLLOW_FLIP_RND",
        "EFFECT_FOLLOW_NO_SCALE",
        "EFFECT_FOLLOW_COLOR",
        "EFFECT_FLW_POS_UNSYNC_VIS",
        "EFFECT_FLW_POS_NO_STOP",
        "FOOT_EFFECT_FLIP",
        "DOWN_EFFECT",
        "EFFECT_ATTR",
    ];
    let used = crate::acmd_src::scan_macro_sites(source, 0..source.len())
        .into_iter()
        .map(|site| site.name)
        .collect::<std::collections::BTreeSet<_>>();
    let layouts = COMPAT_NAMES
        .iter()
        .filter(|name| used.contains(**name))
        .filter_map(|name| effect_spawn_layout(name))
        .collect::<Vec<_>>();
    // The pinned script crate has no wrapper for this measured source spelling.
    let shoot_item_bullet_used = used.contains("SHOOT_ITEM_BULLET");
    let native_wrappers = crate::move_nodes::native_calls::compatibility_wrappers(&used);
    if layouts.is_empty() && !shoot_item_bullet_used && native_wrappers.is_empty() {
        return String::new();
    }
    let mut output = String::from(
        "#[allow(non_snake_case, unused_imports, dead_code, clippy::too_many_arguments)]\nmod macros {\n    pub use smash_script::macros::*;\n    use smash::lua2cpp::L2CAgentBase;\n    use smash::phx::Hash40;\n",
    );
    for layout in layouts {
        let mut parameters = vec!["agent: &mut L2CAgentBase".to_owned()];
        let mut values = Vec::new();
        let mut generic_types = Vec::new();
        for hash in if layout.flip {
            &['g', 'a', 'b'][..]
        } else {
            &['g', 'b'][..]
        } {
            parameters.push(format!("{hash}: Hash40"));
            values.push(hash.to_string());
        }
        for index in 0..7 {
            parameters.push(format!("n{index}: N{index}"));
            generic_types.push(format!("N{index}: ToF32"));
            values.push(format!("n{index}.to_f32()"));
        }
        for (index, (_, kind)) in layout.tail.iter().enumerate() {
            match kind {
                EffectSpawnTailKind::Number => {
                    parameters.push(format!("t{index}: T{index}"));
                    generic_types.push(format!("T{index}: ToF32"));
                    values.push(format!("t{index}.to_f32()"));
                }
                EffectSpawnTailKind::Bool => {
                    parameters.push(format!("t{index}: bool"));
                    values.push(format!("t{index}"));
                }
                EffectSpawnTailKind::Integer => {
                    parameters.push(format!("t{index}: i32"));
                    values.push(format!("t{index}"));
                }
            }
        }
        output.push_str(&format!(
            "    #[inline]\n    pub unsafe fn {}<{}>({}) {{\n        agent.clear_lua_stack();\n        smash_script::lua_args!(agent, {});\n        smash::app::sv_animcmd::{}(agent.lua_state_agent);\n        agent.clear_lua_stack();\n    }}\n",
            layout.name,
            generic_types.join(", "),
            parameters.join(", "),
            values.join(", "),
            layout.name,
        ));
    }
    if shoot_item_bullet_used {
        output.push_str(
            "    #[inline]\n    pub unsafe fn SHOOT_ITEM_BULLET(agent: &mut L2CAgentBase) {\n        agent.clear_lua_stack();\n        smash::app::sv_animcmd::SHOOT_ITEM_BULLET(agent.lua_state_agent);\n        agent.clear_lua_stack();\n    }\n",
        );
    }
    output.push_str(&native_wrappers);
    output.push_str("}\n\n");
    output
}

/// Parse an effect_ script source into an `EffectScript` IR.
pub fn parse_effect_script(source: &str) -> crate::data::EffectScript {
    let effect_fn = extract_function(source, "effect_");
    let effect_fn = match effect_fn {
        Some(ref s) => s.as_str(),
        None => return EffectScript::default(),
    };
    let lines: Vec<&str> = effect_fn.lines().collect();
    let body_lines = if lines.len() >= 2 {
        &lines[1..lines.len() - 1]
    } else {
        &lines[..]
    };
    if let Some(body) = combined_execute_else_body(effect_fn, body_lines) {
        return EffectScript {
            stmts: vec![EffectStmt::Raw(body)],
        };
    }
    let (stmts, _) = parse_effect_stmts(body_lines, 0, true);
    EffectScript {
        stmts: recover_dumped_effect_motion_loop(stmts),
    }
}

/// The body of a `loop` that ends with the motion-sync wait, without that wait: either the
/// bare call or HDR's (and this exporter's) `clear_lua_stack` / call / `pop_lua_stack(1)`.
fn sync_motion_loop_body<'a, 'b>(body: &'a [&'b str]) -> Option<&'a [&'b str]> {
    let mut end = body.len();
    while end > 0 && body[end - 1].trim().is_empty() {
        end -= 1;
    }
    let trimmed = |index: usize| body[index].trim();
    if end >= 3
        && trimmed(end - 1) == "agent.pop_lua_stack(1);"
        && parse_native_motion_loop_sync_call(trimmed(end - 2))
        && trimmed(end - 3) == "agent.clear_lua_stack();"
    {
        return Some(&body[..end - 3]);
    }
    if end >= 1 && parse_native_motion_loop_sync_call(trimmed(end - 1)) {
        return Some(&body[..end - 1]);
    }
    None
}

/// Parse the `sound_` function's timeline, with every command left as `Raw`.
///
/// The structure — `frame`, `wait`, `is_excute` blocks, branches — is read by the same walker
/// the `game_` script uses, because it is the same shape; only the calls inside the blocks
/// differ, and none of them is typed yet. That is deliberate: this pass exists to prove the
/// round trip is byte-exact before anything starts rewriting sound lines.
///
/// `AcmdScript::to_hitboxes` and `to_hurtboxes` are meaningless on the result — a sound script
/// has no collisions — and nothing calls them on it.
///
/// Called only by its own corpus gate for now, which is the whole point of landing it on its
/// own: the round trip has to be proven before a panel or an export is built on top of it. The
/// generated-source pane is not that consumer — it promises what an export would write, and an
/// export does not write sound scripts yet. This remains an intentionally narrow staging parser
/// until sound editing has its own complete surface contract.
pub fn parse_sound_script(source: &str) -> AcmdScript {
    let Some(sound_fn) = extract_function(source, "sound_") else {
        return AcmdScript::default();
    };
    let lines: Vec<&str> = sound_fn.lines().collect();
    let body_lines = if lines.len() >= 2 {
        &lines[1..lines.len() - 1]
    } else {
        &lines[..]
    };
    if let Some(preserved) = preserve_combined_execute_else(&sound_fn, body_lines) {
        return preserved;
    }
    let (stmts, _) = parse_stmts(body_lines, 0, ParseDomain::Sound, false);
    AcmdScript {
        stmts: recover_dumped_motion_loop(stmts),
    }
}

/// Parse the `expression_` function's measured camera/rumble calls.
pub fn parse_expression_script(source: &str) -> AcmdScript {
    let Some(expression_fn) = extract_function(source, "expression_") else {
        return AcmdScript::default();
    };
    let lines: Vec<&str> = expression_fn.lines().collect();
    let body_lines = if lines.len() >= 2 {
        &lines[1..lines.len() - 1]
    } else {
        &lines[..]
    };
    if let Some(preserved) = preserve_combined_execute_else(&expression_fn, body_lines) {
        return preserved;
    }
    let (stmts, _) = parse_stmts(body_lines, 0, ParseDomain::Expression, false);
    AcmdScript {
        stmts: recover_dumped_motion_loop(stmts),
    }
}

fn parse_for_loop_header(line: &str) -> Option<usize> {
    let line = line.trim();
    // Parse a complete, attribute-free, half-open `for _ in 0..N {` header by asking Syn to
    // parse it as a statement. Requiring the final `{` before adding our sentinel close rejects
    // comments/trailing tokens and prevents salvaging a numeric prefix from another expression.
    if !line.ends_with('{') || line.contains("//") || line.contains("/*") || line.contains("*/") {
        return None;
    }
    let block = syn::parse_str::<syn::Block>(&format!("{{\n{line}\n}}\n}}")).ok()?;
    let [syn::Stmt::Expr(syn::Expr::ForLoop(loop_expr), _)] = block.stmts.as_slice() else {
        return None;
    };
    if !loop_expr.attrs.is_empty() || !matches!(loop_expr.pat.as_ref(), syn::Pat::Wild(_)) {
        return None;
    }
    let syn::Expr::Range(range) = loop_expr.expr.as_ref() else {
        return None;
    };
    if !matches!(&range.limits, syn::RangeLimits::HalfOpen(_)) {
        return None;
    }
    let (start, end) = range.start.as_deref().zip(range.end.as_deref())?;
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Int(start),
        ..
    }) = start
    else {
        return None;
    };
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Int(end),
        ..
    }) = end
    else {
        return None;
    };
    if !start.suffix().is_empty()
        || !end.suffix().is_empty()
        || start.to_string() != "0"
        || !end.to_string().bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let count = end.base10_parse::<usize>().ok()?;
    // Zero is a known no-op in the source IR, not an authorable Repeat node. Retaining its
    // exact count lets preview/site walks skip execution while still advancing source ordinals.
    (0..=100).contains(&count).then_some(count)
}

fn is_native_execute_frame_path(function: &syn::Expr) -> bool {
    is_native_timing_path(function, "execute")
}

fn is_native_timing_path(function: &syn::Expr, name: &str) -> bool {
    let syn::Expr::Path(function) = function else {
        return false;
    };
    if !function.attrs.is_empty()
        || function.qself.is_some()
        || function.path.leading_colon.is_some()
        || function
            .path
            .segments
            .iter()
            .any(|part| !matches!(part.arguments, syn::PathArguments::None))
    {
        return false;
    }
    let path = function
        .path
        .segments
        .iter()
        .map(|part| part.ident.to_string())
        .collect::<Vec<_>>();
    path == [name] || path == ["sv_animcmd", name] || path == ["smash", "app", "sv_animcmd", name]
}

#[cfg(test)]
#[path = "acmd_native_execute_frame_tests.rs"]
mod native_execute_frame_tests;

#[cfg(test)]
#[path = "acmd_native_motion_sync_tests.rs"]
mod native_motion_sync_tests;

#[cfg(test)]
#[path = "acmd_wait_loop_clear_tests.rs"]
mod wait_loop_clear_tests;

fn native_execute_frame_source_lines(lines: &[&str]) -> std::collections::BTreeSet<usize> {
    native_timing_source_lines(lines, "execute")
}

fn native_timing_source_lines(lines: &[&str], name: &str) -> std::collections::BTreeSet<usize> {
    use syn::spanned::Spanned;
    let Ok(block) = syn::parse_str::<syn::Block>(&format!("{{\n{}\n}}", lines.join("\n"))) else {
        return Default::default();
    };
    // Bare SDK names and the agent receiver must retain their native meaning.
    // Inspect only bindings in this scope: a nested helper's agent parameter
    // does not shadow the calling ACMD agent.
    if native_timing_has_shadowing(&block, name) {
        return Default::default();
    }
    block
        .stmts
        .iter()
        .filter_map(|statement| {
            let syn::Stmt::Expr(syn::Expr::Call(call), _) = statement else {
                return None;
            };
            is_native_timing_path(&call.func, name)
                .then(|| statement.span().start().line.checked_sub(2))
                .flatten()
        })
        .collect()
}

fn native_timing_has_shadowing(block: &syn::Block, native_name: &str) -> bool {
    use syn::visit::Visit;
    fn relevant(name: &syn::Ident, native_name: &str) -> bool {
        let name = name.to_string();
        name == native_name || matches!(name.as_str(), "agent" | "sv_animcmd" | "smash")
    }
    fn imports(tree: &syn::UseTree, native_name: &str) -> bool {
        match tree {
            syn::UseTree::Name(name) => relevant(&name.ident, native_name),
            syn::UseTree::Rename(name) => relevant(&name.rename, native_name),
            syn::UseTree::Path(path) => imports(&path.tree, native_name),
            syn::UseTree::Group(group) => group.items.iter().any(|tree| imports(tree, native_name)),
            syn::UseTree::Glob(_) => true,
        }
    }
    struct Bindings<'name> {
        found: bool,
        native_name: &'name str,
    }
    impl<'ast> Visit<'ast> for Bindings<'_> {
        fn visit_pat_ident(&mut self, binding: &'ast syn::PatIdent) {
            self.found |= relevant(&binding.ident, self.native_name);
            syn::visit::visit_pat_ident(self, binding);
        }
    }
    block.stmts.iter().any(|statement| match statement {
        syn::Stmt::Local(local) => {
            let mut bindings = Bindings {
                found: false,
                native_name,
            };
            bindings.visit_pat(&local.pat);
            bindings.found
        }
        syn::Stmt::Item(item) => match item {
            syn::Item::Fn(item) => relevant(&item.sig.ident, native_name),
            syn::Item::Mod(item) => relevant(&item.ident, native_name),
            syn::Item::Const(item) => relevant(&item.ident, native_name),
            syn::Item::Static(item) => relevant(&item.ident, native_name),
            syn::Item::Struct(item) => relevant(&item.ident, native_name),
            syn::Item::Enum(item) => relevant(&item.ident, native_name),
            syn::Item::Union(item) => relevant(&item.ident, native_name),
            syn::Item::Type(item) => relevant(&item.ident, native_name),
            syn::Item::Trait(item) => relevant(&item.ident, native_name),
            syn::Item::Use(item) => imports(&item.tree, native_name),
            // Foreign blocks can introduce a local function with the native name.
            syn::Item::ForeignMod(_) => true,
            syn::Item::ExternCrate(item) => item.rename.as_ref().map_or_else(
                || relevant(&item.ident, native_name),
                |(_, name)| relevant(name, native_name),
            ),
            _ => false,
        },
        _ => false,
    })
}

fn parse_native_execute_frame_call(line: &str) -> Option<f32> {
    let syn::Stmt::Expr(syn::Expr::Call(call), Some(_)) = syn::parse_str(line).ok()? else {
        return None;
    };
    if !call.attrs.is_empty() || call.args.len() != 2 || !is_native_execute_frame_path(&call.func) {
        return None;
    }
    let syn::Expr::Field(receiver) = &call.args[0] else {
        return None;
    };
    let syn::Expr::Path(agent) = receiver.base.as_ref() else {
        return None;
    };
    if !receiver.attrs.is_empty()
        || !agent.attrs.is_empty()
        || agent.qself.is_some()
        || !agent.path.is_ident("agent")
        || !matches!(&receiver.member, syn::Member::Named(name) if name == "lua_state_agent")
    {
        return None;
    }
    let (literal, negative) = match &call.args[1] {
        syn::Expr::Unary(unary)
            if unary.attrs.is_empty() && matches!(unary.op, syn::UnOp::Neg(_)) =>
        {
            (unary.expr.as_ref(), true)
        }
        other => (other, false),
    };
    let syn::Expr::Lit(syn::ExprLit {
        attrs,
        lit: syn::Lit::Float(value),
    }) = literal
    else {
        return None;
    };
    if !attrs.is_empty() || !matches!(value.suffix(), "" | "f32") {
        return None;
    }
    let frame = value.base10_parse::<f32>().ok()? * if negative { -1.0 } else { 1.0 };
    (frame.is_finite() && (0.0..=1_000_000.0).contains(&frame)).then_some(frame)
}

fn parse_native_motion_loop_sync_call(line: &str) -> bool {
    let Ok(syn::Stmt::Expr(syn::Expr::Call(call), Some(_))) = syn::parse_str(line) else {
        return false;
    };
    if !call.attrs.is_empty() || !is_native_timing_path(&call.func, "wait_loop_sync_mot") {
        return false;
    }
    // Captured presentation scripts use the bare implicit-context spelling. Only
    // that exact shorthand is normalized; qualified SDK calls must pass the fixed
    // agent Lua state. No alternate receiver or guessed argument list is admitted.
    if call.args.is_empty() {
        return matches!(call.func.as_ref(), syn::Expr::Path(path) if path.path.is_ident("wait_loop_sync_mot"));
    }
    if call.args.len() != 1 {
        return false;
    }
    let syn::Expr::Field(receiver) = &call.args[0] else {
        return false;
    };
    let syn::Expr::Path(agent) = receiver.base.as_ref() else {
        return false;
    };
    receiver.attrs.is_empty()
        && agent.attrs.is_empty()
        && agent.qself.is_none()
        && agent.path.is_ident("agent")
        && matches!(&receiver.member, syn::Member::Named(name) if name == "lua_state_agent")
}

fn parse_frame_call(line: &str) -> Option<f32> {
    let mut search_start = 0;
    while let Some(pos) = line[search_start..].find("frame(") {
        let abs_pos = search_start + pos;
        let before = if abs_pos == 0 {
            ' '
        } else {
            line.as_bytes()
                .get(abs_pos - 1)
                .copied()
                .map(|b| b as char)
                .unwrap_or(' ')
        };
        if !before.is_alphanumeric() && before != '_' {
            let inner = &line[abs_pos + 6..];
            let end = inner.find(')')?;
            let args: Vec<&str> = inner[..end].split(',').collect();
            if let Some(val) = args.get(1).and_then(|s| s.trim().parse::<f32>().ok()) {
                return Some(val);
            }
        }
        search_start = abs_pos + 6;
    }
    None
}

/// `macros::FT_MOTION_RATE(agent, 0.6);` → `0.6`.
///
/// **Matched with its opening parenthesis, never as a prefix.** `FT_MOTION_RATE_RANGE` begins
/// with this macro's entire name and takes a different argument list, so a
/// `contains("FT_MOTION_RATE")` would read one as the other and write back a call the game does
/// not have. That is the family-prefix collision this codebase has already paid for once with
/// `ATTACK`/`ATTACK_ABS`.
///
/// `FT_MOTION_RATE_RANGE` and `FT_DESIRED_RATE` have **zero** calls between them in the corpus,
/// so neither is modelled and both stay [`AcmdStmt::Raw`] — deliberately, because there is no
/// sample to check a parse of them against.
fn parse_motion_rate_call(line: &str) -> Option<f32> {
    let (_, rest) = line.split_once("FT_MOTION_RATE(")?;
    let (inner, _) = rest.split_once(')')?;
    let (agent, rate) = inner.split_once(',')?;
    // The first argument is the agent in every corpus call. Anything else is a shape this has
    // not seen, and guessing at it is how a wrong value reaches the export.
    (agent.trim() == "agent").then(|| rate.trim().parse::<f32>().ok())?
}

fn parse_wait_call(line: &str) -> Option<f32> {
    let mut search_start = 0;
    while let Some(pos) = line[search_start..].find("wait(") {
        let abs_pos = search_start + pos;
        let before = if abs_pos == 0 {
            ' '
        } else {
            line.as_bytes()
                .get(abs_pos - 1)
                .copied()
                .map(|b| b as char)
                .unwrap_or(' ')
        };
        if !before.is_alphanumeric() && before != '_' {
            let inner = &line[abs_pos + 5..];
            let end = inner.find(')')?;
            let args: Vec<&str> = inner[..end].split(',').collect();
            if let Some(val) = args.get(1).and_then(|s| s.trim().parse::<f32>().ok()) {
                return Some(val);
            }
        }
        search_start = abs_pos + 5;
    }
    None
}

/// The `ATTACK`-family macros the editor models, longest name first.
///
/// They share one argument layout, which is why they share one parser, one slot table, and
/// one live wire shape. Nothing else in ACMD may be added here on the strength of a similar
/// name — `ATTACK_ABS` is 16 arguments, not 36, and belongs to its own family.
pub const ATTACK_FUNCS: &[&str] = &["ATTACK_IGNORE_THROW", "ATTACK"];

/// Whether an `ATTACK`-family call carries the optional capsule triple at slots 13-15.
///
/// Decided by shape, not by length: `x2`/`y2`/`z2` are `Option<f32>` in smash-script, so a
/// call that has them spells all three as `None` or `Some(..)`. Anything else at slot 13 —
/// a bare `1.0` hitlag multiplier, in the case this was written for — means the triple was
/// left out and the rest of the call sits three slots earlier.
fn capsule_slots_present(t: &[String]) -> bool {
    let optionish = |s: &String| {
        let s = s.trim();
        s == "None" || s.starts_with("Some(")
    };
    t.len() >= 16 && t[13..16].iter().all(optionish)
}

fn parse_attack_call(line: &str) -> Option<AttackCall> {
    // Longest name first: `macros::ATTACK(` cannot match `macros::ATTACK_IGNORE_THROW(`
    // because of the paren, but ordering it this way keeps that from being load-bearing.
    let (func, start) = ATTACK_FUNCS
        .iter()
        .find_map(|name| Some((*name, line.find(&format!("macros::{name}("))?)))?;
    let inner = &line[start + "macros::".len() + func.len() + 1..];
    let end = inner.rfind(')')?;
    let inner = &inner[..end];
    let t = tokenize_args(inner);
    if t.len() < 13 {
        return None;
    }

    // [0]=agent [1]=id [2]=part [3]=bone [4]=damage [5]=angle [6]=kb_scaling
    // [7]=fkb [8]=kb_base [9]=size [10]=ox [11]=oy [12]=oz
    // [13]=cx2 [14]=cy2 [15]=cz2
    // [16]=hitlag_mult [17]=sdi_mult [18]=setoff_kind [19]=lr_check
    // [20]=is_clang [21]=is_add_attack [22]=hitbox_attr [23]=ground_or_air
    // [24]=is_mtk [25]=is_shield_disable [26]=is_reflectable [27]=is_absorbable
    // [28]=is_landing_attack
    // [29]=situation_mask [30]=category_mask [31]=part_mask
    // [32]=no_finish_camera [33]=collision_attr [34]=sound_level
    // [35]=sound_attr [36]=attack_region
    //
    // Slots 13-15 are OPTIONAL IN THE SOURCE TEXT. The vanilla archive writes every
    // `ATTACK` with them (all 386 in the local corpus are 36 arguments) but writes
    // `ATTACK_IGNORE_THROW` without them (33 arguments), even though smash-script declares
    // both with the same 36 parameters. Reading the shorter form against the full table put
    // hitlag in the capsule, `*ATTACK_LR_CHECK_POS` in hitlag, and shifted every property
    // after it by three. So the capsule triple is detected from the ARGUMENTS rather than
    // from the macro name, and everything past the transform shifts when it is absent.
    let shift = if capsule_slots_present(&t) { 0 } else { 3 };

    let id: u32 = t[1].trim().parse().ok()?;
    let part: u32 = t[2].trim().parse().ok()?;
    let bone_name = extract_hash40_string(&t[3]).unwrap_or_else(|| t[3].trim().to_string());
    let damage: f32 = t[4].trim().parse().ok()?;
    let angle: i32 = t[5]
        .trim()
        .parse::<i32>()
        .or_else(|_| t[5].trim().parse::<f32>().map(|f| f as i32))
        .unwrap_or(0);
    let kb_scaling: i32 = t[6].trim().parse().ok()?;
    let fkb: i32 = t[7].trim().parse().ok()?;
    let kb_base: i32 = t[8].trim().parse().ok()?;
    let size: f32 = t[9].trim().parse().ok()?;
    let offset_x: f32 = t[10].trim().parse().ok()?;
    let offset_y: f32 = t[11].trim().parse().ok()?;
    let offset_z: f32 = t[12].trim().parse().ok()?;

    let capsule_end = if shift == 0 {
        match (
            parse_option_f32(t[13].trim()),
            parse_option_f32(t[14].trim()),
            parse_option_f32(t[15].trim()),
        ) {
            (Some(x), Some(y), Some(z)) => Some([x, y, z]),
            _ => None,
        }
    } else {
        None
    };

    // Slots up to the transform are fixed; past it, a call without the capsule triple has
    // everything three earlier.
    let get = |i: usize| {
        t.get(if i >= 16 { i - shift } else { i })
            .map(|s| s.trim())
            .unwrap_or("")
    };

    let hitlag_mult: f32 = get(16).parse().unwrap_or(1.0);
    let sdi_mult: f32 = get(17).parse().unwrap_or(1.0);
    // Scripts normally spell these as `*NAME`, but dumps of live-captured moves can carry the
    // bare number — decode both to the symbolic name so the property dropdowns match.
    use crate::param_labels as pl;
    let setoff_kind = pl::decode_const(pl::SETOFF_KIND, get(18));
    let lr_check = pl::decode_const(pl::LR_CHECK, get(19));
    let is_clang = get(20) == "true";
    let is_add_attack: i32 = get(21).parse().unwrap_or(0);
    let hitbox_attr: f32 = get(22).parse().unwrap_or(0.0);
    let ground_or_air: i32 = get(23).parse().unwrap_or(0);
    let is_mtk = get(24) == "true";
    let is_shield_disable = get(25) == "true";
    let is_reflectable = get(26) == "true";
    let is_absorbable = get(27) == "true";
    let is_landing_attack = get(28) == "true";
    let situation_mask = pl::decode_const(pl::SITUATION_MASK, get(29));
    let category_mask = pl::decode_const(pl::CATEGORY_MASK, get(30));
    let part_mask = pl::decode_const(pl::PART_MASK, get(31));
    let no_finish_camera = get(32) == "true";
    let collision_attr = extract_hash40_string(get(33)).unwrap_or_else(|| strip_deref(get(33)));
    let sound_level = pl::decode_const(pl::SOUND_LEVEL, get(34));
    let sound_attr = pl::decode_const(pl::SOUND_ATTR, get(35));
    let attack_region = pl::decode_const(pl::ATTACK_REGION, get(36));

    Some(AttackCall {
        func: func.to_string(),
        id,
        part,
        bone_name,
        damage,
        angle,
        kb_scaling,
        fkb,
        kb_base,
        size,
        offset_x,
        offset_y,
        offset_z,
        capsule_end,
        hitlag_mult,
        sdi_mult,
        setoff_kind,
        lr_check,
        is_clang,
        is_add_attack,
        hitbox_attr,
        ground_or_air,
        is_mtk,
        is_shield_disable,
        is_reflectable,
        is_absorbable,
        is_landing_attack,
        situation_mask,
        category_mask,
        part_mask,
        no_finish_camera,
        collision_attr,
        sound_level,
        sound_attr,
        attack_region,
    })
}

/// Parse `macros::CATCH(agent, id, bone, size, x, y, z, x2, y2, z2, status, situation)`.
///
/// `CATCH` is a fixed-arity function — the capsule endpoints are `Option<f32>`, so a
/// spherical grab spells them `None` rather than omitting them. (The vanilla script dumps
/// show a shorter form, but those are Lua; the smashline macro has one signature.)
fn parse_catch_call(line: &str) -> Option<crate::data::CatchCall> {
    let start = line.find("macros::CATCH(")?;
    let inner = &line[start + "macros::CATCH(".len()..];
    let end = inner.rfind(')')?;
    let t = tokenize_args(&inner[..end]);
    if t.len() < 7 {
        return None;
    }

    // [0]=agent [1]=id [2]=bone [3]=size [4]=x [5]=y [6]=z
    // [7]=x2 [8]=y2 [9]=z2 [10]=status [11]=situation — but the Lua-shaped dumps omit 7..=9
    // outright, which moves status and situation up to 7 and 8. See [`is_capsule_slot`].
    let num = |i: usize, default: f32| {
        t.get(i)
            .and_then(|value| value.trim().parse::<f32>().ok())
            .unwrap_or(default)
    };
    let has_capsule_slots = t.get(7).is_some_and(|v| is_capsule_slot(v));
    let capsule_end = match (
        t.get(7).and_then(|v| parse_capsule_coord(v.trim())),
        t.get(8).and_then(|v| parse_capsule_coord(v.trim())),
        t.get(9).and_then(|v| parse_capsule_coord(v.trim())),
    ) {
        (Some(x), Some(y), Some(z)) if has_capsule_slots => Some([x, y, z]),
        _ => None,
    };
    let konst = |i: usize, default: &str| {
        t.get(i)
            .map(|v| strip_deref(v.trim()))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| default.to_string())
    };
    let tail = if has_capsule_slots { 10 } else { 7 };
    Some(crate::data::CatchCall {
        id: t[1].trim().parse().ok()?,
        bone_name: extract_hash40_string(&t[2]).unwrap_or_else(|| t[2].trim().to_string()),
        size: num(3, 1.0),
        offset_x: num(4, 0.0),
        offset_y: num(5, 0.0),
        offset_z: num(6, 0.0),
        capsule_end,
        status: konst(tail, crate::data::CATCH_DEFAULT_STATUS),
        situation: konst(tail + 1, crate::data::CATCH_DEFAULT_SITUATION),
    })
}

/// Parse `macros::SEARCH(agent, id, part, bone, size, x, y, z, x2, y2, z2, collision_kind,
/// hit_status, unk, ground_air, collision_category, collision_parts, unk2)`.
///
/// Dumped in the same two shapes as `CATCH` — the Lua-derived scripts omit the three capsule
/// arguments rather than writing `None` — so the tail is located with [`is_capsule_slot`]
/// rather than by slot number. 4 of the corpus's 7 calls are the short form.
fn parse_search_call(line: &str) -> Option<crate::data::SearchCall> {
    let start = line.find("macros::SEARCH(")?;
    let inner = &line[start + "macros::SEARCH(".len()..];
    let end = inner.rfind(')')?;
    let t = tokenize_args(&inner[..end]);
    if t.len() < 8 {
        return None;
    }

    // [0]=agent [1]=id [2]=part [3]=bone [4]=size [5]=x [6]=y [7]=z, then either
    // [8..=10]=capsule and the tail at 11, or the tail straight away at 8.
    let num = |i: usize, default: f32| {
        t.get(i)
            .and_then(|value| value.trim().parse::<f32>().ok())
            .unwrap_or(default)
    };
    let has_capsule_slots = t.get(8).is_some_and(|v| is_capsule_slot(v));
    let capsule_end = match (
        t.get(8).and_then(|v| parse_capsule_coord(v.trim())),
        t.get(9).and_then(|v| parse_capsule_coord(v.trim())),
        t.get(10).and_then(|v| parse_capsule_coord(v.trim())),
    ) {
        (Some(x), Some(y), Some(z)) if has_capsule_slots => Some([x, y, z]),
        _ => None,
    };
    let tail = if has_capsule_slots { 11 } else { 8 };
    let konst = |i: usize, default: &str| {
        t.get(i)
            .map(|v| strip_deref(v.trim()))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| default.to_string())
    };
    Some(crate::data::SearchCall {
        id: t[1].trim().parse().ok()?,
        part: t[2].trim().parse().ok()?,
        bone_name: extract_hash40_string(&t[3]).unwrap_or_else(|| t[3].trim().to_string()),
        size: num(4, 1.0),
        offset_x: num(5, 0.0),
        offset_y: num(6, 0.0),
        offset_z: num(7, 0.0),
        capsule_end,
        situation_mask: konst(tail + 3, "COLLISION_SITUATION_MASK_GA"),
        category_mask: konst(tail + 4, "COLLISION_CATEGORY_MASK_ALL"),
        part_mask: konst(tail + 5, "COLLISION_PART_MASK_ALL"),
        extras: crate::data::SearchExtras {
            collision_kind: konst(tail, crate::data::SEARCH_DEFAULT_COLLISION_KIND),
            hit_status: konst(tail + 1, crate::data::SEARCH_DEFAULT_HIT_STATUS),
            unk: t
                .get(tail + 2)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0),
            unk2: t
                .get(tail + 6)
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("true")),
        },
    })
}

fn parse_wind_call(line: &str) -> Option<crate::data::WindboxData> {
    for (command, arity) in crate::data::WIND_COMMANDS {
        let needle = format!("{command}(");
        let Some(start) = line.find(&needle) else {
            continue;
        };
        let inner = &line[start + needle.len()..];
        let end = inner.rfind(')')?;
        let tokens = tokenize_args(&inner[..end]);
        let values = if tokens.len() == arity + 1 {
            &tokens[1..]
        } else if tokens.len() == arity {
            &tokens[..]
        } else {
            continue;
        };
        let args: Option<Vec<f32>> = values
            .iter()
            .map(|value| value.trim().parse::<f32>().ok())
            .collect();
        return Some(crate::data::WindboxData {
            command: command.into(),
            args: args?,
        });
    }
    None
}

fn parse_erase_wind(line: &str) -> Option<u32> {
    let start = line.find("erase_wind(")? + "erase_wind(".len();
    let end = line[start..].rfind(')')? + start;
    tokenize_args(&line[start..end])
        .last()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// Parse `macros::ATTACK_ABS(...)`, or `None` if the call is not the shape this family takes.
///
/// One arity across the whole corpus — 16 arguments in all 32 calls, matching the declaration
/// in `macros.rs` exactly — so, per this file's rule for families whose arguments carry no
/// distinguishing shape, a call of any other length is refused rather than reinterpreted.
///
/// Slot order, which is NOT `ATTACK`'s: kind, id, damage, angle, kbg, fkb, bkb, hitlag, unk,
/// lr_check, unk2, unk3, collision_attr, sound_level, sound_attr, attack_region.
fn parse_attack_abs_call(line: &str) -> Option<crate::data::AttackAbsCall> {
    const NEEDLE: &str = "macros::ATTACK_ABS(";
    let start = line.find(NEEDLE)? + NEEDLE.len();
    let end = line[start..].rfind(')')? + start;
    let args = tokenize_args(&line[start..end]);
    // Leading `agent`, then the sixteen.
    let a = args.get(1..)?;
    if a.len() != 16 {
        return None;
    }
    let int = |i: usize| a[i].trim().parse::<i32>().ok();
    Some(crate::data::AttackAbsCall {
        // Carried verbatim, minus the deref. The corpus has a fighter-specific kind
        // (`FIGHTER_DOLLY_ATTACK_ABSOLUTE_KIND_FINAL`), so there is no closed set to map onto.
        kind: strip_deref(&a[0]),
        id: a[1].trim().parse::<u32>().ok()?,
        damage: a[2].trim().parse::<f32>().ok()?,
        angle: int(3)?,
        kb_scaling: int(4)?,
        fkb: int(5)?,
        kb_base: int(6)?,
        hitlag_mult: a[7].trim().parse::<f32>().ok()?,
        lr_check: strip_deref(&a[9]),
        collision_attr: extract_hash40_string(&a[12]).unwrap_or_else(|| a[12].trim().to_string()),
        sound_level: strip_deref(&a[13]),
        sound_attr: strip_deref(&a[14]),
        attack_region: strip_deref(&a[15]),
        unknowns: (
            a[8].trim().parse::<f32>().ok()?,
            a[10].trim().parse::<f32>().ok()?,
            a[11].trim().parse::<bool>().ok()?,
        ),
    })
}

/// Parse `macros::ATTACK_FP(...)`, whose 41 arguments after `agent` are not the ordinary
/// `ATTACK` layout. The family has no corpus oracle, so the slot count is enforced directly
/// against the `smash-script` declaration and every token is retained for lossless export.
fn parse_attack_fp_call(line: &str) -> Option<crate::data::AttackFpCall> {
    const NEEDLE: &str = "macros::ATTACK_FP(";
    let start = line.find(NEEDLE)? + NEEDLE.len();
    let end = line[start..].rfind(')')? + start;
    let args = tokenize_args(&line[start..end]);
    let args = args.get(1..)?;
    if args.len() != crate::data::ATTACK_FP_ARGC {
        return None;
    }
    Some(crate::data::AttackFpCall {
        args: args
            .iter()
            .map(|arg| crate::data::AttackFpArg::Source(arg.clone()))
            .collect(),
    })
}

/// Parse the source-preserved damage-reaction mode command used for super armor and related
/// conditions. Only the measured four-argument shape is typed; anything else remains `Raw` so
/// an unfamiliar damage command cannot lose tokens on export.
fn parse_damage_no_reaction_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let needle = "damage!(";
    let start = line.strip_prefix(needle).map(|_| needle.len())?;
    let end = line[start..].rfind(')')? + start;
    let args = tokenize_args(&line[start..end]);
    let [agent, command, mode, value] = args.as_slice() else {
        return None;
    };
    if agent.trim() != "agent"
        || command.trim().trim_start_matches('*') != "MA_MSC_DAMAGE_DAMAGE_NO_REACTION"
    {
        return None;
    }
    Some(ExcuteStmt::DamageNoReaction(
        crate::data::DamageNoReactionCall {
            command: command.trim().to_string(),
            mode: mode.trim().to_string(),
            value: value.trim().to_string(),
        },
    ))
}

/// Parse one hurtbox-state or colour-blend line, or `None` if this is not one.
///
/// Every member has exactly one arity in the vanilla archive — `HIT_NODE` 30 calls of
/// `(bone, status)`, `HIT_NO` 2 of `(group, status)`, `WHOLE_HIT` 6 of `(status)`, `COL_PRI` 2
/// of `(pri)`, and `COL_NORMAL` and `HIT_RESET_ALL` 8 and 3 of `()` — and each is declared with
/// that same arity in `smash-script`'s `macros.rs`. There is no shape to discriminate on, so per
/// this file's own rule the command name *is* the layout: a call whose argument count disagrees
/// with its name falls through to `Raw` rather than being reinterpreted.
fn parse_hurtbox_call(line: &str) -> Option<ExcuteStmt> {
    // `HIT_NODE` before `HIT_NO`: the shorter name is a prefix of the longer one, so testing
    // it first would read every `HIT_NODE` as a `HIT_NO` with an unparseable group.
    let args = |name: &str| -> Option<Vec<String>> {
        let needle = format!("macros::{name}(");
        let start = line.find(&needle)? + needle.len();
        let end = line[start..].rfind(')')? + start;
        let tokens = tokenize_args(&line[start..end]);
        // Drop the leading `agent`. A call written without one is not a form this parser has
        // ever seen and is left alone rather than guessed at.
        Some(tokens.get(1..)?.to_vec())
    };

    if line.contains("macros::HIT_NODE(") {
        let a = args("HIT_NODE")?;
        let [bone, status] = a.as_slice() else {
            return None;
        };
        return Some(ExcuteStmt::HitStatus {
            target: crate::data::HurtTarget::Bone(extract_hash40_value(bone)?),
            status: strip_deref(status),
        });
    }
    if line.contains("macros::HIT_NO(") {
        let a = args("HIT_NO")?;
        let [group, status] = a.as_slice() else {
            return None;
        };
        return Some(ExcuteStmt::HitStatus {
            target: crate::data::HurtTarget::Group(group.trim().parse::<i64>().ok()?),
            status: strip_deref(status),
        });
    }
    // `WHOLE_HIT` carries its status in the first slot, because its target is the macro name.
    // No prefix hazard: it shares no leading substring with the other four, and the `macros::`
    // prefix keeps it clear of `ATK_HIT_ABS` and the rest of B3's genuinely-hitbox family.
    if line.contains("macros::WHOLE_HIT(") {
        let a = args("WHOLE_HIT")?;
        let [status] = a.as_slice() else {
            return None;
        };
        return Some(ExcuteStmt::HitStatus {
            target: crate::data::HurtTarget::Whole,
            status: strip_deref(status),
        });
    }
    if line.contains("macros::COL_PRI(") {
        let a = args("COL_PRI")?;
        let [pri] = a.as_slice() else {
            return None;
        };
        return Some(ExcuteStmt::ColPri(pri.trim().parse::<i64>().ok()?));
    }
    // The two no-argument members. `args` returning an empty slice is the whole check: a
    // `COL_NORMAL` that somehow carries arguments is not this call.
    if line.contains("macros::COL_NORMAL(") && args("COL_NORMAL")?.is_empty() {
        return Some(ExcuteStmt::ColNormal);
    }
    if line.contains("macros::HIT_RESET_ALL(") && args("HIT_RESET_ALL")?.is_empty() {
        return Some(ExcuteStmt::HitResetAll);
    }
    None
}

/// Parse only the measured `slope!` direct and interpolation calls.
///
/// Both the command and status constants are closed over `skyline-smash`'s pinned
/// `lua_const.rs`; unfamiliar constants, argument counts, or receiver syntax remain `Raw`.
fn parse_slope_call(line: &str) -> Option<ExcuteStmt> {
    use crate::data::{SlopeCall, SlopeStatus, MAX_SLOPE_INTERPOLATION_FRAMES};

    let line = line.trim();
    let body = line.strip_prefix("slope!(")?.strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, command, status, rest @ ..] = args.as_slice() else {
        return None;
    };
    if receiver != "agent" {
        return None;
    }
    let status = match status.as_str() {
        "*SLOPE_STATUS_LR" => SlopeStatus::Lr,
        "*SLOPE_STATUS_TOP" => SlopeStatus::Top,
        "*SLOPE_STATUS_NONE" => SlopeStatus::None,
        "*SLOPE_STATUS_L" => SlopeStatus::Left,
        "*SLOPE_STATUS_R" => SlopeStatus::Right,
        _ => return None,
    };
    match (command.as_str(), rest) {
        ("*MA_MSC_CMD_SLOPE_SLOPE", []) => Some(ExcuteStmt::Slope(SlopeCall::Set { status })),
        ("*MA_MSC_CMD_SLOPE_SLOPE_INTP", [frames])
        | ("*MA_MSC_CMD_SLOPE_SLOPE_INTP", [frames, _]) => {
            let parsed_frames = frames.parse::<i32>().ok()?;
            if parsed_frames.to_string() != frames.as_str()
                || !(0..=MAX_SLOPE_INTERPOLATION_FRAMES).contains(&parsed_frames)
            {
                return None;
            }
            let trailing_flag = match rest.get(1).map(String::as_str) {
                None => None,
                Some("true") => Some(true),
                Some("false") => Some(false),
                Some(_) => return None,
            };
            Some(ExcuteStmt::Slope(SlopeCall::Interpolate {
                status,
                frames: parsed_frames,
                trailing_flag,
            }))
        }
        _ => None,
    }
}

/// Parse the sole measured item-visibility setter shape. Any other receiver, slot, literal,
/// argument count, or suffix remains `Raw` instead of opening a general ItemModule API.
fn parse_item_visibility_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("ItemModule::set_have_item_visibility(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, visible, slot] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || slot != "0" {
        return None;
    }
    let visible = match visible.as_str() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    Some(ExcuteStmt::ItemVisibility { visible })
}

/// Parse only the measured no-payload item-bullet wrapper. Near misses remain Raw.
fn parse_shoot_item_bullet_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("macros::SHOOT_ITEM_BULLET(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    (args.as_slice() == ["agent"]).then_some(ExcuteStmt::ShootItemBullet)
}

/// Parse only the exact pinned no-payload `START_INFO_FLASH_EYE` wrapper call.
fn parse_start_info_flash_eye_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("macros::START_INFO_FLASH_EYE(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    (args.as_slice() == ["agent"]).then_some(ExcuteStmt::StartInfoFlashEye)
}

/// Parse only the exact pinned no-payload `CAM_ZOOM_OUT` wrapper call.
fn parse_camera_zoom_out_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("macros::CAM_ZOOM_OUT(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    (args.as_slice() == ["agent"]).then_some(ExcuteStmt::CameraZoomOut)
}

/// Parse only the exact no-payload FT_START_CUTIN wrapper call.
fn parse_start_cut_in_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("macros::FT_START_CUTIN(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    (args.as_slice() == ["agent"]).then_some(ExcuteStmt::StartCutIn)
}

/// Parse only the exact helper call whose wrapper owns its Execute guard.
fn parse_capture_cut_common_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("macros::game_CaptureCutCommon(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    (args.as_slice() == ["agent"]).then_some(ExcuteStmt::CaptureCutCommon)
}

/// Parse only the observed, closed all-hitbox AttackModule boolean setters.
fn parse_attack_module_set_all_call(line: &str) -> Option<ExcuteStmt> {
    use crate::data::{AttackModuleSetAllKind, ExcuteStmt};

    let line = line.trim();
    let (prefix, kind) = if line.starts_with("AttackModule::set_no_dead_all(") {
        (
            "AttackModule::set_no_dead_all(",
            AttackModuleSetAllKind::NoDeadAll,
        )
    } else if line.starts_with("AttackModule::set_catch_only_all(") {
        (
            "AttackModule::set_catch_only_all(",
            AttackModuleSetAllKind::CatchOnlyAll,
        )
    } else {
        return None;
    };
    let body = line.strip_prefix(prefix)?.strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, arg2, arg3] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let arg2 = match arg2.as_str() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    let arg3 = match arg3.as_str() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    match kind {
        AttackModuleSetAllKind::NoDeadAll if arg2 => {}
        AttackModuleSetAllKind::CatchOnlyAll if arg2 && !arg3 => {}
        _ => return None,
    }
    Some(ExcuteStmt::AttackModuleSetAll { kind, arg2, arg3 })
}

/// Parse only the observed indexed AttackModule calls and their exact boolean tail.
fn parse_attack_module_set_indexed_call(line: &str) -> Option<ExcuteStmt> {
    use crate::data::{AttackModuleSetIndexedKind, ExcuteStmt};

    let line = line.trim();
    let (prefix, kind) = if line.starts_with("AttackModule::set_no_finish_camera(") {
        (
            "AttackModule::set_no_finish_camera(",
            AttackModuleSetIndexedKind::NoFinishCamera,
        )
    } else if line.starts_with("AttackModule::set_force_reaction(") {
        (
            "AttackModule::set_force_reaction(",
            AttackModuleSetIndexedKind::ForceReaction,
        )
    } else {
        return None;
    };
    let body = line.strip_prefix(prefix)?.strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, id_token, arg3, arg4] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || arg3 != "true" || arg4 != "false" {
        return None;
    }
    let id = id_token.parse::<i32>().ok()?;
    if !(0..=3).contains(&id) || id.to_string() != id_token.as_str() {
        return None;
    }
    Some(ExcuteStmt::AttackModuleSetIndexed {
        kind,
        id,
        arg3: true,
        arg4: false,
    })
}

/// Parse the closed AttackHeight selector and exact observed bool tail.
fn parse_attack_module_set_height_all_call(line: &str) -> Option<ExcuteStmt> {
    use crate::data::{AttackHeightAllKind, ExcuteStmt};

    let body = line
        .trim()
        .strip_prefix("AttackModule::set_attack_height_all(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, height_token, arg3] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || arg3 != "false" {
        return None;
    }
    let height = match height_token.as_str() {
        "AttackHeight(*ATTACK_HEIGHT_HIGH)" => AttackHeightAllKind::High,
        "AttackHeight(*ATTACK_HEIGHT_LOW)" => AttackHeightAllKind::Low,
        _ => return None,
    };
    Some(ExcuteStmt::AttackModuleSetHeightAll {
        height,
        arg3: false,
    })
}

/// Parse SDK-backed literal final-finish calls, retaining every native argument position.
fn parse_attack_module_set_final_finish_cut_in_call(line: &str) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("AttackModule::set_final_finish_cut_in(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, id_token, arg3, arg4, arg5, arg6] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let id = id_token.parse::<i32>().ok()?;
    let timing = arg5.parse::<f32>().ok()?;
    if id < 0 || id.to_string() != id_token.as_str() || !timing.is_finite() {
        return None;
    }
    Some(ExcuteStmt::AttackModuleSetFinalFinishCutIn {
        id,
        arg3: bool_token(arg3)?,
        arg4: bool_token(arg4)?,
        arg5: timing,
        arg6: bool_token(arg6)?,
    })
}

/// Parse only `AttackModule::init_attack_pos(agent.module_accessor, 0)`.
fn parse_attack_module_init_attack_pos_call(line: &str) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("AttackModule::init_attack_pos(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, id] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || id != "0" {
        return None;
    }
    Some(ExcuteStmt::AttackModuleInitAttackPos { id: 0 })
}

/// Parse one exact tuple from the measured set_attack_reference_joint_id corpus.
fn parse_attack_module_set_attack_reference_joint_id_call(
    line: &str,
    choices: &[crate::data::AttackReferenceJointKind],
) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("AttackModule::set_attack_reference_joint_id(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, bone, axis3, axis4, axis5] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let bone = bone.strip_prefix("Hash40::new(\"")?.strip_suffix("\")")?;
    fn axis(arg: &str) -> Option<&str> {
        arg.strip_prefix("AttackDirectionAxis(*")?.strip_suffix(')')
    }
    let actual = (bone, axis(axis3)?, axis(axis4)?, axis(axis5)?);
    let kind = choices
        .iter()
        .copied()
        .find(|kind| kind.native_parts() == actual)?;
    Some(ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind })
}

/// Parse only the exact observed native positional boolean pair.
fn parse_attack_module_set_no_damage_fly_smoke_all_call(line: &str) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("AttackModule::set_no_damage_fly_smoke_all(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, arg2, arg3] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || arg2 != "true" || arg3 != "false" {
        return None;
    }
    Some(ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll {
        arg2: true,
        arg3: false,
    })
}

/// Parse one direct, measured floating-point spelling. Unsuffixed integer tokens are left raw:
/// the pinned native binding takes `f32`, and converting an integer token would change source.
fn parse_attack_module_set_damage_shake_scale(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("AttackModule::set_damage_shake_scale(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, token] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    Some(ExcuteStmt::AttackModuleSetDamageShakeScale(
        crate::data::AttackModuleDamageShakeScale::from_source_token(token)?,
    ))
}

/// Parse only the two measured positional boolean pairs; keep their names neutral.
fn parse_attack_module_set_no_uniq_effect_all(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("AttackModule::set_no_uniq_effect_all(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, arg2, arg3] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    Some(ExcuteStmt::AttackModuleSetNoUniqEffectAll(
        crate::data::AttackModuleNoUniqEffectAllPair::from_source_args(arg2, arg3)?,
    ))
}

/// Parse only the exact measured receiver, attack IDs, and fixed NO_IF mask cast. The cast is
/// retained because the pinned binding takes `u32`; alternate masks and token spellings remain
/// source text rather than being normalized.
fn parse_attack_module_set_target_category(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("AttackModule::set_target_category(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, attack_id, mask] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || mask != "*COLLISION_CATEGORY_MASK_NO_IF as u32" {
        return None;
    }
    Some(ExcuteStmt::AttackModuleSetTargetCategory(
        crate::data::AttackModuleTargetCategoryId::from_attack_id(match attack_id.as_str() {
            "0" => 0,
            "1" => 1,
            "2" => 2,
            _ => return None,
        })?,
    ))
}

/// Parse only the exact measured `AttackModule::set_size` receiver/ID/decimal tuples.
/// The decimal token is matched lexically and never converted through `f32`.
fn parse_attack_module_set_size(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("AttackModule::set_size(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, attack_id, size] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    Some(ExcuteStmt::AttackModuleSetSize(
        crate::data::AttackModuleSizePreset::from_source_parts(attack_id, size)?,
    ))
}

/// Parse only a complete measured Vector2f tuple for AttackModule's pinned source API.
/// Vector3f spellings and every unmeasured token remain Raw rather than being converted.
fn parse_attack_module_set_vec_target_pos(line: &str) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("AttackModule::set_vec_target_pos(")?
        .strip_suffix(");")?;
    let args = tokenize_args_balanced(body);
    let [receiver, id, bone, vector, arg5, arg6] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || bone != "Hash40::new(\"top\")" || arg6 != "false" {
        return None;
    }
    let vector = vector.strip_prefix("&Vector2f{")?.strip_suffix('}')?;
    let fields = tokenize_args_balanced(vector);
    let [x_field, y_field] = fields.as_slice() else {
        return None;
    };
    let x = x_field.strip_prefix("x:")?.trim();
    let y = y_field.strip_prefix("y:")?.trim();
    Some(ExcuteStmt::AttackModuleSetVecTargetPos(
        crate::data::AttackModuleVecTargetPosPreset::from_source_parts(id, x, y, arg5)?,
    ))
}

/// Parse the one measured global HitModule reset tuple; other statuses and tails stay Raw.
fn parse_hit_module_set_status_all(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("HitModule::set_status_all(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, status, tail] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor"
        || status != "HitStatus(*HIT_STATUS_NORMAL)"
        || tail != "0"
    {
        return None;
    }
    Some(ExcuteStmt::HitModuleSetStatusAllNormal)
}

/// Parse the two distinct measured reaction-frame APIs without exchanging their names.
/// The receiver and final native flag are fixed and not exposed as editable fields.
fn parse_attack_module_add_reaction_frame_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let (body, revised) =
        if let Some(body) = line.strip_prefix("AttackModule::set_add_reaction_frame(") {
            (body, false)
        } else {
            (
                line.strip_prefix("AttackModule::set_add_reaction_frame_revised(")?,
                true,
            )
        };
    let body = body.strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, id_token, frames_token, tail] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || tail != "false" {
        return None;
    }
    let id = id_token.parse::<u32>().ok()?;
    if id > 8 || id.to_string() != id_token.as_str() {
        return None;
    }
    let frames = parse_finite_float_literal(frames_token)?;
    if num(frames).as_str() != frames_token.as_str() {
        return None;
    }
    Some(if revised {
        ExcuteStmt::AttackModuleAddReactionFrameRevised { id, frames }
    } else {
        ExcuteStmt::AttackModuleAddReactionFrame { id, frames }
    })
}

fn parse_finite_float_literal(token: &str) -> Option<f32> {
    let expression = syn::parse_str::<syn::Expr>(token).ok()?;
    let (literal, negative) = match expression {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Float(literal),
            ..
        }) => (literal, false),
        syn::Expr::Unary(syn::ExprUnary {
            op: syn::UnOp::Neg(_),
            expr,
            ..
        }) => match *expr {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Float(literal),
                ..
            }) => (literal, true),
            _ => return None,
        },
        _ => return None,
    };
    if !literal.suffix().is_empty() {
        return None;
    }
    let value = literal.base10_parse::<f32>().ok()?;
    let value = if negative { -value } else { value };
    value.is_finite().then_some(value)
}

/// Parse only the exact observed GrabModule receiver and literal bool argument.
fn parse_grab_rebound_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("GrabModule::set_rebound(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, enabled] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let enabled = match enabled.as_str() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    Some(ExcuteStmt::GrabRebound { enabled })
}

/// Parse only the complete measured JostleModule setter shape.
fn parse_jostle_set_status_call(line: &str) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("JostleModule::set_status(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, enabled] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let enabled = match enabled.as_str() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    Some(ExcuteStmt::JostleSetStatus { enabled })
}

/// Parse only the exact standalone direct Game forms with literal numeric arguments.
/// Other method paths, receivers, arities, and trailing source remain preserved as Raw.
fn parse_fixed_jostle_area_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let (body, xy_form) = if let Some(body) = line
        .strip_prefix("FighterAreaModuleImpl::enable_fix_jostle_area_xy(")
        .and_then(|body| body.strip_suffix(");"))
    {
        (body, true)
    } else if let Some(body) = line
        .strip_prefix("FighterAreaModuleImpl::enable_fix_jostle_area(")
        .and_then(|body| body.strip_suffix(");"))
    {
        (body, false)
    } else {
        return None;
    };

    let args = tokenize_args(body);
    let expected_len = if xy_form { 5 } else { 3 };
    if args.len() != expected_len || args[0] != "agent.module_accessor" {
        return None;
    }
    let values = args[1..]
        .iter()
        .map(|token| {
            // Keep this exact literal surface closed even though syn can accept expression
            // attributes or comments around a float. Neither is part of the measured calls.
            if !token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || b".-+eE".contains(&byte))
            {
                return None;
            }
            let value = parse_finite_float_literal(token)?;
            (value.abs() <= crate::data::MAX_FIXED_JOSTLE_AREA_ARGUMENT).then_some(value)
        })
        .collect::<Option<Vec<_>>>()?;
    if xy_form {
        Some(ExcuteStmt::FixedJostleArea(
            crate::data::FixedJostleAreaCall::EnableFixXy {
                arg2: values[0],
                arg3: values[1],
                arg4: values[2],
                arg5: values[3],
            },
        ))
    } else {
        Some(ExcuteStmt::FixedJostleArea(
            crate::data::FixedJostleAreaCall::EnableFix {
                arg2: values[0],
                arg3: values[1],
            },
        ))
    }
}

/// Parse the measured closed VisibilityModule hash-name setter shape. Both identifiers must be
/// literal `hash40` strings with the explicit `as i64` casts used by the source corpus.
fn parse_visibility_set_int64_call(line: &str) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("VisibilityModule::set_int64(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, group, status] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let group = parse_hash40_i64_literal(group)?;
    let status = parse_hash40_i64_literal(status)?;
    if !crate::data::body_visibility_catalog::is_measured_pair(&group, &status) {
        return None;
    }
    Some(ExcuteStmt::VisibilitySetInt64 { group, status })
}

/// Parse the SDK-backed `set_whole` bool form only as an exact direct source statement.
fn parse_visibility_set_whole_call(line: &str) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("VisibilityModule::set_whole(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, visible] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let visible = match visible.as_str() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    Some(ExcuteStmt::VisibilitySetWhole { visible })
}

/// Parse only the canonical generated form for the authoring-only model-default group token.
/// Original corpus `Hash40::new(...)` calls intentionally remain Raw and immutable.
fn parse_visibility_set_default_int64_call(line: &str) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("VisibilityModule::set_default_int64(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, group] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let group = parse_hash40_i64_literal(group)?;
    if !crate::data::PINNED_DEFAULT_INT64_GROUP_COUNTS
        .iter()
        .any(|(_, measured_group, _)| *measured_group == group.as_str())
    {
        return None;
    }
    Some(ExcuteStmt::VisibilitySetDefaultInt64 { group })
}

fn parse_hash40_i64_literal(token: &str) -> Option<String> {
    let name = token
        .strip_prefix("hash40(\"")?
        .strip_suffix("\") as i64")?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return None;
    }
    Some(name.to_string())
}

/// Parse only the measured fixed-argument ArticleModule generation/removal forms.
///
/// Registry ownership is intentionally checked later with the concrete fighter; this parser
/// only recognizes the native syntax and a bare named `FIGHTER_*_GENERATE_ARTICLE_*` token.
/// Unsupported receivers, expressions, targets, and argument values remain Raw.
fn parse_article_status_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let (body, if_present) =
        if let Some(body) = line.strip_prefix("ArticleModule::change_status_exist(") {
            (body, true)
        } else {
            (line.strip_prefix("ArticleModule::change_status(")?, false)
        };
    let args = tokenize_args(body.strip_suffix(");")?);
    let (receiver, article, status) = match (if_present, args.as_slice()) {
        (true, [receiver, article, status]) => (receiver, article, status),
        (false, [receiver, article, status, target])
            if target == "ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL)" =>
        {
            (receiver, article, status)
        }
        _ => return None,
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let article = article_symbol(article)?;
    let status = status.strip_prefix('*')?;
    if !crate::data::article_status_catalog::is_measured_call(&article, status, if_present) {
        return None;
    }
    Some(ExcuteStmt::ArticleLifecycle(
        crate::data::ArticleLifecycleCall::ChangeStatus {
            symbol: article,
            status: status.into(),
            if_present,
        },
    ))
}

fn parse_enabled_article_call(line: &str) -> Option<ExcuteStmt> {
    let args = tokenize_args(
        line.trim()
            .strip_prefix("ArticleModule::generate_article_enable(")?
            .strip_suffix(");")?,
    );
    let [receiver, symbol, arg3, arg4] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || arg3 != "false" || arg4 != "-1" {
        return None;
    }
    let symbol = article_symbol(symbol)?;
    if !crate::data::PINNED_ENABLED_ARTICLE_COUNTS
        .iter()
        .any(|(_, measured_symbol, _)| *measured_symbol == symbol)
    {
        return None;
    }
    Some(ExcuteStmt::ArticleLifecycle(
        crate::data::ArticleLifecycleCall::GenerateEnabled { symbol },
    ))
}

fn parse_article_lifecycle_call(line: &str) -> Option<ExcuteStmt> {
    use crate::data::ArticleLifecycleCall;

    let line = line.trim();
    let (prefix, variant) = if line.starts_with("ArticleModule::generate_article(") {
        ("ArticleModule::generate_article(", 0)
    } else if line.starts_with("ArticleModule::remove_exist(") {
        ("ArticleModule::remove_exist(", 1)
    } else {
        return None;
    };
    let body = line.strip_prefix(prefix)?.strip_suffix(");")?;
    let args = tokenize_args(body);
    let symbol = match (variant, args.as_slice()) {
        (0, [receiver, symbol, enabled, index])
            if receiver == "agent.module_accessor" && enabled == "false" && index == "-1" =>
        {
            article_symbol(symbol)?
        }
        (1, [receiver, symbol, target])
            if receiver == "agent.module_accessor"
                && target == "ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL)" =>
        {
            article_symbol(symbol)?
        }
        _ => return None,
    };
    let call = match variant {
        0 => ArticleLifecycleCall::Generate { symbol },
        1 => ArticleLifecycleCall::RemoveAll { symbol },
        _ => unreachable!(),
    };
    Some(ExcuteStmt::ArticleLifecycle(call))
}

/// Parse a track's closed `ArticleModule::change_motion` source form.
/// The caller retains the original Game gate and proves direct-root Expression placement.
fn parse_article_change_motion_call(line: &str, domain: ParseDomain) -> Option<ExcuteStmt> {
    let line = line.trim();
    let body = line
        .strip_prefix("ArticleModule::change_motion(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, symbol, motion, arg4, frame] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || arg4 != "false" || frame != "-1.0" {
        return None;
    }
    let symbol = article_symbol(symbol)?;
    let motion = match domain {
        ParseDomain::Game => extract_hash40_string(motion)?,
        ParseDomain::Expression => motion
            .strip_prefix("Hash40::new(\"")?
            .strip_suffix("\")")?
            .to_owned(),
        ParseDomain::Sound => return None,
    };
    let choices = match domain {
        ParseDomain::Game => crate::data::measured_article_motions(&symbol),
        ParseDomain::Expression => crate::data::measured_expression_article_motions(&symbol),
        ParseDomain::Sound => return None,
    };
    if !choices.contains(&motion.as_str()) {
        return None;
    }
    Some(ExcuteStmt::ArticleLifecycle(
        crate::data::ArticleLifecycleCall::ChangeMotion { symbol, motion },
    ))
}

/// Parse only the measured whole-article visibility and launch calls.
///
/// Game sources admit these only in directly replayable Execute blocks. The pinned corpus also
/// contains whole-article visibility in Expression and one `shoot_exist` call there. These
/// operations keep their exact receiver, article symbol, target and false tail; all other Rust
/// expressions remain Raw for source preservation.
fn parse_article_native_call(line: &str) -> Option<ExcuteStmt> {
    use crate::data::{ArticleLifecycleCall, ArticleTarget};

    let line = line.trim();
    if let Some(body) = line
        .strip_prefix("ArticleModule::set_visibility_whole(")
        .and_then(|body| body.strip_suffix(");"))
    {
        let args = tokenize_args(body);
        let [receiver, symbol, visible, target] = args.as_slice() else {
            return None;
        };
        if receiver != "agent.module_accessor"
            || target != "ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL)"
        {
            return None;
        }
        let visible = match visible.as_str() {
            "true" => true,
            "false" => false,
            _ => return None,
        };
        return Some(ExcuteStmt::ArticleLifecycle(
            ArticleLifecycleCall::SetVisibilityWhole {
                symbol: article_symbol(symbol)?,
                visible,
            },
        ));
    }

    let (body, shoot_exist) = if let Some(body) = line
        .strip_prefix("ArticleModule::shoot(")
        .and_then(|body| body.strip_suffix(");"))
    {
        (body, false)
    } else if let Some(body) = line
        .strip_prefix("ArticleModule::shoot_exist(")
        .and_then(|body| body.strip_suffix(");"))
    {
        (body, true)
    } else {
        return None;
    };
    let args = tokenize_args(body);
    let [receiver, symbol, target, tail] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" || tail != "false" {
        return None;
    }
    let target = match target.as_str() {
        "*ARTICLE_OPE_TARGET_ALL" | "ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL)" => {
            ArticleTarget::All
        }
        "*ARTICLE_OPE_TARGET_LAST" | "ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST)" => {
            ArticleTarget::Last
        }
        _ => return None,
    };
    let symbol = article_symbol(symbol)?;
    Some(ExcuteStmt::ArticleLifecycle(if shoot_exist {
        ArticleLifecycleCall::ShootExist { symbol, target }
    } else {
        ArticleLifecycleCall::Shoot { symbol, target }
    }))
}

fn article_symbol(token: &str) -> Option<String> {
    let symbol = token.strip_prefix("*FIGHTER_")?;
    if token.len() > crate::data::MAX_ARTICLE_SYMBOL_BYTES
        || symbol.is_empty()
        || !symbol
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    let symbol = format!("FIGHTER_{symbol}");
    symbol.contains("_GENERATE_ARTICLE_").then_some(symbol)
}

/// Parse only the exact named-event receiver call generated by the slight bridge. This is
/// source syntax, not a producer: unrecognized receivers, arguments, or Rust expressions stay
/// Raw, and callers gate this parser to direct Game execute blocks.
pub(crate) fn parse_gimmick_event_call(line: &str) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("crate::slight::gimmicks::dispatch_named_from_module(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, name] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let name = syn::parse_str::<syn::LitStr>(name).ok()?.value();
    if name.trim().is_empty()
        || name.len() > crate::gimmicks::live_model::MAX_NAME_BYTES
        || name.contains('\0')
    {
        return None;
    }
    Some(ExcuteStmt::GimmickEvent { name })
}

/// Parse only the exact chart-bearing, zero-payload event bridge. The chart ID
/// is not normalized: padded, reserved-main, NUL-containing, or over-limit IDs
/// remain source text and are rejected by project validation if authored as a
/// typed command.
pub(crate) fn parse_gimmick_chart_event_call(line: &str) -> Option<ExcuteStmt> {
    let body = line
        .trim()
        .strip_prefix("crate::slight::gimmicks::dispatch_named_chart_from_module(")?
        .strip_suffix(");")?;
    let args = tokenize_args(body);
    let [receiver, chart_id, name] = args.as_slice() else {
        return None;
    };
    if receiver != "agent.module_accessor" {
        return None;
    }
    let chart_id = syn::parse_str::<syn::LitStr>(chart_id).ok()?.value();
    let name = syn::parse_str::<syn::LitStr>(name).ok()?.value();
    if !valid_gimmick_chart_id(&chart_id)
        || name.trim().is_empty()
        || name.len() > crate::gimmicks::live_model::MAX_NAME_BYTES
        || name.contains('\0')
    {
        return None;
    }
    Some(ExcuteStmt::GimmickChartEvent { chart_id, name })
}

/// Parse the exact constant-payload chart bridge. The syntax is deliberately closed: no aliases,
/// computed values, casts, alternate receivers, or empty payloads are retained as typed source.
pub(crate) fn parse_gimmick_chart_event_values_call(line: &str) -> Option<ExcuteStmt> {
    let source = line.trim().strip_suffix(';')?;
    let call = syn::parse_str::<syn::ExprCall>(source).ok()?;
    parse_gimmick_chart_event_values_expr(&call)
}

/// AST entry point shared by source parsing and the adapter capability gate.
pub(crate) fn parse_gimmick_chart_event_values_expr(call: &syn::ExprCall) -> Option<ExcuteStmt> {
    use crate::gimmicks::behavior::ConstValue;
    use crate::gimmicks::event_payload::{validate_argument_shape, Argument};
    use syn::{Expr, ExprLit, ExprTuple, Lit};

    fn exact_path(expr: &Expr, expected: &[&str]) -> bool {
        let Expr::Path(path) = expr else { return false };
        path.attrs.is_empty()
            && path.qself.is_none()
            && path.path.leading_colon.is_none()
            && path.path.segments.len() == expected.len()
            && path
                .path
                .segments
                .iter()
                .zip(expected)
                .all(|(segment, expected)| {
                    segment.ident == *expected
                        && matches!(&segment.arguments, syn::PathArguments::None)
                })
    }

    fn exact_receiver(expr: &Expr) -> bool {
        let Expr::Field(field) = expr else {
            return false;
        };
        field.attrs.is_empty()
            && matches!(&field.member, syn::Member::Named(member) if member == "module_accessor")
            && exact_path(&field.base, &["agent"])
    }

    fn signed_literal(expr: &Expr, float: bool) -> Option<(bool, &syn::Lit)> {
        match expr {
            Expr::Lit(ExprLit { attrs, lit })
                if attrs.is_empty() && !float && matches!(lit, Lit::Int(_)) =>
            {
                Some((false, lit))
            }
            Expr::Lit(ExprLit { attrs, lit })
                if attrs.is_empty() && float && matches!(lit, Lit::Float(_)) =>
            {
                Some((false, lit))
            }
            Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Neg(_)) => {
                if !unary.attrs.is_empty() {
                    return None;
                }
                let Expr::Lit(ExprLit { attrs, lit }) = unary.expr.as_ref() else {
                    return None;
                };
                if attrs.is_empty()
                    && ((!float && matches!(lit, Lit::Int(_)))
                        || (float && matches!(lit, Lit::Float(_))))
                {
                    Some((true, lit))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn parse_value(expr: &Expr) -> Option<ConstValue> {
        use syn::Lit;
        let Expr::Call(call) = expr else { return None };
        if !call.attrs.is_empty() || call.args.len() != 1 {
            return None;
        }
        let value = call.args.first()?;
        if exact_path(
            &call.func,
            &["crate", "slight", "gimmicks", "ChartEventValue", "Int"],
        ) {
            let (negative, lit) = signed_literal(value, false)?;
            let Lit::Int(integer) = lit else { return None };
            if !matches!(integer.suffix(), "" | "i32") {
                return None;
            }
            let magnitude = integer.base10_parse::<i64>().ok()?;
            let signed = if negative {
                magnitude.checked_neg()?
            } else {
                magnitude
            };
            let value = i32::try_from(signed).ok()?;
            return Some(ConstValue::Int(value));
        }
        if exact_path(
            &call.func,
            &["crate", "slight", "gimmicks", "ChartEventValue", "Float"],
        ) {
            let (negative, lit) = signed_literal(value, true)?;
            let Lit::Float(float) = lit else { return None };
            if !matches!(float.suffix(), "" | "f32") {
                return None;
            }
            let mut value = float.base10_parse::<f32>().ok()?;
            if negative {
                value = -value;
            }
            return value.is_finite().then_some(ConstValue::Float(value));
        }
        if exact_path(
            &call.func,
            &["crate", "slight", "gimmicks", "ChartEventValue", "Flag"],
        ) {
            let Expr::Lit(ExprLit {
                attrs,
                lit: Lit::Bool(value),
            }) = value
            else {
                return None;
            };
            if !attrs.is_empty() {
                return None;
            }
            return Some(ConstValue::Flag(value.value));
        }
        None
    }

    if !call.attrs.is_empty()
        || !exact_path(
            &call.func,
            &[
                "crate",
                "slight",
                "gimmicks",
                "dispatch_named_chart_values_from_module",
            ],
        )
    {
        return None;
    }
    if call.args.len() != 4 {
        return None;
    }
    let receiver = &call.args[0];
    let chart_id = &call.args[1];
    let name = &call.args[2];
    let payload = &call.args[3];
    if !exact_receiver(receiver) {
        return None;
    }
    let literal_string = |expr: &Expr| match expr {
        Expr::Lit(ExprLit {
            attrs,
            lit: Lit::Str(value),
        }) if attrs.is_empty() => Some(value.value()),
        _ => None,
    };
    let chart_id = literal_string(chart_id)?;
    let name = literal_string(name)?;
    if !valid_gimmick_chart_id(&chart_id)
        || name.trim().is_empty()
        || name.len() > crate::gimmicks::live_model::MAX_NAME_BYTES
        || name.contains('\0')
    {
        return None;
    }
    let Expr::Reference(reference) = payload else {
        return None;
    };
    if !reference.attrs.is_empty() || reference.mutability.is_some() {
        return None;
    }
    let Expr::Array(array) = reference.expr.as_ref() else {
        return None;
    };
    if !array.attrs.is_empty()
        || array.elems.is_empty()
        || array.elems.len() > crate::gimmicks::event_payload::MAX_FIELDS_PER_EVENT
    {
        return None;
    }
    let mut arguments = Vec::with_capacity(array.elems.len());
    for element in &array.elems {
        let Expr::Tuple(ExprTuple { attrs, elems, .. }) = element else {
            return None;
        };
        if !attrs.is_empty() {
            return None;
        }
        if elems.len() != 2 {
            return None;
        }
        let field = &elems[0];
        let value = &elems[1];
        let field = literal_string(field)?;
        let value = parse_value(value)?;
        arguments.push(Argument { field, value });
    }
    validate_argument_shape("source", &name, &arguments).ok()?;
    Some(ExcuteStmt::GimmickChartEventValues {
        chart_id,
        name,
        arguments,
    })
}

fn valid_gimmick_chart_id(chart_id: &str) -> bool {
    !chart_id.trim().is_empty()
        && chart_id != crate::gimmicks::controller_project::MAIN_CHART
        && chart_id.len() <= crate::gimmicks::controller_protocol::MAX_CHART_ID_BYTES
        && !chart_id.as_bytes().contains(&0)
}

/// Recognize exact named-event sends only at direct, unattributed statement boundaries in one
/// Game Execute body. This is an event-specific parser helper; it grants no native write or
/// other command-family permission.
pub(crate) fn unattributed_gimmick_event_calls(
    lines: &[&str],
) -> std::collections::BTreeMap<usize, ExcuteStmt> {
    direct_native_statement_lines(lines)
        .into_iter()
        .filter_map(|index| {
            let (line, _) = logical_line_at(lines, index);
            parse_gimmick_event_call(&line)
                .or_else(|| parse_gimmick_chart_event_call(&line))
                .or_else(|| parse_gimmick_chart_event_values_call(&line))
                .map(|event| (index, event))
        })
        .collect()
}

/// Parse `ATK_POWER` / `ATK_SET_SHIELD_SETOFF_MUL`, or `None` if this is not one.
///
/// Both are `(agent, id: u64, value: ToF32)` in `smash-script`'s `macros.rs`, which is what
/// places them together: `lua_const` has no `MA_MSC_CMD_*` constant for either, and the corpus
/// alone could not have said which slot is which — all 9 `ATK_SET_SHIELD_SETOFF_MUL` calls are
/// the identical `(agent, 0, 7)`. `ATK_POWER`'s two calls then confirm it from the other side,
/// writing `(agent, 0, 10)` and `(agent, 1, 10)` for two hitboxes that share a value.
///
/// Kept out of [`parse_hurtbox_call`] because these are a different family that happens to be
/// parsed nearby, and a call written at the wrong arity falls through to `Raw` by the same rule.
/// The `PLAY_SE` family: macro name, how many `Hash40` arguments it takes, and whether one
/// more argument follows them.
///
/// Arities are read off `smash-script`'s `macros.rs`, not inferred from the call sites — a
/// member whose corpus calls all happen to be the same length would otherwise pin the wrong
/// signature. `SET_PLAY_INHIVIT` is here rather than with the effect-lifetime commands for
/// that reason: its second argument is a `ToF32` suppression window, and all ten of its corpus
/// calls are in `sound_` functions.
///
/// Matched with the trailing paren, never on the bare name. `PLAY_SE` is a prefix of
/// `PLAY_SE_NO_3D` and `PLAY_SE_REMAIN`, and `PLAY_STEP` of `PLAY_STEP_FLIPPABLE`; a bare
/// `contains` would read a two-hash call through the one-hash layout and drop its second
/// sound. This is the same collision `ATTACK` and `ATTACK_ABS` have, and the paren is the
/// same fix.
pub(crate) const SOUND_FUNCS: &[(&str, usize, bool)] = &[
    ("PLAY_SE", 1, false),
    ("PLAY_SE_NO_3D", 1, false),
    ("PLAY_SE_REMAIN", 1, false),
    ("STOP_SE", 1, false),
    ("PLAY_STEP", 1, false),
    ("PLAY_STEP_FLIPPABLE", 2, false),
    ("PLAY_SEQUENCE", 1, false),
    ("PLAY_STATUS", 1, false),
    ("PLAY_LANDING_SE", 1, false),
    ("PLAY_DOWN_SE", 1, false),
    ("PLAY_FLY_VOICE", 2, false),
    ("SET_PLAY_INHIVIT", 1, true),
];

/// Read one `PLAY_SE`-family call, or `None` to leave the line as `Raw`.
///
/// Refuses anything it could not write back byte for byte: a hash argument that is not a
/// literal `Hash40::new("…")`, or an argument count the signature does not have. Every one of
/// the corpus's 610 calls passes a literal, but a hand-written project is free to pass a
/// variable, and a typed call is *regenerated* on export rather than copied — so a form this
/// emitter cannot spell has to stay verbatim text instead.
fn parse_sound_call(line: &str) -> Option<ExcuteStmt> {
    let (func, hashes, has_tail) = SOUND_FUNCS
        .iter()
        .copied()
        .find(|(name, _, _)| line.contains(&format!("macros::{name}(")))?;
    let needle = format!("macros::{func}(");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    // Drop the leading `agent`, then require exactly the signature's arguments — no more, so
    // an argument this parser has no field for cannot vanish on the way back out.
    let args = tokens.get(1..)?;
    if args.len() != hashes + usize::from(has_tail) {
        return None;
    }
    let sounds: Vec<String> = args[..hashes]
        .iter()
        .map(|arg| extract_hash40_string(arg))
        .collect::<Option<_>>()?;
    Some(ExcuteStmt::Sound(crate::data::SoundCall {
        func: func.to_string(),
        sounds,
        tail: has_tail.then(|| args[hashes].clone()),
    }))
}

/// Parse the measured direct `ControlModule::set_rumble` shape.
///
/// The standard public corpus uses `agent.module_accessor`; the HDR source form uses `boma`.
/// The native payload is retained as source tokens, but duration and looped are required to be
/// the measured integer/boolean spellings so the editor can map edits back to the native hook.
/// Wrong arity and other expressions remain raw rather than being regenerated through a guess.
fn parse_direct_expression_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "ControlModule::set_rumble(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args_balanced(&line[start..end]);
    let [receiver, kind, duration, looped, target] = tokens.as_slice() else {
        return None;
    };
    if !matches!(receiver.trim(), "agent.module_accessor" | "boma")
        || kind.trim().is_empty()
        || duration.trim().parse::<i32>().is_err()
        || !matches!(looped.trim(), "true" | "false")
        || target.trim().is_empty()
    {
        return None;
    }
    Some(ExcuteStmt::Expression(
        crate::data::ExpressionCall::ControlModuleSetRumble {
            receiver: receiver.clone(),
            kind: kind.clone(),
            duration: duration.clone(),
            looped: looped.clone(),
            target: target.clone(),
        },
    ))
}

/// Read one of the measured expression calls, or leave the line as `Raw`.
///
/// The expression family has several similarly named raw module calls beside these macros, so
/// the exact `macros::NAME(` spelling and exact arity are both required. Arguments stay as
/// tokens: source constants and live numeric captures must each survive an export.
fn parse_expression_call(line: &str) -> Option<ExcuteStmt> {
    if let Some(call) = parse_direct_expression_call(line) {
        return Some(call);
    }
    let args = |name: &str| -> Option<Vec<String>> {
        let needle = format!("macros::{name}(");
        let start = line.find(&needle)? + needle.len();
        let end = line[start..].rfind(')')? + start;
        let tokens = tokenize_args(&line[start..end]);
        tokens.get(1..).map(ToOwned::to_owned)
    };

    if line.contains("macros::RUMBLE_HIT(") {
        let values = args("RUMBLE_HIT")?;
        let [kind, unk] = values.as_slice() else {
            return None;
        };
        return Some(ExcuteStmt::Expression(
            crate::data::ExpressionCall::RumbleHit {
                kind: kind.clone(),
                unk: unk.clone(),
            },
        ));
    }
    if line.contains("macros::QUAKE(") {
        let values = args("QUAKE")?;
        let [kind] = values.as_slice() else {
            return None;
        };
        return Some(ExcuteStmt::Expression(crate::data::ExpressionCall::Quake {
            kind: kind.clone(),
        }));
    }
    if line.contains("macros::FT_ATTACK_ABS_CAMERA_QUAKE(") {
        let values = args("FT_ATTACK_ABS_CAMERA_QUAKE")?;
        let [attack_abs_kind, quake_kind] = values.as_slice() else {
            return None;
        };
        return Some(ExcuteStmt::Expression(
            crate::data::ExpressionCall::FtAttackAbsCameraQuake {
                attack_abs_kind: attack_abs_kind.clone(),
                quake_kind: quake_kind.clone(),
            },
        ));
    }
    None
}

/// Parse the argument-less `macros::REVERSE_LR(agent)` facing-direction command.
fn parse_reverse_lr_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "macros::REVERSE_LR(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    (tokens.as_slice() == ["agent"]).then_some(ExcuteStmt::ReverseLr)
}

/// Parse the buildable `SET_SPEED_EX(agent, x, y, kinetic_kind)` shape.
///
/// The local smash-script wrapper takes exactly three arguments after `agent`. Dumped files also
/// contain short and over-arity artifacts; those intentionally remain `Raw` so export never
/// manufactures a call that the checked-in wrapper cannot compile.
fn parse_set_speed_ex_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "macros::SET_SPEED_EX(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [agent, speed_x, speed_y, kinetic_kind] = tokens.as_slice() else {
        return None;
    };
    if agent.trim() != "agent" {
        return None;
    }
    Some(ExcuteStmt::SetSpeedEx(crate::data::SetSpeedExCall {
        speed_x: speed_x.trim().parse::<f32>().ok()?,
        speed_y: speed_y.trim().parse::<f32>().ok()?,
        kinetic_kind: kinetic_kind.trim().to_string(),
    }))
}

/// Parse the measured `SET_SPEED(agent, x, y)` source shape, or the exact helper call emitted
/// when a generated project uses the linked primitive directly.
fn parse_set_speed_call(line: &str) -> Option<ExcuteStmt> {
    let needle = if line.contains("macros::SET_SPEED(") {
        "macros::SET_SPEED("
    } else if line.contains("visionary_set_speed(") {
        "visionary_set_speed("
    } else {
        return None;
    };
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [agent, speed_x, speed_y] = tokens.as_slice() else {
        return None;
    };
    if agent.trim() != "agent" {
        return None;
    }
    Some(ExcuteStmt::SetSpeed(crate::data::SetSpeedCall {
        speed_x: speed_x.trim().parse::<f32>().ok()?,
        speed_y: speed_y.trim().parse::<f32>().ok()?,
    }))
}

/// Parse the buildable `ADD_SPEED_NO_LIMIT(agent, x, y)` shape.
fn parse_add_speed_no_limit_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "macros::ADD_SPEED_NO_LIMIT(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [agent, speed_x, speed_y] = tokens.as_slice() else {
        return None;
    };
    if agent.trim() != "agent" {
        return None;
    }
    Some(ExcuteStmt::AddSpeedNoLimit(
        crate::data::AddSpeedNoLimitCall {
            speed_x: speed_x.trim().parse::<f32>().ok()?,
            speed_y: speed_y.trim().parse::<f32>().ok()?,
        },
    ))
}

/// Parse the buildable `CORRECT(agent, kind)` shape, retaining the kind token verbatim.
fn parse_correct_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "macros::CORRECT(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [agent, kind] = tokens.as_slice() else {
        return None;
    };
    if agent.trim() != "agent" || kind.trim().is_empty() {
        return None;
    }
    Some(ExcuteStmt::Correct(crate::data::CorrectCall {
        kind: kind.trim().to_string(),
    }))
}

/// Parse the measured `FT_CATCH_STOP(agent, arg1, arg2)` shape.
///
/// Both payload slots are `ToF32` in the linked wrapper. The exact arity is required so a
/// malformed or newer dump line remains `Raw` and is not regenerated with missing arguments.
fn parse_ft_catch_stop_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "macros::FT_CATCH_STOP(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [agent, arg1, arg2] = tokens.as_slice() else {
        return None;
    };
    if agent.trim() != "agent" {
        return None;
    }
    Some(ExcuteStmt::FtCatchStop(crate::data::FtCatchStopCall {
        arg1: arg1.trim().parse::<f32>().ok()?,
        arg2: arg2.trim().parse::<f32>().ok()?,
    }))
}

/// Parse the measured `FT_START_ADJUST_MOTION_FRAME_arg1(agent, value)` shape.
///
/// The linked wrapper accepts one `f32`, so the exact arity and agent token are required. Related
/// revised or unqualified dump artifacts stay `Raw` rather than being emitted as a guessed call.
fn parse_ft_start_adjust_motion_frame_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "macros::FT_START_ADJUST_MOTION_FRAME_arg1(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [agent, value] = tokens.as_slice() else {
        return None;
    };
    if agent.trim() != "agent" {
        return None;
    }
    Some(ExcuteStmt::FtStartAdjustMotionFrame(
        crate::data::FtStartAdjustMotionFrameCall {
            value: value.trim().parse::<f32>().ok()?,
        },
    ))
}

/// Parse the measured direct `MotionModule::set_rate` shapes:
/// `MotionModule::set_rate(agent.module_accessor, rate)` and the HDR `boma` form.
/// The public corpus contains non-negative numeric rates; negative values in a few dump lines
/// are Lua-stack/decompiler artifacts and remain raw rather than being presented as editable
/// playback rates.
fn parse_motion_module_set_rate_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "MotionModule::set_rate(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [module_accessor, rate] = tokens.as_slice() else {
        return None;
    };
    if !matches!(module_accessor.trim(), "agent.module_accessor" | "boma") {
        return None;
    }
    let rate = rate.trim().parse::<f32>().ok()?;
    (rate.is_finite() && rate >= 0.0).then_some(ExcuteStmt::MotionModuleSetRate(
        crate::data::MotionModuleSetRateCall { rate },
    ))
}

/// Parse the measured direct `MotionModule::set_helper_calculation` shapes:
/// `MotionModule::set_helper_calculation(agent.module_accessor, bool)` and the HDR `boma` form.
/// Only boolean literals are accepted; symbolic or malformed values stay raw rather than being
/// emitted with a guessed runtime meaning.
fn parse_motion_module_set_helper_calculation_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "MotionModule::set_helper_calculation(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [module_accessor, enabled] = tokens.as_slice() else {
        return None;
    };
    if !matches!(module_accessor.trim(), "agent.module_accessor" | "boma") {
        return None;
    }
    let enabled = match enabled.trim() {
        "false" => false,
        "true" => true,
        _ => return None,
    };
    Some(ExcuteStmt::MotionModuleSetHelperCalculation(
        crate::data::MotionModuleSetHelperCalculationCall { enabled },
    ))
}

/// Accept the measured authored part-kind spellings: a dereferenced lua constant or a numeric
/// value from a captured/generated script. Arbitrary expressions remain raw until their runtime
/// identity is measured.
fn parse_motion_module_part_kind(token: &str) -> Option<String> {
    let token = token.trim();
    if token.parse::<i64>().is_ok() {
        return Some(token.to_string());
    }
    let name = token.strip_prefix('*')?;
    (!name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_'))
    .then_some(token.to_string())
}

/// Parse the measured direct `MotionModule::set_rate_partial` shapes:
/// `MotionModule::set_rate_partial(agent.module_accessor, part_kind, rate)` and the HDR `boma`
/// form. The part kind stays as its authored token; only the numeric rate is a value edit.
fn parse_motion_module_set_rate_partial_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "MotionModule::set_rate_partial(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [module_accessor, part_kind, rate] = tokens.as_slice() else {
        return None;
    };
    if !matches!(module_accessor.trim(), "agent.module_accessor" | "boma") {
        return None;
    }
    let part_kind = parse_motion_module_part_kind(part_kind)?;
    let rate = rate.trim().parse::<f32>().ok()?;
    (rate.is_finite() && rate >= 0.0).then_some(ExcuteStmt::MotionModuleSetRatePartial(
        crate::data::MotionModuleSetRatePartialCall { part_kind, rate },
    ))
}

/// Parse the measured direct `MotionModule::set_frame_partial` shapes:
/// `MotionModule::set_frame_partial(agent.module_accessor, part_kind, frame)` and the HDR `boma`
/// form, plus the native four-argument spelling that carries `sync` explicitly.
///
/// The corpus writes only the three-argument form. That is not a truncated call: the
/// version-matched Lua reader counts its arguments and supplies `sync = true` when the third is
/// absent, so the omission is retained as `sync: None` and re-emitted without the argument.
/// Only the seek frame and the authored `sync` are value edits; the part kind stays source-owned.
fn parse_motion_module_set_frame_partial_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "MotionModule::set_frame_partial(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let (module_accessor, part_kind, frame, sync) = match tokens.as_slice() {
        [module_accessor, part_kind, frame] => (module_accessor, part_kind, frame, None),
        [module_accessor, part_kind, frame, sync] => match sync.trim() {
            "true" => (module_accessor, part_kind, frame, Some(true)),
            "false" => (module_accessor, part_kind, frame, Some(false)),
            _ => return None,
        },
        _ => return None,
    };
    if !matches!(module_accessor.trim(), "agent.module_accessor" | "boma") {
        return None;
    }
    let part_kind = parse_motion_module_part_kind(part_kind)?;
    let frame = frame.trim().parse::<f32>().ok()?;
    (frame.is_finite() && frame >= 0.0).then_some(ExcuteStmt::MotionModuleSetFramePartial(
        crate::data::MotionModuleSetFramePartialCall {
            part_kind,
            frame,
            sync,
        },
    ))
}

/// Parse the exact measured `CLR_SPEED(agent, kinetic_id)` shape or its generated helper call.
/// The vendored crate exposes only the generic kinetic primitive macro, so generated source uses
/// the helper while editable source retains the corpus's `macros::CLR_SPEED` spelling.
fn parse_clr_speed_call(line: &str) -> Option<ExcuteStmt> {
    let needle = if line.contains("macros::CLR_SPEED(") {
        "macros::CLR_SPEED("
    } else if line.contains("visionary_clr_speed(") {
        "visionary_clr_speed("
    } else {
        return None;
    };
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [agent, kinetic_kind] = tokens.as_slice() else {
        return None;
    };
    if agent.trim() != "agent" || kinetic_kind.trim().is_empty() {
        return None;
    }
    Some(ExcuteStmt::ClrSpeed(crate::data::ClrSpeedCall {
        kinetic_kind: kinetic_kind.trim().to_string(),
    }))
}

/// Parse the exact measured argument-less `SET_AIR(agent)` shape or its generated helper call.
fn parse_set_air_call(line: &str) -> Option<ExcuteStmt> {
    let needle = if line.contains("macros::SET_AIR(") {
        "macros::SET_AIR("
    } else if line.contains("visionary_set_air(") {
        "visionary_set_air("
    } else {
        return None;
    };
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    (tokens.as_slice() == ["agent"]).then_some(ExcuteStmt::SetAir)
}

/// Parse the measured direct `KineticModule::clear_speed_all` shapes:
/// `KineticModule::clear_speed_all(agent.module_accessor)` and the HDR `boma` form.
/// Other receivers and arities remain `Raw` because this input boundary does not prove that the
/// call belongs to a buildable ACMD game function.
fn parse_kinetic_clear_speed_all_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "KineticModule::clear_speed_all(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [module_accessor] = tokens.as_slice() else {
        return None;
    };
    matches!(module_accessor.trim(), "agent.module_accessor" | "boma")
        .then_some(ExcuteStmt::KineticClearSpeedAll)
}

/// Parse the measured direct `KineticModule::set_consider_ground_friction` shapes:
/// `KineticModule::set_consider_ground_friction(agent.module_accessor, bool, attribute)` and
/// the HDR `boma` receiver form. The bool is decoded because it is a real authored toggle; the
/// reserve attribute remains source text so named lua constants survive a round trip.
fn parse_kinetic_set_consider_ground_friction_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "KineticModule::set_consider_ground_friction(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [module_accessor, consider_ground_friction, kinetic_energy_attribute] = tokens.as_slice()
    else {
        return None;
    };
    if !matches!(module_accessor.trim(), "agent.module_accessor" | "boma") {
        return None;
    }
    let consider_ground_friction = match consider_ground_friction.trim() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    if kinetic_energy_attribute.trim().is_empty() {
        return None;
    }
    Some(ExcuteStmt::KineticSetConsiderGroundFriction(
        crate::data::KineticSetConsiderGroundFrictionCall {
            consider_ground_friction,
            kinetic_energy_attribute: kinetic_energy_attribute.trim().to_string(),
        },
    ))
}

/// Parse the measured direct lua-bind shapes:
/// `KineticModule::change_kinetic(agent.module_accessor, kinetic_type)` and the HDR dump form
/// `KineticModule::change_kinetic(boma, kinetic_type)`, where the surrounding function declares
/// `let boma = agent.boma()`.
/// Qualified generated forms are accepted by the same suffix match; other receivers and arities
/// remain raw because they may belong to status code rather than this ACMD input boundary.
fn parse_change_kinetic_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "KineticModule::change_kinetic(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [module_accessor, kinetic_type] = tokens.as_slice() else {
        return None;
    };
    if !matches!(module_accessor.trim(), "agent.module_accessor" | "boma")
        || kinetic_type.trim().is_empty()
    {
        return None;
    }
    Some(ExcuteStmt::ChangeKinetic(crate::data::ChangeKineticCall {
        kinetic_type: kinetic_type.trim().to_string(),
    }))
}

/// Parse the measured direct kinetic-energy shapes:
/// `KineticModule::{suspend,resume,enable,unable}_energy(agent.module_accessor, *ENERGY_ID)`.
/// HDR uses `boma` for the receiver. Other receivers, arities, and empty authored IDs remain
/// `Raw` because this input boundary does not prove their meaning.
fn parse_kinetic_energy_call(line: &str) -> Option<ExcuteStmt> {
    for (func, action) in [
        (
            "KineticModule::suspend_energy",
            crate::data::KineticEnergyAction::Suspend,
        ),
        (
            "KineticModule::resume_energy",
            crate::data::KineticEnergyAction::Resume,
        ),
        (
            "KineticModule::enable_energy",
            crate::data::KineticEnergyAction::Enable,
        ),
        (
            "KineticModule::unable_energy",
            crate::data::KineticEnergyAction::Unable,
        ),
    ] {
        let needle = format!("{func}(");
        let Some(start) = line.find(&needle).map(|index| index + needle.len()) else {
            continue;
        };
        let end = line[start..].rfind(')')? + start;
        let tokens = tokenize_args(&line[start..end]);
        let [module_accessor, kinetic_energy_id] = tokens.as_slice() else {
            continue;
        };
        if !matches!(module_accessor.trim(), "agent.module_accessor" | "boma")
            || kinetic_energy_id.trim().is_empty()
        {
            continue;
        }
        return Some(ExcuteStmt::KineticEnergy(crate::data::KineticEnergyCall {
            action,
            kinetic_energy_id: kinetic_energy_id.trim().to_string(),
        }));
    }
    None
}

/// Parse the measured direct lua-bind vector shape:
/// `KineticModule::add_speed(agent.module_accessor, &Vector3f{x: ..., y: ..., z: 0.0})`.
/// HDR uses `boma` for the receiver. The source corpus has a numeric zero z component; a
/// non-zero or symbolic third component remains `Raw` until the editor has a measured z field.
fn parse_kinetic_add_speed_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "KineticModule::add_speed(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args_balanced(&line[start..end]);
    let [module_accessor, vector] = tokens.as_slice() else {
        return None;
    };
    if !matches!(module_accessor.trim(), "agent.module_accessor" | "boma") {
        return None;
    }
    let (speed_x, speed_y) = parse_zero_z_vector(vector)?;
    Some(ExcuteStmt::KineticAddSpeed(
        crate::data::KineticAddSpeedCall { speed_x, speed_y },
    ))
}

/// Parse the measured direct `WorkModule::on_flag` / `off_flag` shapes:
/// `WorkModule::{on,off}_flag(agent.module_accessor, flag)` and the HDR `boma` form.
/// Only a numeric or dereferenced identifier flag token is typed; other expressions and arities
/// remain raw until their source/runtime identity is measured.
fn parse_work_flag_call(line: &str) -> Option<ExcuteStmt> {
    for (func, action) in [
        ("WorkModule::on_flag", crate::data::WorkFlagAction::On),
        ("WorkModule::off_flag", crate::data::WorkFlagAction::Off),
    ] {
        let needle = format!("{func}(");
        let Some(start) = line.find(&needle).map(|index| index + needle.len()) else {
            continue;
        };
        let end = line[start..].rfind(')')? + start;
        let tokens = tokenize_args(&line[start..end]);
        let [module_accessor, flag] = tokens.as_slice() else {
            continue;
        };
        if !matches!(module_accessor.trim(), "agent.module_accessor" | "boma") {
            continue;
        }
        let flag = flag.trim();
        let valid_flag = flag.parse::<i64>().is_ok()
            || flag.strip_prefix('*').is_some_and(|name| {
                !name.is_empty()
                    && name
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '_')
            });
        if valid_flag {
            return Some(ExcuteStmt::WorkFlag(crate::data::WorkFlagCall {
                action,
                flag: flag.to_string(),
            }));
        }
    }
    None
}

/// Parse the measured direct WorkModule transition-term and transition-term-group shapes. The
/// ordinary term pair retains its exact `agent.module_accessor` or `boma` receiver; group calls
/// are typed only with `agent.module_accessor`. Only a numeric or dereferenced identifier token
/// is typed; other expressions and arities remain raw until their source/runtime identity is
/// measured.
fn parse_work_transition_term_call(line: &str) -> Option<ExcuteStmt> {
    // This source-export slice deliberately admits one direct statement only. Unlike the
    // permissive parsers around it, searching within the line would accidentally type calls
    // embedded in expressions, comments, or a shadow-qualified `WorkModule` path.
    let line = line.trim();
    for (func, action) in [
        (
            "WorkModule::enable_transition_term",
            crate::data::WorkTransitionTermAction::Enable,
        ),
        (
            "WorkModule::unable_transition_term",
            crate::data::WorkTransitionTermAction::Unable,
        ),
        (
            "WorkModule::enable_transition_term_group",
            crate::data::WorkTransitionTermAction::EnableGroup,
        ),
        (
            "WorkModule::unable_transition_term_group_ex",
            crate::data::WorkTransitionTermAction::UnableGroupEx,
        ),
    ] {
        let needle = format!("{func}(");
        let Some(args) = line
            .strip_prefix(&needle)
            .and_then(|tail| tail.strip_suffix(");"))
        else {
            continue;
        };
        // The closed arguments contain only the measured receiver and an integer/dereferenced
        // identifier. Reject lexical constructs before tokenization so comments, strings,
        // nested expressions, and malformed tails cannot be mistaken for a direct call.
        if !args.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '.' | '*' | ',' | '-' | ' ' | '\t')
        }) {
            continue;
        }
        let tokens = tokenize_args(args);
        let [module_accessor, transition_term] = tokens.as_slice() else {
            continue;
        };
        let receiver = match module_accessor.trim() {
            "agent.module_accessor" => crate::data::WorkTransitionTermReceiver::AgentAccessor,
            "boma"
                if matches!(
                    action,
                    crate::data::WorkTransitionTermAction::Enable
                        | crate::data::WorkTransitionTermAction::Unable
                ) =>
            {
                crate::data::WorkTransitionTermReceiver::Boma
            }
            _ => continue,
        };
        let transition_term = transition_term.trim();
        let valid_transition_term = transition_term.parse::<i64>().is_ok()
            || transition_term.strip_prefix('*').is_some_and(|name| {
                !name.is_empty()
                    && name
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '_')
            });
        if valid_transition_term {
            return Some(ExcuteStmt::WorkTransitionTerm(
                crate::data::WorkTransitionTermCall {
                    action,
                    receiver,
                    transition_term: transition_term.to_string(),
                },
            ));
        }
    }
    None
}

/// Parse the measured direct `WorkModule::inc_int(agent.module_accessor, slot)` shape and its
/// HDR `boma` equivalent. Only a numeric or dereferenced identifier slot is typed.
fn parse_work_module_inc_int_call(line: &str) -> Option<ExcuteStmt> {
    let needle = "WorkModule::inc_int(";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].rfind(')')? + start;
    let tokens = tokenize_args(&line[start..end]);
    let [receiver, slot] = tokens.as_slice() else {
        return None;
    };
    if !matches!(receiver.trim(), "agent.module_accessor" | "boma") {
        return None;
    }
    let slot = slot.trim();
    if !valid_work_module_legacy_token(slot) {
        return None;
    }
    Some(ExcuteStmt::WorkModuleIncInt(
        crate::data::WorkModuleIncIntCall {
            slot: slot.to_string(),
        },
    ))
}

fn valid_work_module_identifier(value: &str) -> bool {
    let mut chars = value.trim().chars();
    chars.next().is_some_and(|character| {
        (character.is_ascii_alphabetic() || character == '_')
            && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

fn valid_work_module_slot_token(value: &str) -> bool {
    let value = value.trim();
    value.parse::<i64>().is_ok()
        || value
            .strip_prefix('*')
            .is_some_and(valid_work_module_identifier)
        || valid_work_module_identifier(value)
}

fn valid_work_module_legacy_token(value: &str) -> bool {
    let value = value.trim();
    value.parse::<i64>().is_ok()
        || value
            .strip_prefix('*')
            .is_some_and(valid_work_module_identifier)
}

fn valid_work_module_int64_value(value: &str) -> bool {
    let value = value.trim();
    value.parse::<i64>().is_ok()
        || valid_work_module_slot_token(value)
        || value
            .strip_prefix("hash40(\"")
            .and_then(|value| value.strip_suffix("\") as i64"))
            .is_some_and(|value| !value.is_empty() && !value.contains('"'))
}

/// Parse the measured direct WorkModule value-set shapes:
/// `WorkModule::set_int(agent.module_accessor, value, slot)`, the equivalent HDR `boma`
/// receiver, and the three-argument `set_int64` shape measured in the standard/HDR corpus.
fn parse_work_module_set_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    for (func, kind) in [
        ("WorkModule::set_int", crate::data::WorkModuleSetKind::Int),
        (
            "WorkModule::set_float",
            crate::data::WorkModuleSetKind::Float,
        ),
        (
            "WorkModule::set_int64",
            crate::data::WorkModuleSetKind::Int64,
        ),
    ] {
        let needle = format!("{func}(");
        let Some(arguments) = line
            .strip_prefix(&needle)
            .and_then(|tail| tail.strip_suffix(");"))
        else {
            continue;
        };
        if arguments.trim_end().ends_with(',') {
            continue;
        }
        let tokens = tokenize_args(arguments);
        let [module_accessor, value, slot] = tokens.as_slice() else {
            continue;
        };
        let receiver = module_accessor.trim();
        if !matches!(receiver, "agent.module_accessor" | "boma") {
            continue;
        }
        let value = value.trim();
        let valid_value = if kind.is_float() {
            value.parse::<f32>().is_ok_and(f32::is_finite)
        } else if kind.is_int64() {
            valid_work_module_int64_value(value)
        } else {
            valid_work_module_legacy_token(value)
        };
        let slot = slot.trim();
        let valid_slot = if kind.is_int64() {
            valid_work_module_slot_token(slot)
        } else {
            valid_work_module_legacy_token(slot)
        };
        if valid_value && valid_slot {
            return Some(ExcuteStmt::WorkModuleSet(crate::data::WorkModuleSetCall {
                kind,
                receiver: receiver.to_string(),
                value: value.to_string(),
                slot: slot.to_string(),
            }));
        }
    }
    None
}

/// Parse only the pinned direct `agent.module_accessor` slowdown call shapes. Prefix/suffix
/// checks keep nested, shadowed, appended, and otherwise decorated expressions as raw source.
fn parse_slow_module_whole_call(line: &str) -> Option<ExcuteStmt> {
    use crate::data::{ExcuteStmt, SlowWholeCall};

    let line = line.trim();
    let set_prefix = "SlowModule::set_whole(";
    if let Some(arguments) = line
        .strip_prefix(set_prefix)
        .and_then(|tail| tail.strip_suffix(");"))
    {
        if arguments.trim_end().ends_with(',') {
            return None;
        }
        let tokens = tokenize_args(arguments);
        let [receiver, multiplier, frames] = tokens.as_slice() else {
            return None;
        };
        if receiver.trim() != "agent.module_accessor" {
            return None;
        }
        let multiplier_token = multiplier.trim();
        let multiplier = multiplier_token.parse::<u8>().ok()?;
        if multiplier_token != multiplier.to_string() {
            return None;
        }
        let frames_token = frames.trim();
        let frames = frames_token.parse::<i32>().ok()?;
        if frames_token != frames.to_string() {
            return None;
        }
        return Some(ExcuteStmt::SlowWhole(SlowWholeCall::Set {
            multiplier,
            frames,
        }));
    }

    let clear_prefix = "SlowModule::clear_whole(";
    let arguments = line
        .strip_prefix(clear_prefix)
        .and_then(|tail| tail.strip_suffix(");"))?;
    if arguments.trim_end().ends_with(',') {
        return None;
    }
    let tokens = tokenize_args(arguments);
    let [receiver] = tokens.as_slice() else {
        return None;
    };
    (receiver.trim() == "agent.module_accessor")
        .then_some(ExcuteStmt::SlowWhole(SlowWholeCall::Clear))
}

/// Parse the closed wrapper call using its source-level f32 fields. Each bound is an observed
/// corpus extremum, not a claimed native unit or semantic limit.
fn parse_slow_opponent_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let arguments = line
        .strip_prefix("macros::SLOW_OPPONENT(")?
        .strip_suffix(");")?;
    if arguments.trim_end().ends_with(',') {
        return None;
    }
    let tokens = tokenize_args(arguments);
    let [receiver, slow_mul, slow_time] = tokens.as_slice() else {
        return None;
    };
    if receiver.trim() != "agent" {
        return None;
    }
    let call = crate::data::SlowOpponentCall {
        slow_mul: slow_mul.trim().parse::<f32>().ok()?,
        slow_time: slow_time.trim().parse::<f32>().ok()?,
    };
    call.is_valid().then_some(ExcuteStmt::SlowOpponent(call))
}

/// Parse an exact, corpus-measured two-literal source tuple. Fighter ownership is checked by
/// timeline admission, where the donor identity is available; generic ACMD parsing has no such
/// context and therefore recognizes only membership in the global literal catalog.
fn parse_finish_camera_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let arguments = line
        .strip_prefix("macros::CHECK_FINISH_CAMERA(")?
        .strip_suffix(");")?;
    if arguments.trim_end().ends_with(',') {
        return None;
    }
    let tokens = tokenize_args_balanced(arguments);
    let [receiver, arg1, arg2] = tokens.as_slice() else {
        return None;
    };
    if receiver.trim() != "agent" {
        return None;
    }
    let call = crate::data::FinishCameraCall {
        arg1: arg1.trim().to_owned(),
        arg2: arg2.trim().to_owned(),
    };
    call.is_measured_tuple()
        .then_some(ExcuteStmt::CheckFinishCamera(call))
}

/// Parse a single exact six-token tuple from the corpus-derived camera catalog. Donor identity
/// is checked later by timeline admission; this generic parser has no fighter argument.
fn parse_final_start_camera_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let arguments = line
        .strip_prefix("macros::CHECK_VALID_FINAL_START_CAMERA(")?
        .strip_suffix(");")?;
    if arguments.trim_end().ends_with(',') {
        return None;
    }
    let tokens = tokenize_args_balanced(arguments);
    let [receiver, arg1, arg2, arg3, arg4, arg5, arg6] = tokens.as_slice() else {
        return None;
    };
    if receiver.trim() != "agent" {
        return None;
    }
    let call = crate::data::FinalStartCameraCall {
        args: [
            arg1.trim().to_owned(),
            arg2.trim().to_owned(),
            arg3.trim().to_owned(),
            arg4.trim().to_owned(),
            arg5.trim().to_owned(),
            arg6.trim().to_owned(),
        ],
    };
    call.is_measured_tuple()
        .then_some(ExcuteStmt::CheckFinalStartCamera(call))
}

/// Parse one exact integer token from the closed fear-face source catalog. The pinned wrapper's
/// `u64` argument is intentionally retained without assigning it a semantic label.
fn parse_final_fear_face_call(line: &str) -> Option<ExcuteStmt> {
    let line = line.trim();
    let arguments = line
        .strip_prefix("macros::FT_SET_FINAL_FEAR_FACE(")?
        .strip_suffix(");")?;
    if arguments.trim_end().ends_with(',') {
        return None;
    }
    let tokens = tokenize_args_balanced(arguments);
    let [receiver, argument] = tokens.as_slice() else {
        return None;
    };
    if receiver.trim() != "agent" {
        return None;
    }
    let call = crate::data::FinalFearFaceCall {
        argument: argument.trim().to_owned(),
    };
    call.is_measured_token()
        .then_some(ExcuteStmt::FinalFearFace(call))
}

/// Parse one direct source-only optional hit-feedback call with value-only Hash40 arguments.
/// The integer remains `arg2`; this boundary does not infer its native semantic label.
fn parse_optional_hit_feedback_call(line: &str) -> Option<ExcuteStmt> {
    use crate::data::{ExcuteStmt, OptionalHitFeedbackCall, OptionalHitFeedbackKind};

    let line = line.trim();
    let (prefix, kind) = if line.starts_with("AttackModule::set_optional_hit_effect(") {
        (
            "AttackModule::set_optional_hit_effect(",
            OptionalHitFeedbackKind::Effect,
        )
    } else if line.starts_with("AttackModule::set_optional_hit_sound(") {
        (
            "AttackModule::set_optional_hit_sound(",
            OptionalHitFeedbackKind::Sound,
        )
    } else {
        return None;
    };
    let arguments = line.strip_prefix(prefix)?.strip_suffix(");")?;
    let tokens = tokenize_args_balanced(arguments);
    let [receiver, arg2, hash] = tokens.as_slice() else {
        return None;
    };
    if receiver.trim() != "agent.module_accessor" {
        return None;
    }
    let arg2 = arg2.trim().parse::<i32>().ok()?;
    let hash = parse_hash40_literal(hash)?;
    let call = OptionalHitFeedbackCall { kind, arg2, hash };
    call.is_valid()
        .then_some(ExcuteStmt::OptionalHitFeedback(call))
}

/// Accept only `Hash40::new(<string literal>)` and `Hash40::new_raw(<hex integer literal>)`.
/// No path, constructor, constant, or expression is retained as executable authored source.
fn parse_hash40_literal(token: &str) -> Option<crate::data::Hash40Literal> {
    let expression = syn::parse_str::<syn::Expr>(token.trim()).ok()?;
    parse_hash40_literal_expr(expression)
}

fn parse_hash40_literal_expr(expression: syn::Expr) -> Option<crate::data::Hash40Literal> {
    use crate::data::Hash40Literal;
    use syn::{Expr, ExprCall, ExprLit, ExprPath, Lit};

    let Expr::Call(ExprCall {
        attrs, func, args, ..
    }) = expression
    else {
        return None;
    };
    if !attrs.is_empty() {
        return None;
    }
    let Expr::Path(ExprPath {
        attrs,
        qself: None,
        path,
        ..
    }) = *func
    else {
        return None;
    };
    if !attrs.is_empty()
        || path.leading_colon.is_some()
        || path.segments.len() != 2
        || path.segments[0].ident != "Hash40"
        || path
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
    {
        return None;
    }
    if args.len() != 1 {
        return None;
    }
    let argument = args.first()?;
    let value = match path.segments[1].ident.to_string().as_str() {
        "new" => {
            let Expr::Lit(ExprLit {
                attrs,
                lit: Lit::Str(value),
                ..
            }) = argument
            else {
                return None;
            };
            if !attrs.is_empty() {
                return None;
            }
            Hash40Literal::Name(value.value())
        }
        "new_raw" => {
            let Expr::Lit(ExprLit {
                attrs,
                lit: Lit::Int(value),
                ..
            }) = argument
            else {
                return None;
            };
            if !attrs.is_empty() {
                return None;
            }
            if !value.suffix().is_empty() {
                return None;
            }
            let source = value.to_string().replace('_', "");
            let digits = source
                .strip_prefix("0x")
                .or_else(|| source.strip_prefix("0X"))?;
            Hash40Literal::Raw(u64::from_str_radix(digits, 16).ok()?)
        }
        _ => return None,
    };
    value.is_valid().then_some(value)
}

fn parse_zero_z_vector(value: &str) -> Option<(f32, f32)> {
    let value = value.trim();
    let value = value.strip_prefix('&')?.trim_start();
    let value = value.strip_prefix("Vector3f")?.trim_start();
    let value = value.strip_prefix('{')?.strip_suffix('}')?;
    let fields = tokenize_args_balanced(value);
    let [x, y, z] = fields.as_slice() else {
        return None;
    };
    let (x_name, x_value) = x.split_once(':')?;
    let (y_name, y_value) = y.split_once(':')?;
    let (z_name, z_value) = z.split_once(':')?;
    if x_name.trim() != "x" || y_name.trim() != "y" || z_name.trim() != "z" {
        return None;
    }
    let speed_x = x_value.trim().parse::<f32>().ok()?;
    let speed_y = y_value.trim().parse::<f32>().ok()?;
    let speed_z = z_value.trim().parse::<f32>().ok()?;
    (speed_z.is_finite() && speed_z == 0.0).then_some((speed_x, speed_y))
}

fn emit_sound(call: &crate::data::SoundCall, indent: &str) -> String {
    let args = call
        .sounds
        .iter()
        .map(|name| format!("Hash40::new(\"{name}\")"))
        .chain(call.tail.clone())
        .collect::<Vec<_>>()
        .join(", ");
    format!("{indent}macros::{}(agent, {args});", call.func)
}

fn emit_expression(call: &crate::data::ExpressionCall, indent: &str) -> String {
    match call {
        crate::data::ExpressionCall::RumbleHit { kind, unk } => {
            format!("{indent}macros::{}(agent, {kind}, {unk});", call.func())
        }
        crate::data::ExpressionCall::Quake { kind } => {
            format!("{indent}macros::{}(agent, {kind});", call.func())
        }
        crate::data::ExpressionCall::FtAttackAbsCameraQuake {
            attack_abs_kind,
            quake_kind,
        } => format!(
            "{indent}macros::{}(agent, {attack_abs_kind}, {quake_kind});",
            call.func()
        ),
        crate::data::ExpressionCall::ControlModuleSetRumble {
            receiver,
            kind,
            duration,
            looped,
            target,
        } => format!(
            "{indent}ControlModule::set_rumble({receiver}, {kind}, {duration}, {looped}, {target});"
        ),
    }
}

fn parse_attack_mod_call(line: &str) -> Option<ExcuteStmt> {
    let canonical;
    let line = if line.trim_start().starts_with("ATK_SET_SHIELD_SETOFF_MUL2(") {
        canonical = crate::move_nodes::native_calls::parse(line)?.source();
        canonical.as_str()
    } else {
        line
    };
    for kind in crate::data::AttackModKind::ALL {
        let needle = format!("macros::{}(", kind.macro_name());
        if !line.contains(&needle) {
            continue;
        }
        let start = line.find(&needle)? + needle.len();
        let end = line[start..].rfind(')')? + start;
        let tokens = tokenize_args(&line[start..end]);
        // Drop the leading `agent`, then require exactly the two arguments the signature has.
        let [id, value] = tokens.get(1..)? else {
            return None;
        };
        return Some(ExcuteStmt::AttackMod {
            kind,
            id: id.trim().parse::<i64>().ok()?,
            value: value.trim().parse::<f32>().ok()?,
        });
    }
    None
}

/// Render an `ATK_POWER` / `ATK_SET_SHIELD_SETOFF_MUL` value the way the vanilla scripts spell it.
///
/// Deliberately *not* [`num`], and the difference is not an oversight. `num` appends a `.0` to
/// whole numbers because most float arguments sit in slots declared `f32`, where a bare `6` is a
/// type error. These two slots are declared `ToF32` instead, which `smash-script` implements for
/// `i32`, so both spellings compile — and every one of the 11 vanilla calls writes the bare
/// integer (`7`, `10`). Emitting `7.0` over a `7` the user never touched would be diff noise in
/// an exported mod, so the integer form is kept when the value is whole.
pub(crate) fn attack_mod_num(value: f32) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        return format!("{}", value as i64);
    }
    num(value)
}

fn parse_attack_clear(line: &str) -> Option<u32> {
    let start = line.find("AttackModule::clear(")? + "AttackModule::clear(".len();
    let end = line[start..].find(')')? + start;
    tokenize_args(&line[start..end])
        .get(1)?
        .trim()
        .parse::<u32>()
        .ok()
}

/// A source argument that is literally `true` or `false`, or `None` for anything else.
///
/// Anything else is a constant or an expression, and the caller's job is then to keep the whole
/// call verbatim rather than to substitute a value for the part it could not read.
fn bool_token(s: &str) -> Option<bool> {
    match s.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// [`bool_token`] for one slot of an extracted argument list, absent slot included.
fn bool_token_at(args: &[String], slot: usize) -> Option<bool> {
    bool_token(args.get(slot)?)
}

/// Strip leading `*` dereference from constant names like `*ATTACK_SETOFF_KIND_ON`.
fn strip_deref(s: &str) -> String {
    s.trim_start_matches('*').to_string()
}

/// Write a hit status back the way the scripts write it.
///
/// A symbolic name needs the `*` deref it was parsed without; a bare number must not get one,
/// since `*2` does not compile. Deciding on the *text* rather than on whether the name is in
/// [`HIT_STATUS`](crate::param_labels::HIT_STATUS) is deliberate — a script using a constant
/// this build has never heard of still exports as the constant it wrote.
pub fn emit_status(status: &str) -> String {
    let s = status.trim();
    if s.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '*') {
        s.to_string()
    } else {
        format!("*{s}")
    }
}

/// Parse `Some(3.0)` → `Some(3.0)`, `None` → `None`.
/// Does the token at the head of a collision's capsule slots actually hold a coordinate?
///
/// `CATCH` and `SEARCH` are both dumped in two shapes. The smashline signature is fixed-arity
/// and spells an absent capsule `None`, but the vanilla dumps this parser is calibrated against
/// come from Lua, where the three endpoint arguments are simply **not written at all** — so the
/// slot that holds `x2` in one call holds the next *constant* in another.
///
/// Reading it positionally is therefore wrong, and wrong in the quiet direction: the arguments
/// after the capsule get read one slot short, run off the end of the token list, and fall back
/// to defaults. That is how every short-form `CATCH` in the corpus lost its status kind.
///
/// A coordinate is a bare float, `Some(..)`, or `None`; anything else — `*COLLISION_KIND_MASK_*`,
/// `*FIGHTER_STATUS_KIND_*` — is the short form's next argument and means the capsule was
/// omitted. Deliberately not a token *count* test: a call with a trailing comment or a spliced
/// argument would change the count without changing which slot holds what.
pub(crate) fn is_capsule_slot(token: &str) -> bool {
    let token = token.trim();
    token == "None" || token.starts_with("Some(") || token.parse::<f32>().is_ok()
}

fn parse_option_f32(s: &str) -> Option<f32> {
    let s = s.trim();
    if s == "None" {
        return None;
    }
    let inner = s.strip_prefix("Some(")?.strip_suffix(')')?;
    inner.trim().parse().ok()
}

/// One capsule endpoint coordinate, in any of the three spellings that occur.
///
/// `Some(1.0)` and `None` are the smashline signature's. The Lua-derived dumps write the
/// coordinate **bare** — `macros::SEARCH(.., 0.0, 7.0, 13.0, *COLLISION_KIND_MASK_ATTACK, ..)` —
/// so a reader that only understood `Some(..)` would see a stretched box as spherical and
/// quietly shorten it to a point.
///
/// Only ever called on a slot [`is_capsule_slot`] has already claimed, which is what keeps a
/// bare number here from swallowing the argument that follows an omitted capsule.
fn parse_capsule_coord(s: &str) -> Option<f32> {
    let s = s.trim();
    if s == "None" {
        return None;
    }
    match s.strip_prefix("Some(").and_then(|v| v.strip_suffix(')')) {
        Some(inner) => inner.trim().parse().ok(),
        None => s.parse().ok(),
    }
}

fn tokenize_args(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for ch in s.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                tokens.push(current.trim().to_string());
                current = String::new();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.trim().is_empty() {
        tokens.push(current.trim().to_string());
    }
    tokens
}

/// Split arguments while treating Rust tuples/arrays/struct literals as one argument. The
/// regular tokenizer above predates direct `Vector3f{...}` calls and intentionally remains
/// unchanged for the macro grammar it serves.
pub(crate) fn tokenize_args_balanced(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut current = String::new();
    for ch in s.chars() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                tokens.push(current.trim().to_string());
                current = String::new();
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    if !current.trim().is_empty() {
        tokens.push(current.trim().to_string());
    }
    tokens
}

fn extract_hash40_string(s: &str) -> Option<String> {
    let s = s.trim();
    let inner = s.strip_prefix("Hash40::new(\"")?.strip_suffix("\")")?;
    Some(inner.to_string())
}

/// Extract an effect Hash40 name while retaining the original token inside raw-hash wrappers.
fn extract_effect_hash40_name(s: &str) -> Option<String> {
    if let Some(name) = extract_hash40_string(s) {
        return Some(name);
    }
    let s = s.trim();
    // Keep the original raw value token rather than the wrapper expression. `hash_arg` writes
    // numeric names as `Hash40::new_raw(...)`; returning that expression as the effect name on
    // reparse makes the next export treat the wrapper itself as a name and changes the statement
    // tree even though the hash payload is unchanged.
    let raw = s
        .strip_prefix("Hash40::new_raw(")?
        .strip_suffix(')')?
        .trim();
    let digits = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))?
        .replace('_', "");
    u64::from_str_radix(&digits, 16).ok()?;
    Some(raw.to_owned())
}

/// Read either spelling of a Hash40 expression used for a bone argument.
///
/// Named bones are preferred in authored source, but a live capture can contain a valid bone
/// whose name is not present in the loaded skeleton. Keep that hash as an explicit raw value so
/// the call remains editable and can be emitted without guessing a different bone.
fn extract_hash40_value(s: &str) -> Option<String> {
    if let Some(name) = extract_hash40_string(s) {
        return Some(name);
    }
    let raw = s
        .trim()
        .strip_prefix("Hash40::new_raw(")?
        .strip_suffix(')')?
        .trim()
        .replace('_', "");
    let digits = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(&raw);
    let value = u64::from_str_radix(digits, 16).ok()?;
    Some(format!("{value:#x}"))
}

// ── Source code export ────────────────────────────────────────────────────────

/// A generated file path + contents ready to write to disk.
pub struct GeneratedFile {
    /// Relative path within the project root, e.g. `"src/mario/acmd.rs"`
    pub rel_path: String,
    pub contents: String,
}

/// All files that make up one exported mod project.
pub struct ModProject {
    /// The root folder name, e.g. `"my_mod"`
    pub name: String,
    pub files: Vec<GeneratedFile>,
}

/// Emit a single `macros::ATTACK(...)` call as a source line.
/// A lua-const expression: named consts emit as `*NAME`, but live-captured hitboxes store
/// the RAW numeric value ("3") — emit those bare (a `*3` would not compile).
/// Spell a hitbox property the way an `ATTACK` argument spells it: a lua const gets its
/// leading `*`, a value that is already a number stays one. Shared with the source
/// write-back so both routes put the identical text in that slot.
/// A collision-attr name as the `Hash40` expression an export writes.
///
/// Live-captured hitboxes carry the attr as a raw hash rather than a name, because the game
/// only ever had the hash — so those emit as `Hash40::new_raw`, which is the one form that
/// reproduces the captured value exactly.
fn hash40_expr(attr: &str) -> String {
    match attr.strip_prefix("0x") {
        Some(hex) => format!("Hash40::new_raw(0x{hex})"),
        None => format!("Hash40::new(\"{attr}\")"),
    }
}

/// Hash40 renderer shared by the FP IR conversion and this module's emitters.
pub(crate) fn hash40_expr_for_data(attr: &str) -> String {
    hash40_expr(attr)
}

pub fn const_expr(s: &str) -> String {
    let t = s.trim();
    if t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok() {
        t.to_string()
    } else {
        format!("*{t}")
    }
}

/// Render a float the way an ACMD argument has to be spelled.
///
/// Two requirements pull against each other here. The value must survive the round trip
/// exactly: the old `{:.1}` turned a vanilla `0.35` hitbox attribute into `0.3`, and a grab
/// box sitting at `-17.25` into `-17.2`, so exporting a move quietly changed it. And it must
/// stay a *float* literal, because several of these arguments are declared `f32` rather than
/// generic over `ToF32` — a bare `6` is a type error in a slot where `6.0` compiles.
///
/// So: the shortest text that reads back as the same `f32`, with a decimal point put back on
/// when the shortest form does not have one. A value that is not a number at all is spelled
/// as the constant rather than as `NaN`, which is not Rust; the verifier refuses the export
/// either way, and this keeps that one problem from also looking like a syntax error.
pub(crate) fn num(value: f32) -> String {
    if value.is_nan() {
        return "f32::NAN".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "f32::INFINITY"
        } else {
            "f32::NEG_INFINITY"
        }
        .to_string();
    }
    let text = value.to_string();
    if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    }
}

fn emit_attack(call: &AttackCall, indent: &str) -> String {
    // Skeleton files expose display-case names such as `FootR`, while ACMD hashes the
    // lowercase resource name (`footr`). Live injection follows the same contract.
    let bone = format!("Hash40::new(\"{}\")", call.bone_name.to_ascii_lowercase());
    let capsule = match call.capsule_end {
        Some([x, y, z]) => format!("Some({}), Some({}), Some({})", num(x), num(y), num(z)),
        None => "None, None, None".to_string(),
    };
    let collision_attr = hash40_expr(&call.collision_attr);
    // Always written with the capsule triple, even for a call parsed from the shorter
    // archive form: smash-script declares `x2`/`y2`/`z2` on every member of the family, so
    // the long form is the one that builds. Re-parsing it yields the same hitbox.
    format!(
        "{indent}macros::{func}(agent, {id}, {part}, {bone}, {dmg}, {angle}, {kbs}, {fkb}, {kbb}, \
{size}, {ox}, {oy}, {oz}, {capsule}, \
{hitlag}, {sdi}, {setoff}, {lr}, {clang}, {add_atk}, {hb_attr}, {goa}, \
{mtk}, {shield}, {reflect}, {absorb}, {landing}, \
{sit}, {cat}, {part_mask}, {no_cam}, {col_attr}, {snd_lvl}, {snd_attr}, {region});",
        indent = indent,
        func = call.func,
        id = call.id,
        part = call.part,
        bone = bone,
        dmg = num(call.damage),
        angle = call.angle,
        kbs = call.kb_scaling,
        fkb = call.fkb,
        kbb = call.kb_base,
        size = num(call.size),
        ox = num(call.offset_x),
        oy = num(call.offset_y),
        oz = num(call.offset_z),
        capsule = capsule,
        hitlag = num(call.hitlag_mult),
        sdi = num(call.sdi_mult),
        setoff = const_expr(&call.setoff_kind),
        lr = const_expr(&call.lr_check),
        clang = call.is_clang,
        add_atk = call.is_add_attack,
        hb_attr = num(call.hitbox_attr),
        goa = call.ground_or_air,
        mtk = call.is_mtk,
        shield = call.is_shield_disable,
        reflect = call.is_reflectable,
        absorb = call.is_absorbable,
        landing = call.is_landing_attack,
        sit = const_expr(&call.situation_mask),
        cat = const_expr(&call.category_mask),
        part_mask = const_expr(&call.part_mask),
        no_cam = call.no_finish_camera,
        col_attr = collision_attr,
        snd_lvl = const_expr(&call.sound_level),
        snd_attr = const_expr(&call.sound_attr),
        region = const_expr(&call.attack_region),
    )
}

/// Emit `macros::ATTACK_FP` from its own complete slot table. Unsupported or unknown slots are
/// emitted from the preserved token/type rather than reconstructed from ordinary ATTACK fields.
fn emit_attack_fp(call: &crate::data::AttackFpCall, indent: &str) -> String {
    let args = call
        .args
        .iter()
        .map(crate::data::AttackFpArg::source)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{indent}macros::ATTACK_FP(agent, {args});")
}

/// Emit the `macros::CATCH` call for a grab box.
///
/// `CATCH` is a fixed-arity function whose capsule endpoints are `Option<f32>`, so a
/// spherical grab spells them `None` exactly the way `ATTACK` does. Grabs used to be
/// exported through [`emit_attack`], which wrote a zero-damage attack hitbox in place of the
/// user's grab box and stopped the move grabbing at all.
fn emit_catch(call: &crate::data::CatchCall, indent: &str) -> String {
    let bone = format!("Hash40::new(\"{}\")", call.bone_name.to_ascii_lowercase());
    let capsule = match call.capsule_end {
        Some([x, y, z]) => format!("Some({}), Some({}), Some({})", num(x), num(y), num(z)),
        None => "None, None, None".to_string(),
    };
    format!(
        "{indent}macros::CATCH(agent, {id}, {bone}, {size}, {x}, {y}, {z}, \
{capsule}, {status}, {situation});",
        id = call.id,
        size = num(call.size),
        x = num(call.offset_x),
        y = num(call.offset_y),
        z = num(call.offset_z),
        status = const_expr(&call.status),
        situation = const_expr(&call.situation),
    )
}

/// Emit `macros::SEARCH`, in its own slot order.
///
/// Always the full form, with the capsule spelled `Some(..)`/`None`, even when the call was
/// read from the shorter Lua-shaped dump. That shorter form is not valid Rust — `macros::SEARCH`
/// is fixed-arity — so a mod exported in it would not build. Same choice `emit_catch` makes.
fn emit_search(call: &crate::data::SearchCall, indent: &str) -> String {
    let bone = format!("Hash40::new(\"{}\")", call.bone_name.to_ascii_lowercase());
    let capsule = match call.capsule_end {
        Some([x, y, z]) => format!("Some({}), Some({}), Some({})", num(x), num(y), num(z)),
        None => "None, None, None".to_string(),
    };
    format!(
        "{indent}macros::SEARCH(agent, {id}, {part}, {bone}, {size}, {x}, {y}, {z}, \
{capsule}, {kind}, {status}, {unk}, {situation}, {category}, {parts}, {unk2});",
        id = call.id,
        part = call.part,
        size = num(call.size),
        x = num(call.offset_x),
        y = num(call.offset_y),
        z = num(call.offset_z),
        kind = const_expr(&call.extras.collision_kind),
        status = const_expr(&call.extras.hit_status),
        unk = call.extras.unk,
        situation = const_expr(&call.situation_mask),
        category = const_expr(&call.category_mask),
        parts = const_expr(&call.part_mask),
        unk2 = call.extras.unk2,
    )
}

/// Emit `macros::ATTACK_ABS`, in its own slot order.
///
/// `damage` and `hitlag` go through `num` because the archive writes them with a decimal
/// point; the knockback triple and the angle are whole numbers there and are written bare.
/// Matching each family's own spelling is what keeps a re-exported vanilla script textually
/// identical to the one it came from.
fn emit_attack_abs(call: &crate::data::AttackAbsCall, indent: &str) -> String {
    let (unk, unk2, unk3) = call.unknowns;
    format!(
        "{indent}macros::ATTACK_ABS(agent, {kind}, {id}, {damage}, {angle}, {kbg}, {fkb}, \
{bkb}, {hitlag}, {unk}, {lr}, {unk2}, {unk3}, {attr}, {level}, {sound}, {region});",
        kind = const_expr(&call.kind),
        id = call.id,
        damage = num(call.damage),
        angle = call.angle,
        kbg = call.kb_scaling,
        fkb = call.fkb,
        bkb = call.kb_base,
        hitlag = num(call.hitlag_mult),
        unk = num(unk),
        lr = const_expr(&call.lr_check),
        unk2 = num(unk2),
        attr = hash40_expr(&call.collision_attr),
        level = const_expr(&call.sound_level),
        sound = const_expr(&call.sound_attr),
        region = const_expr(&call.attack_region),
    )
}

fn emit_excute_stmts(stmts: &[crate::data::ExcuteStmt], indent: &str) -> Vec<String> {
    stmts
        .iter()
        .map(|s| match s {
            crate::data::ExcuteStmt::Attack(call) => emit_attack(call, indent),
            crate::data::ExcuteStmt::Catch(call) => emit_catch(call, indent),
            crate::data::ExcuteStmt::AttackAbs(call) => emit_attack_abs(call, indent),
            crate::data::ExcuteStmt::AttackAbsApplyThrow => ATTACK_ABS_APPLY_THROW_SOURCE
                .iter()
                .map(|line| format!("{indent}{line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            crate::data::ExcuteStmt::AttackFp(call) => emit_attack_fp(call, indent),
            crate::data::ExcuteStmt::Search(call) => emit_search(call, indent),
            crate::data::ExcuteStmt::GrabClearAll => {
                format!("{indent}GrabModule::clear_all(agent.module_accessor);")
            }
            crate::data::ExcuteStmt::GrabRebound { enabled } => {
                format!("{indent}GrabModule::set_rebound(agent.module_accessor, {enabled});")
            }
            crate::data::ExcuteStmt::Wind(wind) => format!(
                "{indent}macros::{}(agent, {});",
                wind.command,
                // Deliberately not `num`: a wind payload is kept verbatim rather than
                // recomposed from editable fields, and `f32::to_string` is already both exact
                // and shortest, so an untouched wind command replays byte for byte. Every
                // argument slot is generic over `ToF32`, so a whole number needs no `.0`.
                wind.args
                    .iter()
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            crate::data::ExcuteStmt::EraseWind(id) => {
                format!("{indent}AreaModule::erase_wind(agent.module_accessor, {id});")
            }
            crate::data::ExcuteStmt::Clear(id) => {
                format!("{indent}AttackModule::clear(agent.module_accessor, {id}, false);")
            }
            crate::data::ExcuteStmt::ClearAll => {
                format!("{indent}AttackModule::clear_all(agent.module_accessor);")
            }
            // A status is re-emitted with the `*` the scripts write, because it is a lua const
            // deref rather than a Rust path. `emit_status` puts it back only where the parse
            // stripped one, so a call written with a bare number stays a bare number.
            crate::data::ExcuteStmt::HitStatus { target, status } => match target {
                crate::data::HurtTarget::Bone(bone) => format!(
                    "{indent}macros::HIT_NODE(agent, {}, {});",
                    hash_arg(&bone.to_ascii_lowercase()),
                    emit_status(status)
                ),
                crate::data::HurtTarget::Group(group) => format!(
                    "{indent}macros::HIT_NO(agent, {group}, {});",
                    emit_status(status)
                ),
                // The status moves into the first slot: this macro's target is its name.
                crate::data::HurtTarget::Whole => {
                    format!("{indent}macros::WHOLE_HIT(agent, {});", emit_status(status))
                }
            },
            crate::data::ExcuteStmt::HitResetAll => {
                format!("{indent}macros::HIT_RESET_ALL(agent);")
            }
            crate::data::ExcuteStmt::DamageNoReaction(call) => format!(
                "{indent}damage!(agent, {}, {}, {});",
                call.command, call.mode, call.value
            ),
            crate::data::ExcuteStmt::ColPri(pri) => {
                format!("{indent}macros::COL_PRI(agent, {pri});")
            }
            crate::data::ExcuteStmt::ColNormal => format!("{indent}macros::COL_NORMAL(agent);"),
            crate::data::ExcuteStmt::Slope(call) => match call {
                crate::data::SlopeCall::Set { status } => format!(
                    "{indent}slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_{});",
                    status.source_name()
                ),
                crate::data::SlopeCall::Interpolate {
                    status,
                    frames,
                    trailing_flag,
                } => match trailing_flag {
                    Some(flag) => format!(
                        "{indent}slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_{}, {frames}, {flag});",
                        status.source_name()
                    ),
                    None => format!(
                        "{indent}slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_{}, {frames});",
                        status.source_name()
                    ),
                },
            },
            crate::data::ExcuteStmt::ItemVisibility { visible } => format!(
                "{indent}ItemModule::set_have_item_visibility(agent.module_accessor, {visible}, 0);"
            ),
            crate::data::ExcuteStmt::Effect(command) => {
                preview_effect_macro(command).lines().map(|line| format!("{indent}{line}")).collect::<Vec<_>>().join("\n")
            },
            crate::data::ExcuteStmt::NativeCall(call) => format!("{indent}{}", call.source()),
            crate::data::ExcuteStmt::ShootItemBullet => {
                format!("{indent}macros::SHOOT_ITEM_BULLET(agent);")
            }
            crate::data::ExcuteStmt::StartInfoFlashEye => {
                format!("{indent}macros::START_INFO_FLASH_EYE(agent);")
            }
            crate::data::ExcuteStmt::CameraZoomOut => {
                format!("{indent}macros::CAM_ZOOM_OUT(agent);")
            }
            crate::data::ExcuteStmt::StartCutIn => {
                format!("{indent}macros::FT_START_CUTIN(agent);")
            }
            crate::data::ExcuteStmt::CaptureCutCommon => {
                format!("{indent}macros::game_CaptureCutCommon(agent);")
            }
            crate::data::ExcuteStmt::JostleSetStatus { enabled } => format!(
                "{indent}JostleModule::set_status(agent.module_accessor, {enabled});"
            ),
            crate::data::ExcuteStmt::FixedJostleArea(call) => match call {
                crate::data::FixedJostleAreaCall::EnableFix { arg2, arg3 } => format!(
                    "{indent}FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, {}, {});",
                    num(*arg2),
                    num(*arg3)
                ),
                crate::data::FixedJostleAreaCall::EnableFixXy {
                    arg2,
                    arg3,
                    arg4,
                    arg5,
                } => format!(
                    "{indent}FighterAreaModuleImpl::enable_fix_jostle_area_xy(agent.module_accessor, {}, {}, {}, {});",
                    num(*arg2),
                    num(*arg3),
                    num(*arg4),
                    num(*arg5)
                ),
            },
            crate::data::ExcuteStmt::VisibilitySetInt64 { group, status } => format!(
                "{indent}VisibilityModule::set_int64(agent.module_accessor, hash40(\"{group}\") as i64, hash40(\"{status}\") as i64);"
            ),
            crate::data::ExcuteStmt::VisibilitySetDefaultInt64 { group } => format!(
                "{indent}VisibilityModule::set_default_int64(agent.module_accessor, hash40(\"{group}\") as i64);"
            ),
            crate::data::ExcuteStmt::VisibilitySetWhole { visible } => format!(
                "{indent}VisibilityModule::set_whole(agent.module_accessor, {visible});"
            ),
            crate::data::ExcuteStmt::AttackModuleSetAll { kind, arg2, arg3 } => {
                let function = match kind {
                    crate::data::AttackModuleSetAllKind::NoDeadAll => "set_no_dead_all",
                    crate::data::AttackModuleSetAllKind::CatchOnlyAll => "set_catch_only_all",
                };
                format!(
                    "{indent}AttackModule::{function}(agent.module_accessor, {arg2}, {arg3});"
                )
            }
            crate::data::ExcuteStmt::AttackModuleSetIndexed {
                kind,
                id,
                arg3,
                arg4,
            } => {
                let function = match kind {
                    crate::data::AttackModuleSetIndexedKind::NoFinishCamera => {
                        "set_no_finish_camera"
                    }
                    crate::data::AttackModuleSetIndexedKind::ForceReaction => {
                        "set_force_reaction"
                    }
                };
                format!(
                    "{indent}AttackModule::{function}(agent.module_accessor, {id}, {arg3}, {arg4});"
                )
            }
            crate::data::ExcuteStmt::AttackModuleSetHeightAll { height, arg3 } => {
                let height = match height {
                    crate::data::AttackHeightAllKind::High => "HIGH",
                    crate::data::AttackHeightAllKind::Low => "LOW",
                };
                format!(
                    "{indent}AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_{height}), {arg3});"
                )
            }
            crate::data::ExcuteStmt::AttackModuleSetFinalFinishCutIn {
                id,
                arg3,
                arg4,
                arg5,
                arg6,
            } => format!(
                "{indent}AttackModule::set_final_finish_cut_in(agent.module_accessor, {id}, {arg3}, {arg4}, {}, {arg6});",
                num(*arg5)
            ),
            crate::data::ExcuteStmt::AttackModuleInitAttackPos { id } => format!(
                "{indent}AttackModule::init_attack_pos(agent.module_accessor, {id});"
            ),
            crate::data::ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind } => {
                let (bone, axis3, axis4, axis5) = kind.native_parts();
                format!(
                    "{indent}AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new(\"{bone}\"), AttackDirectionAxis(*{axis3}), AttackDirectionAxis(*{axis4}), AttackDirectionAxis(*{axis5}));"
                )
            }
            crate::data::ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll { arg2, arg3 } => format!(
                "{indent}AttackModule::set_no_damage_fly_smoke_all(agent.module_accessor, {arg2}, {arg3});"
            ),
            crate::data::ExcuteStmt::AttackModuleSetDamageShakeScale(scale) => format!(
                "{indent}AttackModule::set_damage_shake_scale(agent.module_accessor, {});",
                scale.source_token()
            ),
            crate::data::ExcuteStmt::AttackModuleSetNoUniqEffectAll(pair) => {
                let (arg2, arg3) = pair.source_args();
                format!(
                    "{indent}AttackModule::set_no_uniq_effect_all(agent.module_accessor, {arg2}, {arg3});"
                )
            }
            crate::data::ExcuteStmt::AttackModuleSetTargetCategory(id) => format!(
                "{indent}AttackModule::set_target_category(agent.module_accessor, {}, *COLLISION_CATEGORY_MASK_NO_IF as u32);",
                id.attack_id()
            ),
            crate::data::ExcuteStmt::HitModuleSetStatusAllNormal => format!(
                "{indent}HitModule::set_status_all(agent.module_accessor, HitStatus(*HIT_STATUS_NORMAL), 0);"
            ),
            crate::data::ExcuteStmt::AttackModuleSetSize(preset) => format!(
                "{indent}AttackModule::set_size(agent.module_accessor, {}, {});",
                preset.attack_id(),
                preset.size_token()
            ),
            crate::data::ExcuteStmt::AttackModuleSetVecTargetPos(preset) => {
                format!("{indent}{}", preset.source_line())
            }
            crate::data::ExcuteStmt::OptionalHitFeedback(call) => {
                let function = match call.kind {
                    crate::data::OptionalHitFeedbackKind::Effect => "set_optional_hit_effect",
                    crate::data::OptionalHitFeedbackKind::Sound => "set_optional_hit_sound",
                };
                format!(
                    "{indent}AttackModule::{function}(agent.module_accessor, {}, {});",
                    call.arg2,
                    call.hash.source_expr()
                )
            }
            crate::data::ExcuteStmt::AttackModuleAddReactionFrame { id, frames } => format!(
                "{indent}AttackModule::set_add_reaction_frame(agent.module_accessor, {id}, {}, false);",
                num(*frames)
            ),
            crate::data::ExcuteStmt::AttackModuleAddReactionFrameRevised { id, frames } => format!(
                "{indent}AttackModule::set_add_reaction_frame_revised(agent.module_accessor, {id}, {}, false);",
                num(*frames)
            ),
            crate::data::ExcuteStmt::ArticleLifecycle(call) => match call {
                crate::data::ArticleLifecycleCall::Generate { symbol } => format!(
                    "{indent}ArticleModule::generate_article(agent.module_accessor, *{symbol}, false, -1);"
                ),
                crate::data::ArticleLifecycleCall::GenerateEnabled { symbol } => format!(
                    "{indent}ArticleModule::generate_article_enable(agent.module_accessor, *{symbol}, false, -1);"
                ),
                crate::data::ArticleLifecycleCall::ChangeStatus { symbol, status, if_present } => {
                    if *if_present {
                        format!("{indent}ArticleModule::change_status_exist(agent.module_accessor, *{symbol}, *{status});")
                    } else {
                        format!("{indent}ArticleModule::change_status(agent.module_accessor, *{symbol}, *{status}, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));")
                    }
                }
                crate::data::ArticleLifecycleCall::RemoveAll { symbol } => format!(
                    "{indent}ArticleModule::remove_exist(agent.module_accessor, *{symbol}, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));"
                ),
                crate::data::ArticleLifecycleCall::ChangeMotion { symbol, motion } => format!(
                    "{indent}ArticleModule::change_motion(agent.module_accessor, *{symbol}, Hash40::new(\"{motion}\"), false, -1.0);"
                ),
                crate::data::ArticleLifecycleCall::SetVisibilityWhole { symbol, visible } => format!(
                    "{indent}ArticleModule::set_visibility_whole(agent.module_accessor, *{symbol}, {visible}, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));"
                ),
                crate::data::ArticleLifecycleCall::Shoot { symbol, target } => format!(
                    "{indent}ArticleModule::shoot(agent.module_accessor, *{symbol}, ArticleOperationTarget({}), false);",
                    target.source_token()
                ),
                crate::data::ArticleLifecycleCall::ShootExist { symbol, target } => format!(
                    "{indent}ArticleModule::shoot_exist(agent.module_accessor, *{symbol}, ArticleOperationTarget({}), false);",
                    target.source_token()
                ),
                crate::data::ArticleLifecycleCall::AddMotionPartial { preset } => {
                    format!("{indent}{}", preset.source_line())
                }
            },
            crate::data::ExcuteStmt::GimmickEvent { name } => format!(
                "{indent}crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, {name:?});"
            ),
            crate::data::ExcuteStmt::GimmickChartEvent { chart_id, name } => format!(
                "{indent}crate::slight::gimmicks::dispatch_named_chart_from_module(agent.module_accessor, {chart_id:?}, {name:?});"
            ),
            crate::data::ExcuteStmt::GimmickChartEventValues { chart_id, name, arguments } => {
                use crate::gimmicks::behavior::ConstValue;
                let values = arguments
                    .iter()
                    .map(|argument| {
                        let value = match argument.value {
                            ConstValue::Int(value) => format!(
                                "crate::slight::gimmicks::ChartEventValue::Int({value})"
                            ),
                            ConstValue::Float(value) => {
                                let mut literal = format!("{value:?}");
                                if !literal.chars().any(|character| matches!(character, '.' | 'e' | 'E')) {
                                    literal.push_str(".0");
                                }
                                format!(
                                    "crate::slight::gimmicks::ChartEventValue::Float({literal}f32)"
                                )
                            }
                            ConstValue::Flag(value) => format!(
                                "crate::slight::gimmicks::ChartEventValue::Flag({value})"
                            ),
                        };
                        format!("({:?}, {value})", argument.field.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{indent}crate::slight::gimmicks::dispatch_named_chart_values_from_module(agent.module_accessor, {chart_id:?}, {name:?}, &[{values}]);"
                )
            }
            // Re-emitted as its own call at its own frame, never folded into the `ATTACK` it
            // retunes: the corpus writes it both in that call's own block and several frames
            // later, and only the separate line can express the second.
            crate::data::ExcuteStmt::AttackMod { kind, id, value } => format!(
                "{indent}macros::{}(agent, {id}, {});",
                kind.macro_name(),
                attack_mod_num(*value)
            ),
            crate::data::ExcuteStmt::Sound(call) => emit_sound(call, indent),
            crate::data::ExcuteStmt::Expression(call) => emit_expression(call, indent),
            crate::data::ExcuteStmt::ReverseLr => {
                format!("{indent}macros::REVERSE_LR(agent);")
            }
            crate::data::ExcuteStmt::CheckFinishCamera(call) => format!(
                "{indent}macros::CHECK_FINISH_CAMERA(agent, {}, {});",
                call.arg1, call.arg2
            ),
            crate::data::ExcuteStmt::CheckFinalStartCamera(call) => format!(
                "{indent}macros::CHECK_VALID_FINAL_START_CAMERA(agent, {}, {}, {}, {}, {}, {});",
                call.args[0],
                call.args[1],
                call.args[2],
                call.args[3],
                call.args[4],
                call.args[5]
            ),
            crate::data::ExcuteStmt::FinalFearFace(call) => format!(
                "{indent}macros::FT_SET_FINAL_FEAR_FACE(agent, {});",
                call.argument
            ),
            crate::data::ExcuteStmt::SetSpeedEx(call) => format!(
                "{indent}macros::SET_SPEED_EX(agent, {}, {}, {});",
                num(call.speed_x),
                num(call.speed_y),
                call.kinetic_kind
            ),
            crate::data::ExcuteStmt::SetSpeed(call) => format!(
                "{indent}visionary_set_speed(agent, {}, {});",
                num(call.speed_x),
                num(call.speed_y)
            ),
            crate::data::ExcuteStmt::AddSpeedNoLimit(call) => format!(
                "{indent}macros::ADD_SPEED_NO_LIMIT(agent, {}, {});",
                num(call.speed_x),
                num(call.speed_y)
            ),
            crate::data::ExcuteStmt::Correct(call) => {
                format!("{indent}macros::CORRECT(agent, {});", call.kind)
            }
            crate::data::ExcuteStmt::FtCatchStop(call) => format!(
                "{indent}macros::FT_CATCH_STOP(agent, {}, {});",
                num(call.arg1),
                num(call.arg2)
            ),
            crate::data::ExcuteStmt::FtStartAdjustMotionFrame(call) => format!(
                "{indent}macros::FT_START_ADJUST_MOTION_FRAME_arg1(agent, {});",
                num(call.value)
            ),
            crate::data::ExcuteStmt::MotionModuleSetRate(call) => format!(
                "{indent}MotionModule::set_rate(agent.module_accessor, {});",
                num(call.rate)
            ),
            crate::data::ExcuteStmt::MotionModuleSetHelperCalculation(call) => format!(
                "{indent}MotionModule::set_helper_calculation(agent.module_accessor, {});",
                call.enabled
            ),
            crate::data::ExcuteStmt::MotionModuleSetRatePartial(call) => format!(
                "{indent}MotionModule::set_rate_partial(agent.module_accessor, {}, {});",
                call.part_kind,
                num(call.rate)
            ),
            crate::data::ExcuteStmt::MotionModuleSetFramePartial(call) => match call.sync {
                // An omitted `sync` is re-emitted omitted. The game's own reader defaults it, so
                // spelling it out would change the author's source for no behavioural gain.
                None => format!(
                    "{indent}MotionModule::set_frame_partial(agent.module_accessor, {}, {});",
                    call.part_kind,
                    num(call.frame)
                ),
                Some(sync) => format!(
                    "{indent}MotionModule::set_frame_partial(agent.module_accessor, {}, {}, {});",
                    call.part_kind,
                    num(call.frame),
                    sync
                ),
            },
            crate::data::ExcuteStmt::ClrSpeed(call) => {
                format!("{indent}visionary_clr_speed(agent, {});", call.kinetic_kind)
            }
            crate::data::ExcuteStmt::SetAir => {
                format!("{indent}visionary_set_air(agent);")
            }
            crate::data::ExcuteStmt::KineticClearSpeedAll => {
                format!(
                    "{indent}KineticModule::clear_speed_all(agent.module_accessor);"
                )
            }
            crate::data::ExcuteStmt::KineticSetConsiderGroundFriction(call) => format!(
                "{indent}KineticModule::set_consider_ground_friction(agent.module_accessor, {}, {});",
                call.consider_ground_friction,
                call.kinetic_energy_attribute
            ),
            crate::data::ExcuteStmt::ChangeKinetic(call) => format!(
                "{indent}KineticModule::change_kinetic(agent.module_accessor, {});",
                call.kinetic_type
            ),
            crate::data::ExcuteStmt::KineticEnergy(call) => format!(
                "{indent}{}(agent.module_accessor, {});",
                call.func(),
                call.kinetic_energy_id
            ),
            crate::data::ExcuteStmt::KineticAddSpeed(call) => format!(
                "{indent}KineticModule::add_speed(agent.module_accessor, &Vector3f{{x: {}, y: {}, z: 0.0}});",
                num(call.speed_x),
                num(call.speed_y)
            ),
            crate::data::ExcuteStmt::WorkFlag(call) => format!(
                "{indent}{}(agent.module_accessor, {});",
                call.func(),
                call.flag
            ),
            crate::data::ExcuteStmt::WorkTransitionTerm(call) => format!(
                "{indent}{}({}, {});",
                call.func(),
                call.receiver.token(),
                call.transition_term
            ),
            crate::data::ExcuteStmt::WorkModuleIncInt(call) => format!(
                "{indent}{}(agent.module_accessor, {});",
                call.func(),
                call.slot
            ),
            crate::data::ExcuteStmt::WorkModuleSet(call) => format!(
                "{indent}{}({}, {}, {});",
                call.func(),
                call.receiver,
                call.value,
                call.slot
            ),
            crate::data::ExcuteStmt::SlowWhole(crate::data::SlowWholeCall::Set {
                multiplier,
                frames,
            }) => format!(
                "{indent}SlowModule::set_whole(agent.module_accessor, {multiplier}, {frames});"
            ),
            crate::data::ExcuteStmt::SlowWhole(crate::data::SlowWholeCall::Clear) => {
                format!("{indent}SlowModule::clear_whole(agent.module_accessor);")
            }
            crate::data::ExcuteStmt::SlowOpponent(call) => format!(
                "{indent}macros::SLOW_OPPONENT(agent, {}, {});",
                num(call.slow_mul),
                num(call.slow_time)
            ),
            crate::data::ExcuteStmt::Raw(line) => format!("{indent}{line}"),
        })
        .collect()
}

pub(crate) fn emit_stmts(stmts: &[crate::data::AcmdStmt], indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for stmt in stmts {
        match stmt {
            crate::data::AcmdStmt::Frame(f) => lines.push(format!(
                "{indent}frame(agent.lua_state_agent, {});",
                num(*f)
            )),
            crate::data::AcmdStmt::NativeExecuteFrame(frame) => lines.push(format!(
                "{indent}smash::app::sv_animcmd::execute(agent.lua_state_agent, {});",
                num(*frame)
            )),
            crate::data::AcmdStmt::NativeMotionLoopSync => lines.push(format!(
                "{indent}smash::app::sv_animcmd::wait_loop_sync_mot(agent.lua_state_agent);"
            )),
            crate::data::AcmdStmt::Wait(w) => {
                lines.push(format!("{indent}wait(agent.lua_state_agent, {});", num(*w)))
            }
            crate::data::AcmdStmt::WaitLoopClear => {
                lines.push(format!("{indent}{WAIT_LOOP_CLEAR_SOURCE}"))
            }
            crate::data::AcmdStmt::MotionRate(rate) => lines.push(format!(
                "{indent}macros::FT_MOTION_RATE(agent, {});",
                num(*rate)
            )),
            crate::data::AcmdStmt::Excute(inner) => {
                lines.push(format!("{indent}if macros::is_excute(agent) {{"));
                lines.extend(emit_excute_stmts(inner, &format!("{indent}    ")));
                lines.push(format!("{indent}}}"));
            }
            crate::data::AcmdStmt::Loop {
                body,
                motion_loop: crate::data::MotionLoop::Clear,
                ..
            } => {
                lines.push(format!("{indent}loop {{"));
                lines.extend(emit_stmts(body, &format!("{indent}    ")));
                lines.push(format!("{indent}    {WAIT_LOOP_CLEAR_SOURCE}"));
                lines.push(format!("{indent}}}"));
            }
            // The raw call leaves its result on the Lua stack; in an endless loop that would
            // grow every cycle, so it is wrapped the way HDR writes it.
            crate::data::AcmdStmt::Loop {
                body,
                motion_loop: crate::data::MotionLoop::SyncMotion,
                ..
            } => {
                lines.push(format!("{indent}loop {{"));
                lines.extend(emit_stmts(body, &format!("{indent}    ")));
                lines.push(format!("{indent}    agent.clear_lua_stack();"));
                lines.push(format!(
                    "{indent}    smash::app::sv_animcmd::wait_loop_sync_mot(agent.lua_state_agent);"
                ));
                lines.push(format!("{indent}    agent.pop_lua_stack(1);"));
                lines.push(format!("{indent}}}"));
            }
            crate::data::AcmdStmt::Loop { count, body, .. } => {
                lines.push(format!("{indent}for _ in 0..{count} {{"));
                lines.extend(emit_stmts(body, &format!("{indent}    ")));
                lines.push(format!("{indent}}}"));
            }
            // The header is carried verbatim — it can be any Rust the dumper wrote, including
            // a raw address call — and the brace it opens is closed here.
            crate::data::AcmdStmt::RawBlock { header, body } => {
                lines.push(format!("{indent}{header}"));
                lines.extend(emit_stmts(body, &format!("{indent}    ")));
                lines.push(format!("{indent}}}"));
            }
            // At the caller's own indent and with no wrapper: the source wrote this command
            // outside every `is_excute` block, and adding one back would change when it runs.
            crate::data::AcmdStmt::Bare(inner) => lines.extend(emit_excute_stmts(
                std::slice::from_ref(inner.as_ref()),
                indent,
            )),
            crate::data::AcmdStmt::Raw(line) => lines.push(format!("{indent}{line}")),
        }
    }
    lines
}

fn contains_set_speed(stmts: &[crate::data::AcmdStmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        crate::data::AcmdStmt::Excute(inner) => inner
            .iter()
            .any(|stmt| matches!(stmt, crate::data::ExcuteStmt::SetSpeed(_))),
        crate::data::AcmdStmt::Bare(inner) => {
            matches!(inner.as_ref(), crate::data::ExcuteStmt::SetSpeed(_))
        }
        crate::data::AcmdStmt::Loop { body, .. } | crate::data::AcmdStmt::RawBlock { body, .. } => {
            contains_set_speed(body)
        }
        _ => false,
    })
}

fn contains_clr_speed(stmts: &[crate::data::AcmdStmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        crate::data::AcmdStmt::Excute(inner) => inner
            .iter()
            .any(|stmt| matches!(stmt, crate::data::ExcuteStmt::ClrSpeed(_))),
        crate::data::AcmdStmt::Bare(inner) => {
            matches!(inner.as_ref(), crate::data::ExcuteStmt::ClrSpeed(_))
        }
        crate::data::AcmdStmt::Loop { body, .. } | crate::data::AcmdStmt::RawBlock { body, .. } => {
            contains_clr_speed(body)
        }
        _ => false,
    })
}

fn contains_set_air(stmts: &[crate::data::AcmdStmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        crate::data::AcmdStmt::Excute(inner) => inner
            .iter()
            .any(|stmt| matches!(stmt, crate::data::ExcuteStmt::SetAir)),
        crate::data::AcmdStmt::Bare(inner) => {
            matches!(inner.as_ref(), crate::data::ExcuteStmt::SetAir)
        }
        crate::data::AcmdStmt::Loop { body, .. } | crate::data::AcmdStmt::RawBlock { body, .. } => {
            contains_set_air(body)
        }
        _ => false,
    })
}

/// `smash-script` has no Rust wrapper for `sv_animcmd::SET_SPEED`. Generated projects therefore
/// use the linked primitive with the same Lua-stack setup as the crate's other wrappers. The
/// helper is nested in a generated function so the preview and shipped source have identical
/// text, while the parser can still recognise its call as the typed SET_SPEED family.
fn emit_set_speed_helper(indent: &str) -> Vec<String> {
    vec![
        format!("{indent}#[inline]"),
        format!("{indent}#[allow(dead_code)]"),
        format!(
            "{indent}unsafe fn visionary_set_speed(agent: &mut L2CAgentBase, speed_x: f32, speed_y: f32) {{"
        ),
        format!("{indent}    agent.clear_lua_stack();"),
        format!("{indent}    lua_args!(agent, speed_x, speed_y);"),
        format!("{indent}    smash::app::sv_animcmd::SET_SPEED(agent.lua_state_agent);"),
        format!("{indent}    agent.clear_lua_stack();"),
        format!("{indent}}}"),
    ]
}

/// `smash-script` has no safe `CLR_SPEED` wrapper. Generated projects use the linked kinetic
/// primitive with the same Lua-stack setup as the generic wrapper macro.
fn emit_clr_speed_helper(indent: &str) -> Vec<String> {
    vec![
        format!("{indent}#[inline]"),
        format!("{indent}#[allow(dead_code)]"),
        format!(
            "{indent}unsafe fn visionary_clr_speed(agent: &mut L2CAgentBase, kinetic_kind: i32) {{"
        ),
        format!("{indent}    agent.clear_lua_stack();"),
        format!("{indent}    lua_args!(agent, kinetic_kind);"),
        format!("{indent}    smash::app::sv_kinetic_energy::clear_speed(agent.lua_state_agent);"),
        format!("{indent}    agent.clear_lua_stack();"),
        format!("{indent}}}"),
    ]
}

/// `smash-script` has no safe `SET_AIR` wrapper. Generated projects call the linked animcmd
/// primitive directly after preparing an empty Lua stack.
fn emit_set_air_helper(indent: &str) -> Vec<String> {
    vec![
        format!("{indent}#[inline]"),
        format!("{indent}#[allow(dead_code)]"),
        format!("{indent}unsafe fn visionary_set_air(agent: &mut L2CAgentBase) {{"),
        format!("{indent}    agent.clear_lua_stack();"),
        format!("{indent}    smash::app::sv_animcmd::SET_AIR(agent.lua_state_agent);"),
        format!("{indent}    agent.clear_lua_stack();"),
        format!("{indent}}}"),
    ]
}

/// The ACMD script name for a move: `attack_air_n` + `game` → `game_attackairn`.
///
/// This is the game's own naming, so it doubles as the key an existing smashline project
/// registers its scripts under — see `acmd_src`.
pub fn acmd_script_name(prefix: &str, move_name: &str) -> String {
    script_function_name(prefix, move_name)
}

/// Whether `name` is one of the effect spawn macros this module knows how to read.
pub fn is_effect_spawn_macro(name: &str) -> bool {
    effect_spawn_macro_layout(&format!("macros::{name}(")).is_some_and(|(m, _, _)| m == name)
}

/// Emit one `unsafe extern "C" fn` for a single move and return
/// `(fn_name, source_block)`.
fn script_function_name(prefix: &str, move_name: &str) -> String {
    let suffix: String = move_name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    format!(
        "{prefix}_{}",
        if suffix.is_empty() { "move" } else { &suffix }
    )
}

/// The Rust function a source block declares (`fn game_attacks3(` → `game_attacks3`).
fn declared_fn_name(source: &str) -> Option<String> {
    let start = source.find("fn ")? + 3;
    let open = source[start..].find('(')?;
    let name = source[start..start + open].trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn rust_module_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_');
    let mut out = if out.is_empty() {
        "fighter".to_string()
    } else {
        out.to_string()
    };
    const KEYWORDS: &[&str] = &[
        "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
        "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
        "use", "where", "while", "async", "await", "dyn",
    ];
    if out.as_bytes()[0].is_ascii_digit() || KEYWORDS.contains(&out.as_str()) {
        out.insert_str(0, "fighter_");
    }
    out
}

fn emit_move_fn(script: &crate::data::AcmdScript, move_name: &str) -> (String, String) {
    // Function name matches the ACMD script convention: game_{movename_no_underscores}
    let fn_name = script_function_name("game", move_name);
    let body = emit_stmts(&script.stmts, "    ");
    let mut out = String::new();
    out.push_str(&format!(
        "unsafe extern \"C\" fn {fn_name}(agent: &mut L2CAgentBase) {{\n"
    ));
    if contains_set_speed(&script.stmts) {
        for line in emit_set_speed_helper("    ") {
            out.push_str(&line);
            out.push('\n');
        }
    }
    if contains_clr_speed(&script.stmts) {
        for line in emit_clr_speed_helper("    ") {
            out.push_str(&line);
            out.push('\n');
        }
    }
    if contains_set_air(&script.stmts) {
        for line in emit_set_air_helper("    ") {
            out.push_str(&line);
            out.push('\n');
        }
    }
    for line in &body {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("}\n");
    (fn_name, out)
}

/// Emit one `sound_` function: its name and its full source.
///
/// Shares [`emit_stmts`] with [`emit_move_fn`] rather than wrapping [`emit_sound_body`], so the
/// two categories cannot drift in how they spell a statement. The only difference between them
/// is the prefix on the function name — which is the whole of what the game uses to decide
/// which of a fighter's four scripts this one replaces.
fn emit_sound_move_fn(script: &crate::data::AcmdScript, move_name: &str) -> (String, String) {
    let fn_name = script_function_name("sound", move_name);
    let mut out = format!("unsafe extern \"C\" fn {fn_name}(agent: &mut L2CAgentBase) {{\n");
    if contains_set_speed(&script.stmts) {
        for line in emit_set_speed_helper("    ") {
            out.push_str(&line);
            out.push('\n');
        }
    }
    if contains_clr_speed(&script.stmts) {
        for line in emit_clr_speed_helper("    ") {
            out.push_str(&line);
            out.push('\n');
        }
    }
    if contains_set_air(&script.stmts) {
        for line in emit_set_air_helper("    ") {
            out.push_str(&line);
            out.push('\n');
        }
    }
    for line in emit_stmts(&script.stmts, "    ") {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("}\n");
    (fn_name, out)
}

/// Emit one `expression_` function: its name and full source.
fn emit_expression_move_fn(script: &crate::data::AcmdScript, move_name: &str) -> (String, String) {
    let fn_name = script_function_name("expression", move_name);
    let mut out = format!("unsafe extern \"C\" fn {fn_name}(agent: &mut L2CAgentBase) {{\n");
    for line in emit_stmts(&script.stmts, "    ") {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("}\n");
    (fn_name, out)
}

/// hash40 of an effect name (or parse a hex "0x…" placeholder) — the id live tweaks are
/// keyed by. Mirrors app::effect_name_hash.
fn tweak_hash(name: &str) -> u64 {
    let n = name.trim();
    if let Some(hex) = n.strip_prefix("0x") {
        if let Ok(v) = u64::from_str_radix(hex, 16) {
            return v;
        }
    }
    hash40::hash40(&n.to_lowercase()).0
}

/// Render one graphic/joint argument.
///
/// A parsed name is usually the string inside a `Hash40::new("…")`, but the dumped scripts
/// also pass consts, locals, and raw hashes in these slots. Those come back out of the
/// parser as the expression text, and wrapping them in `Hash40::new("…")` again would emit
/// a graphic literally named `LOCAL_VARIABLE` — so pass anything expression-shaped through.
pub(crate) fn hash_arg(name: &str) -> String {
    let name = name.trim();
    if let Some(hex) = name.strip_prefix("0x") {
        if u64::from_str_radix(hex, 16).is_ok() {
            return format!("Hash40::new_raw({name})");
        }
    }
    if name.starts_with('*') || name.contains("::") || name.contains('(') || name.contains('"') {
        return name.to_string();
    }
    format!("Hash40::new(\"{name}\")")
}

/// Emit a typed effect point event. Controls are kept separate from the spawn path because none
/// of them owns an effect lifetime or a transform.
fn emit_effect_control(control: &crate::data::EffectControl, indent: &str) -> String {
    use crate::data::EffectControl;
    let line = match control {
        EffectControl::DetachKind { effect_name, unk } => format!(
            "macros::EFFECT_DETACH_KIND(agent, {}, {unk});",
            hash_arg(effect_name)
        ),
        EffectControl::DetachKindWork { work, unk } => format!(
            "macros::EFFECT_DETACH_KIND_WORK(agent, {}, {unk});",
            const_expr(work)
        ),
        EffectControl::OffKindWork { preset_id } => {
            let Some(preset) = crate::data::EffectOffKindWorkPreset::by_id(preset_id) else {
                return format!("{indent}compile_error!(\"invalid off-kind Work preset ID\");\n");
            };
            format!(
                "macros::EFFECT_OFF_KIND_WORK(agent, *{}, {}, {});",
                preset.work_symbol, preset.flags[0], preset.flags[1]
            )
        }
        EffectControl::EnableArea { kind } => {
            format!("macros::ENABLE_AREA(agent, {});", const_expr(kind))
        }
        EffectControl::UnableArea { kind } => {
            format!("macros::UNABLE_AREA(agent, {});", const_expr(kind))
        }
        EffectControl::SetDisableRenderOffsetLast => {
            "EffectModule::set_disable_render_offset_last(agent.module_accessor);".to_string()
        }
        EffectControl::EnableSyncInitPosLast => {
            "EffectModule::enable_sync_init_pos_last(agent.module_accessor);".to_string()
        }
        EffectControl::ReqScreen { preset_id } => {
            let Some(preset) = crate::data::EffectReqScreenPreset::by_id(preset_id) else {
                return format!("{indent}compile_error!(\"invalid req_screen preset ID\");\n");
            };
            format!(
                "EffectModule::req_screen(agent.module_accessor, Hash40::new(\"{}\"), {}, {}, {});",
                preset.effect_name, preset.flags[0], preset.flags[1], preset.flags[2]
            )
        }
        EffectControl::SetDisableCameraDepthInfluence { value } => format!(
            "ColorBlendModule::set_disable_camera_depth_influence(agent.module_accessor, {value});"
        ),
    };
    format!("{indent}{line}\n")
}

/// Emit the spawn call for one effect, reproducing the macro the script actually used.
///
/// This used to collapse every spawn to `EFFECT` or `EFFECT_FOLLOW` based on `follows_bone`
/// alone, so exporting a move that used `EFFECT_FOLLOW_FLIP` (or any of the ~20 other
/// families) silently swapped it for a different macro with different behaviour and dropped
/// its second graphic.
///
/// `spawn_func` names the macro and `extra_args` holds its tail verbatim, so the original
/// comes back out intact. A call whose tail is unknown — one the user added from scratch, or
/// one loaded from a project saved before `extra_args` existed — cannot be reissued under its
/// own name without inventing arguments for a signature this code does not know, so it falls
/// back to the plain pair. That is the old, compilable behaviour. A tail that is known to be
/// *empty* is a different thing entirely and is reissued as-is.
pub(crate) const EFFECT_FOLLOW_FALLBACK_TAIL: &[&str] = &["true"];
pub(crate) const EFFECT_FALLBACK_TAIL: &[&str] = &["0", "0", "0", "0", "0", "0", "false"];

/// The deterministic plain spawn that can represent an effect whose original tail is unknown.
///
/// Keep the macro name and tail together: the verifier uses the same table when it compares a
/// legacy `None` tail with the generated source, while non-plain macro families remain strict and
/// are still reported as downgrades.
pub(crate) fn plain_spawn_fallback(follows_bone: bool) -> (&'static str, &'static [&'static str]) {
    if follows_bone {
        ("EFFECT_FOLLOW", EFFECT_FOLLOW_FALLBACK_TAIL)
    } else {
        ("EFFECT", EFFECT_FALLBACK_TAIL)
    }
}

/// Return the fallback tail for one of the two plain spawn macro names.
pub(crate) fn plain_spawn_fallback_tail(spawn_func: &str) -> Option<&'static [&'static str]> {
    match spawn_func {
        "EFFECT_FOLLOW" => Some(EFFECT_FOLLOW_FALLBACK_TAIL),
        "EFFECT" => Some(EFFECT_FALLBACK_TAIL),
        _ => None,
    }
}

fn emit_spawn_call(call: &crate::data::EffectCall, indent: &str) -> String {
    if let Some(control) = &call.control {
        return emit_effect_control(control, indent);
    }
    // A colour command is not a spawn at all: no graphic, no joint, no transform. Everything
    // below this point would write arguments it does not have.
    if let Some(color) = &call.color {
        return emit_color_call(&call.spawn_func, color, indent);
    }

    // Trail macros take textures and trail parameters, not a transform — there is nothing
    // to rebuild them from, so the source line rides along verbatim apart from the two
    // arguments the editor does expose.
    if let Some(raw) = &call.raw_line {
        return format!("{indent}{}\n", retarget_trail_line(raw, call));
    }

    // Skeleton files expose display-case joint names; ACMD hashes the lowercase one.
    let bone = hash_arg(&call.bone_name.to_ascii_lowercase());
    let graphic = hash_arg(&call.effect_name);
    let [x, y, z] = call.offset;
    // The macros take rotation as zr, yr, xr, not in [x, y, z] order.
    let [rx, ry, rz] = call.rotation;
    let transform = format!(
        "{}, {}, {}, {}, {}, {}, {}",
        num(x),
        num(y),
        num(z),
        num(rz),
        num(ry),
        num(rx),
        num(call.scale)
    );

    let Some(source_tail) = call
        .extra_args
        .as_ref()
        .filter(|_| !call.spawn_func.is_empty())
    else {
        // The fallback pair takes ONE graphic, so a flipped call's second one is dropped
        // here rather than shifting every following argument by a slot.
        let (fallback_func, fallback_tail) = plain_spawn_fallback(call.follows_bone);
        let tail = fallback_tail.join(", ");
        return format!(
            "{indent}macros::{fallback_func}(agent, {graphic}, {bone}, {transform}, {tail});\n"
        );
    };

    // `flip_axis` is the typed view of one token retained in `extra_args` for unknown-tail
    // fidelity. Keep export correct even when a project/API caller changed the typed field
    // without also patching the duplicate tail slot; the panel normally keeps both in sync.
    let mut tail = source_tail.clone();
    if let (Some(axis), Some(tail_index)) = (
        call.flip_axis.as_ref(),
        effect_flip_axis_tail_index(&call.spawn_func),
    ) {
        if let Some(slot) = tail.get_mut(tail_index) {
            *slot = axis.clone();
        }
    }

    let graphics = match &call.effect_name_alt {
        Some(alt) => format!("{graphic}, {}", hash_arg(alt)),
        None => graphic,
    };
    let mut args = format!("agent, {graphics}, {bone}, {transform}");
    for extra in tail {
        args.push_str(", ");
        args.push_str(&extra);
    }
    format!("{indent}macros::{}({args});\n", call.spawn_func)
}

/// Emit one `FLASH` / `BURN_COLOR` line.
///
/// The arguments the command does not take are not written, and the ones it does are written
/// with plain `to_string` rather than `num`: every slot in this family is generic over
/// `ToF32`, so `2` compiles and is what the archive and the source write-back both spell.
/// Using `num` here would put a decimal point on, and the two export paths would then disagree
/// about the text for the same value — the bug A1 fixed for wind and A3 for the effect rate.
///
/// A colour whose command takes none is dropped rather than appended, and a missing one for a
/// command that does take a colour is written as zeroes. Both cases mean the editor's state
/// and the command name have come apart, which `check_effect_values` reports; emitting the
/// wrong arity here would break the build instead of the check.
fn emit_color_call(command: &str, color: &crate::data::ColorCall, indent: &str) -> String {
    let (has_transition, has_rgba) = crate::data::color_command_layout(command)
        .unwrap_or((color.transition.is_some(), color.rgba.is_some()));
    let mut args = String::from("agent");
    if has_transition {
        args.push_str(&format!(", {}", color.transition.unwrap_or(0.0)));
    }
    if has_rgba {
        for component in color.rgba.unwrap_or([0.0; 4]) {
            args.push_str(&format!(", {component}"));
        }
    }
    format!("{indent}macros::{command}({args});\n")
}

/// Argument slots of an AFTER_IMAGE trail that the editor surfaces as editable fields: the
/// first texture stands in for the effect name, and arguments 4 and 8 are the two joints the
/// ribbon is stretched between. Matches what `parse_excute_block_effects` reads back out of
/// the same call.
///
/// Slots 4 and 8 are `trail_bone1` and `trail_bone2` on the strength of the `smash-script`
/// declaration, not of the corpus — every vanilla call writes the same joint into both (and
/// into `flare_bone` at 14), so no real call can tell them apart. Anything asserting which of
/// these is which must vary the values itself; a corpus-shaped fixture agrees with a swap.
const TRAIL_GRAPHIC_SLOT: usize = 1;
const TRAIL_JOINT_SLOT: usize = 4;
const TRAIL_JOINT2_SLOT: usize = 8;

/// Re-render the trail arguments the panels let the user change.
///
/// The rest of a trail call is textures and per-frame trail parameters that no editor field
/// maps to, so it rides along untouched — but the graphic and the two joints ARE editable, and
/// replaying the line unconditionally dropped those edits with nothing to say so. Only slots
/// whose value actually differs are spliced, so an untouched trail comes back byte-identical
/// and a round trip through the emitter still reproduces the original line exactly.
///
/// The second joint is spliced only when the call carried one. A trail whose layout the parser
/// would not vouch for stores `None`, and writing slot 8 of such a call would edit whatever
/// argument happens to sit there.
fn retarget_trail_line(raw: &str, call: &crate::data::EffectCall) -> String {
    let Some(site) = crate::acmd_src::scan_macro_sites(raw, 0..raw.len())
        .into_iter()
        .find(|site| site.name.starts_with("AFTER_IMAGE"))
    else {
        return raw.to_string();
    };

    let mut out = raw.to_string();
    // Descending, so an earlier splice cannot shift a later slot's span out from under it.
    let slots = [
        call.trail_bone2
            .as_ref()
            .map(|bone| (TRAIL_JOINT2_SLOT, bone.to_ascii_lowercase())),
        Some((TRAIL_JOINT_SLOT, call.bone_name.to_ascii_lowercase())),
        Some((TRAIL_GRAPHIC_SLOT, call.effect_name.clone())),
    ];
    for (slot, wanted) in slots.into_iter().flatten() {
        let Some(span) = site.args.get(slot) else {
            continue;
        };
        let current = raw[span.clone()].trim();
        // Compare against what the PARSER would read from this slot, not the raw text, so
        // `Hash40::new("x")` and a bare `x` are not mistaken for a rename of each other.
        let parsed = extract_hash40_string(current).unwrap_or_else(|| current.to_string());
        if parsed.eq_ignore_ascii_case(&wanted) {
            continue;
        }
        out.replace_range(span.clone(), &hash_arg(&wanted));
    }
    out
}

/// Emit the call that ends `call`'s active window.
///
/// A trail is closed by `AFTER_IMAGE_OFF`, not by killing an effect kind — the name a trail
/// carries is a texture, and `EFFECT_OFF_KIND` on it terminates nothing.
fn emit_spawn_stop(call: &crate::data::EffectCall, indent: &str) -> String {
    if call.spawn_func == "AFTER_IMAGE_ON" {
        // The argument is not optional. `macros::AFTER_IMAGE_OFF` is declared
        // `<F: ToF32>(agent, unk: F)`, so the bare call this used to emit does not compile —
        // every exported move with a sword trail was an unbuildable project.
        //
        // `attack_mod_num`, not `num`: the slot is `ToF32`-generic, so both `0` and `0.0`
        // compile, and measured source uses multiple bare integer values.
        let arg = call.trail_off.unwrap_or(crate::data::TRAIL_OFF_DEFAULT);
        return format!(
            "{indent}macros::AFTER_IMAGE_OFF(agent, {});\n",
            attack_mod_num(arg)
        );
    }
    // The author's own booleans, exactly as the trail above keeps its `AFTER_IMAGE_OFF`
    // argument. Hardcoding the pair here rewrote 59% of the corpus's kills — including all
    // four that end `ganon_sword_flare`, the effect bound to the sword article Ganondorf's
    // smash attacks generate and remove within the move.
    let (default_fade, default_detach) = crate::data::EFFECT_OFF_KIND_DEFAULT;
    let fade = call.off_fade.unwrap_or(default_fade);
    let detach = call.off_detach.unwrap_or(default_detach);
    format!(
        "{indent}macros::EFFECT_OFF_KIND(agent, {}, {fade}, {detach});\n",
        hash_arg(&call.effect_name)
    )
}

/// Source lines for one added effect call, without frame wrapper or indentation.
///
/// This is what source sync inserts for a spawn the script never had: the same
/// spawn line the export emits, plus its `LAST_EFFECT_SET_*` modifier lines.
/// Colour and control calls are already complete single lines. Disabled calls
/// produce nothing, matching the export which omits them.
pub(crate) fn effect_call_insert_lines(call: &crate::data::EffectCall) -> Vec<String> {
    if call.disabled {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    for line in call.leading.iter() {
        let line = line.trim();
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    for line in emit_spawn_call(call, "").lines() {
        let line = line.trim();
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    if call.color.is_some() || call.control.is_some() {
        for line in call.trailing.iter() {
            let line = line.trim();
            if !line.is_empty() {
                lines.push(line.to_string());
            }
        }
        return lines;
    }
    if let Some(work) = &call.work_int {
        lines.push(format!(
            "visionary_last_effect_set_work_int(agent, {});",
            const_expr(work)
        ));
    }
    if let Some(offset) = call.camera_offset {
        lines.push(format!(
            "macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, {offset});"
        ));
    }
    if let Some([r, g, b]) = call.tint {
        lines.push(format!(
            "macros::LAST_EFFECT_SET_COLOR(agent, {}, {}, {});",
            num(r),
            num(g),
            num(b)
        ));
    }
    if let Some([r, g, b]) = call.particle_tint {
        lines.push(format!(
            "macros::LAST_PARTICLE_SET_COLOR(agent, {}, {}, {});",
            num(r),
            num(g),
            num(b)
        ));
    }
    if let Some(alpha) = call.alpha {
        lines.push(format!(
            "macros::LAST_EFFECT_SET_ALPHA(agent, {});",
            num(alpha)
        ));
    }
    if let Some(values) = &call.scale_w {
        lines.push(format!(
            "visionary_last_effect_set_scale_w(agent, &[{}]);",
            values
                .iter()
                .map(|value| num(*value))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(rate) = call.rate {
        lines.push(format!("macros::LAST_EFFECT_SET_RATE(agent, {rate});"));
    }
    for line in call.trailing.iter() {
        let line = line.trim();
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

/// Stop line for one added effect call, when its lifetime needs its own frame.
///
/// Returns `None` for one-shots and open-ended follows, which emit no stop.
/// Otherwise the same `EFFECT_OFF_KIND` / `AFTER_IMAGE_OFF` line the export
/// emits, without indentation.
pub(crate) fn effect_call_stop_insert_line(call: &crate::data::EffectCall) -> Option<String> {
    if call.disabled {
        return None;
    }
    let needs_stop = if crate::data::is_color_command(&call.spawn_func) || call.control.is_some() {
        false
    } else if call.raw_line.is_some()
        || call.trail_command.is_some()
        || call.spawn_func == "AFTER_IMAGE_ON"
    {
        call.active_end != crate::data::OPEN_ENDED_EFFECT_FRAME
    } else {
        call.follows_bone && call.active_end != crate::data::OPEN_ENDED_EFFECT_FRAME
    };
    if !needs_stop {
        return None;
    }
    let line = emit_spawn_stop(call, "");
    let line = line.trim().to_string();
    (!line.is_empty()).then_some(line)
}

/// Generate a smashline `effect_*` ACMD function that replays the (edited) effect-call
/// list: calls grouped by spawn frame, disabled calls omitted. `tweaks` (keyed by effect
/// hash) adds LAST_EFFECT_SET_COLOR / LAST_EFFECT_SET_RATE lines after matching spawns so
/// live color/speed multipliers ship with the mod. Authored per-spawn modifiers, including
/// `LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT` and `LAST_PARTICLE_SET_COLOR` are emitted from their
/// `EffectCall` fields.
/// `residue` is the second half of
/// [`EffectScript::to_effect_calls_and_residue`](crate::data::EffectScript::to_effect_calls_and_residue):
/// wrapped lines that belong to a frame rather than to any one call, because their frame block
/// held no spawn for them to ride on. There is no defaulted overload on purpose — passing an
/// empty map is a claim, and a caller that made it by accident would delete those lines exactly
/// as the export did before E3.
fn emit_effect_move_fn(
    calls: &[crate::data::EffectCall],
    move_name: &str,
    tweaks: &std::collections::HashMap<u64, crate::mod_project::LiveTweak>,
    residue: &std::collections::BTreeMap<u32, Vec<String>>,
) -> (String, String) {
    // Saved projects from before timing canonicalization can still reach the preview/emitter
    // directly. Work from a repaired copy so a stale one-shot end frame can never create an
    // invented stop or inflate the generated timeline, while leaving the caller's data intact.
    let canonical_calls: Vec<crate::data::EffectCall> = calls
        .iter()
        .cloned()
        .map(crate::data::EffectCall::normalized_timing)
        .collect();
    let calls = canonical_calls.as_slice();
    let fn_name = script_function_name("effect", move_name);
    // A follow effect's end frame is an ACMD event, not an intrinsic particle lifetime.
    // Keep starts and stops in one ordered timeline so a finite `active_end` actually emits
    // EFFECT_OFF_KIND. Stops for an older instance run before starts at the same frame; a
    // zero-duration call stops after its own start.
    let mut events: std::collections::BTreeMap<
        u32,
        (Vec<&crate::data::EffectCall>, Vec<&crate::data::EffectCall>),
    > = std::collections::BTreeMap::new();
    for call in calls.iter().filter(|call| !call.disabled) {
        events.entry(call.active_start).or_default().1.push(call);
        if call.follows_bone && call.active_end != crate::data::OPEN_ENDED_EFFECT_FRAME {
            events
                .entry(call.active_end.max(call.active_start))
                .or_default()
                .0
                .push(call);
        }
    }
    // A frame can exist because of residue alone — the source wrote a block there and this
    // editor modelled nothing in it. Give it an entry so the loop below writes its `frame()`.
    for frame in residue.keys() {
        events.entry(*frame).or_default();
    }

    let mut out = String::new();
    out.push_str(&format!(
        "unsafe extern \"C\" fn {fn_name}(agent: &mut L2CAgentBase) {{\n"
    ));
    if calls
        .iter()
        .any(|call| !call.disabled && call.work_int.is_some())
    {
        for line in emit_last_effect_set_work_int_helper("    ") {
            out.push_str(&line);
            out.push('\n');
        }
    }
    if calls
        .iter()
        .any(|call| !call.disabled && call.scale_w.is_some())
        || residue
            .values()
            .flatten()
            .any(|line| line.contains("visionary_last_effect_set_scale_w("))
    {
        for line in emit_last_effect_set_scale_w_helper("    ") {
            out.push_str(&line);
            out.push('\n');
        }
    }
    for (frame, (stops, starts)) in events {
        out.push_str(&format!("    frame(agent.lua_state_agent, {frame}.0);\n"));

        // Frame-anchored residue comes first: it was written before anything this editor
        // modelled, because the only reason it is here rather than on a call is that the walk
        // reached the end of the frame block without finding a spawn to hand it to. It arrives
        // with its own `is_excute` wrapper and, where the source had one, its own guard.
        if let Some(lines) = residue.get(&frame) {
            push_carried(&mut out, lines, "    ");
        }

        // Split the frame's spawns into the runs that can share one `is_excute` block. A run
        // ends where the guard changes or where a carried line has to come between two spawns,
        // because those lines arrive already wrapped in their own `if`/`is_excute` and cannot
        // be emitted inside somebody else's block.
        //
        // A frame with no guard and no carried lines produces exactly one run, which is the
        // single block this emitter has always written — the shape below is a superset of the
        // old one, not a replacement for it.
        let mut chunks: Vec<Chunk> = Vec::new();
        for call in &starts {
            if !call.leading.is_empty() {
                chunks.push(Chunk::Carried(&call.leading));
            }
            chunks.push(Chunk::Spawn(call));
            if !call.trailing.is_empty() {
                chunks.push(Chunk::Carried(&call.trailing));
            }
        }
        let mut runs: Vec<Run> = Vec::new();
        for chunk in chunks {
            match chunk {
                Chunk::Carried(lines) => runs.push(Run::Carried(lines)),
                Chunk::Spawn(call) => match runs.last_mut() {
                    Some(Run::Spawns { guard, calls }) if *guard == call.guard.as_deref() => {
                        calls.push(call)
                    }
                    _ => runs.push(Run::Spawns {
                        guard: call.guard.as_deref(),
                        calls: vec![call],
                    }),
                },
            }
        }
        // Stops still need a home when every spawn at this frame was disabled or retimed away.
        // Gated on there being a stop to place: a frame that exists only because of residue has
        // neither, and an unconditional fallback would open an empty `is_excute` block after it.
        if !stops.is_empty() && !runs.iter().any(|r| matches!(r, Run::Spawns { .. })) {
            runs.push(Run::Spawns {
                guard: None,
                calls: Vec::new(),
            });
        }
        let first_spawn_run = runs
            .iter()
            .position(|r| matches!(r, Run::Spawns { .. }))
            .unwrap_or(0);
        let last_spawn_run = runs
            .iter()
            .rposition(|r| matches!(r, Run::Spawns { .. }))
            .unwrap_or(0);

        // Dedupe on the emitted line: `EFFECT_OFF_KIND` kills every live instance of a
        // kind at once, and `AFTER_IMAGE_OFF` takes no name to key on at all.
        let mut emitted_stops = std::collections::HashSet::new();

        for (index, run) in runs.iter().enumerate() {
            let (guard, run_calls) = match run {
                Run::Carried(lines) => {
                    push_carried(&mut out, lines, "    ");
                    continue;
                }
                Run::Spawns { guard, calls } => (*guard, calls),
            };
            // Deliberately not guarding the stops with it. `EFFECT_OFF_KIND` is
            // `EffectModule::kill_kind`, which is a no-op when nothing of that kind is live, so
            // an unguarded stop for a guarded spawn is harmless; a *guarded* stop would leave
            // the effect running forever on the branch that did not take the guard.
            let inner = if let Some(header) = guard {
                out.push_str(&format!("    {header}\n"));
                "        "
            } else {
                "    "
            };
            out.push_str(&format!("{inner}if macros::is_excute(agent) {{\n"));
            let body = format!("{inner}    ");

            if index == first_spawn_run {
                for call in stops
                    .iter()
                    .copied()
                    .filter(|call| call.active_start < frame)
                {
                    let stop = emit_spawn_stop(call, &body);
                    if emitted_stops.insert(stop.trim_start().to_string()) {
                        out.push_str(&stop);
                    }
                }
            }

            for call in run_calls.iter().copied() {
                out.push_str(&emit_spawn_call(call, &body));
                // Colour and C4 control events are already complete point commands. They do
                // not have a last-effect target, graphic kind, or spawn transform to reconcile.
                if call.color.is_some() || call.control.is_some() {
                    continue;
                }
                if let Some(work) = &call.work_int {
                    out.push_str(&format!(
                        "{body}visionary_last_effect_set_work_int(agent, {});\n",
                        const_expr(work)
                    ));
                }
                if let Some(offset) = call.camera_offset {
                    out.push_str(&format!(
                        "{body}macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, {offset});\n"
                    ));
                }
                let tweak = tweaks.get(&tweak_hash(&call.effect_name));
                // One tint line, never two, on exactly the terms the rate below uses: a live colour
                // multiplier is a deliberate replacement of this kind's tint, so it wins over the
                // spawn's own `LAST_EFFECT_SET_COLOR`. Before this the script's line was not in
                // `EffectCall` at all, so there was nothing to reconcile with — the export wrote the
                // tweak's tint and silently deleted the script's, which is the loss C5 measured at
                // 33 occurrences and the reason this is the biggest single item in the family.
                //
                // The tweak's fourth component is deliberately ignored. It is the live form's alpha,
                // which no panel exposes and `live_tweak_from_override` does not test for identity,
                // so emitting it would ship an opacity the user never set.
                let tint = tweak
                    .and_then(|tw| tw.color)
                    .map(|[r, g, b, _a]| [r, g, b])
                    .or(call.tint);
                if let Some([r, g, b]) = tint {
                    // `num`, not the bare `to_string` the rate uses, because every one of the 65
                    // colour calls in the archive is written with a decimal point while its rates
                    // are whole numbers. Matching each macro's own spelling is what keeps a
                    // re-exported vanilla script textually identical to the one it came from.
                    out.push_str(&format!(
                        "{body}macros::LAST_EFFECT_SET_COLOR(agent, {}, {}, {});\n",
                        num(r),
                        num(g),
                        num(b)
                    ));
                }
                if let Some([r, g, b]) = call.particle_tint {
                    out.push_str(&format!(
                        "{body}macros::LAST_PARTICLE_SET_COLOR(agent, {}, {}, {});\n",
                        num(r),
                        num(g),
                        num(b)
                    ));
                }
                if let Some(alpha) = call.alpha {
                    // No tweak counterpart to reconcile with: the live override has no opacity
                    // control, so a spawn's alpha can only ever come from its own script line.
                    out.push_str(&format!(
                        "{body}macros::LAST_EFFECT_SET_ALPHA(agent, {});\n",
                        num(alpha)
                    ));
                }
                if let Some(values) = &call.scale_w {
                    out.push_str(&format!(
                        "{body}visionary_last_effect_set_scale_w(agent, &[{}]);\n",
                        values
                            .iter()
                            .map(|value| num(*value))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                // One rate line, never two. A live speed tweak is a deliberate override of this
                // kind's playback rate, so it wins over the spawn's own — emitting both would
                // leave the second one winning anyway, and reading the pair back would attribute
                // the tweak's value to the script. Otherwise the spawn's rate is written, which
                // is what makes a vanilla `LAST_EFFECT_SET_RATE` survive an export at all: it is
                // read into the call and re-emitted here, rather than dropped on the floor.
                if let Some(rate) = tweak.and_then(|tw| tw.speed).or(call.rate) {
                    // Deliberately not `num`, which puts a decimal point back on: the rate slot is
                    // generic over `ToF32`, so `2` compiles and is what both the archive and the
                    // source write-back spell. Emitting `2.0` here would mean the two export paths
                    // wrote different text for the same value. `check_effect_values` refuses a
                    // non-finite rate, which is the one thing `to_string` cannot spell as Rust.
                    out.push_str(&format!(
                        "{body}macros::LAST_EFFECT_SET_RATE(agent, {rate});\n"
                    ));
                }
            }

            if index == last_spawn_run {
                for call in stops
                    .iter()
                    .copied()
                    .filter(|call| call.active_start >= frame)
                {
                    let stop = emit_spawn_stop(call, &body);
                    if emitted_stops.insert(stop.trim_start().to_string()) {
                        out.push_str(&stop);
                    }
                }
            }
            out.push_str(&format!("{inner}}}\n"));
            if guard.is_some() {
                out.push_str("    }\n");
            }
        }
    }
    out.push_str("}\n");
    (fn_name, out)
}

/// `smash-script` declares the linked `LAST_EFFECT_SET_WORK_INT` primitive but has no Rust
/// wrapper for it. Generated effect projects therefore use the same Lua-stack setup as the
/// crate's wrappers, retaining the authored Work ID token at the call site.
pub(crate) fn emit_last_effect_set_work_int_helper(indent: &str) -> Vec<String> {
    vec![
        format!("{indent}#[inline]"),
        format!("{indent}#[allow(dead_code)]"),
        format!(
            "{indent}unsafe fn visionary_last_effect_set_work_int(agent: &mut L2CAgentBase, work: i32) {{"
        ),
        format!("{indent}    agent.clear_lua_stack();"),
        format!("{indent}    lua_args!(agent, work);"),
        format!(
            "{indent}    smash::app::sv_animcmd::LAST_EFFECT_SET_WORK_INT(agent.lua_state_agent);"
        ),
        format!("{indent}    agent.clear_lua_stack();"),
        format!("{indent}}}"),
    ]
}

/// The native `LAST_EFFECT_SET_SCALE_W` primitive consumes one to three values from the Lua
/// stack, but the vendored `smash-script` wrapper only exposes three typed arguments. Generated
/// effect projects therefore push the authored number of values directly and call the linked
/// primitive, preserving the dump's dynamic-arity semantics.
pub(crate) fn emit_last_effect_set_scale_w_helper(indent: &str) -> Vec<String> {
    vec![
        format!("{indent}#[inline]"),
        format!("{indent}#[allow(dead_code)]"),
        format!(
            "{indent}unsafe fn visionary_last_effect_set_scale_w(agent: &mut L2CAgentBase, values: &[f32]) {{"
        ),
        format!("{indent}    agent.clear_lua_stack();"),
        format!("{indent}    for value in values {{"),
        format!(
            "{indent}        let mut value = smash::lib::L2CValue::new_num(*value);"
        ),
        format!("{indent}        agent.push_lua_stack(&mut value);"),
        format!("{indent}    }}"),
        format!(
            "{indent}    smash::app::sv_animcmd::LAST_EFFECT_SET_SCALE_W(agent.lua_state_agent);"
        ),
        format!("{indent}    agent.clear_lua_stack();"),
        format!("{indent}}}"),
    ]
}

/// One piece of a frame block, before consecutive spawns are merged into shared `is_excute`s.
enum Chunk<'a> {
    Spawn(&'a crate::data::EffectCall),
    Carried(&'a [String]),
}

/// One emitted unit inside a frame block: either an `is_excute` block full of spawns that share
/// a guard, or a run of carried lines that arrive with their own wrapper already on them.
enum Run<'a> {
    Spawns {
        guard: Option<&'a str>,
        calls: Vec<&'a crate::data::EffectCall>,
    },
    Carried(&'a [String]),
}

/// Write carried lines, re-indenting them to sit under `base`.
///
/// The stored lines are unindented and brace-balanced as a group, so the nesting is recomputed
/// here rather than saved. Saving it would freeze one indentation into the project file and
/// leave a reloaded move mis-aligned the moment the surrounding block changed depth.
fn push_carried(out: &mut String, lines: &[String], base: &str) {
    let mut depth: i32 = 0;
    for line in lines {
        let opens = line.matches('{').count() as i32;
        let closes = line.matches('}').count() as i32;
        // A line that closes more than it opens dedents itself before printing, so `}` lines
        // up with the header that opened it rather than with the body.
        let at = if closes > opens {
            depth - (closes - opens)
        } else {
            depth
        };
        out.push_str(base);
        for _ in 0..at.max(0) {
            out.push_str("    ");
        }
        out.push_str(line);
        out.push('\n');
        depth += opens - closes;
    }
}

// ── Export preview ────────────────────────────────────────────────────────────

/// The exact `game_*` function an export would write for this move.
///
/// Calls the same emitter [`build_mod_project_full`] does, so what the preview shows and
/// what lands in `acmd_source/` cannot drift apart.
pub fn preview_game_fn(script: &crate::data::AcmdScript, move_name: &str) -> String {
    emit_move_fn(script, move_name).1
}

/// The exact lines an export would write inside one `is_excute` block.
///
/// Only the verifier needs this: comparing what the emitter *produces* is the one way to tell
/// two statements apart that differ in the editor but collapse to the same call in the file.
pub fn preview_excute_stmts(stmts: &[crate::data::ExcuteStmt]) -> Vec<String> {
    emit_excute_stmts(stmts, "")
}

/// Render one effect command through the established typed spawn/control owners.
/// Callers validate authored payloads before turning this source into node output.
pub fn preview_effect_macro(command: &crate::data::EffectMacro) -> String {
    if let crate::data::EffectMacro::EffectFollowWork {
        work_symbol,
        bone_name,
        offset,
        rotation,
        scale,
        unk8,
    } = command
    {
        return format!(
            "macros::EFFECT_FOLLOW_WORK(agent, *{work_symbol}, Hash40::new(\"{bone_name}\"), {}, {}, {}, {}, {}, {}, {}, {unk8});",
            offset[0], offset[1], offset[2], rotation[2], rotation[1], rotation[0], scale
        );
    }
    let script = crate::data::EffectScript {
        stmts: vec![crate::data::EffectStmt::Excute(vec![command.clone()])],
    };
    let (calls, residue) = script.to_effect_calls_and_residue();
    let mut source = String::new();
    for call in &calls {
        source.push_str(&emit_spawn_call(call, ""));
    }
    for lines in residue.values() {
        for line in lines {
            source.push_str(line);
            source.push('\n');
        }
    }
    strip_effect_execute_wrapper(&source).unwrap_or(source)
}

/// The singleton preview is embedded inside the timeline's own structured `Excute` node.
/// Some residue renderers include that same block in their source, so remove only a verified
/// outer `if macros::is_excute(agent)` while retaining its contents.
fn strip_effect_execute_wrapper(source: &str) -> Option<String> {
    let wrapped = format!(
        "unsafe extern \"C\" fn __effect_preview(agent: &mut L2CAgentBase) {{\n{source}\n}}\n"
    );
    let file = syn::parse_file(&wrapped).ok()?;
    let function = file.items.iter().find_map(|item| match item {
        syn::Item::Fn(function) if function.sig.ident == "__effect_preview" => Some(function),
        _ => None,
    })?;
    let [syn::Stmt::Expr(syn::Expr::If(branch), _)] = function.block.stmts.as_slice() else {
        return None;
    };
    if branch.else_branch.is_some() || !is_excute_condition(&branch.cond) {
        return None;
    }
    let lines = source.lines().collect::<Vec<_>>();
    if lines.first()?.trim() != "if macros::is_excute(agent) {" || lines.last()?.trim() != "}" {
        return None;
    }
    let body = &lines[1..lines.len() - 1];
    if body.is_empty() {
        return None;
    }
    Some(body.join("\n").trim().to_owned())
}

fn is_excute_condition(condition: &syn::Expr) -> bool {
    let syn::Expr::Call(call) = condition else {
        return false;
    };
    let syn::Expr::Path(function) = call.func.as_ref() else {
        return false;
    };
    let path = &function.path;
    let argument_is_agent = call.args.len() == 1
        && matches!(call.args.first(), Some(syn::Expr::Path(path)) if path.path.is_ident("agent"));
    path.segments.len() == 2
        && path.segments[0].ident == "macros"
        && path.segments[1].ident == "is_excute"
        && argument_is_agent
}

/// Emit a complete effects statement tree without flattening its control flow.
pub fn preview_effect_structure_fn(script: &crate::data::AcmdScript, move_name: &str) -> String {
    let fn_name = script_function_name("effect", move_name);
    let mut source = format!("unsafe extern \"C\" fn {fn_name}(agent: &mut L2CAgentBase) {{\n");
    if serde_json::to_string(script)
        .is_ok_and(|json| json.contains("visionary_last_effect_set_scale_w"))
    {
        for line in emit_last_effect_set_scale_w_helper("    ") {
            source.push_str(&line);
            source.push('\n');
        }
    }
    for line in emit_stmts(&script.stmts, "    ") {
        source.push_str(&line);
        source.push('\n');
    }
    source.push_str("}\n");
    source
}

/// The exact `sound_*` function an export would write for this move.
pub fn preview_sound_fn(script: &crate::data::AcmdScript, move_name: &str) -> String {
    emit_sound_move_fn(script, move_name).1
}

/// The exact `expression_*` function an export would write for this move.
pub fn preview_expression_fn(script: &crate::data::AcmdScript, move_name: &str) -> String {
    emit_expression_move_fn(script, move_name).1
}

/// The exact `effect_*` function an export would write for this move.
pub fn preview_effect_fn(
    calls: &[crate::data::EffectCall],
    move_name: &str,
    live_tweaks: &[crate::mod_project::LiveTweak],
    residue: &std::collections::BTreeMap<u32, Vec<String>>,
) -> String {
    let tweaks = live_tweaks
        .iter()
        .map(|t| (tweak_hash(&t.effect_name), t.clone()))
        .collect();
    emit_effect_move_fn(calls, move_name, &tweaks, residue).1
}

/// Spawns that an export cannot reissue under the macro their script used.
///
/// Returns `(spawn function, effect name)` for each. This is the one way a generated script
/// still departs from the original, and it is a data problem rather than a code one: the
/// call's trailing arguments were never recorded, so there is no way to spell its signature.
/// Reloading the move from a script or a live capture fills them in.
pub fn export_spawn_downgrades(calls: &[crate::data::EffectCall]) -> Vec<(String, String)> {
    calls
        .iter()
        .filter(|call| {
            !call.disabled
                && call.control.is_none()
                && call.raw_line.is_none()
                // A colour command has no tail to be missing — its arguments are its whole
                // content and are held as values, so it always re-emits under its own name.
                && call.color.is_none()
                && !call.spawn_func.is_empty()
                && call.extra_args.is_none()
                // The fallback emits exactly these two, so they are not a downgrade.
                && call.spawn_func != "EFFECT"
                && call.spawn_func != "EFFECT_FOLLOW"
        })
        .map(|call| (call.spawn_func.clone(), call.effect_name.clone()))
        .collect()
}

/// The lines of an effect script that an export would not write back.
///
/// [`emit_effect_move_fn`] rebuilds the whole function out of `EffectCall`s, so a line that
/// became no call and rode along on no call is not reproduced anywhere in the output. It is
/// deleted, and until C5 nothing anywhere said so.
///
/// C6 moved most of this list into the export rather than shortening the report artificially,
/// so what is named here is what is genuinely still lost. Three things reach it:
///
/// - **Statement-level `Raw`.** `wait_loop_sync_mot`, bare `EffectModule::` calls, script
///   plumbing. Deliberately never carried — see the `EffectStmt::Raw` arm of
///   `eval_effect_stmts` for why re-emitting a timing primitive would be worse than dropping it.
/// - **A nested conditional's header.** One guard per spawn is modelled; an inner one would
///   have to overwrite the outer, so it is reported instead.
///
/// A third source used to feed this list and no longer does: residue whose frame block held no
/// spawn to ride on. E3 gave those lines a frame of their own in the output instead of a report
/// — see [`crate::data::EffectScript::to_effect_calls_and_residue`]. This function is therefore
/// decided entirely from the statement tree now, which is why it does not consult the walk.
///
/// Lines carrying no letters or digits are left out. The emitter regenerates every brace it
/// needs, so a bare `}` is not a loss, and listing one per block would bury the lines that are.
pub fn unexportable_effect_lines(script: &crate::data::EffectScript) -> Vec<String> {
    fn keep(line: &str, out: &mut Vec<String>) {
        let line = line.trim();
        if line.chars().any(|c| c.is_alphanumeric()) {
            out.push(line.to_string());
        }
    }
    fn walk(stmts: &[EffectStmt], depth: usize, out: &mut Vec<String>) {
        for (index, stmt) in stmts.iter().enumerate() {
            match stmt {
                EffectStmt::BareNativeCall(call) => keep(&call.source(), out),
                EffectStmt::BareEffect(command) => keep(&preview_effect_macro(command), out),
                EffectStmt::Raw(line) => keep(line, out),
                EffectStmt::NativeExecuteFrame(frame) => keep(
                    &format!(
                        "smash::app::sv_animcmd::execute(agent.lua_state_agent, {});",
                        num(*frame)
                    ),
                    out,
                ),
                EffectStmt::NativeMotionLoopSync => keep(
                    "smash::app::sv_animcmd::wait_loop_sync_mot(agent.lua_state_agent);",
                    out,
                ),
                // Not reported from the tree. Whether an untyped macro survives depends on
                // whether a spawn shares its frame block, which only the resolving walk knows;
                // reporting from here would name lines the export does write.
                EffectStmt::Excute(_) => {}
                EffectStmt::Loop {
                    body, motion_loop, ..
                } => {
                    walk(body, depth, out);
                    // The flat export writes one pass; the loop's wait does not survive it.
                    if *motion_loop == crate::data::MotionLoop::SyncMotion {
                        keep(
                            "smash::app::sv_animcmd::wait_loop_sync_mot(agent.lua_state_agent);",
                            out,
                        );
                    }
                }
                EffectStmt::Cond { header, body } => {
                    if depth > 0 || crate::data::effect_execute_arm(stmts, index) {
                        keep(header, out);
                    }
                    walk(body, depth + 1, out);
                }
                EffectStmt::Frame(_) | EffectStmt::Wait(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&script.stmts, 0, &mut out);
    out
}

/// Build a complete, compilable skyline-rs mod project for all the provided edits.
///
/// `edits` — list of `(fighter_name, move_name, script)` tuples (all fighters combined).
/// `plugin_name` — the Cargo package name, e.g. `"my_hitbox_mod"`.
#[cfg(test)]
pub fn build_mod_project(
    edits: &[(String, String, crate::data::AcmdScript)],
    plugin_name: &str,
) -> ModProject {
    build_mod_project_full(edits, &[], &[], &[], plugin_name)
}

/// One move's effect script as an export needs it: `(fighter, move, full edited call list,
/// frame-anchored residue)`.
///
/// The fourth element is the second half of
/// [`EffectScript::to_effect_calls_and_residue`](crate::data::EffectScript::to_effect_calls_and_residue),
/// carried this far because nothing downstream can recover it — the emitter regenerates the
/// whole function from the call list, and these lines are not in it. It is a tuple rather than a
/// struct to stay the shape `sound_edits` and `edits` already are.
pub type EffectExport = (
    String,
    String,
    Vec<crate::data::EffectCall>,
    std::collections::BTreeMap<u32, Vec<String>>,
);

/// Like [`build_mod_project`] but also generates `effect_*` and `sound_*` ACMD scripts.
///
/// `effect_edits` — see [`EffectExport`]; the generated effect script
/// REPLACES the move's original effect script, so the list must be the complete set of
/// calls (pristine + user edits applied), not just the changed ones.
///
/// `sound_edits` — `(fighter, move, whole sound script)`, on the same terms and for the same
/// reason: a `sound_` function the plugin installs replaces the fighter's own, so a partial one
/// would delete every call it left out. Passing only *changed* moves is correct and passing
/// only *changed calls* is not.
#[cfg(test)]
pub fn build_mod_project_full(
    edits: &[(String, String, crate::data::AcmdScript)],
    effect_edits: &[EffectExport],
    sound_edits: &[(String, String, crate::data::AcmdScript)],
    live_tweaks: &[crate::mod_project::LiveTweak],
    plugin_name: &str,
) -> ModProject {
    build_mod_project_full_with_expression(
        edits,
        effect_edits,
        sound_edits,
        &[],
        live_tweaks,
        plugin_name,
    )
}

/// One move that belongs to a single costume rather than to the fighter as a whole:
/// `(fighter, move) → (slots, the fighter's own script source)`.
///
/// The original source is not optional. A Smashline script replaces the move for *every*
/// costume, so a gate with no else arm removes the move from the donor and every other costume
/// of it — a break that only shows up when someone plays the donor normally.
/// `slots` holds every costume the gate covers, ascending (one entry for a
/// single-slot character, 8 for a c08–c15 one).
pub type SlotGates = std::collections::HashMap<(String, String), (Vec<u8>, String)>;

/// Like [`build_mod_project_full`] with editable `expression_` scripts included.
#[cfg(test)]
pub fn build_mod_project_full_with_expression(
    edits: &[(String, String, crate::data::AcmdScript)],
    effect_edits: &[EffectExport],
    sound_edits: &[(String, String, crate::data::AcmdScript)],
    expression_edits: &[(String, String, crate::data::AcmdScript)],
    live_tweaks: &[crate::mod_project::LiveTweak],
    plugin_name: &str,
) -> ModProject {
    build_mod_project_with_slot_gates(
        edits,
        effect_edits,
        sound_edits,
        expression_edits,
        live_tweaks,
        plugin_name,
        &SlotGates::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn append_category_source(
    output: &mut String,
    gates: &SlotGates,
    fighter: &str,
    move_name: &str,
    category: &str,
    registered_name: &str,
    source: &str,
) {
    match gates.get(&(fighter.to_string(), format!("{category}:{move_name}"))) {
        Some((slots, original)) => {
            output.push_str(&crate::roster::scaffold::costume_gated_source_multi(
                registered_name,
                slots,
                source,
                original,
            ))
        }
        None => output.push_str(source),
    }
}

/// Like [`build_mod_project_full_with_expression`], with `game_` scripts that belong to one
/// costume emitted behind a costume gate.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn build_mod_project_with_slot_gates(
    edits: &[(String, String, crate::data::AcmdScript)],
    effect_edits: &[EffectExport],
    sound_edits: &[(String, String, crate::data::AcmdScript)],
    expression_edits: &[(String, String, crate::data::AcmdScript)],
    live_tweaks: &[crate::mod_project::LiveTweak],
    plugin_name: &str,
    slot_gates: &SlotGates,
) -> ModProject {
    build_mod_project_with_effect_structures(
        edits,
        effect_edits,
        &[],
        sound_edits,
        expression_edits,
        live_tweaks,
        plugin_name,
        slot_gates,
    )
}

/// Render full effect control trees alongside the legacy flat effect editor.
#[allow(clippy::too_many_arguments)]
pub fn build_mod_project_with_effect_structures(
    edits: &[(String, String, crate::data::AcmdScript)],
    effect_edits: &[EffectExport],
    effect_scripts: &[(String, String, crate::data::AcmdScript)],
    sound_edits: &[(String, String, crate::data::AcmdScript)],
    expression_edits: &[(String, String, crate::data::AcmdScript)],
    live_tweaks: &[crate::mod_project::LiveTweak],
    plugin_name: &str,
    slot_gates: &SlotGates,
) -> ModProject {
    use std::collections::HashMap;

    let tweaks: HashMap<u64, crate::mod_project::LiveTweak> = live_tweaks
        .iter()
        .map(|t| (tweak_hash(&t.effect_name), t.clone()))
        .collect();

    // Group by fighter
    let mut by_fighter: HashMap<&str, Vec<(&str, &crate::data::AcmdScript)>> = HashMap::new();
    for (fighter, move_name, script) in edits {
        by_fighter
            .entry(fighter.as_str())
            .or_default()
            .push((move_name.as_str(), script));
    }
    #[allow(clippy::type_complexity)]
    let mut fx_by_fighter: HashMap<
        &str,
        Vec<(
            &str,
            &Vec<crate::data::EffectCall>,
            &std::collections::BTreeMap<u32, Vec<String>>,
        )>,
    > = HashMap::new();
    for (fighter, move_name, calls, residue) in effect_edits {
        fx_by_fighter.entry(fighter.as_str()).or_default().push((
            move_name.as_str(),
            calls,
            residue,
        ));
        // Ensure the fighter appears even with no hitbox edits.
        by_fighter.entry(fighter.as_str()).or_default();
    }
    let mut sfx_by_fighter: HashMap<&str, Vec<(&str, &crate::data::AcmdScript)>> = HashMap::new();
    let mut full_fx_by_fighter: HashMap<&str, Vec<(&str, &crate::data::AcmdScript)>> =
        HashMap::new();
    for (fighter, move_name, script) in effect_scripts {
        full_fx_by_fighter
            .entry(fighter.as_str())
            .or_default()
            .push((move_name.as_str(), script));
        by_fighter.entry(fighter.as_str()).or_default();
    }
    for (fighter, move_name, script) in sound_edits {
        sfx_by_fighter
            .entry(fighter.as_str())
            .or_default()
            .push((move_name.as_str(), script));
        by_fighter.entry(fighter.as_str()).or_default();
    }
    let mut expr_by_fighter: HashMap<&str, Vec<(&str, &crate::data::AcmdScript)>> = HashMap::new();
    for (fighter, move_name, script) in expression_edits {
        expr_by_fighter
            .entry(fighter.as_str())
            .or_default()
            .push((move_name.as_str(), script));
        by_fighter.entry(fighter.as_str()).or_default();
    }

    let mut files: Vec<GeneratedFile> = Vec::new();

    // ── rust-toolchain.toml ───────────────────────────────────────────────
    // cargo-skyline installs its own "skyline-v3" toolchain via update-std,
    // which bundles the correct stdlib and an older Cargo. Plain nightly is
    // correct here — cargo-skyline ignores this file and uses skyline-v3.
    files.push(GeneratedFile {
        rel_path: "rust-toolchain.toml".into(),
        contents: r#"[toolchain]
channel = "nightly"
"#
        .to_string(),
    });

    // ── Cargo.toml ────────────────────────────────────────────────────────
    files.push(GeneratedFile {
        rel_path: "Cargo.toml".into(),
        contents: format!(
r#"[package]
name = "{plugin_name}"
version = "0.1.0"
edition = "2018"

[package.metadata.skyline]
titleid = "01006A800016E000"

[lib]
crate-type = ["cdylib"]

[dependencies]
skyline = {{ git = "https://github.com/ultimate-research/skyline-rs" }}
skyline_smash = {{ git = "https://github.com/ultimate-research/skyline-smash", features = ["weak_l2cvalue"] }}
smash_script = {{ git = "https://github.com/WuBoytH/smash-script", rev = "24c5b69b79c7d7b041c3993ecd766ceccf950c2b" }}
smashline = {{ git = "https://github.com/hdr-development/smashline", rev = "df194a03b918116adb7bda1c2b4565dcd82a4756" }}

[profile.dev]
panic = "abort"

[profile.release]
opt-level = "z"
panic = "abort"
lto = true
codegen-units = 1
"#,
            plugin_name = plugin_name,
        ),
    });

    // ── src/lib.rs ────────────────────────────────────────────────────────
    let mut fighter_names: Vec<&str> = by_fighter.keys().copied().collect();
    fighter_names.sort();

    let mut used_modules: HashMap<String, usize> = HashMap::new();
    let fighter_modules: Vec<(&str, String)> = fighter_names
        .iter()
        .map(|fighter| {
            let base = rust_module_name(fighter);
            let count = used_modules.entry(base.clone()).or_default();
            *count += 1;
            let module = if *count == 1 {
                base
            } else {
                format!("{base}_{}", *count)
            };
            (*fighter, module)
        })
        .collect();
    let mod_decls: String = fighter_modules
        .iter()
        .map(|(_, module)| format!("mod {module};\n"))
        .collect();
    let installs: String = fighter_modules
        .iter()
        .map(|(_, module)| format!("    {module}::install();\n"))
        .collect();

    files.push(GeneratedFile {
        rel_path: "src/lib.rs".into(),
        contents: format!(
            r#"// Auto-generated by Visionary
#![feature(proc_macro_hygiene)]
#![allow(unused_macros, unused_imports)]

{mod_decls}
#[skyline::main(name = "{plugin_name}")]
pub fn main() {{
{installs}}}
"#,
            mod_decls = mod_decls,
            plugin_name = plugin_name,
            installs = installs,
        ),
    });

    // ── Per-fighter files ─────────────────────────────────────────────────
    for (fighter, module) in &fighter_modules {
        let moves = &by_fighter[fighter];

        // src/{fighter}/mod.rs
        files.push(GeneratedFile {
            rel_path: format!("src/{module}/mod.rs"),
            contents: format!(
                r#"mod acmd;

pub fn install() {{
    let agent = &mut smashline::Agent::new("{fighter}");
    acmd::install(agent);
    agent.install();
}}
"#,
                fighter = fighter,
            ),
        });

        // src/{fighter}/acmd.rs — all moves for this fighter in one file
        let mut acmd_src = String::new();

        let mut sorted_moves = moves.clone();
        sorted_moves.sort_by_key(|(m, _)| *m);

        // (fn_name, acmd_script_name) pairs for the install block
        let mut fn_entries: Vec<(String, String)> = Vec::new();

        for (move_name, script) in &sorted_moves {
            // `attack_s3_s` installs as `game_attacks3` upstream (Ryu/Ken's real
            // `game_attacks3s` excepted): resolve the motion to the script's
            // spelling so the emitted function is the one the game calls.
            let mut script_move = resolve_script_move_name(fighter, move_name);
            if let Some((_, original)) =
                slot_gates.get(&((*fighter).to_string(), (*move_name).to_string()))
            {
                // A costume gate carries the donor's own script as the else arm;
                // its declared name is the spelling that fighter actually runs.
                if let Some(declared) = declared_fn_name(original) {
                    let moves = move_script_aliases(move_name);
                    let scripts = acmd_script_candidates("game", move_name);
                    if let Some(index) = scripts.iter().position(|name| name == &declared) {
                        if let Some(matched) = moves.get(index) {
                            script_move = matched.clone();
                        }
                    }
                }
            }
            let (fn_name, fn_src) = emit_move_fn(script, &script_move);
            // The acmd script name used in agent.acmd() is "game_{movename_no_underscores}"
            let acmd_name = script_function_name("game", &script_move);
            match slot_gates.get(&((*fighter).to_string(), (*move_name).to_string())) {
                Some((slots, original)) => {
                    // The registered name stays `fn_name`; the authored body moves to a
                    // private name and the fighter's own script becomes the other arm.
                    acmd_src.push_str(&crate::roster::scaffold::costume_gated_source_multi(
                        &fn_name, slots, &fn_src, original,
                    ));
                }
                None => acmd_src.push_str(&fn_src),
            }
            acmd_src.push('\n');
            fn_entries.push((fn_name, acmd_name));
        }

        // Effect scripts (edited spawn lists) for this fighter
        if let Some(fx_moves) = fx_by_fighter.get(fighter) {
            let mut sorted_fx = fx_moves.clone();
            sorted_fx.sort_by_key(|(m, _, _)| *m);
            for (move_name, calls, residue) in &sorted_fx {
                let script_move = resolve_script_move_name(fighter, move_name);
                let (fn_name, fn_src) = emit_effect_move_fn(calls, &script_move, &tweaks, residue);
                append_category_source(
                    &mut acmd_src,
                    slot_gates,
                    fighter,
                    move_name,
                    "effect",
                    &fn_name,
                    &fn_src,
                );
                acmd_src.push('\n');
                fn_entries.push((fn_name.clone(), fn_name));
            }
        }

        // Sound scripts for this fighter.
        if let Some(effect_moves) = full_fx_by_fighter.get(fighter) {
            let mut sorted = effect_moves.clone();
            sorted.sort_by_key(|(name, _)| *name);
            for (move_name, script) in sorted {
                let script_move = resolve_script_move_name(fighter, move_name);
                let name = script_function_name("effect", &script_move);
                append_category_source(
                    &mut acmd_src,
                    slot_gates,
                    fighter,
                    move_name,
                    "effect",
                    &name,
                    &preview_effect_structure_fn(script, &script_move),
                );
                acmd_src.push('\n');
                fn_entries.push((name.clone(), name));
            }
        }

        if let Some(sfx_moves) = sfx_by_fighter.get(fighter) {
            let mut sorted_sfx = sfx_moves.clone();
            sorted_sfx.sort_by_key(|(m, _)| *m);
            for (move_name, script) in &sorted_sfx {
                let script_move = resolve_script_move_name(fighter, move_name);
                let (fn_name, fn_src) = emit_sound_move_fn(script, &script_move);
                append_category_source(
                    &mut acmd_src,
                    slot_gates,
                    fighter,
                    move_name,
                    "sound",
                    &fn_name,
                    &fn_src,
                );
                acmd_src.push('\n');
                fn_entries.push((fn_name.clone(), fn_name));
            }
        }

        // Expression scripts for this fighter.
        if let Some(expression_moves) = expr_by_fighter.get(fighter) {
            let mut sorted_expression = expression_moves.clone();
            sorted_expression.sort_by_key(|(m, _)| *m);
            for (move_name, script) in &sorted_expression {
                let script_move = resolve_script_move_name(fighter, move_name);
                let (fn_name, fn_src) = emit_expression_move_fn(script, &script_move);
                append_category_source(
                    &mut acmd_src,
                    slot_gates,
                    fighter,
                    move_name,
                    "expression",
                    &fn_name,
                    &fn_src,
                );
                acmd_src.push('\n');
                fn_entries.push((fn_name.clone(), fn_name));
            }
        }

        // install fn
        acmd_src.push_str("pub fn install(agent: &mut smashline::Agent) {\n");
        for (fn_name, acmd_name) in &fn_entries {
            acmd_src.push_str(&format!(
                "    agent.acmd(\"{acmd_name}\", {fn_name}, smashline::Priority::Default);\n"
            ));
        }
        acmd_src.push_str("}\n");

        let attack_height_used = acmd_src.contains("AttackHeight(");
        let hit_status_used = acmd_src.contains("HitStatus(");
        let attack_direction_axis_used = acmd_src.contains("AttackDirectionAxis(");
        let mut app_imports = "sv_animcmd::*, lua_bind::*, ArticleOperationTarget".to_string();
        if acmd_src.contains("singletons::") {
            app_imports.push_str(", lua_bind");
        }
        if acmd_src.contains("FighterSpecializer_Demon::") {
            app_imports.push_str(", FighterSpecializer_Demon");
        }
        if attack_direction_axis_used {
            app_imports.push_str(", AttackDirectionAxis");
        }
        if attack_height_used {
            app_imports.push_str(", AttackHeight");
        }
        if hit_status_used {
            app_imports.push_str(", HitStatus");
        }
        let mut with_imports = String::new();
        with_imports.push_str("use {\n");
        with_imports.push_str("    smash::{\n");
        with_imports.push_str("        hash40,\n");
        with_imports.push_str("        lua2cpp::*,\n");
        with_imports.push_str("        phx::*,\n");
        with_imports.push_str(&format!("        app::{{{app_imports}}},\n"));
        with_imports.push_str("        lib::lua_const::{self, *}\n");
        with_imports.push_str("    },\n");
        with_imports.push_str("    smashline::*,\n");
        with_imports.push_str("    smash_script::*\n");
        with_imports.push_str("};\n\n");
        with_imports.push_str(&effect_source_compatibility_macros(&acmd_src));
        with_imports.push_str(&crate::move_nodes::native_calls::support_source(&acmd_src));
        with_imports.push_str(&acmd_src);
        acmd_src = with_imports;

        files.push(GeneratedFile {
            rel_path: format!("src/{module}/acmd.rs"),
            contents: acmd_src,
        });
    }

    // ── README.md ─────────────────────────────────────────────────────────
    let mut script_list: Vec<String> =
        edits
            .iter()
            .map(|(fighter, move_name, _)| format!("- {fighter}: {move_name} (hitboxes)"))
            .chain(effect_edits.iter().map(|(fighter, move_name, _, _)| {
                format!("- {fighter}: {move_name} (effect spawns)")
            }))
            .chain(
                sound_edits
                    .iter()
                    .map(|(fighter, move_name, _)| format!("- {fighter}: {move_name} (sounds)")),
            )
            .chain(
                expression_edits.iter().map(|(fighter, move_name, _)| {
                    format!("- {fighter}: {move_name} (expression)")
                }),
            )
            .collect();
    script_list.sort();
    let move_list = script_list.join("\n");

    files.push(GeneratedFile {
        rel_path: "README.md".into(),
        contents: format!(
r#"# {plugin_name}

Skyline ACMD mod for Super Smash Bros. Ultimate.

## Edited scripts

{move_list}

## Building

Run the included build script for your platform — it handles everything automatically.

Windows:

```bat
build.bat
```

Linux and macOS:

```sh
bash build.sh
```

The compiled plugin will be at:
```
target/aarch64-skyline-switch/release/lib{plugin_name}.nro
```

## Installing on your Switch

Rename the compiled `.nro` to `plugin.nro` and place it at the root of your
ARCropolis mod folder:
```
ultimate/mods/<mod folder>/plugin.nro
```

### Required plugins (if not already installed)
Make sure the base Skyline/ARCropolis installation already provides these
runtime components. They are not copied into this mod:
- [Skyline](https://github.com/skyline-dev/skyline/releases) — copy the `exefs/` folder to `atmosphere/contents/01006A800016E000/`
- [nro_hook](https://github.com/ultimate-research/nro-hook-plugin/releases) — `libnro_hook.nro`
- [Smashline](https://github.com/HDR-Development/smashline/releases) — `libsmashline_plugin.nro`
"#,
            plugin_name = plugin_name,
            move_list = move_list,
        ),
    });

    // ── info.toml (ARCropolis mod metadata) ───────────────────────────────
    files.push(GeneratedFile {
        rel_path: "info.toml".into(),
        contents: format!(
            r#"display_name = "{plugin_name}"
authors = "Visionary"
version = "1.0"
description = """
ACMD mod generated by Visionary.
Edited scripts:
{move_list}
"""
category = "Fighter"
"#,
            plugin_name = plugin_name,
            move_list = move_list,
        ),
    });

    // ── build.sh (Linux/macOS) ────────────────────────────────────────────
    files.push(GeneratedFile {
        rel_path: "build.sh".into(),
        contents: r#"#!/usr/bin/env bash
set -e

# ── 1. Install rustup if missing ─────────────────────────────────────────────
if ! command -v rustup &>/dev/null; then
    echo "Installing rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
    source "$HOME/.cargo/env"
fi

# ── 2. Ensure nightly is installed ───────────────────────────────────────────
rustup toolchain install nightly

# ── 3. Install cargo-skyline if missing ──────────────────────────────────────
if ! cargo skyline --version &>/dev/null 2>&1; then
    echo "Installing cargo-skyline..."
    cargo install cargo-skyline
fi

# ── 4. Install the skyline-v3 toolchain + custom stdlib ──────────────────────
# This is required by cargo-skyline to cross-compile for the Switch.
# It only needs to run once; subsequent builds skip it automatically.
echo "Setting up skyline-v3 toolchain (this may take a few minutes on first run)..."
cargo skyline update-std

# ── 5. Build ─────────────────────────────────────────────────────────────────
echo "Building..."
cargo skyline build --release

echo ""
echo "Done! Your plugin is at:"
echo "  target/aarch64-skyline-switch/release/$(basename "$PWD" | tr '-' '_' | sed 's/^lib//')*.nro"
echo ""
echo "Rename it to plugin.nro and place it at:"
echo "  ultimate/mods/<mod folder>/plugin.nro"
"#.to_string(),
    });

    // ── build.bat (Windows) ───────────────────────────────────────────────
    files.push(GeneratedFile {
        rel_path: "build.bat".into(),
        contents: r#"@echo off
setlocal

:: ── 1. Check for rustup ──────────────────────────────────────────────────────
where rustup >nul 2>&1
if %errorlevel% neq 0 (
    echo rustup not found.
    echo Please install Rust from https://rustup.rs then re-run this script.
    pause
    exit /b 1
)

:: ── 2. Ensure nightly is installed ───────────────────────────────────────────
rustup toolchain install nightly

:: ── 3. Install cargo-skyline if missing ──────────────────────────────────────
cargo skyline --version >nul 2>&1
if %errorlevel% neq 0 (
    echo Installing cargo-skyline...
    cargo install cargo-skyline
)

:: ── 4. Install the skyline-v3 toolchain + custom stdlib ──────────────────────
echo Setting up skyline-v3 toolchain (first run may take a few minutes)...
cargo skyline update-std

:: ── 5. Build ─────────────────────────────────────────────────────────────────
echo Building...
cargo skyline build --release

echo.
echo Done! Your plugin is in target\aarch64-skyline-switch\release\
echo Rename the .nro to plugin.nro and place it at:
echo   ultimate\mods\^<mod folder^>\plugin.nro
pause
"#
        .to_string(),
    });

    ModProject {
        name: plugin_name.to_string(),
        files,
    }
}

#[cfg(test)]
/// Re-emit a `sound_` function body at the corpus's own indentation.
///
/// The inverse of [`parse_sound_script`] up to the function header and closing brace, which the
/// caller owns. Fidelity over the corpus is asserted by
/// `every_sound_script_in_the_corpus_survives_a_round_trip`.
pub fn emit_sound_body(script: &AcmdScript) -> String {
    emit_stmts(&script.stmts, "    ")
        .into_iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

#[cfg(test)]
/// Convenience: export a single move as a standalone project.
/// Returns the `src/{fighter}/acmd.rs` content only — use `build_mod_project` for a full project.
pub fn export_acmd_source(
    script: &crate::data::AcmdScript,
    fighter: &str,
    move_name: &str,
) -> String {
    let edits = vec![(fighter.to_string(), move_name.to_string(), script.clone())];
    let project = build_mod_project(&edits, &format!("{fighter}_{move_name}_mod"));
    // Return all files joined with separators for single-file save (legacy path)
    project
        .files
        .iter()
        .map(|f| format!("// === {} ===\n{}", f.rel_path, f.contents))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::move_nodes::test_support::assert_json_eq;

    #[test]
    fn body_visibility_new_pairs_require_unattributed_root_execute_context() {
        fn typed(stmts: &[AcmdStmt]) -> usize {
            stmts
                .iter()
                .map(|stmt| match stmt {
                    AcmdStmt::Excute(commands) => commands
                        .iter()
                        .filter(|c| matches!(c, ExcuteStmt::VisibilitySetInt64 { .. }))
                        .count(),
                    AcmdStmt::Loop { body, .. } | AcmdStmt::RawBlock { body, .. } => typed(body),
                    _ => 0,
                })
                .sum()
        }
        const CALL: &str = "VisibilityModule::set_int64(agent.module_accessor, hash40(\"sword\") as i64, hash40(\"sword_l\") as i64);";
        let root = format!("if macros::is_excute(agent) {{\n{CALL}\n}}");
        for category in ["game", "expression"] {
            let source = format!(
                "unsafe extern \"C\" fn {category}_test(agent: &mut L2CAgentBase) {{\n{root}\n}}\n"
            );
            let parsed = if category == "game" {
                parse_acmd_script(&source)
            } else {
                parse_expression_script(&source)
            };
            assert_eq!(typed(&parsed.stmts), 1);
            for body in [
                CALL.to_owned(),
                format!("if runtime_condition() {{\n{root}\n}}"),
                format!("for _ in 0..2 {{\n{root}\n}}"),
                format!("#[cfg(any())]\n\n// still attributed\n{root}"),
                format!("#[cfg(\nany()\n)]\n{root}"),
                format!("if macros::is_excute(agent) {{\n#[cfg(any())]\n\n// still attributed\n{CALL}\n}}"),
                root.replace("macros::is_excute(agent)", "!macros::is_excute(agent)"),
            ] {
                let source = format!("unsafe extern \"C\" fn {category}_test(agent: &mut L2CAgentBase) {{\n{body}\n}}\n");
                let parsed = if category == "game" { parse_acmd_script(&source) } else { parse_expression_script(&source) };
                assert_eq!(typed(&parsed.stmts), 0, "{source}");
                let emitted = emit_stmts(&parsed.stmts, "    ").join("\n");
                assert!(emitted.contains(CALL), "new non-root calls remain source");
            }
        }
        let sound =
            format!("unsafe extern \"C\" fn sound_test(agent: &mut L2CAgentBase) {{\n{root}\n}}\n");
        assert_eq!(typed(&parse_sound_script(&sound).stmts), 0);
        let unknown = CALL.replace("sword_l", "unknown_sword_state");
        assert!(parse_visibility_set_int64_call(&unknown).is_none());
        let source = format!("unsafe extern \"C\" fn game_test(agent: &mut L2CAgentBase) {{\nif macros::is_excute(agent) {{\n{unknown}\n}}\n}}\n");
        let parsed = parse_acmd_script(&source);
        assert!(
            matches!(parsed.stmts.as_slice(), [AcmdStmt::Excute(commands)]
            if matches!(commands.as_slice(), [ExcuteStmt::Raw(raw)] if raw == &unknown))
        );
    }

    #[test]
    fn whole_visibility_is_literal_only_at_clean_root_game_expression_execute() {
        fn count(stmts: &[AcmdStmt]) -> usize {
            stmts
                .iter()
                .map(|stmt| match stmt {
                    AcmdStmt::Excute(commands) => commands
                        .iter()
                        .filter(|command| matches!(command, ExcuteStmt::VisibilitySetWhole { .. }))
                        .count(),
                    AcmdStmt::Loop { body, .. } | AcmdStmt::RawBlock { body, .. } => count(body),
                    _ => 0,
                })
                .sum()
        }
        fn parse(category: &str, body: &str) -> AcmdScript {
            let source = format!(
                "unsafe extern \"C\" fn {category}_whole_visibility(agent: &mut L2CAgentBase) {{\n{body}\n}}\n"
            );
            match category {
                "game" => parse_acmd_script(&source),
                "expression" => parse_expression_script(&source),
                "sound" => parse_sound_script(&source),
                _ => unreachable!(),
            }
        }

        for category in ["game", "expression"] {
            for visible in [true, false] {
                let call =
                    format!("VisibilityModule::set_whole(agent.module_accessor, {visible});");
                let parsed = parse(
                    category,
                    &format!("if macros::is_excute(agent) {{\n    {call}\n}}"),
                );
                assert!(
                    matches!(parsed.stmts.as_slice(), [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::VisibilitySetWhole { visible: actual }] if *actual == visible))
                );
                let emitted = emit_stmts(&parsed.stmts, "").join("\n");
                assert_eq!(count(&parse(category, &emitted).stmts), 1);
                assert!(emitted.contains(&call));
            }
        }

        let independent_permission = [
            "if macros::is_excute(agent) {",
            "    VisibilityModule::set_whole(agent.module_accessor, true);",
            "}",
        ];
        let (parsed, _) = parse_stmts_with_fear_face_guard(
            &independent_permission,
            0,
            ParseDomain::Game,
            false,
            false,
            false,
            RootStatementPermissions {
                visibility: true,
                motion_loop_sync: false,
            },
        );
        assert_eq!(
            count(&parsed),
            1,
            "visibility permission is not native-attack permission"
        );

        let mixed_else_source = r#"unsafe extern "C" fn game_visibility_mixed_else(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        VisibilityModule::set_int64(agent.module_accessor, hash40("sword") as i64, hash40("sword_l") as i64);
        VisibilityModule::set_whole(agent.module_accessor, true);
    }
    else {
        wait(agent.lua_state_agent, 1.0);
    }
}
"#;
        let mixed_else = parse_acmd_script(mixed_else_source);
        assert!(
            matches!(mixed_else.stmts.first(), Some(AcmdStmt::Excute(commands))
            if matches!(commands.as_slice(), [
                ExcuteStmt::VisibilitySetInt64 { group, status },
                ExcuteStmt::Raw(raw),
            ] if group == "sword" && status == "sword_l"
                && raw == "VisibilityModule::set_whole(agent.module_accessor, true);"))
        );

        let malformed = [
            "VisibilityModule::set_whole(boma, true);",
            "VisibilityModule::set_whole(agent.module_accessor, visible);",
            "VisibilityModule::set_whole(agent.module_accessor);",
            "VisibilityModule::set_whole(agent.module_accessor, true, false);",
        ];
        for call in malformed {
            let parsed = parse(
                "game",
                &format!("if macros::is_excute(agent) {{\n    {call}\n}}"),
            );
            assert_eq!(count(&parsed.stmts), 0, "{call}");
            let emitted = emit_stmts(&parsed.stmts, "").join("\n");
            assert!(emitted.contains(call), "{call}");
        }

        let call = "VisibilityModule::set_whole(agent.module_accessor, true);";
        for body in [
            format!("if macros::is_excute(agent) {{\n    #[cfg(any())]\n    {call}\n}}"),
            format!("#[cfg(any())]\nif macros::is_excute(agent) {{\n    {call}\n}}"),
            format!("if macros::is_excute(agent) {{\n    #[cfg(\n        any()\n    )]\n    {call}\n}}"),
            format!("if runtime_condition() {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}"),
            format!("for _ in 0..2 {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}"),
            call.to_owned(),
            "if macros::is_excute(agent) {\n    VisibilityModule::set_whole(alias.module_accessor, true);\n}".to_owned(),
            "if macros::is_excute(agent) {\n    VisibilityModule::set_whole(agent.module_accessor as _, true);\n}".to_owned(),
            "if macros::is_excute(agent) {\n    visibility::VisibilityModule::set_whole(agent.module_accessor, true);\n}".to_owned(),
        ] {
            assert_eq!(count(&parse("game", &body).stmts), 0, "{body}");
        }
        for category in ["sound", "effect"] {
            let source = format!(
                "unsafe extern \"C\" fn {category}_whole_visibility(agent: &mut L2CAgentBase) {{\nif macros::is_excute(agent) {{\n    {call}\n}}\n}}\n"
            );
            if category == "sound" {
                assert_eq!(count(&parse_sound_script(&source).stmts), 0, "sound");
            } else {
                let effect = parse_effect_script(&source);
                assert!(
                    matches!(effect.stmts.as_slice(), [crate::data::EffectStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [crate::data::EffectMacro::Raw(raw)] if raw == call))
                );
            }
        }
    }

    #[test]
    fn model_default_visibility_parser_rejects_noncanonical_receivers_and_arguments() {
        for call in [
            "VisibilityModule::set_default_int64(agent.module_accessor, Hash40::new(\"panel\"));",
            "VisibilityModule::set_default_int64(alias, hash40(\"panel\") as i64);",
            "VisibilityModule::set_default_int64(agent.module_accessor, hash40(\"panel\"));",
            "VisibilityModule::set_default_int64(agent.module_accessor, hash40(\"panel\") as u64);",
            "VisibilityModule::set_default_int64(agent.module_accessor, hash40(\"panel\") as i64, false);",
            "VisibilityModule::set_default_int64(agent.module_accessor, hash40(group) as i64);",
            "VisibilityModule::set_default_int64(agent.module_accessor, 0);",
            "VisibilityModule::set_default_int64(agent.module_accessor, hash40(\"unmeasured\") as i64);",
        ] {
            assert!(parse_visibility_set_default_int64_call(call).is_none(), "{call}");
        }
        for &(_, group, _) in crate::data::PINNED_DEFAULT_INT64_GROUP_COUNTS {
            let call = format!("VisibilityModule::set_default_int64(agent.module_accessor, hash40(\"{group}\") as i64);");
            assert!(matches!(parse_visibility_set_default_int64_call(&call),
                Some(ExcuteStmt::VisibilitySetDefaultInt64 { group: actual }) if actual == group));
        }
    }

    #[test]
    fn enabled_generation_parser_is_exact_and_only_promotes_clean_root_execute() {
        let canonical = "ArticleModule::generate_article_enable(agent.module_accessor, *FIGHTER_KOOPAJR_GENERATE_ARTICLE_HAMMER, false, -1);";
        assert!(parse_enabled_article_call(canonical).is_some());
        for call in [
            canonical.replace("agent.module_accessor", "other"),
            canonical.replace("false", "true"),
            canonical.replace(", -1", ""),
            canonical.replace("-1", "0"),
            canonical.replace("HAMMER", "CANNONBALL"),
            canonical.replace("*FIGHTER_KOOPAJR_GENERATE_ARTICLE_HAMMER", "0"),
            canonical.replace("-1", "get_index()"),
        ] {
            assert!(parse_enabled_article_call(&call).is_none(), "{call}");
        }
        for body in [
            canonical.into(),
            format!("if runtime_condition() {{\n    if macros::is_excute(agent) {{\n        {canonical}\n    }}\n}}"),
            format!("if macros::is_excute(agent) {{\n    {canonical}\n}} else {{\n    wait(agent.lua_state_agent, 1.0);\n}}"),
            format!("#[cfg(any())]\nif macros::is_excute(agent) {{\n    {canonical}\n}}"),
            format!("if macros::is_excute(agent) {{\n    #[cfg(any())]\n    {canonical}\n}}"),
            format!("for _ in 0..2 {{\n    if macros::is_excute(agent) {{\n        {canonical}\n    }}\n}}"),
        ] {
            let source = format!("unsafe extern \"C\" fn game_enabled(agent: &mut L2CAgentBase) {{\n{body}\n}}\n");
            let parsed = parse_acmd_script(&source);
            fn typed(stmts: &[AcmdStmt]) -> usize {
                stmts.iter().map(|stmt| match stmt {
                    AcmdStmt::Excute(commands) => commands.iter().filter(|command| matches!(command,
                        ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::GenerateEnabled { .. }))).count(),
                    AcmdStmt::Loop { body, .. } | AcmdStmt::RawBlock { body, .. } => typed(body),
                    _ => 0,
                }).sum()
            }
            assert_eq!(typed(&parsed.stmts), 0, "{source}");
        }
    }

    #[test]
    fn article_state_parser_is_exact_and_keeps_unsupported_scopes_as_source() {
        let call = "ArticleModule::change_status(agent.module_accessor, *FIGHTER_MASTER_GENERATE_ARTICLE_SWORD, *WEAPON_MASTER_SWORD_STATUS_KIND_EXTEND, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));";
        assert!(parse_article_status_call(call).is_some());
        for invalid in [
            call.replace("agent.module_accessor", "alias"),
            call.replace("ARTICLE_OPE_TARGET_ALL", "ARTICLE_OPE_TARGET_LAST"),
            call.replace("WEAPON_MASTER_SWORD_STATUS_KIND_EXTEND", "0"),
            call.replace("WEAPON_MASTER_SWORD", "WEAPON_JACK_WIREROPE"),
            call.replace(", ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL)", ""),
        ] {
            assert!(parse_article_status_call(&invalid).is_none(), "{invalid}");
        }
        fn typed(stmts: &[AcmdStmt]) -> usize {
            stmts
                .iter()
                .map(|stmt| match stmt {
                    AcmdStmt::Excute(commands) => commands
                        .iter()
                        .filter(|command| {
                            matches!(
                                command,
                                ExcuteStmt::ArticleLifecycle(
                                    crate::data::ArticleLifecycleCall::ChangeStatus { .. }
                                )
                            )
                        })
                        .count(),
                    AcmdStmt::Loop { body, .. } | AcmdStmt::RawBlock { body, .. } => typed(body),
                    _ => 0,
                })
                .sum()
        }
        for body in [call.into(), format!("if unknown() {{\nif macros::is_excute(agent) {{\n{call}\n}}\n}}"), format!("if macros::is_excute(agent) {{\n{call}\n}} else {{\nwait(agent.lua_state_agent, 1.0);\n}}"), format!("#[cfg(any())]\nif macros::is_excute(agent) {{\n{call}\n}}"), format!("if macros::is_excute(agent) {{\n#[cfg(any())]\n{call}\n}}"), format!("for _ in 0..2 {{\nif macros::is_excute(agent) {{\n{call}\n}}\n}}" )] {
            let source = format!("unsafe extern \"C\" fn game_state(agent: &mut L2CAgentBase) {{\n{body}\n}}\n");
            assert_eq!(typed(&parse_acmd_script(&source).stmts), 0, "{source}");
        }
        let source = format!("unsafe extern \"C\" fn sound_state(agent: &mut L2CAgentBase) {{\nif macros::is_excute(agent) {{\n{call}\n}}\n}}\n");
        assert_eq!(typed(&parse_sound_script(&source).stmts), 0);
    }

    #[test]
    fn body_visibility_legacy_pair_keeps_nested_source_compatibility() {
        let source = r#"unsafe extern "C" fn expression_test(agent: &mut L2CAgentBase) {
    for _ in 0..2 {
        if macros::is_excute(agent) {
            VisibilityModule::set_int64(agent.module_accessor, hash40("body") as i64, hash40("body_normal") as i64);
        }
    }
}
"#;
        let parsed = parse_expression_script(source);
        let AcmdStmt::Loop { body, .. } = &parsed.stmts[0] else {
            panic!("loop retained");
        };
        assert!(matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
            if matches!(commands.as_slice(), [ExcuteStmt::VisibilitySetInt64 { group, status }]
                if group == "body" && status == "body_normal")));
        let emitted = preview_expression_fn(&parsed, "test");
        assert_json_eq!(&parsed, parse_expression_script(&emitted));
    }

    #[test]
    fn slope_presentation_forms_are_typed_strict_and_roundtrip() {
        let source = r#"unsafe extern "C" fn game_slope(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_LR);
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_TOP);
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_NONE, 3);
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_L, 6, true);
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_R, 4, false);
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_UNKNOWN);
        slope!(other, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_LR);
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_LR, -1);
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_LR, 601);
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_LR, 3, maybe);
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_LR, 3);
    }
}
"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected one execute block: {:#?}", parsed.stmts);
        };
        use crate::data::{ExcuteStmt, SlopeCall, SlopeStatus};
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::Slope(_)))
                .count(),
            5
        );
        assert!(matches!(
            commands.first(),
            Some(ExcuteStmt::Slope(SlopeCall::Set {
                status: SlopeStatus::Lr
            }))
        ));
        assert!(matches!(
            commands.get(2),
            Some(ExcuteStmt::Slope(SlopeCall::Interpolate {
                status: SlopeStatus::None,
                frames: 3,
                trailing_flag: None,
            }))
        ));
        assert!(matches!(
            commands.get(3),
            Some(ExcuteStmt::Slope(SlopeCall::Interpolate {
                status: SlopeStatus::Left,
                frames: 6,
                trailing_flag: Some(true),
            }))
        ));
        assert!(
            commands
                .iter()
                .filter_map(|command| match command {
                    ExcuteStmt::Raw(line) => Some(line),
                    _ => None,
                })
                .count()
                == 6
        );
        let emitted = preview_game_fn(&parsed, "slope");
        let reparsed = parse_acmd_script(&emitted);
        assert_json_eq!(reparsed, parsed);
    }

    #[test]
    fn item_visibility_has_one_fixed_typed_shape() {
        let source = r#"unsafe extern "C" fn game_item_visibility(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        ItemModule::set_have_item_visibility(agent.module_accessor, false, 0);
        ItemModule::set_have_item_visibility(agent.module_accessor, true, 0);
        ItemModule::set_have_item_visibility(boma, false, 0);
        ItemModule::set_have_item_visibility(agent.module_accessor, false, 1);
        ItemModule::set_have_item_visibility(agent.module_accessor, 0, 0);
        ItemModule::set_have_item_visibility(agent.module_accessor, visible, 0);
        ItemModule::set_have_item_visibility(agent.module_accessor, false);
        ItemModule::set_have_item_visibility(agent.module_accessor, false, 0, true);
    }
}
"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected one execute block: {:#?}", parsed.stmts);
        };
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::ItemVisibility { .. }))
                .count(),
            2
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::Raw(_)))
                .count(),
            6
        );
        assert!(matches!(
            commands.first(),
            Some(ExcuteStmt::ItemVisibility { visible: false })
        ));
        let emitted = preview_game_fn(&parsed, "item_visibility");
        assert!(emitted
            .contains("ItemModule::set_have_item_visibility(agent.module_accessor, false, 0);"));
        assert!(emitted
            .contains("ItemModule::set_have_item_visibility(agent.module_accessor, true, 0);"));
        assert!(emitted.contains("ItemModule::set_have_item_visibility(boma, false, 0);"));
        assert_json_eq!(parse_acmd_script(&emitted).stmts, parsed.stmts);
    }

    #[test]
    fn jostle_and_visibility_native_calls_use_closed_shapes() {
        let source = r#"unsafe extern "C" fn game_native_calls(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        JostleModule::set_status(agent.module_accessor, false);
        JostleModule::set_status(agent.module_accessor, true);
        JostleModule::set_status(boma, false);
        JostleModule::set_status(agent.module_accessor, enabled);
        JostleModule::set_status(agent.module_accessor, false, true);
        VisibilityModule::set_int64(agent.module_accessor, hash40("body") as i64, hash40("body_normal") as i64);
        VisibilityModule::set_int64(boma, hash40("body") as i64, hash40("body_normal") as i64);
        VisibilityModule::set_int64(agent.module_accessor, hash40("body"), hash40("body_normal") as i64);
        VisibilityModule::set_int64(agent.module_accessor, hash40("body") as i64, 7);
        VisibilityModule::set_int64(agent.module_accessor, group, status);
        VisibilityModule::set_int64(agent.module_accessor, hash40("group") as i64, hash40("status") as i64);
        VisibilityModule::set_int64(agent.module_accessor, hash40("bad-name") as i64, hash40("body_normal") as i64);
    }

}
"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected one execute block: {:#?}", parsed.stmts);
        };
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::JostleSetStatus { .. }))
                .count(),
            2
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::VisibilitySetInt64 { .. }))
                .count(),
            1
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::Raw(_)))
                .count(),
            9
        );
        assert!(matches!(
            commands.get(5),
            Some(ExcuteStmt::VisibilitySetInt64 { group, status })
                if group == "body" && status == "body_normal"
        ));
        let emitted = preview_game_fn(&parsed, "native_calls");
        assert!(emitted.contains("JostleModule::set_status(agent.module_accessor, false);"));
        assert!(emitted.contains(
            "VisibilityModule::set_int64(agent.module_accessor, hash40(\"body\") as i64, hash40(\"body_normal\") as i64);"
        ));
        assert_json_eq!(parse_acmd_script(&emitted).stmts, parsed.stmts);
    }

    #[test]
    fn fixed_jostle_area_unsupported_shapes_stay_raw() {
        use crate::data::{AcmdStmt, ExcuteStmt};

        const SOURCE: &str = r#"unsafe extern "C" fn game_fixed_jostle_area_reject(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(boma, 1.0, 2.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1.0 + 2.0, 2.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, f32::NAN, 2.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 10001.0, 2.0);
        wrapper::FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1.0, 2.0);
        use_area(FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1.0, 2.0));
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1.0, 2.0); trailing;
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1.0, 2.0); ();
        FighterAreaModuleImpl::enable_fix_jostle_area_xy(agent.module_accessor, 1.0, 2.0, 3.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 4.5.0, 3.0, 6.5, 4.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1, 2.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1.0f32, 2.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, #[cfg(false)] 1.0, 2.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1e99, 2.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1.0, 2.0); FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 3.0, 4.0);
        FighterAreaModuleImpl::disable_fix_jostle_area(agent.module_accessor);
    }
}
"#;
        let parsed = parse_acmd_script(SOURCE);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected one execute block")
        };
        assert_eq!(commands.len(), 17);
        assert!(commands
            .iter()
            .all(|command| matches!(command, ExcuteStmt::Raw(_))));
        assert!(matches!(
            commands.get(10),
            Some(ExcuteStmt::Raw(line)) if line.contains("4.5.0, 3.0, 6.5, 4.0")
        ));
    }

    #[test]
    fn attack_module_all_setters_parse_only_observed_game_execute_shapes() {
        use crate::data::{AttackModuleSetAllKind, ExcuteStmt};

        let source = r#"unsafe extern "C" fn game_attack_module_setters(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_no_dead_all(agent.module_accessor, true, false);
        AttackModule::set_no_dead_all(agent.module_accessor, true, true);
        AttackModule::set_catch_only_all(agent.module_accessor, true, false);
        AttackModule::set_no_dead_all(agent.module_accessor, false, false);
        AttackModule::set_no_dead_all(agent.module_accessor, true);
        AttackModule::set_no_dead_all(agent.module_accessor, enabled, false);
        AttackModule::set_catch_only_all(agent.module_accessor, true, true);
        AttackModule::set_catch_only_all(boma, true, false);
        AttackModule::set_catch_only_all(agent.module_accessor, true, false, true);
        AttackModule::set_no_dead_all(agent.module_accessor, true, false); trailing;
        AttackModule::set_no_damage_orbit_all(agent.module_accessor, true, false);
    }
}
"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected one execute block: {:#?}", parsed.stmts);
        };
        assert!(matches!(
            commands.first(),
            Some(ExcuteStmt::AttackModuleSetAll {
                kind: AttackModuleSetAllKind::NoDeadAll,
                arg2: true,
                arg3: false,
            })
        ));
        assert!(matches!(
            commands.get(1),
            Some(ExcuteStmt::AttackModuleSetAll {
                kind: AttackModuleSetAllKind::NoDeadAll,
                arg2: true,
                arg3: true,
            })
        ));
        assert!(matches!(
            commands.get(2),
            Some(ExcuteStmt::AttackModuleSetAll {
                kind: AttackModuleSetAllKind::CatchOnlyAll,
                arg2: true,
                arg3: false,
            })
        ));
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::Raw(_)))
                .count(),
            8
        );
        let emitted = preview_game_fn(&parsed, "attack_module_setters");
        assert!(
            emitted.contains("AttackModule::set_no_dead_all(agent.module_accessor, true, false);")
        );
        assert!(emitted
            .contains("AttackModule::set_catch_only_all(agent.module_accessor, true, false);"));
        assert_json_eq!(parse_acmd_script(&emitted).stmts, parsed.stmts);

        let expression = r#"unsafe extern "C" fn expression_native_wrong_track(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_no_dead_all(agent.module_accessor, true, false);
    }
}
"#;
        let parsed_expression = parse_expression_script(expression);
        assert!(matches!(
            parsed_expression.stmts.as_slice(),
            [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::Raw(line)]
                    if line == "AttackModule::set_no_dead_all(agent.module_accessor, true, false);")
        ));

        let control_flow = r#"unsafe extern "C" fn game_native_opaque_branch(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD) {
        if macros::is_excute(agent) {
            AttackModule::set_no_dead_all(agent.module_accessor, true, false);
        }
    }
}
"#;
        let parsed_control_flow = parse_acmd_script(control_flow);
        assert!(matches!(
            parsed_control_flow.stmts.as_slice(),
            [AcmdStmt::RawBlock { body, .. }]
                if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::Raw(line)]
                        if line == "AttackModule::set_no_dead_all(agent.module_accessor, true, false);"))
        ));
        let looped = r#"unsafe extern "C" fn game_native_loop(agent: &mut L2CAgentBase) {
    for _ in 0..2 {
        if macros::is_excute(agent) {
            AttackModule::set_no_dead_all(agent.module_accessor, true, false);
        }
    }
}
"#;
        let parsed_loop = parse_acmd_script(looped);
        assert!(matches!(
            parsed_loop.stmts.as_slice(),
            [AcmdStmt::Loop { body, .. }]
                if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::Raw(line)]
                        if line == "AttackModule::set_no_dead_all(agent.module_accessor, true, false);"))
        ));
    }

    #[test]
    fn indexed_attack_and_grab_rebound_calls_are_closed_game_execute_forms() {
        use crate::data::{AttackModuleSetIndexedKind, ExcuteStmt};

        let source = r#"unsafe extern "C" fn game_indexed_native_calls(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_no_finish_camera(agent.module_accessor, 0, true, false);
        AttackModule::set_no_finish_camera(agent.module_accessor, 3, true, false);
        AttackModule::set_force_reaction(agent.module_accessor, 1, true, false);
        AttackModule::set_force_reaction(agent.module_accessor, 2, true, false);
        AttackModule::set_force_reaction(agent.module_accessor, 4, true, false);
        AttackModule::set_no_finish_camera(agent.module_accessor, 01, true, false);
        AttackModule::set_no_finish_camera(agent.module_accessor, 0, false, false);
        AttackModule::set_no_finish_camera(agent.module_accessor, 0, true, true);
        AttackModule::set_no_finish_camera(boma, 0, true, false);
        AttackModule::set_no_finish_camera(agent.module_accessor, 0, true);
        GrabModule::set_rebound(agent.module_accessor, true);
        GrabModule::set_rebound(agent.module_accessor, false);
        GrabModule::set_rebound(boma, true);
        GrabModule::set_rebound(agent.module_accessor, enabled);
        GrabModule::set_rebound(agent.module_accessor, false, true);
    }
    for _ in 0..2 {
        if macros::is_excute(agent) {
            GrabModule::set_rebound(agent.module_accessor, true);
            AttackModule::set_force_reaction(agent.module_accessor, 0, true, false);
        }
    }
}
"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected Game execute block: {:#?}", parsed.stmts);
        };
        assert!(matches!(
            commands.first(),
            Some(ExcuteStmt::AttackModuleSetIndexed {
                kind: AttackModuleSetIndexedKind::NoFinishCamera,
                id: 0,
                arg3: true,
                arg4: false,
            })
        ));
        assert!(matches!(
            commands.get(2),
            Some(ExcuteStmt::AttackModuleSetIndexed {
                kind: AttackModuleSetIndexedKind::ForceReaction,
                id: 1,
                arg3: true,
                arg4: false,
            })
        ));
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::AttackModuleSetIndexed { .. }))
                .count(),
            4
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::GrabRebound { .. }))
                .count(),
            2
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::Raw(_)))
                .count(),
            8,
            "unsupported IDs, tails, receivers, and argument counts must remain Raw"
        );
        assert!(commands.iter().any(|command| matches!(command,
            ExcuteStmt::NativeCall(call) if call.callee == "AttackModule::set_force_reaction"
        )));
        assert!(
            matches!(parsed.stmts.get(1), Some(AcmdStmt::Loop { body, .. })
            if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::Raw(_), ExcuteStmt::NativeCall(call)] if call.callee == "AttackModule::set_force_reaction")))
        );

        let expression = r#"unsafe extern "C" fn expression_native_calls(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        GrabModule::set_rebound(agent.module_accessor, true);
        AttackModule::set_force_reaction(agent.module_accessor, 0, true, false);
    }
}
"#;
        let parsed_expression = parse_expression_script(expression);
        assert!(matches!(parsed_expression.stmts.as_slice(),
            [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::Raw(_), ExcuteStmt::NativeCall(call)] if call.callee == "AttackModule::set_force_reaction")));

        let emitted = preview_game_fn(&parsed, "indexed_native_calls");
        assert!(emitted.contains(
            "AttackModule::set_no_finish_camera(agent.module_accessor, 0, true, false);"
        ));
        assert!(emitted
            .contains("AttackModule::set_force_reaction(agent.module_accessor, 1, true, false);"));
        assert!(emitted.contains("GrabModule::set_rebound(agent.module_accessor, false);"));
        assert_json_eq!(
            parse_acmd_script(&emitted).stmts,
            parsed.stmts,
            "typed calls and every rejected native sibling must round-trip"
        );
    }

    #[test]
    fn corpus_final_finish_cut_ins_keep_flags_and_timing_in_the_existing_node() {
        use crate::move_nodes::{
            coverage,
            model::{MoveGraph, NodeKind, Track},
            timeline,
        };
        for (fighter, origin, line, expected) in [
            (
                "diddy", "diddy/FinalFinishBurst.txt",
                "AttackModule::set_final_finish_cut_in(agent.module_accessor, 1, true, true, -1, false);",
                (1, true, true, -1.0, false),
            ),
            (
                "miifighter", "miifighter/FinalAttack.txt",
                "AttackModule::set_final_finish_cut_in(agent.module_accessor, 0, true, false, 91, false);",
                (0, true, false, 91.0, false),
            ),
        ] {
            let source = format!("unsafe extern \"C\" fn game_fixture(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}");
            let parsed = parse_acmd_script(&source);
            let AcmdStmt::Excute(commands) = &parsed.stmts[0] else { panic!("{origin}") };
            let [ExcuteStmt::AttackModuleSetFinalFinishCutIn { id, arg3, arg4, arg5, arg6 }] = commands.as_slice() else { panic!("{origin}") };
            assert_eq!((*id, *arg3, *arg4, *arg5, *arg6), expected, "{origin}");
            let imported = timeline::import_for_fighter(fighter, Track::Game, &parsed);
            assert_eq!(coverage::track_summary(&imported).editable_command_count, 1);
            let graph = MoveGraph { tracks: vec![imported], ..Default::default() };
            let mut restored: MoveGraph = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
            let compiled = timeline::compile(fighter, "fixture", &restored).unwrap();
            assert_json_eq!(&compiled[0].1, &parsed);
            let emitted = preview_game_fn(&compiled[0].1, "fixture");
            syn::parse_file(&emitted).unwrap();
            assert_json_eq!(parse_acmd_script(&emitted), &parsed);
            // An edit is validated and emitted through the same dedicated command variant.
            let NodeKind::Execute { commands } = &mut restored.tracks[0].nodes[0].kind else { panic!() };
            let ExcuteStmt::AttackModuleSetFinalFinishCutIn { arg4, .. } = &mut commands[0] else { panic!() };
            *arg4 = !*arg4;
            let edited = timeline::compile(fighter, "fixture", &restored).unwrap();
            assert_json_eq!(parse_acmd_script(&preview_game_fn(&edited[0].1, "fixture")), &edited[0].1);

            for rejected in [
                line.replace("agent.module_accessor", "other"),
                line.replacen(");", ", true);", 1),
                line.replacen("true", "dynamic_flag()", 1),
                line.replacen("true", "0", 1),
                format!("#[cfg(any())] {line}"),
            ] {
                let rejected_source = source.replace(line, &rejected);
                let imported = timeline::import_for_fighter(fighter, Track::Game, &parse_acmd_script(&rejected_source));
                assert_eq!(coverage::track_summary(&imported).editable_command_count, 0, "{origin}: {rejected}");
            }
            let nested = source.replace("    if macros::is_excute(agent) {", "    if unknown_condition() {\n    if macros::is_excute(agent) {").replace("\n}", "\n}\n}");
            let imported = timeline::import_for_fighter(fighter, Track::Game, &parse_acmd_script(&nested));
            assert_eq!(coverage::track_summary(&imported).editable_command_count, 0);
            let wrong_track = source.replace("game_fixture", "expression_fixture");
            let imported = timeline::import_for_fighter(fighter, Track::Expression, &parse_expression_script(&wrong_track));
            assert_eq!(coverage::track_summary(&imported).editable_command_count, 0);
        }
    }

    #[test]
    fn attack_height_and_final_finish_calls_are_closed_game_execute_forms() {
        use crate::data::{AttackHeightAllKind, ExcuteStmt};

        let source = r#"unsafe extern "C" fn game_attack_module_native_calls(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_HIGH), false);
        AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_LOW), false);
        AttackModule::set_final_finish_cut_in(agent.module_accessor, 0, true, false, -1, false);
        AttackModule::set_final_finish_cut_in(agent.module_accessor, 3, true, false, -1.0, false);
        AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_MIDDLE), false);
        AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_HIGH), true);
        AttackModule::set_attack_height_all(boma, AttackHeight(*ATTACK_HEIGHT_LOW), false);
        AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_HIGH));
        AttackModule::set_final_finish_cut_in(agent.module_accessor, -1, true, false, -1, false);
        AttackModule::set_final_finish_cut_in(agent.module_accessor, 01, true, false, -1, false);
        AttackModule::set_final_finish_cut_in(agent.module_accessor, 0, dynamic_flag(), false, -1, false);
        AttackModule::set_final_finish_cut_in(agent.module_accessor, 0, true, false, dynamic_frame(), false);
        AttackModule::set_final_finish_cut_in(boma, 0, true, false, -1, false);
        AttackModule::set_final_finish_cut_in(agent.module_accessor, 0, true, false, -1, false, true);
    }
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD) {
        if macros::is_excute(agent) {
            AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_HIGH), false);
            AttackModule::set_final_finish_cut_in(agent.module_accessor, 0, true, false, -1, false);
        }
    }
    for _ in 0..2 {
        if macros::is_excute(agent) {
            AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_LOW), false);
            AttackModule::set_final_finish_cut_in(agent.module_accessor, 0, true, false, -1, false);
        }
    }
    AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_HIGH), false);
    AttackModule::set_final_finish_cut_in(agent.module_accessor, 0, true, false, -1, false);
}
"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected direct Game execute block: {:#?}", parsed.stmts);
        };
        assert!(matches!(
            commands[0],
            ExcuteStmt::AttackModuleSetHeightAll {
                height: AttackHeightAllKind::High,
                arg3: false,
            }
        ));
        assert!(matches!(
            commands[1],
            ExcuteStmt::AttackModuleSetHeightAll {
                height: AttackHeightAllKind::Low,
                arg3: false,
            }
        ));
        assert!(matches!(
            commands[2],
            ExcuteStmt::AttackModuleSetFinalFinishCutIn {
                id: 0,
                arg3: true,
                arg4: false,
                arg5: -1.0,
                arg6: false,
            }
        ));
        assert!(matches!(
            commands[3],
            ExcuteStmt::AttackModuleSetFinalFinishCutIn { id: 3, .. }
        ));
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::Raw(_)))
                .count(),
            10,
            "unmeasured selectors, receivers, IDs, tails, and arities stay Raw"
        );
        assert!(matches!(
            &parsed.stmts[1],
            AcmdStmt::RawBlock { body, .. }
                if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::AttackModuleSetHeightAll { .. }, ExcuteStmt::Raw(_)]))
        ));
        assert!(matches!(
            &parsed.stmts[2],
            AcmdStmt::Loop { body, .. }
                if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::AttackModuleSetHeightAll { .. }, ExcuteStmt::Raw(_)]))
        ));
        assert!(matches!(parsed.stmts.get(3), Some(AcmdStmt::Raw(_))));

        let expression = r#"unsafe extern "C" fn expression_wrong_track(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_attack_height_all(agent.module_accessor, AttackHeight(*ATTACK_HEIGHT_HIGH), false);
        AttackModule::set_final_finish_cut_in(agent.module_accessor, 0, true, false, -1, false);
    }
}
"#;
        let parsed_expression = parse_expression_script(expression);
        assert!(matches!(
            parsed_expression.stmts.as_slice(),
            [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::Raw(_), ExcuteStmt::Raw(_)])
        ));

        let emitted = preview_game_fn(&parsed, "attack_module_native_calls");
        assert!(emitted.contains(
            "AttackModule::set_final_finish_cut_in(agent.module_accessor, 0, true, false, -1.0, false);"
        ));
        assert_json_eq!(
            parse_acmd_script(&emitted).stmts,
            parsed.stmts,
            "typed calls and rejected/native-neighbor source must round-trip"
        );
    }

    #[test]
    fn expression_reference_joint_catalog_is_closed_and_round_trips() {
        use crate::data::{AttackReferenceJointKind, ExcuteStmt};
        use std::collections::HashSet;

        let unique_tuples = AttackReferenceJointKind::EXPRESSION_CHOICES
            .iter()
            .map(|kind| kind.native_parts())
            .collect::<HashSet<_>>();
        assert_eq!(
            unique_tuples.len(),
            AttackReferenceJointKind::EXPRESSION_CHOICES.len()
        );

        let source_calls = AttackReferenceJointKind::EXPRESSION_CHOICES
            .iter()
            .map(|kind| {
                let (bone, axis3, axis4, axis5) = kind.native_parts();
                format!(
                    "AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new(\"{bone}\"), AttackDirectionAxis(*{axis3}), AttackDirectionAxis(*{axis4}), AttackDirectionAxis(*{axis5}));"
                )
            })
            .collect::<Vec<_>>()
            .join("\n        ");
        let source = format!(
            "unsafe extern \"C\" fn expression_reference_catalog(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {source_calls}\n    }}\n}}\n"
        );
        let parsed = parse_expression_script(&source);
        let [AcmdStmt::Excute(commands)] = parsed.stmts.as_slice() else {
            panic!(
                "expected one direct Expression Execute: {:#?}",
                parsed.stmts
            );
        };
        assert_eq!(
            commands.len(),
            AttackReferenceJointKind::EXPRESSION_CHOICES.len()
        );
        assert!(commands.iter().all(|command| matches!(
            command,
            ExcuteStmt::AttackModuleSetAttackReferenceJointId { .. }
        )));
        assert!(parsed.to_expression_events().is_empty());
        let emitted = preview_expression_fn(&parsed, "reference_catalog");
        assert_json_eq!(parse_expression_script(&emitted).stmts, parsed.stmts);

        let game_source_calls = AttackReferenceJointKind::EXPRESSION_CHOICES
            .iter()
            .filter(|kind| !AttackReferenceJointKind::GAME_CHOICES.contains(kind))
            .map(|kind| {
                let (bone, axis3, axis4, axis5) = kind.native_parts();
                format!(
                    "AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new(\"{bone}\"), AttackDirectionAxis(*{axis3}), AttackDirectionAxis(*{axis4}), AttackDirectionAxis(*{axis5}));"
                )
            })
            .collect::<Vec<_>>()
            .join("\n        ");
        let game_source = format!(
            "unsafe extern \"C\" fn game_reference_catalog(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {game_source_calls}\n    }}\n}}\n"
        );
        let game = parse_acmd_script(&game_source);
        let [AcmdStmt::Excute(commands)] = game.stmts.as_slice() else {
            panic!("expected one direct Game Execute: {:#?}", game.stmts);
        };
        assert_eq!(commands.len(), 81);
        assert!(commands
            .iter()
            .all(|command| matches!(command, ExcuteStmt::Raw(_))));

        let kind = AttackReferenceJointKind::EXPRESSION_CHOICES[3];
        let (bone, axis3, axis4, axis5) = kind.native_parts();
        let valid_call = format!(
            "AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new(\"{bone}\"), AttackDirectionAxis(*{axis3}), AttackDirectionAxis(*{axis4}), AttackDirectionAxis(*{axis5}));"
        );
        let expression_source = |body: &str| {
            format!(
            "unsafe extern \"C\" fn expression_reference_negative(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {body}\n    }}\n}}\n"
        )
        };
        let assert_not_typed = |source: &str| {
            let parsed = parse_expression_script(source);
            let encoded = serde_json::to_string(&parsed).unwrap();
            assert!(
                !encoded.contains("AttackModuleSetAttackReferenceJointId"),
                "{source}"
            );
        };
        assert_not_typed(&expression_source(
            &valid_call.replace("agent.module_accessor", "boma"),
        ));
        let wrong_arity_split = valid_call.rfind(", AttackDirectionAxis(*").unwrap();
        let wrong_arity = format!("{});", &valid_call[..wrong_arity_split]);
        assert_not_typed(&expression_source(&wrong_arity));
        for (joint, a, b, c) in [
            (
                "rot",
                "ATTACK_DIRECTION_X",
                "ATTACK_DIRECTION_Z",
                "ATTACK_DIRECTION_Y",
            ),
            (
                "top",
                "ATTACK_DIRECTION_Y",
                "ATTACK_DIRECTION_X",
                "ATTACK_DIRECTION_Z",
            ),
        ] {
            let unlisted = format!(
                "AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new(\"{joint}\"), AttackDirectionAxis(*{a}), AttackDirectionAxis(*{b}), AttackDirectionAxis(*{c}));"
            );
            assert_not_typed(&expression_source(&unlisted));
        }
        assert_not_typed(&expression_source(&format!("if true {{ {valid_call} }}")));
        assert_not_typed(&expression_source(&format!(
            "#[cfg(any())]\n        {valid_call}"
        )));
        assert_not_typed(&expression_source(&format!("wrapper!({valid_call});")));
        let bare_expression = format!(
            "unsafe extern \"C\" fn expression_reference_bare(agent: &mut L2CAgentBase) {{\n    {valid_call}\n}}\n"
        );
        assert_not_typed(&bare_expression);
        let sound_source =
            bare_expression.replace("expression_reference_bare", "sound_reference_bare");
        assert!(!serde_json::to_string(&parse_sound_script(&sound_source))
            .unwrap()
            .contains("AttackModuleSetAttackReferenceJointId"));

        let mixed_source = expression_source(&format!(
            "unknown_before();\n        {valid_call}\n        unknown_after();"
        ));
        let mixed = parse_expression_script(&mixed_source);
        let [AcmdStmt::Excute(commands)] = mixed.stmts.as_slice() else {
            panic!("expected mixed Execute block: {:#?}", mixed.stmts);
        };
        assert!(matches!(
            commands.as_slice(),
            [ExcuteStmt::Raw(_), ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind: found }, ExcuteStmt::Raw(_)]
                if *found == kind
        ));
        let mixed_export = preview_expression_fn(&mixed, "reference_mixed");
        assert_json_eq!(parse_expression_script(&mixed_export).stmts, mixed.stmts);
    }

    #[test]
    fn attack_module_init_reference_and_smoke_calls_are_closed_game_forms() {
        use crate::data::{AttackReferenceJointKind, ExcuteStmt};

        let source = r#"unsafe extern "C" fn game_attack_module_exact_calls(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::init_attack_pos(agent.module_accessor, 0);
        AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new("top"), AttackDirectionAxis(*ATTACK_DIRECTION_Y), AttackDirectionAxis(*ATTACK_DIRECTION_Z), AttackDirectionAxis(*ATTACK_DIRECTION_X));
        AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new("top"), AttackDirectionAxis(*ATTACK_DIRECTION_Y), AttackDirectionAxis(*ATTACK_DIRECTION_NONE), AttackDirectionAxis(*ATTACK_DIRECTION_NONE));
        AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new("toer"), AttackDirectionAxis(*ATTACK_DIRECTION_Y_MINUS), AttackDirectionAxis(*ATTACK_DIRECTION_X), AttackDirectionAxis(*ATTACK_DIRECTION_Z));
        AttackModule::set_no_damage_fly_smoke_all(agent.module_accessor, true, false);
        AttackModule::init_attack_pos(agent.module_accessor, 1);
        AttackModule::init_attack_pos(boma, 0);
        AttackModule::init_attack_pos(agent.module_accessor, 0, true);
        AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new("top"), AttackDirectionAxis(*ATTACK_DIRECTION_Y), AttackDirectionAxis(*ATTACK_DIRECTION_X), AttackDirectionAxis(*ATTACK_DIRECTION_Z));
        AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new( "top" ), AttackDirectionAxis(*ATTACK_DIRECTION_Y), AttackDirectionAxis(*ATTACK_DIRECTION_Z), AttackDirectionAxis(*ATTACK_DIRECTION_X));
        AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new("top"), AttackDirectionAxis( *ATTACK_DIRECTION_Y ), AttackDirectionAxis(*ATTACK_DIRECTION_Z), AttackDirectionAxis(*ATTACK_DIRECTION_X));
        AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new_raw(0x0000000000000000), AttackDirectionAxis(*ATTACK_DIRECTION_Y), AttackDirectionAxis(*ATTACK_DIRECTION_Z), AttackDirectionAxis(*ATTACK_DIRECTION_X));
        AttackModule::set_attack_reference_joint_id(boma, Hash40::new("top"), AttackDirectionAxis(*ATTACK_DIRECTION_Y), AttackDirectionAxis(*ATTACK_DIRECTION_Z), AttackDirectionAxis(*ATTACK_DIRECTION_X));
        AttackModule::set_no_damage_fly_smoke_all(agent.module_accessor, true, true);
        AttackModule::set_no_damage_fly_smoke_all(agent.module_accessor, false, false);
        AttackModule::set_no_damage_fly_smoke_all(boma, true, false);
        AttackModule::set_no_damage_fly_smoke_all(agent.module_accessor, true, false, true);
    }
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD) {
        if macros::is_excute(agent) {
            AttackModule::init_attack_pos(agent.module_accessor, 0);
            AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new("top"), AttackDirectionAxis(*ATTACK_DIRECTION_Y), AttackDirectionAxis(*ATTACK_DIRECTION_Z), AttackDirectionAxis(*ATTACK_DIRECTION_X));
            AttackModule::set_no_damage_fly_smoke_all(agent.module_accessor, true, false);
        }
    }
    for _ in 0..2 {
        if macros::is_excute(agent) {
            AttackModule::init_attack_pos(agent.module_accessor, 0);
            AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new("top"), AttackDirectionAxis(*ATTACK_DIRECTION_Y), AttackDirectionAxis(*ATTACK_DIRECTION_Z), AttackDirectionAxis(*ATTACK_DIRECTION_X));
            AttackModule::set_no_damage_fly_smoke_all(agent.module_accessor, true, false);
        }
    }
    AttackModule::init_attack_pos(agent.module_accessor, 0);
    AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new("top"), AttackDirectionAxis(*ATTACK_DIRECTION_Y), AttackDirectionAxis(*ATTACK_DIRECTION_Z), AttackDirectionAxis(*ATTACK_DIRECTION_X));
    AttackModule::set_no_damage_fly_smoke_all(agent.module_accessor, true, false);
}
"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected direct Game execute block: {:#?}", parsed.stmts);
        };
        assert!(matches!(
            commands.first(),
            Some(ExcuteStmt::AttackModuleInitAttackPos { id: 0 })
        ));
        assert!(matches!(
            commands.get(1),
            Some(ExcuteStmt::AttackModuleSetAttackReferenceJointId {
                kind: AttackReferenceJointKind::TopYZX,
            })
        ));
        assert!(matches!(
            commands.get(2),
            Some(ExcuteStmt::AttackModuleSetAttackReferenceJointId {
                kind: AttackReferenceJointKind::TopYNoneNone,
            })
        ));
        assert!(matches!(
            commands.get(3),
            Some(ExcuteStmt::AttackModuleSetAttackReferenceJointId {
                kind: AttackReferenceJointKind::ToerYMinusXZ,
            })
        ));
        assert!(matches!(
            commands.get(4),
            Some(ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll {
                arg2: true,
                arg3: false,
            })
        ));
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::Raw(_)))
                .count(),
            12,
            "unmeasured IDs, tuples, receivers, tails, and arities stay Raw"
        );
        for index in [1, 2] {
            assert!(matches!(
                &parsed.stmts[index],
                AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. }
                    if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                        if commands.iter().all(|command| matches!(command, ExcuteStmt::Raw(_))))
            ));
        }
        assert!(matches!(parsed.stmts.get(3), Some(AcmdStmt::Raw(_))));

        let expression = r#"unsafe extern "C" fn expression_wrong_track(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::init_attack_pos(agent.module_accessor, 0);
        AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new("top"), AttackDirectionAxis(*ATTACK_DIRECTION_Y), AttackDirectionAxis(*ATTACK_DIRECTION_Z), AttackDirectionAxis(*ATTACK_DIRECTION_X));
        AttackModule::set_no_damage_fly_smoke_all(agent.module_accessor, true, false);
    }
}
"#;
        let parsed_expression = parse_expression_script(expression);
        assert!(matches!(
            parsed_expression.stmts.as_slice(),
            [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [
                    ExcuteStmt::Raw(_),
                    ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind: AttackReferenceJointKind::TopYZX },
                    ExcuteStmt::Raw(_)
                ])
        ));

        let emitted = preview_game_fn(&parsed, "attack_module_exact_calls");
        assert!(emitted.contains(
            "AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new(\"toer\"), AttackDirectionAxis(*ATTACK_DIRECTION_Y_MINUS), AttackDirectionAxis(*ATTACK_DIRECTION_X), AttackDirectionAxis(*ATTACK_DIRECTION_Z));"
        ));
        assert_json_eq!(
            parse_acmd_script(&emitted).stmts,
            parsed.stmts,
            "typed calls and rejected/native-neighbor source must round-trip"
        );
    }

    #[test]
    fn revised_reaction_frames_retain_the_distinct_api_and_reject_unmeasured_forms() {
        let source = r#"unsafe extern "C" fn game_revised(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, 11.0, false);
        AttackModule::set_add_reaction_frame(agent.module_accessor, 4, 3.0, false);
        AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 8, -2.5, false);
    }
}"#;
        let mut parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &mut parsed.stmts[0] else {
            panic!("direct guard")
        };
        assert!(matches!(commands.as_slice(), [
            ExcuteStmt::AttackModuleAddReactionFrameRevised { id: 0, frames: first },
            ExcuteStmt::AttackModuleAddReactionFrame { id: 4, frames: second },
            ExcuteStmt::AttackModuleAddReactionFrameRevised { id: 8, frames: third },
        ] if *first == 11.0 && *second == 3.0 && *third == -2.5));
        let ExcuteStmt::AttackModuleAddReactionFrameRevised { id, frames } = &mut commands[0]
        else {
            unreachable!()
        };
        *id = 2;
        *frames = 12.5;
        let emitted = preview_game_fn(&parsed, "revised");
        assert!(emitted.contains(
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 2, 12.5, false);"
        ));
        assert!(emitted.contains(
            "AttackModule::set_add_reaction_frame(agent.module_accessor, 4, 3.0, false);"
        ));
        syn::parse_file(&emitted).unwrap();
        assert_json_eq!(&parsed, parse_acmd_script(&emitted));

        for bad in [
            "AttackModule::set_add_reaction_frame_revised(boma, 0, 11.0, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 9, 11.0, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, -1, 11.0, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 00, 11.0, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, 11, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, 11.00, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, 11.0f32, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, #[cfg(any())] 11.0, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, 11.0 + 1.0, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, f32::NAN, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, 1e99, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, 11.0, true);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, 11.0, false, true);",
            "AttackModule::set_add_reaction_frame_revised::<f32>(agent.module_accessor, 0, 11.0, false);",
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, 11.0, false); helper();",
        ] {
            let body = format!("unsafe extern \"C\" fn game_bad(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {bad}\n    }}\n}}");
            let parsed = parse_acmd_script(&body);
            assert!(matches!(parsed.stmts.as_slice(), [AcmdStmt::Excute(commands)] if commands.iter().all(|command| matches!(command, ExcuteStmt::Raw(_)))), "unmeasured form promoted: {bad}");
        }
        let effect = parse_effect_script(&source.replace("game_revised", "effect_revised"));
        assert!(
            matches!(effect.stmts.as_slice(), [crate::data::EffectStmt::Excute(commands)]
            if commands.iter().all(|command| matches!(command, crate::data::EffectMacro::Raw(_))))
        );
        for header in ["sound_revised", "expression_revised"] {
            let body = source.replace("game_revised", header);
            let parsed = if header.starts_with("sound") {
                parse_sound_script(&body)
            } else {
                parse_expression_script(&body)
            };
            assert!(
                matches!(parsed.stmts.as_slice(), [AcmdStmt::Excute(commands)] if commands.iter().all(|command| matches!(command, ExcuteStmt::Raw(_))))
            );
        }
    }

    #[test]
    fn add_reaction_frame_is_a_closed_guarded_game_call() {
        use crate::data::ExcuteStmt;

        let source = r#"unsafe extern "C" fn game_reaction_frame(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0, false);
        AttackModule::set_add_reaction_frame(agent.module_accessor, 8, -10.0, false);
        AttackModule::set_add_reaction_frame(agent.module_accessor, 00, 2.0, false);
        AttackModule::set_add_reaction_frame(agent.module_accessor, 9, 2.0, false);
        AttackModule::set_add_reaction_frame(boma, 0, 2.0, false);
        AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0, true);
        AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.00, false);
        AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0 + 1.0, false);
        AttackModule::set_add_reaction_frame(agent.module_accessor, 0, f32::NAN, false);
        AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0, false, true);
        if some_other_condition(agent) {
            AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 3.0, false);
        }
    }
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD) {
        if macros::is_excute(agent) {
            AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0, false);
        }
    }
    for _ in 0..2 {
        if macros::is_excute(agent) {
            AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0, false);
        }
    }
    AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0, false);
}
"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected a direct Game execute block: {:#?}", parsed.stmts);
        };
        // Ten direct calls plus the preserved nested header, call and closing brace.
        assert_eq!(commands.len(), 13);
        assert!(matches!(
            commands.first(),
            Some(ExcuteStmt::AttackModuleAddReactionFrame { id: 0, frames }) if *frames == 2.0
        ));
        assert!(matches!(
            commands.get(1),
            Some(ExcuteStmt::AttackModuleAddReactionFrame { id: 8, frames }) if *frames == -10.0
        ));
        assert!(commands[2..]
            .iter()
            .all(|command| matches!(command, ExcuteStmt::Raw(_))));
        assert!(matches!(
            &parsed.stmts[1],
            AcmdStmt::RawBlock { body, .. }
                if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::Raw(_)]))
        ));
        assert!(matches!(
            &parsed.stmts[2],
            AcmdStmt::Loop { body, .. }
                if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::Raw(_)]))
        ));
        assert!(matches!(parsed.stmts.get(3), Some(AcmdStmt::Raw(_))));

        let expression = r#"unsafe extern "C" fn expression_wrong_track(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0, false);
    }
}
"#;
        let wrong_track = parse_expression_script(expression);
        assert!(matches!(
            wrong_track.stmts.as_slice(),
            [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::Raw(_)])
        ));

        for ambiguous_line in ["// comment brace }", "helper(\"literal } brace\");"] {
            let guarded = format!(
                "unsafe extern \"C\" fn game_reaction_frame(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {ambiguous_line}\n        AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0, false);\n    }}\n}}\n"
            );
            let guarded = parse_acmd_script(&guarded);
            assert!(
                matches!(
                    guarded.stmts.as_slice(),
                    [AcmdStmt::Excute(commands)]
                        if matches!(commands.as_slice(), [ExcuteStmt::Raw(_), ExcuteStmt::AttackModuleAddReactionFrame { id: 0, frames }]
                            if *frames == 2.0)
                ),
                "literal braces must not move a genuine root command out of its native guard"
            );
            let emitted = preview_game_fn(&guarded, "reaction_frame_guard");
            assert_json_eq!(
                guarded,
                parse_acmd_script(&emitted),
                "literal braces and guarded commands must round-trip source"
            );
        }

        let emitted = preview_game_fn(&parsed, "reaction_frame");
        assert!(emitted.contains(
            "AttackModule::set_add_reaction_frame(agent.module_accessor, 8, -10.0, false);"
        ));
        assert_json_eq!(
            parse_acmd_script(&emitted).stmts,
            parsed.stmts,
            "typed calls and all rejected contexts/shapes should reparse identically"
        );
    }

    #[test]
    fn malformed_is_excute_dump_artifact_stays_raw_and_does_not_open_execute_block() {
        let source = r#"unsafe extern "C" fn game_invalid_dump_artifact(agent: &mut L2CAgentBase) {
    methodlib::L2CValue::operatorbool()const(is_excute);
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        AttackModule::init_attack_pos(agent.module_accessor, 0);
    }
}
"#;
        let parsed = parse_acmd_script(source);
        assert!(matches!(
            parsed.stmts.as_slice(),
            [
                AcmdStmt::Raw(raw),
                AcmdStmt::Frame(frame),
                AcmdStmt::Excute(commands)
            ] if raw == "methodlib::L2CValue::operatorbool()const(is_excute);"
                && *frame == 1.0
                && matches!(commands.as_slice(), [ExcuteStmt::AttackModuleInitAttackPos { id: 0 }])
        ));

        let emitted = preview_game_fn(&parsed, "invalid_dump_artifact");
        assert!(emitted.contains("methodlib::L2CValue::operatorbool()const(is_excute);"));
        assert!(
            syn::parse_file(&emitted).is_err(),
            "the invalid source line must not be normalized into executable Rust"
        );
        assert_json_eq!(parse_acmd_script(&emitted).stmts, parsed.stmts);
    }

    #[test]
    fn article_lifecycle_forms_are_fixed_symbolic_calls() {
        use crate::data::{ArticleLifecycleCall, ExcuteStmt};

        let source = r#"unsafe extern "C" fn game_article(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        ArticleModule::generate_article(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER, false, -1);
        ArticleModule::remove_exist(agent.module_accessor, *FIGHTER_GANON_GENERATE_ARTICLE_SWORD, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));
        ArticleModule::set_visibility_whole(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_BARRELJETS, false, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));
        ArticleModule::set_visibility_whole(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_BARRELJETS, true, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_ALL, false);
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_LAST, false);
        ArticleModule::shoot_exist(agent.module_accessor, *FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1, *ARTICLE_OPE_TARGET_LAST, false);
        ArticleModule::generate_article(agent.module_accessor, 24, false, -1);
        ArticleModule::generate_article(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER, true, -1);
        ArticleModule::generate_article(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER, false, 0);
        ArticleModule::generate_article(boma, *FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER, false, -1);
        ArticleModule::remove_exist(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER, ArticleOperationTarget(0));
        ArticleModule::remove_exist(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL), true);
        ArticleModule::generate_article(agent.module_accessor, ARTICLE_SYMBOL, false, -1);
        ArticleModule::set_visibility_whole(boma, *FIGHTER_DIDDY_GENERATE_ARTICLE_BARRELJETS, false, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));
        ArticleModule::set_visibility_whole(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_BARRELJETS, 1, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));
        ArticleModule::set_visibility_whole(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_BARRELJETS, false, ArticleOperationTarget(0));
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_FIRST, false);
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_ALL, true);
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, ArticleOperationTarget(0), false);
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, ArticleOperationTarget(*ARTICLE_OPE_TARGET_OTHER), false);
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, ArticleOperationTarget(ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL)), false);
        ArticleModule::shoot_exist(agent.module_accessor, *FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1, *ARTICLE_OPE_TARGET_ALL, false, true);
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_ALL, false); panic!();
        wrapper(ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_ALL, false));
    }
}
"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected one execute block: {:#?}", parsed.stmts);
        };
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::ArticleLifecycle(_)))
                .count(),
            7
        );
        assert!(matches!(
            commands.first(),
            Some(ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::Generate { symbol }))
                if symbol == "FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER"
        ));
        assert!(matches!(
            commands.get(1),
            Some(ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::RemoveAll { symbol }))
                if symbol == "FIGHTER_GANON_GENERATE_ARTICLE_SWORD"
        ));
        assert!(matches!(
            commands.get(2),
            Some(ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::SetVisibilityWhole { symbol, visible: false }))
                if symbol == "FIGHTER_DIDDY_GENERATE_ARTICLE_BARRELJETS"
        ));
        assert!(matches!(
            commands.get(3),
            Some(ExcuteStmt::ArticleLifecycle(
                ArticleLifecycleCall::SetVisibilityWhole { visible: true, .. }
            ))
        ));
        assert!(matches!(
            commands.get(4),
            Some(ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::Shoot { symbol, target: crate::data::ArticleTarget::All }))
                if symbol == "FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA"
        ));
        assert!(matches!(
            commands.get(5),
            Some(ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::Shoot {
                target: crate::data::ArticleTarget::Last,
                ..
            }))
        ));
        assert!(matches!(
            commands.get(6),
            Some(ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::ShootExist { symbol, target: crate::data::ArticleTarget::Last }))
                if symbol == "FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1"
        ));
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, ExcuteStmt::Raw(_)))
                .count(),
            15
        );
        for (index, enabled, article_index, receiver) in [
            (8, "true", "-1", None),
            (9, "false", "0", None),
            (10, "false", "-1", Some("boma")),
        ] {
            assert!(
                matches!(commands.get(index), Some(ExcuteStmt::NativeCall(call))
                if call.callee == "ArticleModule::generate_article"
                    && call.arguments == ["*FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER", enabled, article_index]
                    && call.receiver.as_deref() == receiver)
            );
        }
        let emitted = preview_game_fn(&parsed, "article");
        assert!(emitted.contains(
            "ArticleModule::generate_article(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER, false, -1);"
        ));
        assert!(emitted.contains(
            "ArticleModule::remove_exist(agent.module_accessor, *FIGHTER_GANON_GENERATE_ARTICLE_SWORD, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));"
        ));
        assert!(emitted.contains(
            "ArticleModule::set_visibility_whole(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_BARRELJETS, false, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));"
        ));
        assert!(emitted.contains(
            "ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST), false);"
        ));
        assert!(emitted.contains(
            "ArticleModule::shoot_exist(agent.module_accessor, *FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1, ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST), false);"
        ));
        assert_json_eq!(parse_acmd_script(&emitted).stmts, parsed.stmts);
    }

    #[test]
    fn combined_execute_else_keeps_the_complete_original_body_on_all_acmd_tracks() {
        for (category, parse, emit) in [
            (
                "game",
                parse_acmd_script as fn(&str) -> AcmdScript,
                preview_game_fn as fn(&AcmdScript, &str) -> String,
            ),
            ("expression", parse_expression_script, preview_expression_fn),
            ("sound", parse_sound_script, preview_sound_fn),
            (
                "effect",
                |source| AcmdScript {
                    stmts: parse_effect_script(source)
                        .stmts
                        .iter()
                        .map(crate::move_nodes::effects::effect_stmt_to_acmd)
                        .collect(),
                },
                preview_effect_structure_fn,
            ),
        ] {
            for (guard, alternate, prefix, suffix) in [
                ("if macros::is_excute(agent) {", "} else {", "", ""),
                (
                    "if is_excute(agent) {",
                    "} /* retained comment */ else {",
                    "",
                    "",
                ),
                (
                    "if macros::is_excute(agent) {",
                    "} else if unknown_condition() {",
                    "if outer_condition() {\n",
                    "\n}",
                ),
                (
                    "if macros::is_excute(agent) {",
                    "} else {",
                    "for _ in 0..2 {\n",
                    "\n}",
                ),
                (
                    "if macros::is_excute(agent) {",
                    "} else {",
                    "#[cfg(any())]\n",
                    "",
                ),
            ] {
                let body = format!("frame(agent.lua_state_agent, 1.0);\n{prefix}{guard}\n    unknown_then();\n{alternate}\n    unknown_else();\n}}{suffix}\nwait(agent.lua_state_agent, 2.0);");
                let source = format!("unsafe extern \"C\" fn {category}_else_witness(agent: &mut L2CAgentBase) {{\n{body}\n}}\n");
                syn::parse_file(&source).unwrap();
                let parsed = parse(&source);
                assert!(
                    matches!(parsed.stmts.as_slice(), [AcmdStmt::Raw(raw)] if raw == &body),
                    "{source}"
                );
                let emitted = emit(&parsed, "else_witness");
                syn::parse_file(&emitted).unwrap();
                let emitted_body = emitted
                    .split_once('{')
                    .unwrap()
                    .1
                    .rsplit_once('}')
                    .unwrap()
                    .0
                    .trim();
                assert_eq!(
                    emitted_body, body,
                    "complete original source, not only the already-parsed tree"
                );
                assert_json_eq!(&parsed, parse(&emitted));
            }
        }
    }

    #[test]
    fn combined_execute_else_detection_ignores_literals_comments_and_other_conditions() {
        for source in [
            "unsafe fn game_else_witness(agent: &mut L2CAgentBase) {\n    // if macros::is_excute(agent) { } else { }\n    if macros::is_excute(agent) {\n        unknown();\n    }\n}\n",
            "unsafe fn game_else_witness(agent: &mut L2CAgentBase) {\n    let text = r#\"if macros::is_excute(agent) { } else { }\"#;\n}\n",
            "unsafe fn game_else_witness(agent: &mut L2CAgentBase) {\n    if other::is_excute(agent) {\n        one();\n    } else {\n        two();\n    }\n}\n",
            "unsafe fn game_else_witness(agent: &mut L2CAgentBase) {\n    if macros::is_excute(agent) {\n        one();\n    }\n    else {\n        two();\n    }\n}\n",
        ] {
            let lines: Vec<_> = source.lines().collect();
            assert!(preserve_combined_execute_else(source, &lines[1..lines.len()-1]).is_none());
        }
    }

    #[test]
    fn effect_execute_else_remains_one_immutable_witness_not_live_spawns() {
        use crate::move_nodes::{effects, model, timeline};

        let spawn = |name| {
            format!("macros::EFFECT(agent, Hash40::new(\"{name}\"), Hash40::new(\"top\"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, true);")
        };
        let body = format!("frame(agent.lua_state_agent, 7.0);\nif macros::is_excute(agent) {{\n    {}\n}} else {{\n    {}\n}}\nwait(agent.lua_state_agent, 2.0);", spawn("then_effect"), spawn("else_effect"));
        let source = format!(
            "unsafe extern \"C\" fn effect_else_witness(agent: &mut L2CAgentBase) {{\n{body}\n}}\n"
        );
        let parsed = parse_effect_script(&source);
        assert!(matches!(parsed.stmts.as_slice(), [EffectStmt::Raw(raw)] if raw == &body));
        let (calls, residue) = parsed.to_effect_calls_and_residue();
        assert!(
            calls.is_empty(),
            "neither arm may become an unconditional live spawn"
        );
        assert!(residue.is_empty());
        assert_eq!(unexportable_effect_lines(&parsed), vec![body.clone()]);

        let graph = model::MoveGraph {
            tracks: vec![effects::import_for_fighter("mario", &parsed)],
            ..Default::default()
        };
        assert!(
            matches!(graph.tracks[0].nodes.as_slice(), [model::MoveNode { kind: model::NodeKind::Preserved { statement: AcmdStmt::Raw(raw) }, .. }] if raw == &body)
        );
        let restored: model::MoveGraph =
            serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        let compiled = timeline::compile("mario", "else_witness", &restored).unwrap();
        let emitted = preview_effect_structure_fn(&compiled[0].1, "else_witness");
        assert_eq!(
            emitted
                .split_once('{')
                .unwrap()
                .1
                .rsplit_once('}')
                .unwrap()
                .0
                .trim(),
            body
        );
        syn::parse_file(&emitted).unwrap();
        assert_json_eq!(parse_effect_script(&emitted), &parsed);

        let mut forged = graph;
        let model::NodeKind::Preserved {
            statement: AcmdStmt::Raw(raw),
        } = &mut forged.tracks[0].nodes[0].kind
        else {
            panic!("expected source witness")
        };
        *raw = raw.replace("else_effect", "changed_effect");
        assert!(timeline::compile("mario", "else_witness", &forged).is_err());

        let ordinary = format!("unsafe extern \"C\" fn effect_ordinary(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {}\n    }}\n}}\n", spawn("ordinary_effect"));
        let ordinary = parse_effect_script(&ordinary);
        assert_eq!(
            ordinary.to_effect_calls().len(),
            1,
            "ordinary Execute remains editable"
        );
        assert!(matches!(ordinary.stmts.as_slice(), [EffectStmt::Excute(_)]));
    }

    #[test]
    fn expression_article_motion_catalog_is_exact_track_scoped_and_roundtrips() {
        use crate::data::{ArticleLifecycleCall, EXPRESSION_ARTICLE_MOTIONS};
        let mut unique = std::collections::HashSet::new();
        assert_eq!(EXPRESSION_ARTICLE_MOTIONS.len(), 19);
        for &(symbol, motions) in EXPRESSION_ARTICLE_MOTIONS {
            for &motion in motions {
                assert!(unique.insert((symbol, motion)));
                let call = format!("ArticleModule::change_motion(agent.module_accessor, *{symbol}, Hash40::new(\"{motion}\"), false, -1.0);");
                let source = format!("unsafe extern \"C\" fn expression_article_motion(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n");
                let parsed = parse_expression_script(&source);
                assert!(
                    matches!(parsed.stmts.as_slice(), [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::ChangeMotion { symbol: found_symbol, motion: found_motion })]
                        if found_symbol == symbol && found_motion == motion)),
                    "{source}"
                );
                assert!(parsed.to_expression_events().is_empty());
                let emitted = preview_expression_fn(&parsed, "article_motion");
                syn::parse_file(&emitted).unwrap();
                assert_json_eq!(&parsed, parse_expression_script(&emitted));
                for other in [
                    parse_acmd_script(
                        &source.replace("expression_article_motion", "game_article_motion"),
                    ),
                    parse_sound_script(
                        &source.replace("expression_article_motion", "sound_article_motion"),
                    ),
                ] {
                    assert!(!serde_json::to_string(&other)
                        .unwrap()
                        .contains("ChangeMotion"));
                }
                let mixed_source = source.replace(
                    &call,
                    &format!("unknown_before();\n        {call}\n        unknown_after();"),
                );
                let mixed = parse_expression_script(&mixed_source);
                assert!(
                    matches!(mixed.stmts.as_slice(), [AcmdStmt::Excute(commands)] if matches!(commands.as_slice(), [ExcuteStmt::Raw(before), ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::ChangeMotion { .. }), ExcuteStmt::Raw(after)] if before == "unknown_before();" && after == "unknown_after();"))
                );
                assert_json_eq!(
                    &mixed,
                    parse_expression_script(&preview_expression_fn(&mixed, "article_motion"))
                );
            }
        }
        assert_eq!(unique.len(), 86);
    }

    #[test]
    fn expression_article_motion_rejects_unmeasured_tokens_and_execution_contexts() {
        let call = "ArticleModule::change_motion(agent.module_accessor, *FIGHTER_METAKNIGHT_GENERATE_ARTICLE_MANTLE, Hash40::new(\"guard\"), false, -1.0);";
        let wrap = |body: &str| {
            format!("unsafe extern \"C\" fn expression_motion_negative(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {body}\n    }}\n}}\n")
        };
        let assert_preserved = |source: &str| {
            let parsed = parse_expression_script(source);
            assert!(
                !serde_json::to_string(&parsed)
                    .unwrap()
                    .contains("ChangeMotion"),
                "{source}"
            );
            assert_json_eq!(
                &parsed,
                parse_expression_script(&preview_expression_fn(&parsed, "motion_negative")),
                "{source}"
            );
        };
        for bad in [
            call.replace("agent.module_accessor", "boma"),
            call.replace("false", "true"),
            call.replace("-1.0", "-1"),
            call.replace("-1.0", "0.0"),
            call.replace("false, -1.0", "false, -1.0, true"),
            call.replace("*FIGHTER_", "FIGHTER_"),
            call.replace("METAKNIGHT", "MARIO"),
            call.replace("\"guard\"", "\"unmeasured_clip\""),
            call.replace("\"guard\"", "\"win2\""),
            call.replace("Hash40::new(\"guard\")", "Hash40::new_raw(0x1234)"),
            call.replace("Hash40::new(\"guard\")", "wrapper(Hash40::new(\"guard\"))"),
            call.replace("Hash40::new(\"guard\")", "Hash40::new( \"guard\" )"),
            call.replace("change_motion(", "change_motion_from_no("),
            format!("wrapper!({call});"),
            format!("#[cfg(any())]\n        {call}"),
            format!("if true {{\n            {call}\n        }}"),
        ] {
            assert_preserved(&wrap(&bad));
        }
        for source in [
            format!("unsafe extern \"C\" fn expression_motion_negative(agent: &mut L2CAgentBase) {{\n    {call}\n}}\n"),
            format!("unsafe extern \"C\" fn expression_motion_negative(agent: &mut L2CAgentBase) {{\n    for _ in 0..2 {{\n        if macros::is_excute(agent) {{\n            {call}\n        }}\n    }}\n}}\n"),
            format!("unsafe extern \"C\" fn expression_motion_negative(agent: &mut L2CAgentBase) {{\n    if true {{\n        if macros::is_excute(agent) {{\n            {call}\n        }}\n    }}\n}}\n"),
            format!("unsafe extern \"C\" fn expression_motion_negative(agent: &mut L2CAgentBase) {{\n    #[cfg(any())]\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n"),
            format!("unsafe extern \"C\" fn expression_motion_negative(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {call}\n    }} else {{\n        unknown_else();\n    }}\n}}\n"),
        ] {
            assert_preserved(&source);
        }
        assert!(crate::data::measured_expression_article_motions(
            "FIGHTER_KIRBY_GENERATE_ARTICLE_STONE"
        )
        .is_empty());
        assert!(crate::data::measured_article_motions(
            "FIGHTER_METAKNIGHT_GENERATE_ARTICLE_MANTLE"
        )
        .is_empty());
    }

    #[test]
    fn article_change_motion_is_a_closed_game_execute_source_form() {
        use crate::data::{ArticleLifecycleCall, ExcuteStmt};

        let source = r#"unsafe extern "C" fn game_article_motion(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        ArticleModule::change_motion(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_STONE, Hash40::new("special_lw_to_ground"), false, -1.0);
        ArticleModule::change_motion(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_STONE, Hash40::new("unmeasured_clip"), false, -1.0);
        ArticleModule::change_motion(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_STONE, Hash40::new("special_lw"), true, -1.0);
        ArticleModule::change_motion(boma, *FIGHTER_KIRBY_GENERATE_ARTICLE_STONE, Hash40::new("special_lw"), false, -1.0);
        ArticleModule::change_motion(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_STONE, Hash40::new("special_lw"), false, 0.0);
        ArticleModule::change_motion(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER, Hash40::new("special_lw"), false, -1.0);
        ArticleModule::change_motion(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_STONE, Hash40::new_raw(0x1234), false, -1.0);
        ArticleModule::change_motion(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_STONE, Hash40::new("special_lw"), false, -1.0, true);
    }
    for _ in 0..2 {
        if macros::is_excute(agent) {
            ArticleModule::change_motion(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_STONE, Hash40::new("special_lw"), false, -1.0);
        }
    }
}
"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected Game execute block: {:#?}", parsed.stmts);
        };
        assert!(matches!(
            commands.first(),
            Some(ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::ChangeMotion {
                symbol,
                motion,
            })) if symbol == "FIGHTER_KIRBY_GENERATE_ARTICLE_STONE" && motion == "special_lw_to_ground"
        ));
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(
                    command,
                    ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::ChangeMotion { .. })
                ))
                .count(),
            1,
            "measured motions retain their specialized representation"
        );
        assert!(
            matches!(parsed.stmts.get(1), Some(AcmdStmt::Loop { body, .. })
            if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::NativeCall(call)]
                    if call.callee == "ArticleModule::change_motion")))
        );

        let expression = parse_expression_script(
            &source.replace("game_article_motion", "expression_article_motion"),
        );
        assert!(
            matches!(expression.stmts.first(), Some(AcmdStmt::Excute(commands))
            if matches!(commands.first(), Some(ExcuteStmt::NativeCall(call))
                if call.callee == "ArticleModule::change_motion"))
        );

        let emitted = preview_game_fn(&parsed, "article_motion");
        assert!(emitted.contains(
            "ArticleModule::change_motion(agent.module_accessor, *FIGHTER_KIRBY_GENERATE_ARTICLE_STONE, Hash40::new(\"special_lw_to_ground\"), false, -1.0);"
        ));
        assert_json_eq!(
            parse_acmd_script(&emitted).stmts,
            parsed.stmts,
            "the complete statement tree must round-trip"
        );
    }

    #[test]
    fn attack_module_shake_and_unique_effect_calls_use_closed_game_source_forms() {
        use crate::data::{
            AcmdStmt, AttackModuleDamageShakeScale, AttackModuleNoUniqEffectAllPair, ExcuteStmt,
        };

        let source = r#"unsafe extern "C" fn game_attack_module_closed(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_damage_shake_scale(agent.module_accessor, 0.1);
        AttackModule::set_damage_shake_scale(agent.module_accessor, 0.18);
        AttackModule::set_damage_shake_scale(agent.module_accessor, 0.3);
        AttackModule::set_damage_shake_scale(agent.module_accessor, 0.5);
        AttackModule::set_damage_shake_scale(agent.module_accessor, 0.67);
        AttackModule::set_damage_shake_scale(agent.module_accessor, 1.5);
        AttackModule::set_no_uniq_effect_all(agent.module_accessor, true, false);
        AttackModule::set_no_uniq_effect_all(agent.module_accessor, true, true);
        AttackModule::set_damage_shake_scale(agent.module_accessor, 0);
        AttackModule::set_damage_shake_scale(agent.module_accessor, 1);
    }
}"#;
        let parsed = parse_acmd_script(source);
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("expected Game execute block")
        };
        assert!(matches!(
            commands.as_slice(),
            [
                ExcuteStmt::AttackModuleSetDamageShakeScale(AttackModuleDamageShakeScale::PointOne),
                ExcuteStmt::AttackModuleSetDamageShakeScale(AttackModuleDamageShakeScale::PointEighteen),
                ExcuteStmt::AttackModuleSetDamageShakeScale(AttackModuleDamageShakeScale::PointThree),
                ExcuteStmt::AttackModuleSetDamageShakeScale(AttackModuleDamageShakeScale::PointFive),
                ExcuteStmt::AttackModuleSetDamageShakeScale(AttackModuleDamageShakeScale::PointSixtySeven),
                ExcuteStmt::AttackModuleSetDamageShakeScale(AttackModuleDamageShakeScale::OnePointFive),
                ExcuteStmt::AttackModuleSetNoUniqEffectAll(AttackModuleNoUniqEffectAllPair::TrueFalse),
                ExcuteStmt::AttackModuleSetNoUniqEffectAll(AttackModuleNoUniqEffectAllPair::TrueTrue),
                ExcuteStmt::Raw(zero),
                ExcuteStmt::Raw(one),
            ] if zero.contains("set_damage_shake_scale") && zero.ends_with(", 0);")
                && one.contains("set_damage_shake_scale") && one.ends_with(", 1);")
        ));
        for scale in AttackModuleDamageShakeScale::ALL {
            let value = serde_json::to_value(scale).unwrap();
            assert_json_eq!(
                serde_json::from_value::<AttackModuleDamageShakeScale>(value).unwrap(),
                scale
            );
        }
        for pair in AttackModuleNoUniqEffectAllPair::ALL {
            let value = serde_json::to_value(pair).unwrap();
            assert_json_eq!(
                serde_json::from_value::<AttackModuleNoUniqEffectAllPair>(value).unwrap(),
                pair
            );
        }

        let emitted = preview_game_fn(&parsed, "attack_module_closed");
        for scale in AttackModuleDamageShakeScale::ALL {
            assert!(emitted.contains(&format!(
                "AttackModule::set_damage_shake_scale(agent.module_accessor, {});",
                scale.source_token()
            )));
        }
        assert!(emitted
            .contains("AttackModule::set_no_uniq_effect_all(agent.module_accessor, true, false);"));
        assert!(emitted
            .contains("AttackModule::set_no_uniq_effect_all(agent.module_accessor, true, true);"));
        assert_json_eq!(
            parse_acmd_script(&emitted).stmts,
            parsed.stmts,
            "typed calls and the off-table integer source must round-trip"
        );
        syn::parse_file(&emitted).unwrap();

        for bad in [
            "AttackModule::set_damage_shake_scale(boma, 0.5);",
            "AttackModule::set_damage_shake_scale(other.module_accessor, 0.5);",
            "AttackModule::set_damage_shake_scale(agent.module_accessor, 0.50);",
            "AttackModule::set_damage_shake_scale(agent.module_accessor, 0.5f32);",
            "AttackModule::set_damage_shake_scale(agent.module_accessor, 0.5 + 0.1);",
            "AttackModule::set_damage_shake_scale(agent.module_accessor, 0.25);",
            "AttackModule::set_damage_shake_scale(agent.module_accessor, f32::NAN);",
            "AttackModule::set_damage_shake_scale(agent.module_accessor, 0.5, false);",
            "AttackModule::set_no_uniq_effect_all(boma, true, false);",
            "AttackModule::set_no_uniq_effect_all(agent.module_accessor, false, false);",
            "AttackModule::set_no_uniq_effect_all(agent.module_accessor, true, false, true);",
            "AttackModule::set_no_uniq_effect_all(agent.module_accessor, flag, false);",
            "AttackModule::set_no_uniq_effect_all(agent.module_accessor, true, false); helper();",
        ] {
            let bad_source = format!(
                "unsafe extern \"C\" fn game_bad_attack_module(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {bad}\n    }}\n}}"
            );
            let rejected = parse_acmd_script(&bad_source);
            assert!(
                matches!(
                    rejected.stmts.as_slice(),
                    [AcmdStmt::Excute(commands)] if matches!(commands.as_slice(), [ExcuteStmt::Raw(raw)] if raw.trim() == bad)
                ),
                "unmeasured source was promoted: {bad}"
            );
        }
    }

    #[test]
    fn attack_module_closed_calls_require_direct_game_execute_heads() {
        use crate::data::{AcmdStmt, ExcuteStmt};

        let source = r##"unsafe extern "C" fn game_attack_module_context(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_damage_shake_scale(agent.module_accessor, 0.5);
        AttackModule::set_no_uniq_effect_all(agent.module_accessor, true, false);
        // AttackModule::set_damage_shake_scale(agent.module_accessor, 0.5);
        let source_text = r#"AttackModule::set_damage_shake_scale(agent.module_accessor, 0.5);"#;
        wrapper(AttackModule::set_damage_shake_scale(agent.module_accessor, 0.5));
        #[allow(unused)]
        AttackModule::set_damage_shake_scale(agent.module_accessor, 0.5);
    }
    AttackModule::set_damage_shake_scale(agent.module_accessor, 0.5);
}"##;
        let parsed = parse_acmd_script(source);
        assert!(matches!(
            parsed.stmts.as_slice(),
                [AcmdStmt::Excute(commands), AcmdStmt::Raw(top)]
                if matches!(commands.as_slice(), [
                    ExcuteStmt::AttackModuleSetDamageShakeScale(_),
                    ExcuteStmt::AttackModuleSetNoUniqEffectAll(_),
                    ExcuteStmt::Raw(comment),
                    ExcuteStmt::Raw(local),
                    ExcuteStmt::Raw(wrapper),
                    ExcuteStmt::Raw(attribute),
                    ExcuteStmt::Raw(attributed_call),
                ]
                if comment.contains("// AttackModule::")
                    && local.contains("let source_text")
                    && wrapper.contains("wrapper(AttackModule::")
                    && attribute.contains("#[allow(unused)]")
                    && attributed_call.contains("AttackModule::set_damage_shake_scale"))
                    && top.contains("AttackModule::set_damage_shake_scale")
        ));

        for (name, parse) in [
            (
                "expression_attack_module_context",
                parse_expression_script as fn(&str) -> AcmdScript,
            ),
            (
                "sound_attack_module_context",
                parse_sound_script as fn(&str) -> AcmdScript,
            ),
        ] {
            let other_track = parse(&source.replace("game_attack_module_context", name));
            assert!(
                matches!(
                    other_track.stmts.first(),
                    Some(AcmdStmt::Excute(commands)) if commands.iter().all(|command| matches!(command, ExcuteStmt::Raw(_)))
                ),
                "native source-only calls became typed in {name}"
            );
        }
    }

    #[test]
    fn hit_status_normal_and_attack_size_use_closed_game_source_tuples() {
        use crate::data::{AcmdStmt, AttackModuleSizePreset, ExcuteStmt};

        let source_calls = AttackModuleSizePreset::ALL
            .into_iter()
            .map(|preset| {
                format!(
                    "AttackModule::set_size(agent.module_accessor, {}, {});",
                    preset.attack_id(),
                    preset.size_token()
                )
            })
            .chain(std::iter::once(
                "HitModule::set_status_all(agent.module_accessor, HitStatus(*HIT_STATUS_NORMAL), 0);"
                    .to_owned(),
            ))
            .collect::<Vec<_>>();
        let source = format!(
            "unsafe extern \"C\" fn game_closed_hit_module(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {}\n    }}\n}}",
            source_calls.join("\n        ")
        );
        let parsed = parse_acmd_script(&source);
        let expected = AttackModuleSizePreset::ALL
            .into_iter()
            .map(ExcuteStmt::AttackModuleSetSize)
            .chain(std::iter::once(ExcuteStmt::HitModuleSetStatusAllNormal))
            .collect::<Vec<_>>();
        assert!(matches!(
            parsed.stmts.as_slice(),
            [AcmdStmt::Excute(commands)]
                if serde_json::to_value(commands).unwrap()
                    == serde_json::to_value(&expected).unwrap()
        ));
        for preset in AttackModuleSizePreset::ALL {
            let json = serde_json::to_value(preset).unwrap();
            assert_json_eq!(
                serde_json::from_value::<AttackModuleSizePreset>(json).unwrap(),
                preset
            );
        }
        assert!(
            serde_json::from_value::<AttackModuleSizePreset>(serde_json::json!("id_3_size_5"))
                .is_err()
        );

        let emitted = preview_game_fn(&parsed, "closed_hit_module_roundtrip");
        assert!(emitted.contains(
            "HitModule::set_status_all(agent.module_accessor, HitStatus(*HIT_STATUS_NORMAL), 0);"
        ));
        assert_json_eq!(parse_acmd_script(&emitted).stmts, parsed.stmts);
        syn::parse_file(&emitted).unwrap();

        for bad in [
            "AttackModule::set_size(boma, 0, 0.1);",
            "AttackModule::set_size(agent.module_accessor, 3, 0.1);",
            "AttackModule::set_size(agent.module_accessor, 1, 4.700);",
            "AttackModule::set_size(agent.module_accessor, 1, 4.7f32);",
            "AttackModule::set_size(agent.module_accessor, 1, 4.7 + 0.0);",
            "AttackModule::set_size(agent.module_accessor, 1, 0.5);",
            "AttackModule::set_size(agent.module_accessor, 1, 4.7, false);",
            "HitModule::set_status_all(boma, HitStatus(*HIT_STATUS_NORMAL), 0);",
            "HitModule::set_status_all(agent.module_accessor, HitStatus(*HIT_STATUS_XLU), 0);",
            "HitModule::set_status_all(agent.module_accessor, HitStatus(*HIT_STATUS_NORMAL), 1);",
            "HitModule::set_status_all(agent.module_accessor, HitStatus(*HIT_STATUS_NORMAL), 0, false);",
        ] {
            let bad_source = format!(
                "unsafe extern \"C\" fn game_bad_hit_module(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {bad}\n    }}\n}}"
            );
            let rejected = parse_acmd_script(&bad_source);
            assert!(matches!(
                rejected.stmts.as_slice(),
                [AcmdStmt::Excute(commands)] if matches!(commands.as_slice(), [ExcuteStmt::Raw(raw)] if raw.trim() == bad)
            ), "unmeasured source was promoted: {bad}");
        }

        let contexts = r##"unsafe extern "C" fn game_hit_module_context(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        HitModule::set_status_all(agent.module_accessor, HitStatus(*HIT_STATUS_NORMAL), 0);
        AttackModule::set_size(agent.module_accessor, 0, 0.1);
        // AttackModule::set_size(agent.module_accessor, 0, 0.1);
        let source_text = r#"AttackModule::set_size(agent.module_accessor, 0, 0.1);"#;
        wrapper(AttackModule::set_size(agent.module_accessor, 0, 0.1));
        #[allow(unused)]
        AttackModule::set_size(agent.module_accessor, 0, 0.1);
    }
    AttackModule::set_size(agent.module_accessor, 0, 0.1);
}"##;
        let contextual = parse_acmd_script(contexts);
        assert!(matches!(
            contextual.stmts.as_slice(),
            [AcmdStmt::Excute(commands), AcmdStmt::Raw(top)]
                if matches!(commands.as_slice(), [
                    ExcuteStmt::HitModuleSetStatusAllNormal,
                    ExcuteStmt::AttackModuleSetSize(AttackModuleSizePreset::Id0Size0_1),
                    ExcuteStmt::Raw(comment),
                    ExcuteStmt::Raw(local),
                    ExcuteStmt::Raw(wrapper),
                    ExcuteStmt::Raw(attribute),
                    ExcuteStmt::Raw(attributed),
                ] if comment.contains("// AttackModule::")
                    && local.contains("let source_text")
                    && wrapper.contains("wrapper(AttackModule::")
                    && attribute.contains("#[allow(unused)]")
                    && attributed.contains("AttackModule::set_size"))
                    && top.contains("AttackModule::set_size")
        ));
        for (name, parse) in [
            (
                "expression_hit_module_context",
                parse_expression_script as fn(&str) -> AcmdScript,
            ),
            (
                "sound_hit_module_context",
                parse_sound_script as fn(&str) -> AcmdScript,
            ),
        ] {
            let other_track = parse(&contexts.replace("game_hit_module_context", name));
            assert!(
                matches!(
                    other_track.stmts.first(),
                    Some(AcmdStmt::Excute(commands)) if commands.iter().all(|command| matches!(command, ExcuteStmt::Raw(_)))
                ),
                "source-only native calls became typed in {name}"
            );
        }
    }

    #[test]
    fn vector_target_pos_parses_only_the_71_closed_vector2_tuples_and_serde_roundtrips() {
        use crate::data::{AcmdStmt, AttackModuleVecTargetPosPreset, ExcuteStmt};

        assert_eq!(AttackModuleVecTargetPosPreset::ALL.len(), 71);
        let source_calls = AttackModuleVecTargetPosPreset::ALL
            .into_iter()
            .map(|preset| {
                let (id, x, y, arg5) = preset.source_parts();
                assert_eq!(
                    AttackModuleVecTargetPosPreset::from_source_parts(id, x, y, arg5),
                    Some(preset)
                );
                let encoded = serde_json::to_value(preset).unwrap();
                let source_key = |token: &str| token.replace('-', "neg").replace('.', "_");
                assert_eq!(
                    encoded,
                    serde_json::json!(format!(
                        "id{id}_x{}_y{}_arg{arg5}",
                        source_key(x),
                        source_key(y)
                    )),
                    "the serialized preset ID must name its complete lexical tuple"
                );
                let decoded: AttackModuleVecTargetPosPreset =
                    serde_json::from_value(encoded.clone()).unwrap();
                assert_eq!(decoded, preset);
                assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
                preset.source_line()
            })
            .collect::<Vec<_>>();
        let source = format!(
            "unsafe extern \"C\" fn game_vector_target_pos(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {}\n    }}\n}}",
            source_calls.join("\n        ")
        );
        let parsed = parse_acmd_script(&source);
        let expected = AttackModuleVecTargetPosPreset::ALL
            .into_iter()
            .map(ExcuteStmt::AttackModuleSetVecTargetPos)
            .collect::<Vec<_>>();
        assert!(matches!(
            parsed.stmts.as_slice(),
            [AcmdStmt::Excute(commands)]
                if serde_json::to_value(commands).unwrap()
                    == serde_json::to_value(&expected).unwrap()
        ));

        let emitted = preview_game_fn(&parsed, "vector_target_pos");
        assert_json_eq!(
            parse_acmd_script(&emitted).stmts,
            parsed.stmts,
            "all exact source lexemes survive source export and parse-back"
        );
        assert!(
            serde_json::from_value::<AttackModuleVecTargetPosPreset>(serde_json::json!(
                "id0_x0_0_y8_0_arg0"
            ))
            .is_err(),
            "serde accepts only stable IDs for measured complete tuples"
        );
    }

    #[test]
    fn vector_target_pos_keeps_invalid_shapes_and_contexts_raw() {
        use crate::data::AttackModuleVecTargetPosPreset;

        let valid = AttackModuleVecTargetPosPreset::ALL[0].source_line();
        let (_, x, _, arg5) = AttackModuleVecTargetPosPreset::ALL[0].source_parts();
        let invalid = [
            valid.replace("&Vector2f", "&Vector3f"),
            valid.replace("Hash40::new(\"top\")", "Hash40::new(\"waist\")"),
            valid.replace(", false);", ", true);"),
            valid.replace(&format!("x: {x}"), "x: 123.0"),
            valid.replace(&format!(", {arg5}, false);"), ", 99, false);"),
            valid.replace(", false);", ", false, 0);"),
        ];
        for line in invalid {
            let source = format!(
                "unsafe extern \"C\" fn game_vector_target_invalid(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}"
            );
            let parsed = parse_acmd_script(&source);
            assert!(
                !format!("{parsed:?}").contains("AttackModuleSetVecTargetPos"),
                "unmeasured shape became typed: {line}"
            );
            let emitted = preview_game_fn(&parsed, "vector_target_invalid");
            assert!(
                emitted.contains(&line),
                "unmeasured source was not preserved: {line}"
            );
        }

        let direct = valid;
        let contexts = [
            format!(
                "unsafe extern \"C\" fn game_vector_target_bare(agent: &mut L2CAgentBase) {{\n    {direct}\n}}"
            ),
            format!(
                "unsafe extern \"C\" fn game_vector_target_nested(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        if true {{\n            {direct}\n        }}\n    }}\n}}"
            ),
            format!(
                "unsafe extern \"C\" fn game_vector_target_wrapped(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        wrapper({direct})\n    }}\n}}"
            ),
            format!(
                "unsafe extern \"C\" fn game_vector_target_attributed(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        #[allow(unused)]\n        {direct}\n    }}\n}}"
            ),
        ];
        for source in contexts {
            assert!(
                !format!("{:?}", parse_acmd_script(&source))
                    .contains("AttackModuleSetVecTargetPos"),
                "non-direct or bare call became typed:\n{source}"
            );
        }

        let expression = format!(
            "unsafe extern \"C\" fn expression_vector_target(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {direct}\n    }}\n}}"
        );
        assert!(
            !format!("{:?}", parse_expression_script(&expression))
                .contains("AttackModuleSetVecTargetPos"),
            "Game-only native call became typed on Expression"
        );
    }

    #[test]
    fn target_category_is_a_closed_direct_game_source_form() {
        use crate::data::{AcmdStmt, AttackModuleTargetCategoryId, ExcuteStmt};

        let source = r#"unsafe extern "C" fn game_target_category(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32);
        AttackModule::set_target_category(agent.module_accessor, 1, *COLLISION_CATEGORY_MASK_NO_IF as u32);
        AttackModule::set_target_category(agent.module_accessor, 2, *COLLISION_CATEGORY_MASK_NO_IF as u32);
    }
}"#;
        let parsed = parse_acmd_script(source);
        assert!(matches!(
            parsed.stmts.as_slice(),
            [AcmdStmt::Excute(commands)] if matches!(commands.as_slice(), [
                ExcuteStmt::AttackModuleSetTargetCategory(AttackModuleTargetCategoryId::Zero),
                ExcuteStmt::AttackModuleSetTargetCategory(AttackModuleTargetCategoryId::One),
                ExcuteStmt::AttackModuleSetTargetCategory(AttackModuleTargetCategoryId::Two),
            ])
        ));
        for id in AttackModuleTargetCategoryId::ALL {
            let value = serde_json::to_value(id).unwrap();
            assert_json_eq!(
                serde_json::from_value::<AttackModuleTargetCategoryId>(value).unwrap(),
                id
            );
        }
        assert!(
            serde_json::from_value::<AttackModuleTargetCategoryId>(serde_json::json!("three"))
                .is_err()
        );

        let emitted = preview_game_fn(&parsed, "target_category_roundtrip");
        for id in 0..=2 {
            assert!(emitted.contains(&format!(
                "AttackModule::set_target_category(agent.module_accessor, {id}, *COLLISION_CATEGORY_MASK_NO_IF as u32);"
            )));
        }
        syn::parse_file(&emitted).unwrap();
        assert_json_eq!(parse_acmd_script(&emitted).stmts, parsed.stmts);

        for bad in [
            "AttackModule::set_target_category(boma, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32);",
            "AttackModule::set_target_category(other.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32);",
            "AttackModule::set_target_category(agent.module_accessor, -1, *COLLISION_CATEGORY_MASK_NO_IF as u32);",
            "AttackModule::set_target_category(agent.module_accessor, 3, *COLLISION_CATEGORY_MASK_NO_IF as u32);",
            "AttackModule::set_target_category(agent.module_accessor, 00, *COLLISION_CATEGORY_MASK_NO_IF as u32);",
            "AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF);",
            "AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as i32);",
            "AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32, true);",
            "AttackModule::set_target_category(agent.module_accessor, 0, 42u32);",
            "AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32); helper();",
        ] {
            let bad_source = format!(
                "unsafe extern \"C\" fn game_bad_target_category(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {bad}\n    }}\n}}"
            );
            let rejected = parse_acmd_script(&bad_source);
            assert!(matches!(
                rejected.stmts.as_slice(),
                [AcmdStmt::Excute(commands)] if matches!(commands.as_slice(), [ExcuteStmt::Raw(raw)] if raw.trim() == bad)
            ), "unmeasured source was promoted: {bad}");
        }

        let contexts = r##"unsafe extern "C" fn game_target_category_context(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32);
        // AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32);
        let source_text = r#"AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32);"#;
        wrapper(AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32));
        #[allow(unused)]
        AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32);
    }
    AttackModule::set_target_category(agent.module_accessor, 0, *COLLISION_CATEGORY_MASK_NO_IF as u32);
}"##;
        let context_parsed = parse_acmd_script(contexts);
        assert!(matches!(
            context_parsed.stmts.as_slice(),
            [AcmdStmt::Excute(commands), AcmdStmt::Raw(top)]
                if matches!(commands.as_slice(), [
                    ExcuteStmt::AttackModuleSetTargetCategory(_),
                    ExcuteStmt::Raw(comment),
                    ExcuteStmt::Raw(local),
                    ExcuteStmt::Raw(wrapper),
                    ExcuteStmt::Raw(attribute),
                    ExcuteStmt::Raw(attributed_call),
                ]
                if comment.contains("// AttackModule::")
                    && local.contains("let source_text")
                    && wrapper.contains("wrapper(AttackModule::")
                    && attribute.contains("#[allow(unused)]")
                    && attributed_call.contains("AttackModule::set_target_category"))
                    && top.contains("AttackModule::set_target_category")
        ));
        for (name, parse) in [
            (
                "expression_target_category",
                parse_expression_script as fn(&str) -> AcmdScript,
            ),
            (
                "sound_target_category",
                parse_sound_script as fn(&str) -> AcmdScript,
            ),
        ] {
            let other_track = parse(&contexts.replace("game_target_category_context", name));
            assert!(
                matches!(
                    other_track.stmts.first(),
                    Some(AcmdStmt::Excute(commands)) if commands.iter().all(|command| matches!(command, ExcuteStmt::Raw(_)))
                ),
                "native source-only calls became typed in {name}"
            );
        }
    }

    #[test]
    fn article_add_motion_partial_accepts_only_the_twelve_closed_game_rows() {
        use crate::data::{ArticleLifecycleCall, ArticlePartialMotionPreset, ExcuteStmt};

        let mut source = String::from(
            "unsafe extern \"C\" fn game_article_partial(agent: &mut L2CAgentBase) {\n    if macros::is_excute(agent) {\n",
        );
        for preset in ArticlePartialMotionPreset::ALL {
            source.push_str("        ");
            source.push_str(&preset.source_line());
            source.push('\n');
        }
        source.push_str("    }\n}\n");

        let parsed = parse_acmd_script(&source);
        assert!(matches!(
            parsed.stmts.as_slice(),
            [AcmdStmt::Excute(commands)] if commands.len() == 12
                && commands.iter().zip(ArticlePartialMotionPreset::ALL).all(|(command, preset)| {
                    matches!(command, ExcuteStmt::ArticleLifecycle(
                        ArticleLifecycleCall::AddMotionPartial { preset: actual }
                    ) if *actual == preset)
                })
        ));
        let emitted = preview_game_fn(&parsed, "article_partial");
        assert_json_eq!(
            parse_acmd_script(&emitted).stmts,
            parsed.stmts,
            "closed tuples and exact timing lexemes round-trip"
        );

        for preset in ArticlePartialMotionPreset::ALL {
            let call = ArticleLifecycleCall::AddMotionPartial { preset };
            let encoded = serde_json::to_value(&call).unwrap();
            let decoded: ArticleLifecycleCall = serde_json::from_value(encoded).unwrap();
            assert_json_eq!(decoded, call);
        }

        // Each native position is intentionally closed: changing the receiver, either
        // owner-bound symbol, motion, timing spelling/value, any tail field, or arity must
        // leave the source opaque rather than create a near-match typed command.
        let canonical = ArticlePartialMotionPreset::EflameOpen5.source_line();
        let tail = ", false, false, 0.0, false, true, false);";
        let mut mutations = vec![
            canonical.replace("agent.module_accessor", "boma"),
            canonical.replace(
                "FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD",
                "FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD",
            ),
            canonical.replace(
                "WEAPON_EFLAME_ESWORD_MOTION_PART_SET_KIND_OPEM_CLOSE",
                "WEAPON_ELIGHT_ESWORD_MOTION_PART_SET_KIND_OPEM_CLOSE",
            ),
            canonical.replace("Hash40::new(\"to_open\")", "Hash40::new(\"open\")"),
            canonical.replace(", 5.0, 5.0,", ", 4.0, 5.0,"),
            canonical.replace(", 5.0, 5.0,", ", 5.00, 5.0,"),
            canonical.replace(tail, ", true, false, 0.0, false, true, false);"),
            canonical.replace(tail, ", false, true, 0.0, false, true, false);"),
            canonical.replace(tail, ", false, false, 0.1, false, true, false);"),
            canonical.replace(tail, ", false, false, 0.0, true, true, false);"),
            canonical.replace(tail, ", false, false, 0.0, false, false, false);"),
            canonical.replace(tail, ", false, false, 0.0, false, true, true);"),
            canonical.replace(");", ", false);"),
        ];
        mutations.dedup();
        assert_eq!(mutations.len(), 13);
        for mutation in mutations {
            let source = format!(
                "unsafe extern \"C\" fn game_article_partial_rejected(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {mutation}\n    }}\n}}\n"
            );
            let parsed = parse_acmd_script(&source);
            assert!(
                matches!(
                    parsed.stmts.as_slice(),
                    [AcmdStmt::Excute(commands)] if matches!(commands.as_slice(), [ExcuteStmt::Raw(raw)] if raw.trim() == mutation)
                ),
                "mutated native position became editable: {mutation}"
            );
            let emitted = preview_game_fn(&parsed, "article_partial_rejected");
            assert_json_eq!(
                parse_acmd_script(&emitted).stmts,
                parsed.stmts,
                "rejected tuple must remain byte-shaped opaque source"
            );
        }
    }

    #[test]
    fn article_add_motion_partial_lexical_head_is_game_execute_only_and_lossless() {
        use crate::data::{ArticleLifecycleCall, ArticlePartialMotionPreset, ExcuteStmt};

        let canonical = ArticlePartialMotionPreset::EflameOpen5.source_line();
        let mut game = String::from(
            "unsafe extern \"C\" fn game_article_partial_context(agent: &mut L2CAgentBase) {\n",
        );
        game.push_str("    if macros::IS_EXIST_ARTICLE(agent, *FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD) {\n        if macros::is_excute(agent) {\n            ");
        game.push_str(&canonical);
        game.push_str("\n        }\n    }\n");
        game.push_str(
            "    if unreviewed_guard(agent) {\n        if macros::is_excute(agent) {\n            ",
        );
        game.push_str(&canonical);
        game.push_str("\n        }\n    }\n");
        game.push_str("    for _ in 0..2 {\n        if macros::is_excute(agent) {\n            ");
        game.push_str(&canonical);
        game.push_str("\n        }\n    }\n");
        game.push_str("    if macros::is_excute(agent) {\n        // ");
        game.push_str(&canonical);
        game.push_str("\n        let preserved = r#\"");
        game.push_str(&canonical);
        game.push_str("\"#;\n        wrapper(");
        game.push_str(canonical.trim_end_matches(';'));
        game.push_str(");\n        #[allow(unused)]\n        ");
        game.push_str(&canonical);
        game.push_str("\n    }\n    ");
        game.push_str(&canonical);
        game.push_str("\n}\n");

        let parsed = parse_acmd_script(&game);
        let mut typed = 0usize;
        let mut raw = 0usize;
        fn count(stmts: &[AcmdStmt], typed: &mut usize, raw: &mut usize) {
            for stmt in stmts {
                match stmt {
                    AcmdStmt::Excute(commands) => {
                        for command in commands {
                            match command {
                                ExcuteStmt::ArticleLifecycle(
                                    ArticleLifecycleCall::AddMotionPartial { .. },
                                ) => *typed += 1,
                                ExcuteStmt::Raw(source)
                                    if source.contains("ArticleModule::add_motion_partial(") =>
                                {
                                    *raw += 1
                                }
                                _ => {}
                            }
                        }
                    }
                    AcmdStmt::Raw(source)
                        if source.contains("ArticleModule::add_motion_partial(") =>
                    {
                        *raw += 1
                    }
                    AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. } => {
                        count(body, typed, raw)
                    }
                    _ => {}
                }
            }
        }
        count(&parsed.stmts, &mut typed, &mut raw);
        assert_eq!(
            typed, 3,
            "guard, unknown wrapper, and finite loop are direct Game executes"
        );
        assert_eq!(
            raw, 5,
            "comment/string/nested/attributed/top-level source stays raw"
        );
        let emitted = preview_game_fn(&parsed, "article_partial_context");
        assert_json_eq!(
            parse_acmd_script(&emitted).stmts,
            parsed.stmts,
            "typed rows inside opaque wrappers must preserve their exact row source"
        );

        let expression = parse_expression_script(&game.replace(
            "game_article_partial_context",
            "expression_article_partial_context",
        ));
        let sound = parse_sound_script(&game.replace(
            "game_article_partial_context",
            "sound_article_partial_context",
        ));
        fn contains_typed_partial_motion(stmts: &[AcmdStmt]) -> bool {
            stmts.iter().any(|statement| match statement {
                AcmdStmt::Excute(commands) => commands.iter().any(|command| {
                    matches!(
                        command,
                        ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::AddMotionPartial { .. })
                    )
                }),
                AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. } => {
                    contains_typed_partial_motion(body)
                }
                _ => false,
            })
        }
        assert!(!contains_typed_partial_motion(&expression.stmts));
        assert!(!contains_typed_partial_motion(&sound.stmts));
    }

    #[test]
    fn article_visibility_and_shoot_exist_are_measured_on_expression_track_only() {
        use crate::data::{ArticleLifecycleCall, ArticleTarget, ExcuteStmt};

        let expression = r#"unsafe extern "C" fn expression_article(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        ArticleModule::set_visibility_whole(agent.module_accessor, *FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO, false, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));
        ArticleModule::shoot_exist(agent.module_accessor, *FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1, *ARTICLE_OPE_TARGET_ALL, false);
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_ALL, false);
    }
}
"#;
        let parsed = parse_expression_script(expression);
        assert!(
            matches!(
                parsed.stmts.as_slice(),
                [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [
                        ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::SetVisibilityWhole {
                            symbol,
                            visible: false,
                        }),
                        ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::ShootExist {
                            symbol: shot_symbol,
                            target: ArticleTarget::All,
                        }),
                        ExcuteStmt::Raw(raw),
                    ] if symbol == "FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO"
                        && shot_symbol == "FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1"
                        && raw.contains("ArticleModule::shoot("))
            ),
            "Expression ArticleModule::shoot is not an eligible operation and must remain Raw"
        );
        let emitted = preview_expression_fn(&parsed, "article");
        assert!(emitted.contains(
            "ArticleModule::set_visibility_whole(agent.module_accessor, *FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO, false, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));"
        ));
        assert!(emitted.contains(
            "ArticleModule::shoot_exist(agent.module_accessor, *FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL), false);"
        ));
        assert_json_eq!(parse_expression_script(&emitted).stmts, parsed.stmts);
    }

    #[test]
    fn article_shoot_target_native_and_newtype_forms_share_the_same_closed_ast() {
        let bare = r#"unsafe extern "C" fn game_article_targets(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_ALL, false);
        ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_LAST, false);
        ArticleModule::shoot_exist(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_ALL, false);
        ArticleModule::shoot_exist(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, *ARTICLE_OPE_TARGET_LAST, false);
    }
}
"#;
        let wrapped = bare
            .replace(
                ", *ARTICLE_OPE_TARGET_ALL, false);",
                ", ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL), false);",
            )
            .replace(
                ", *ARTICLE_OPE_TARGET_LAST, false);",
                ", ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST), false);",
            );
        let native_tree = parse_acmd_script(bare);
        let generated_tree = parse_acmd_script(&wrapped);
        assert_json_eq!(
            &generated_tree.stmts,
            &native_tree.stmts,
            "both exact source and generated newtype spellings must map to the same closed targets"
        );
        let emitted = preview_game_fn(&native_tree, "article_targets");
        assert!(emitted.contains(
            "ArticleModule::shoot(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL), false);"
        ));
        assert!(emitted.contains(
            "ArticleModule::shoot_exist(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA, ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST), false);"
        ));
    }

    #[test]
    fn gimmick_event_call_is_exact_direct_game_execute_source() {
        use crate::data::ExcuteStmt;

        assert_eq!(
            serde_json::to_value(ExcuteStmt::GimmickEvent {
                name: "ready".into()
            })
            .unwrap(),
            serde_json::json!({"GimmickEvent": {"name": "ready"}}),
        );

        let source = r#"unsafe extern "C" fn game_named_event(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "ready\\\"now");
        crate::slight::gimmicks::dispatch_named_from_module(boma, "ready");
        crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, EVENT_NAME);
        crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "ready", true);
    }
    if is_excute(agent) {
        crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "plain");
    }
    if !macros::is_excute(agent) {
        crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "negated");
    }
    if macros::is_excute(agent) && condition(agent) {
        crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "combined");
    }
    crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "bare");
    for _ in 0..2 {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "looped");
        }
    }
}
"#;
        let parsed = parse_acmd_script(source);
        assert!(matches!(
            parsed.stmts.first(),
            Some(AcmdStmt::Excute(commands))
                if matches!(commands.as_slice(), [
                    ExcuteStmt::GimmickEvent { name },
                    ExcuteStmt::Raw(_), ExcuteStmt::Raw(_), ExcuteStmt::Raw(_)
                ] if name == "ready\\\"now")
        ));
        assert!(matches!(
            parsed.stmts.get(1),
            Some(AcmdStmt::Excute(commands))
                if matches!(commands.as_slice(), [ExcuteStmt::GimmickEvent { name }] if name == "plain")
        ));
        for index in [2, 3] {
            assert!(
                matches!(
                    parsed.stmts.get(index),
                    Some(AcmdStmt::RawBlock { body, .. })
                        if body.iter().any(|statement| matches!(statement,
                            AcmdStmt::Raw(line) if line.contains("dispatch_named_from_module")))
                ),
                "non-plain execute guard {index} must remain opaque"
            );
        }
        assert!(matches!(
            parsed.stmts.get(4),
            Some(AcmdStmt::Raw(line)) if line.contains("dispatch_named_from_module")
        ));
        assert!(matches!(
            parsed.stmts.get(5),
            Some(AcmdStmt::Loop { body, .. })
                if matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::Raw(line)]
                        if line.contains("dispatch_named_from_module")))
        ));
        for name in ["", "   ", "bad\0name"] {
            let call = format!(
                "crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, {name:?});"
            );
            assert!(parse_gimmick_event_call(&call).is_none());
        }
        let too_long = "x".repeat(crate::gimmicks::live_model::MAX_NAME_BYTES + 1);
        let call = format!(
            "crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, {too_long:?});"
        );
        assert!(parse_gimmick_event_call(&call).is_none());

        let expression =
            parse_expression_script(&source.replace("game_named_event", "expression_named_event"));
        assert!(matches!(
            expression.stmts.first(),
            Some(AcmdStmt::Excute(commands))
                if matches!(commands.first(), Some(ExcuteStmt::Raw(line))
                    if line.contains("dispatch_named_from_module"))
        ));

        let emitted = preview_game_fn(&parsed, "named_event");
        assert!(emitted.contains(&format!(
            "crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, {:?});",
            "ready\\\"now"
        )));
        assert_json_eq!(parse_acmd_script(&emitted).stmts, parsed.stmts);
    }

    #[test]
    fn gimmick_chart_event_call_is_exact_and_keeps_legacy_source_separate() {
        use crate::data::ExcuteStmt;

        let source = r#"unsafe extern "C" fn game_chart_event(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        crate::slight::gimmicks::dispatch_named_chart_from_module(agent.module_accessor, " charge ", "pulse\"now");
    }
}
"#;
        let parsed = parse_acmd_script(source);
        assert!(matches!(
            parsed.stmts.as_slice(),
            [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::GimmickChartEvent { chart_id, name }]
                    if chart_id == " charge " && name == "pulse\"now")
        ));
        let emitted = preview_game_fn(&parsed, "chart_event");
        assert!(emitted.contains(
            "crate::slight::gimmicks::dispatch_named_chart_from_module(agent.module_accessor, \" charge \", \"pulse\\\"now\");"
        ));
        assert_json_eq!(parse_acmd_script(&emitted).stmts, parsed.stmts);

        let too_long_chart =
            "x".repeat(crate::gimmicks::controller_protocol::MAX_CHART_ID_BYTES + 1);
        for (chart, event) in [
            ("", "pulse"),
            ("  ", "pulse"),
            ("main", "pulse"),
            ("charge\0", "pulse"),
            (too_long_chart.as_str(), "pulse"),
            ("charge", ""),
            ("charge", "bad\0event"),
        ] {
            let call = format!(
                "crate::slight::gimmicks::dispatch_named_chart_from_module(agent.module_accessor, {chart:?}, {event:?});"
            );
            assert!(
                parse_gimmick_chart_event_call(&call).is_none(),
                "accepted {call}"
            );
        }
        for call in [
            "crate::slight::gimmicks::dispatch_named_chart_from_module(boma, \"charge\", \"pulse\");",
            "crate::slight::gimmicks::dispatch_named_chart_from_module(agent.module_accessor, CHART, \"pulse\");",
            "crate::slight::gimmicks::dispatch_named_chart_from_module(agent.module_accessor, \"charge\", EVENT);",
            "crate::slight::gimmicks::dispatch_named_chart_from_module(agent.module_accessor, \"charge\");",
            "crate::slight::gimmicks::dispatch_named_chart_from_module(agent.module_accessor, \"charge\", \"pulse\", false);",
        ] {
            assert!(parse_gimmick_chart_event_call(call).is_none(), "accepted {call}");
        }

        let expression =
            parse_expression_script(&source.replace("game_chart_event", "expression_chart_event"));
        assert!(matches!(
            expression.stmts.first(),
            Some(AcmdStmt::Excute(commands))
                if matches!(commands.as_slice(), [ExcuteStmt::Raw(raw)]
                    if raw.contains("dispatch_named_chart_from_module"))
        ));
        let bare = parse_acmd_script(
            "unsafe extern \"C\" fn game_chart_bare(agent: &mut L2CAgentBase) {\n    crate::slight::gimmicks::dispatch_named_chart_from_module(agent.module_accessor, \"charge\", \"pulse\");\n}\n",
        );
        assert!(matches!(
            bare.stmts.as_slice(),
            [AcmdStmt::Raw(raw)] if raw.contains("dispatch_named_chart_from_module")
        ));
    }

    #[test]
    fn gimmick_chart_event_values_roundtrip_is_constant_bounded_and_additive() {
        use crate::data::ExcuteStmt;

        let call = "crate::slight::gimmicks::dispatch_named_chart_values_from_module(agent.module_accessor, \"right\", \"pulse\", &[(\"amount\", crate::slight::gimmicks::ChartEventValue::Int(3)), (\"gain\", crate::slight::gimmicks::ChartEventValue::Float(0.5)), (\"on\", crate::slight::gimmicks::ChartEventValue::Flag(true))]);";
        let parsed = parse_gimmick_chart_event_values_call(call).unwrap();
        assert!(matches!(
            &parsed,
            ExcuteStmt::GimmickChartEventValues { chart_id, name, arguments }
                if chart_id == "right" && name == "pulse" && arguments.len() == 3
        ));
        let emitted = emit_excute_stmts(std::slice::from_ref(&parsed), "    ").join("\n");
        assert!(emitted.contains("ChartEventValue::Float(0.5f32)"));
        assert_json_eq!(
            parse_gimmick_chart_event_values_call(&emitted).unwrap(),
            parsed
        );

        let legacy = ExcuteStmt::GimmickChartEvent {
            chart_id: "right".into(),
            name: "pulse".into(),
        };
        assert_eq!(
            serde_json::to_string(&legacy).unwrap(),
            r#"{"GimmickChartEvent":{"chart_id":"right","name":"pulse"}}"#
        );
        assert!(matches!(
            serde_json::from_str::<ExcuteStmt>(
                r#"{"GimmickChartEvent":{"chart_id":"right","name":"pulse"}}"#
            )
            .unwrap(),
            ExcuteStmt::GimmickChartEvent { .. }
        ));

        let eight = format!(
            "crate::slight::gimmicks::dispatch_named_chart_values_from_module(agent.module_accessor, \"right\", \"pulse\", &[{}]);",
            (0..8).map(|index| format!("(\"field{index}\", crate::slight::gimmicks::ChartEventValue::Int({}))", if index == 7 { "1000000" } else { "1" })).collect::<Vec<_>>().join(", ")
        );
        assert!(parse_gimmick_chart_event_values_call(&eight).is_some());
        let signed = call
            .replace("Int(3)", "Int(-3)")
            .replace("Float(0.5)", "Float(-0.5)");
        assert!(parse_gimmick_chart_event_values_call(&signed).is_some());
        let too_long_name = "x".repeat(crate::gimmicks::live_model::MAX_NAME_BYTES + 1);
        assert!(parse_gimmick_chart_event_values_call(
            &call.replace("\"pulse\"", &format!("{too_long_name:?}"))
        )
        .is_none());
        let empty = "crate::slight::gimmicks::dispatch_named_chart_values_from_module(agent.module_accessor, \"right\", \"pulse\", &[]);";
        for invalid in [
            call.replace("agent.module_accessor", "boma"),
            call.replace(
                "crate::slight::gimmicks::ChartEventValue::Int",
                "alias::ChartEventValue::Int",
            ),
            call.replace("Int(3)", "Int(amount())"),
            call.replace("Float(0.5)", "Float(1)"),
            call.replace("Int(3)", "Int(3u32)"),
            call.replace("Float(0.5)", "Float(0.5f64)"),
            call.replace("Int(3)", "Int(3 as i32)"),
            call.replace("Flag(true)", "Flag(ON)"),
            call.replace("Int(3)", "Int()"),
            call.replace(
                "(\"on\", crate::slight::gimmicks::ChartEventValue::Flag(true))",
                "(\"amount\", crate::slight::gimmicks::ChartEventValue::Flag(true))",
            ),
            call.replace("Float(0.5)", "Float(1000001.0)"),
            call.replace("Int(3)", "Int(1000001)"),
            call.replace("Float(0.5)", "Float(f32::INFINITY)"),
            empty.to_owned(),
            call.replace("\"amount\"", "\"bad\\0name\""),
        ] {
            assert!(
                parse_gimmick_chart_event_values_call(&invalid).is_none(),
                "accepted {invalid}"
            );
        }
        let nine = format!(
            "crate::slight::gimmicks::dispatch_named_chart_values_from_module(agent.module_accessor, \"right\", \"pulse\", &[{}]);",
            (0..9).map(|index| format!("(\"field{index}\", crate::slight::gimmicks::ChartEventValue::Int(1))")).collect::<Vec<_>>().join(", ")
        );
        assert!(parse_gimmick_chart_event_values_call(&nine).is_none());
        assert!(
            parse_gimmick_chart_event_values_call(&format!("#[allow(unused)] {call}")).is_none()
        );

        let game = parse_acmd_script(&format!(
            "unsafe extern \"C\" fn game_values(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n"
        ));
        assert!(
            matches!(game.stmts.as_slice(), [AcmdStmt::Excute(commands)] if matches!(commands.as_slice(), [ExcuteStmt::GimmickChartEventValues { .. }]))
        );
        let game_wire = serde_json::to_string(&game.stmts).unwrap();
        assert!(game_wire.contains("GimmickChartEventValues"));

        let attributed_execute = parse_acmd_script(&format!(
            "unsafe extern \"C\" fn game_values_attr(agent: &mut L2CAgentBase) {{\n    #[cfg(any())]\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n"
        ));
        let attributed_wire = serde_json::to_string(&attributed_execute.stmts).unwrap();
        assert!(!attributed_wire.contains("GimmickChartEventValues"));
        assert!(emit_stmts(&attributed_execute.stmts, "    ")
            .join("\n")
            .contains("dispatch_named_chart_values_from_module"));

        let bare = parse_acmd_script(&format!(
            "unsafe extern \"C\" fn game_values_bare(agent: &mut L2CAgentBase) {{\n    {call}\n}}\n"
        ));
        assert!(!serde_json::to_string(&bare.stmts)
            .unwrap()
            .contains("GimmickChartEventValues"));
        assert!(emit_stmts(&bare.stmts, "    ")
            .join("\n")
            .contains("dispatch_named_chart_values_from_module"));

        let sound_source = format!(
            "unsafe extern \"C\" fn sound_values(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n"
        );
        let sound = parse_sound_script(&sound_source);
        assert!(!serde_json::to_string(&sound.stmts)
            .unwrap()
            .contains("GimmickChartEventValues"));
        assert!(emit_sound_body(&sound).contains("dispatch_named_chart_values_from_module"));

        let expression = parse_expression_script(&format!(
            "unsafe extern \"C\" fn expression_values(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n"
        ));
        assert!(
            matches!(expression.stmts.first(), Some(AcmdStmt::Excute(commands)) if matches!(commands.as_slice(), [ExcuteStmt::Raw(raw)] if raw.contains("dispatch_named_chart_values_from_module")))
        );

        let unsafe_parent = parse_acmd_script(&format!(
            "unsafe extern \"C\" fn game_values_parent(agent: &mut L2CAgentBase) {{\n    unsafe {{\n        if macros::is_excute(agent) {{\n            {call}\n        }}\n    }}\n}}\n"
        ));
        assert!(!serde_json::to_string(&unsafe_parent.stmts)
            .unwrap()
            .contains("GimmickChartEventValues"));
        assert!(emit_stmts(&unsafe_parent.stmts, "    ")
            .join("\n")
            .contains("dispatch_named_chart_values_from_module"));

        let imported_alias = call.replace(
            "crate::slight::gimmicks::dispatch_named_chart_values_from_module",
            "dispatch_values",
        );
        let import_scope = parse_acmd_script(&format!(
            "unsafe extern \"C\" fn game_values_import(agent: &mut L2CAgentBase) {{\n    use crate::slight::gimmicks::dispatch_named_chart_values_from_module as dispatch_values;\n    if macros::is_excute(agent) {{\n        {imported_alias}\n    }}\n}}\n"
        ));
        assert!(!serde_json::to_string(&import_scope.stmts)
            .unwrap()
            .contains("GimmickChartEventValues"));
        assert!(emit_stmts(&import_scope.stmts, "    ")
            .join("\n")
            .contains("dispatch_values"));
    }

    #[test]
    fn named_event_parser_rejects_execute_and_call_attributes() {
        let source = r#"unsafe extern "C" fn game_named_event_attrs(agent: &mut L2CAgentBase) {
    #[cfg(
        any()
    )] // trivia does not detach an Execute attribute
    if macros::is_excute(agent) {
        crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "guard_attr");
    }
    if macros::is_excute(agent) {
        #[cfg(
            any()
        )] // trivia does not detach a call attribute
        crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "call_attr");
        crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "plain");
    }
}
"#;
        let parsed = parse_acmd_script(source);
        let typed_names = parsed
            .stmts
            .iter()
            .filter_map(|statement| match statement {
                AcmdStmt::Excute(commands) => Some(commands.as_slice()),
                _ => None,
            })
            .flatten()
            .filter_map(|command| match command {
                ExcuteStmt::GimmickEvent { name } => Some(name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(typed_names, vec!["plain"]);
        let rendered = emit_stmts(&parsed.stmts, "    ").join("\n");
        assert!(rendered.contains("#[cfg("), "{rendered}");
        assert!(rendered.contains("trivia does not detach an Execute attribute"));
        assert!(rendered.contains("trivia does not detach a call attribute"));
        assert!(
            rendered.contains("dispatch_named_from_module(agent.module_accessor, \"guard_attr\")")
        );
        assert!(
            rendered.contains("dispatch_named_from_module(agent.module_accessor, \"call_attr\")")
        );
        let emitted = preview_game_fn(&parsed, "named_event_attrs");
        assert_json_eq!(
            parse_acmd_script(&emitted).stmts,
            parsed.stmts,
            "root Execute/call attributes and the one plain typed send stay source-stable"
        );
    }

    #[test]
    fn named_event_multiline_calls_keep_direct_statement_attribute_boundaries() {
        let parsed = parse_acmd_script(
            r#"unsafe extern "C" fn game_multiline_events(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        crate::slight::gimmicks::dispatch_named_from_module(
            agent.module_accessor,
            "plain"
        );
        #[cfg(any())]
        // A comment must not detach the call's attribute.
        crate::slight::gimmicks::dispatch_named_from_module(
            agent.module_accessor,
            "attributed"
        );
    }
}
"#,
        );
        let AcmdStmt::Excute(commands) = &parsed.stmts[0] else {
            panic!("direct Execute should remain intact");
        };
        let events = commands
            .iter()
            .filter_map(|command| match command {
                ExcuteStmt::GimmickEvent { name } => Some(name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(events, ["plain"]);
        let emitted = emit_stmts(&parsed.stmts, "    ").join("\n");
        assert!(emitted.contains("#[cfg(any())]"));
        assert!(emitted.contains("\"attributed\""));
    }

    #[test]
    fn exact_repeat_parser_accepts_measured_counts_on_all_tracks_and_preserves_unknown_headers() {
        fn acmd_loop_counts(stmts: &[AcmdStmt], out: &mut Vec<usize>) {
            for stmt in stmts {
                match stmt {
                    AcmdStmt::Loop { count, body, .. } => {
                        out.push(*count);
                        acmd_loop_counts(body, out);
                    }
                    AcmdStmt::RawBlock { body, .. } => acmd_loop_counts(body, out),
                    _ => {}
                }
            }
        }
        fn effect_loop_counts(stmts: &[crate::data::EffectStmt], out: &mut Vec<usize>) {
            for stmt in stmts {
                match stmt {
                    crate::data::EffectStmt::Loop { count, body, .. } => {
                        out.push(*count);
                        effect_loop_counts(body, out);
                    }
                    crate::data::EffectStmt::Cond { body, .. } => effect_loop_counts(body, out),
                    _ => {}
                }
            }
        }
        fn parse_acmd_track(track: &str, header: &str) -> Vec<usize> {
            let source = format!(
                "unsafe extern \"C\" fn {track}_repeat(agent: &mut L2CAgentBase) {{\n    {header}\n        frame(agent.lua_state_agent, 1.0);\n    }}\n}}\n"
            );
            let script = match track {
                "game" => parse_acmd_script(&source),
                "sound" => parse_sound_script(&source),
                "expression" => parse_expression_script(&source),
                _ => unreachable!(),
            };
            let mut out = Vec::new();
            acmd_loop_counts(&script.stmts, &mut out);
            out
        }

        for count in [0usize, 1, 20, 22, 24, 31, 34, 47, 49, 100] {
            let header = format!("for _ in 0..{count} {{");
            for track in ["game", "sound", "expression"] {
                assert_eq!(parse_acmd_track(track, &header), [count], "track {track}");
            }
            let effect_source = format!(
                "unsafe extern \"C\" fn effect_repeat(agent: &mut L2CAgentBase) {{\n    {header}\n        frame(agent.lua_state_agent, 1.0);\n    }}\n}}\n"
            );
            let mut out = Vec::new();
            effect_loop_counts(&parse_effect_script(&effect_source).stmts, &mut out);
            assert_eq!(out, [count], "effect track");
        }

        let unsupported = [
            "for _ in 0..=24 {",
            "for index in 0..24 {",
            "for (index, _) in 0..24 {",
            "for _ in 1..25 {",
            "for _ in 0..(24 + 1) {",
            "for _ in 0..24 + 1 {",
            "for _ in 0..24usize {",
            "for _ in 0..101 {",
            "for _ in 0..184467440737095516160000 {",
            "for _ in (0..24).step_by(2) {",
            "for /* header trivia */ _ in 0..24 {",
            "for _ in 0..24 { // trailing header comment",
        ];
        for header in unsupported {
            for track in ["game", "sound", "expression"] {
                let source = format!(
                    "unsafe extern \"C\" fn {track}_repeat(agent: &mut L2CAgentBase) {{\n    {header}\n        frame(agent.lua_state_agent, 1.0);\n    }}\n}}\n"
                );
                let parsed = match track {
                    "game" => parse_acmd_script(&source),
                    "sound" => parse_sound_script(&source),
                    _ => parse_expression_script(&source),
                };
                let mut out = Vec::new();
                acmd_loop_counts(&parsed.stmts, &mut out);
                assert!(out.is_empty(), "{track}: {header}");
                let emitted = emit_stmts(&parsed.stmts, "    ").join("\n");
                assert!(emitted.contains(header), "{track} lost header {header}");
                assert!(emitted.contains("frame(agent.lua_state_agent, 1.0);"));
            }
            let effect_source = format!(
                "unsafe extern \"C\" fn effect_repeat(agent: &mut L2CAgentBase) {{\n    {header}\n        frame(agent.lua_state_agent, 1.0);\n    }}\n}}\n"
            );
            let parsed = parse_effect_script(&effect_source);
            let mut out = Vec::new();
            effect_loop_counts(&parsed.stmts, &mut out);
            assert!(out.is_empty(), "effect: {header}");
            assert!(matches!(
                parsed.stmts.first(),
                Some(crate::data::EffectStmt::Cond { header: preserved, .. })
                    if preserved == header
            ));
        }

        let attributed =
            "#[cfg(\n    any()\n)]\n// stays attached to the source loop\nfor _ in 0..24 {";
        for track in ["game", "sound", "expression"] {
            let source = format!(
                "unsafe extern \"C\" fn {track}_repeat(agent: &mut L2CAgentBase) {{\n    {attributed}\n        frame(agent.lua_state_agent, 1.0);\n    }}\n}}\n"
            );
            let parsed = match track {
                "game" => parse_acmd_script(&source),
                "sound" => parse_sound_script(&source),
                _ => parse_expression_script(&source),
            };
            let mut out = Vec::new();
            acmd_loop_counts(&parsed.stmts, &mut out);
            assert!(out.is_empty(), "{track} attributed loop became editable");
            let emitted = emit_stmts(&parsed.stmts, "    ").join("\n");
            assert!(emitted.contains("#[cfg("), "{track} lost the attribute");
            assert!(
                emitted.contains("for _ in 0..24 {"),
                "{track} lost the loop"
            );
        }
        let effect_source = format!(
            "unsafe extern \"C\" fn effect_repeat(agent: &mut L2CAgentBase) {{\n    {attributed}\n        frame(agent.lua_state_agent, 1.0);\n    }}\n}}\n"
        );
        let parsed = parse_effect_script(&effect_source);
        let mut out = Vec::new();
        effect_loop_counts(&parsed.stmts, &mut out);
        assert!(out.is_empty(), "effect attributed loop became editable");
        assert!(matches!(
            parsed.stmts.iter().find(|stmt| matches!(
                stmt,
                crate::data::EffectStmt::Cond { header, .. } if header == "for _ in 0..24 {"
            )),
            Some(crate::data::EffectStmt::Cond { .. })
        ));
    }

    #[test]
    fn only_plain_positive_execute_guards_are_typed() {
        let source = r#"unsafe extern "C" fn game_guards(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::init_attack_pos(agent.module_accessor, 0);
    }
    if is_excute(agent) {
        AttackModule::init_attack_pos(agent.module_accessor, 0);
    }
    if !is_excute(agent) {
        AttackModule::init_attack_pos(agent.module_accessor, 0);
    }
    if macros::is_excute(agent) && another_condition(agent) {
        AttackModule::init_attack_pos(agent.module_accessor, 0);
    }
}
"#;
        let parsed = parse_acmd_script(source);
        assert!(matches!(
            parsed.stmts.as_slice(),
            [
                AcmdStmt::Excute(_),
                AcmdStmt::Excute(_),
                AcmdStmt::RawBlock { header, .. },
                AcmdStmt::RawBlock { header: combined, .. }
            ] if header == "if !is_excute(agent) {"
                && combined == "if macros::is_excute(agent) && another_condition(agent) {"
        ));
        assert!(matches!(
            &parsed.stmts[2],
            AcmdStmt::RawBlock { body, .. }
                if !body.iter().any(|stmt| matches!(stmt, AcmdStmt::Excute(_)))
        ));
    }

    #[test]
    fn a_motion_rate_call_parses_to_a_typed_statement_and_not_to_raw() {
        let script = parse_acmd_script(
            "unsafe extern \"C\" fn game_attackhi4(agent: &mut L2CAgentBase) {\n\
                 frame(agent.lua_state_agent, 9.0);\n\
                 macros::FT_MOTION_RATE(agent, 0.6);\n\
             }\n",
        );
        assert!(
            matches!(
                script.stmts.as_slice(),
                [
                    crate::data::AcmdStmt::Frame(f),
                    crate::data::AcmdStmt::MotionRate(r),
                ] if *f == 9.0 && *r == 0.6
            ),
            "expected a typed rate statement, got {:?}",
            script.stmts
        );
    }

    #[test]
    fn a_reverse_lr_call_parses_to_a_typed_point_and_round_trips() {
        let source = r#"unsafe extern "C" fn game_throwf(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::REVERSE_LR(agent);
    }
}
"#;
        let script = parse_acmd_script(source);
        assert_eq!(
            script.to_reverse_lr_events(),
            vec![crate::data::ReverseLrEvent { frame: 4, site: 0 }]
        );
        assert!(
            script.stmts.iter().any(|stmt| matches!(
                stmt,
                crate::data::AcmdStmt::Excute(inner)
                    if matches!(inner.as_slice(), [crate::data::ExcuteStmt::ReverseLr])
            )),
            "the call must be typed inside its execute block: {:?}",
            script.stmts
        );

        assert!(
            parse_reverse_lr_call("macros::REVERSE_LR(agent, true);").is_none(),
            "a different arity must remain raw until its own measured signature exists"
        );
        let emitted = preview_game_fn(&script, "throw_f");
        assert!(emitted.contains("macros::REVERSE_LR(agent);"));
        assert_eq!(
            parse_acmd_script(&emitted).to_reverse_lr_events(),
            script.to_reverse_lr_events()
        );
    }

    #[test]
    fn shoot_item_bullet_near_misses_stay_raw() {
        for bad in [
            "macros::SHOOT_ITEM_BULLET();",
            "macros::SHOOT_ITEM_BULLET(agent, 1);",
            "macros::SHOOT_ITEM_BULLET(other);",
            "macros::SHOOT_ITEM_BULLET(agent)",
            "macros::SHOOT_ITEM_BULLET_CHARGE(agent);",
            "other::SHOOT_ITEM_BULLET(agent);",
        ] {
            assert!(
                parse_shoot_item_bullet_call(bad).is_none(),
                "accepted {bad}"
            );
            let source = format!(
                "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {bad}\n    }}\n}}"
            );
            assert!(matches!(
                parse_acmd_script(&source).stmts.as_slice(),
                [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::Raw(_)])
            ));
        }
    }

    #[test]
    fn item_fire_scope_stays_root_game_execute_only() {
        for body in [
            "macros::SHOOT_ITEM_BULLET(agent);",
            "if condition {\n    if macros::is_excute(agent) {\n        macros::SHOOT_ITEM_BULLET(agent);\n    }\n}",
            "for _ in 0..2 {\n    if macros::is_excute(agent) {\n        macros::SHOOT_ITEM_BULLET(agent);\n    }\n}",
            "#[cfg(any())]\nif macros::is_excute(agent) {\n    macros::SHOOT_ITEM_BULLET(agent);\n}",
            "if macros::is_excute(agent) {\n    #[cfg(any())]\n    macros::SHOOT_ITEM_BULLET(agent);\n}",
        ] {
            let source = format!("unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n{body}\n}}\n");
            let script = parse_acmd_script(&source);
            assert!(!serde_json::to_string(&script).unwrap().contains("ShootItemBullet"));
            assert!(!effect_source_compatibility_macros("// macros::SHOOT_ITEM_BULLET(agent);").contains("pub unsafe fn SHOOT_ITEM_BULLET"));
        }
        for (prefix, parse) in [
            ("sound", parse_sound_script as fn(&str) -> AcmdScript),
            (
                "expression",
                parse_expression_script as fn(&str) -> AcmdScript,
            ),
        ] {
            let source = format!("unsafe extern \"C\" fn {prefix}_x(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        macros::SHOOT_ITEM_BULLET(agent);\n    }}\n}}\n");
            let script = parse(&source);
            assert!(!serde_json::to_string(&script)
                .unwrap()
                .contains("ShootItemBullet"));
        }
    }

    #[test]
    fn start_info_flash_eye_rejects_near_misses_and_keeps_their_source() {
        for bad in [
            "macros::START_INFO_FLASH_EYE();",
            "macros::START_INFO_FLASH_EYE(agent, false);",
            "macros::START_INFO_FLASH_EYE(other);",
            "macros::START_INFO_FLASH_EYE(agent)",
            "other::START_INFO_FLASH_EYE(agent);",
            "macros::START_INFO_FLASH_EYE_EXTRA(agent);",
        ] {
            assert!(
                parse_start_info_flash_eye_call(bad).is_none(),
                "accepted {bad}"
            );
            let source = format!(
                "unsafe extern \"C\" fn game_near_miss(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {bad}\n    }}\n}}\n"
            );
            let parsed = parse_acmd_script(&source);
            assert!(!serde_json::to_string(&parsed)
                .unwrap()
                .contains("StartInfoFlashEye"));
            let emitted = preview_game_fn(&parsed, "near_miss");
            assert!(emitted.contains(bad), "source spelling was lost: {bad}");
        }
    }

    #[test]
    fn start_info_flash_eye_stays_at_root_unattributed_game_or_expression_execute() {
        let call = "macros::START_INFO_FLASH_EYE(agent);";
        for body in [
            call.to_owned(),
            format!("if runtime_condition() {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}"),
            format!("for _ in 0..2 {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}"),
            format!("#[cfg(any())]\nif macros::is_excute(agent) {{\n    {call}\n}}"),
            format!("if macros::is_excute(agent) {{\n    #[cfg(any())]\n    {call}\n}}"),
            format!("if macros::is_excute(agent) {{\n    {call}\n}} else {{\n    wait(agent.lua_state_agent, 1.0);\n}}"),
        ] {
            let game = parse_acmd_script(&format!(
                "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n{body}\n}}\n"
            ));
            assert!(!serde_json::to_string(&game).unwrap().contains("StartInfoFlashEye"),
                "typed a disallowed Game placement: {body}");
            assert!(preview_game_fn(&game, "x").contains(call),
                "disallowed Game source was not retained: {body}");
            let expression = parse_expression_script(&format!(
                "unsafe extern \"C\" fn expression_x(agent: &mut L2CAgentBase) {{\n{body}\n}}\n"
            ));
            assert!(!serde_json::to_string(&expression).unwrap().contains("StartInfoFlashEye"),
                "typed a disallowed Expression placement: {body}");
            assert!(preview_expression_fn(&expression, "x").contains(call),
                "disallowed Expression source was not retained: {body}");
        }

        let sound = parse_sound_script(&format!(
            "unsafe extern \"C\" fn sound_x(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n"
        ));
        assert!(!serde_json::to_string(&sound)
            .unwrap()
            .contains("StartInfoFlashEye"));
    }

    #[test]
    fn camera_zoom_out_rejects_near_misses_and_preserves_their_source() {
        for bad in [
            "macros::CAM_ZOOM_OUT();",
            "macros::CAM_ZOOM_OUT(agent, false);",
            "macros::CAM_ZOOM_OUT(other);",
            "macros::CAM_ZOOM_OUT(agent)",
            "other::CAM_ZOOM_OUT(agent);",
            "macros::CAM_ZOOM_OUT_FINAL(agent);",
            "macros::CAM_ZOOM_IN(agent);",
        ] {
            assert!(parse_camera_zoom_out_call(bad).is_none(), "accepted {bad}");
            let source = format!(
                "unsafe extern \"C\" fn game_near_miss(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {bad}\n    }}\n}}\n"
            );
            let parsed = parse_acmd_script(&source);
            assert!(!serde_json::to_string(&parsed)
                .unwrap()
                .contains("CameraZoomOut"));
            assert!(
                preview_game_fn(&parsed, "near_miss").contains(bad),
                "source spelling was lost: {bad}"
            );
        }
    }

    #[test]
    fn camera_zoom_out_stays_at_root_unattributed_game_execute_only() {
        let call = "macros::CAM_ZOOM_OUT(agent);";
        for body in [
            format!("if runtime_condition() {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}"),
            format!("for _ in 0..2 {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}"),
            format!("#[cfg(any())]\nif macros::is_excute(agent) {{\n    {call}\n}}"),
            format!("if macros::is_excute(agent) {{\n    #[cfg(any())]\n    {call}\n}}"),
            format!("if macros::is_excute(agent) {{\n    {call}\n}} else {{\n    wait(agent.lua_state_agent, 1.0);\n}}"),
        ] {
            let game = parse_acmd_script(&format!(
                "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n{body}\n}}\n"
            ));
            assert!(!serde_json::to_string(&game).unwrap().contains("CameraZoomOut"),
                "typed a disallowed Game placement: {body}");
            assert!(preview_game_fn(&game, "x").contains(call),
                "disallowed Game source was not retained: {body}");
        }

        let bare = parse_acmd_script(&format!(
            "unsafe extern \"C\" fn game_bare(agent: &mut L2CAgentBase) {{\n    {call}\n}}\n"
        ));
        assert!(!serde_json::to_string(&bare)
            .unwrap()
            .contains("CameraZoomOut"));
        assert!(preview_game_fn(&bare, "bare").contains(call));

        let expression = parse_expression_script(&format!(
            "unsafe extern \"C\" fn expression_x(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n"
        ));
        assert!(!serde_json::to_string(&expression)
            .unwrap()
            .contains("CameraZoomOut"));
        assert!(preview_expression_fn(&expression, "other_track").contains(call));
        let sound = parse_sound_script(&format!(
            "unsafe extern \"C\" fn sound_x(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n"
        ));
        assert!(!serde_json::to_string(&sound)
            .unwrap()
            .contains("CameraZoomOut"));
        assert!(preview_sound_fn(&sound, "other_track").contains(call));
    }

    fn typed_capture_cut_common_count(statements: &[AcmdStmt]) -> usize {
        statements
            .iter()
            .map(|statement| match statement {
                AcmdStmt::Excute(commands) => commands
                    .iter()
                    .filter(|command| matches!(command, ExcuteStmt::CaptureCutCommon))
                    .count(),
                AcmdStmt::Bare(command) => {
                    if matches!(command.as_ref(), ExcuteStmt::CaptureCutCommon) {
                        1
                    } else {
                        0
                    }
                }
                AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. } => {
                    typed_capture_cut_common_count(body)
                }
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn start_cut_in_is_execute_only_and_rejects_near_misses_and_unsafe_contexts() {
        let call = "macros::FT_START_CUTIN(agent);";
        for bad in [
            "macros::FT_START_CUTIN();",
            "macros::FT_START_CUTIN(agent, false);",
            "macros::FT_START_CUTIN(other);",
            "macros::FT_START_CUTIN(agent)",
            "other::FT_START_CUTIN(agent);",
            "macros::FT_START_CUTIN_EXTRA(agent);",
            "consume(macros::FT_START_CUTIN(agent));",
        ] {
            assert!(parse_start_cut_in_call(bad).is_none(), "accepted {bad}");
            let source = format!(
                "unsafe extern \"C\" fn game_bad(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {bad}\n    }}\n}}\n"
            );
            let parsed = parse_acmd_script(&source);
            assert!(!serde_json::to_string(&parsed)
                .unwrap()
                .contains("StartCutIn"));
            assert!(preview_game_fn(&parsed, "bad").contains(bad));
        }

        let bare = parse_acmd_script(&format!(
            "unsafe extern \"C\" fn game_bare(agent: &mut L2CAgentBase) {{\n    {call}\n}}\n"
        ));
        assert!(!serde_json::to_string(&bare).unwrap().contains("StartCutIn"));
        assert!(preview_game_fn(&bare, "bare").contains(call));

        for body in [
            format!("if runtime_condition() {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}"),
            format!("for _ in 0..2 {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}"),
            format!("#[cfg(any())]\nif macros::is_excute(agent) {{\n    {call}\n}}"),
            format!("if macros::is_excute(agent) {{\n    #[cfg(any())]\n    {call}\n}}"),
            format!("if macros::is_excute(agent) {{\n    {call}\n    let broken = ;\n}}"),
        ] {
            let source = format!(
                "unsafe extern \"C\" fn game_nested(agent: &mut L2CAgentBase) {{\n{body}\n}}\n"
            );
            let parsed = parse_acmd_script(&source);
            assert!(!serde_json::to_string(&parsed).unwrap().contains("StartCutIn"));
            assert!(preview_game_fn(&parsed, "nested").contains(call));
        }

        let expression = parse_expression_script(&format!(
            "unsafe extern \"C\" fn expression_x(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n"
        ));
        assert!(!serde_json::to_string(&expression)
            .unwrap()
            .contains("StartCutIn"));
        assert!(preview_expression_fn(&expression, "other_track").contains(call));
        let sound = parse_sound_script(&format!(
            "unsafe extern \"C\" fn sound_x(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {call}\n    }}\n}}\n"
        ));
        assert!(!serde_json::to_string(&sound)
            .unwrap()
            .contains("StartCutIn"));
        assert!(preview_sound_fn(&sound, "other_track").contains(call));
    }

    #[test]
    fn capture_cut_common_is_root_game_bare_only_and_keeps_other_placements_raw() {
        let call = "macros::game_CaptureCutCommon(agent);";
        for bad in [
            "macros::game_CaptureCutCommon();",
            "macros::game_CaptureCutCommon(agent, false);",
            "macros::game_CaptureCutCommon(other);",
            "macros::game_CaptureCutCommon(agent)",
            "other::game_CaptureCutCommon(agent);",
            "macros::game_CaptureCutCommonExtra(agent);",
            "consume(macros::game_CaptureCutCommon(agent));",
        ] {
            assert!(
                parse_capture_cut_common_call(bad).is_none(),
                "accepted {bad}"
            );
            let parsed = parse_acmd_script(&format!(
                "unsafe extern \"C\" fn game_bad(agent: &mut L2CAgentBase) {{\n    {bad}\n}}\n"
            ));
            assert_eq!(typed_capture_cut_common_count(&parsed.stmts), 0);
            assert!(preview_game_fn(&parsed, "bad").contains(bad));
        }

        for body in [
            format!("if macros::is_excute(agent) {{\n    {call}\n}}"),
            format!("if runtime_condition() {{\n    {call}\n}}"),
            format!("for _ in 0..2 {{\n    {call}\n}}"),
            format!("#[cfg(any())]\n{call}"),
            format!("{call}\nlet broken = ;"),
        ] {
            let parsed = parse_acmd_script(&format!(
                "unsafe extern \"C\" fn game_nonroot(agent: &mut L2CAgentBase) {{\n{body}\n}}\n"
            ));
            assert_eq!(
                typed_capture_cut_common_count(&parsed.stmts),
                0,
                "typed a disallowed root bare placement: {body}"
            );
            assert!(preview_game_fn(&parsed, "nonroot").contains(call));
        }

        let expression = parse_expression_script(&format!(
            "unsafe extern \"C\" fn expression_x(agent: &mut L2CAgentBase) {{\n    {call}\n}}\n"
        ));
        assert_eq!(typed_capture_cut_common_count(&expression.stmts), 0);
        assert!(preview_expression_fn(&expression, "other_track").contains(call));
        let sound = parse_sound_script(&format!(
            "unsafe extern \"C\" fn sound_x(agent: &mut L2CAgentBase) {{\n    {call}\n}}\n"
        ));
        assert_eq!(typed_capture_cut_common_count(&sound.stmts), 0);
        assert!(preview_sound_fn(&sound, "other_track").contains(call));
    }

    #[test]
    fn the_longer_rate_macros_are_not_read_as_a_motion_rate() {
        for line in [
            "    macros::FT_MOTION_RATE_RANGE(agent, 5.0, 10.0, 0.5);",
            "    macros::FT_DESIRED_RATE(agent, 12.0);",
            "    macros::FT_MOTION_RATE_SYNTHETIC(agent, 0.5);",
        ] {
            assert_eq!(
                parse_motion_rate_call(line),
                None,
                "{line} must not be read as FT_MOTION_RATE"
            );
            let script = parse_acmd_script(&format!(
                "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n{line}\n}}\n"
            ));
            assert!(
                matches!(script.stmts.as_slice(), [crate::data::AcmdStmt::Raw(_)]),
                "{line} should still be kept verbatim, got {:?}",
                script.stmts
            );
        }
    }

    #[test]
    fn a_cached_error_page_is_not_read_as_a_script_but_a_real_one_still_is() {
        let dir = tempfile::tempdir().unwrap();

        let miss = dir.path().join("SpecialBStart.txt");
        std::fs::write(&miss, "404: Not Found").unwrap();
        assert_eq!(
            cached_script_body_at(&miss).as_deref(),
            Some(""),
            "an HTTP error page must not reach the parser as script source"
        );

        let real = dir.path().join("AttackHi4.txt");
        let body = "unsafe extern \"C\" fn game_attackhi4(agent: &mut L2CAgentBase) {\n\
                        frame(agent.lua_state_agent, 4.0);\n\
                    }\n";
        std::fs::write(&real, body).unwrap();
        assert_eq!(
            cached_script_body_at(&real).as_deref(),
            Some(body),
            "a real cached script must come back byte for byte"
        );
    }

    #[test]
    fn forgetting_a_fighter_cache_removes_only_that_fighter() {
        let dir = tempfile::tempdir().unwrap();
        let mario = dir.path().join("script-cache/mario");
        let kirby = dir.path().join("script-cache/kirby");
        std::fs::create_dir_all(&mario).unwrap();
        std::fs::create_dir_all(&kirby).unwrap();
        std::fs::write(mario.join("Attack11.txt"), "mario").unwrap();
        std::fs::write(kirby.join("Attack11.txt"), "kirby").unwrap();

        assert!(forget_fighter_cache_at(dir.path(), "mario").unwrap());
        assert!(
            !mario.exists(),
            "the forgotten fighter cache must be removed"
        );
        assert!(kirby.exists(), "another fighter's cache must remain");
        assert!(!forget_fighter_cache_at(dir.path(), "mario").unwrap());
    }

    #[test]
    fn forgetting_a_fighter_cache_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let error = forget_fighter_cache_at(dir.path(), "../outside").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_cached_miss_still_counts_as_fetched_so_a_scan_does_not_re_request_it() {
        let dir = tempfile::tempdir().unwrap();
        let miss = dir.path().join("SpecialBAttack.txt");
        std::fs::write(&miss, "404: Not Found").unwrap();

        assert!(
            cached_script_body_at(&miss).is_some(),
            "a cached miss is still cached; returning None sends it back to the network"
        );
        assert!(
            cached_script_body_at(&dir.path().join("NeverFetched.txt")).is_none(),
            "a move that was never fetched must stay distinguishable from one that missed"
        );
    }

    #[test]
    fn exporting_a_move_with_no_upstream_script_does_not_emit_the_error_page() {
        // Unnormalised, to show the export path really would have written it.
        let raw = parse_acmd_script("404: Not Found");
        assert!(
            preview_game_fn(&raw, "special_b_start").contains("404: Not Found"),
            "if this stops holding the export no longer round-trips Raw lines, and the \
             normalisation below is guarding nothing — re-check the fix, do not delete this"
        );

        let normalised = parse_acmd_script(&script_source_from_body("404: Not Found"));
        let emitted = preview_game_fn(&normalised, "special_b_start");
        assert!(
            !emitted.contains("404"),
            "a move with no upstream script must export as an empty function, got:\n{emitted}"
        );
    }

    #[test]
    fn a_collision_cleared_on_the_next_frame_is_out_for_that_frame() {
        // kirby/Attack100Sub: a hitbox with no `frame()` before it, cleared one frame later.
        let source = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 0.2, 361, 35, 0, 7, 5.5, 0.0, 5.5, 15.0, None, None, None, 0.5, 0.4, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_rush"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);
    }
    wait(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
    }
}
"#;
        let hitboxes = parse_acmd_script(source).to_hitboxes();
        assert_eq!(
            (hitboxes[0].active_start, hitboxes[0].active_end),
            (1, 1),
            "the jab hitbox is out on frame 1"
        );
    }

    /// kirby/ThrowF, verbatim — two `ATTACK_ABS` in one block sharing id 0 and differing only
    const THROW_ABS: &str = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::ATTACK_ABS(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, 0, 5.0, 75, 125, 0, 40, 0.0, 1.0, *ATTACK_LR_CHECK_F, 0.0, true, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_NONE, *ATTACK_REGION_THROW);
        macros::ATTACK_ABS(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_CATCH, 0, 3.0, 361, 100, 0, 60, 0.0, 1.0, *ATTACK_LR_CHECK_F, 0.0, true, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_NONE, *ATTACK_REGION_THROW);
    }
}
"#;

    #[test]
    fn throw_hit_application_types_only_the_closed_four_line_game_bundle() {
        use crate::data::{AcmdStmt, ExcuteStmt};

        let bundle = ATTACK_ABS_APPLY_THROW_SOURCE.join("\n        ");
        let source = format!(
        "unsafe extern \"C\" fn game_throw(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {bundle}\n        AttackModule::clear_all(agent.module_accessor);\n    }}\n}}\n"
    );
        let parsed = parse_acmd_script(&source);
        assert!(matches!(
            parsed.stmts.as_slice(),
            [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [
                    ExcuteStmt::AttackAbsApplyThrow,
                    ExcuteStmt::ClearAll,
                ])
        ));
        let emitted = preview_game_fn(&parsed, "throw");
        let reparsed = parse_acmd_script(&emitted);
        assert_json_eq!(
            parsed,
            reparsed,
            "the bundle and later command must retain their order and scope"
        );

        let mut malformed = vec![
        source.replace(
            "FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, Hash40::new(\"throw\")",
            "FIGHTER_ATTACK_ABSOLUTE_KIND_THROW_MEWTWO, Hash40::new(\"throw\")",
        ),
        source.replace("Hash40::new(\"throw\")", "Hash40::new(\"catch\")"),
        source.replace(
            "target, target_group, target_no);",
            "target, target_group, target_group);",
        ),
        source.replace(
            "FIGHTER_STATUS_THROW_WORK_INT_TARGET_OBJECT",
            "FIGHTER_DOLLY_STATUS_SUPER_SPECIAL2_WORK_INT_TARGET_OBJECT_ID",
        ),
        source.replace(
            "agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_OBJECT",
            "boma, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_OBJECT",
        ),
        source.replace(
            "macros::ATK_HIT_ABS(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_THROW",
            "macros::ATK_HIT_ABS(boma, *FIGHTER_ATTACK_ABSOLUTE_KIND_THROW",
        ),
        source.replace(
            "target, target_group, target_no);",
            "*FIGHTER_DOLLY_STATUS_SUPER_SPECIAL2_WORK_INT_TARGET_OBJECT_ID, *FIGHTER_DOLLY_STATUS_SUPER_SPECIAL2_WORK_INT_TARGET_HIT_GROUP, *FIGHTER_DOLLY_STATUS_SUPER_SPECIAL2_WORK_INT_TARGET_HIT_NO);",
        ),
        source.replace(
            "target, target_group, target_no);",
            "target, target_group, target_no, 0);",
        ),
        source.replace(
            "let target_group = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_GROUP);\n        let target_no = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_NO);",
            "let target_no = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_NO);\n        let target_group = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_GROUP);",
        ),
        source.replace(
            "let target_group = WorkModule::get_int64",
            "unknown_interrupt(agent);\n        let target_group = WorkModule::get_int64",
        ),
        source.replace(
            &format!("        {bundle}"),
            &format!("        if branch(agent) {{\n            {}\n        }}", bundle.replace("\n        ", "\n            ")),
        ),
        format!(
            "unsafe extern \"C\" fn game_bare(agent: &mut L2CAgentBase) {{\n    {}\n}}\n",
            bundle.replace("\n        ", "\n    ")
        ),
    ];
        for malformed_source in malformed.drain(..) {
            let opaque = parse_acmd_script(&malformed_source);
            assert!(opaque.stmts.iter().all(|statement| match statement {
                AcmdStmt::Excute(commands) => commands
                    .iter()
                    .all(|command| !matches!(command, ExcuteStmt::AttackAbsApplyThrow)),
                AcmdStmt::RawBlock { body, .. } => body.iter().all(|statement| match statement {
                    AcmdStmt::Excute(commands) => commands
                        .iter()
                        .all(|command| !matches!(command, ExcuteStmt::AttackAbsApplyThrow)),
                    _ => true,
                }),
                _ => true,
            }));
            let emitted = preview_game_fn(&opaque, "throw");
            assert_json_eq!(
                opaque,
                parse_acmd_script(&emitted),
                "nonmatching, interrupted, nested, and bare calls remain opaque"
            );
        }

        for prefix in ["// comment brace }", "helper(\"literal } brace\");"] {
            let guarded_source = source.replace(
                &format!("        {bundle}"),
                &format!("        {prefix}\n        {bundle}"),
            );
            let guarded = parse_acmd_script(&guarded_source);
            assert!(
                matches!(guarded.stmts.as_slice(), [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::Raw(_), ExcuteStmt::AttackAbsApplyThrow, ExcuteStmt::ClearAll])),
                "{guarded:#?}"
            );
            let emitted = preview_game_fn(&guarded, "throw");
            syn::parse_file(&emitted).unwrap();
            assert_json_eq!(&guarded, parse_acmd_script(&emitted));
        }

        let sound_source = source.replace("game_throw", "sound_throw");
        let sound = parse_sound_script(&sound_source);
        assert!(sound.stmts.iter().all(|statement| !matches!(
            statement,
            AcmdStmt::Excute(commands)
                if commands.iter().any(|command| matches!(command, ExcuteStmt::AttackAbsApplyThrow))
        )));
    }

    #[test]
    fn lexical_block_boundaries_ignore_nested_comments_and_rust_literals() {
        let lines = [
            "if macros::is_excute(agent) {",
            "    // a comment can contain a closing }",
            "    /* outer } /* nested } */ { */",
            "    let _unicode = '\\u{7b}';",
            "    let _hex = '\\x7d';",
            "    let _char = '界';",
            "    let _raw = br###\"",
            "        } if native_guard(agent) {",
            "    \"###;",
            "    helper(\"escaped \\\" and closing }\");",
            "    if native_guard(agent) {",
            "        helper();",
            "    }",
            "}",
            "outside();",
        ];
        assert_eq!(find_block_end(&lines, 0), (13, 0));
        assert_eq!(find_block_end(&lines, 10), (12, 0));
        let source = format!("{{\n{}\n}}", lines.join("\n"));
        syn::parse_str::<syn::Block>(&source).unwrap();
        for literal in [
            "let example = r#\"{\"#;",
            "helper(\"}\");",
            "/* { */",
            "let brace = '\\u{7b}';",
        ] {
            assert!(
                !opens_block(literal),
                "literal is not a control opener: {literal}"
            );
        }
        assert!(opens_block("if native_guard(r#\"}\"#) {"));
    }

    #[test]
    fn new_native_commands_require_actual_unattributed_root_statements() {
        let reaction =
            "AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0, false);";
        let body = [
            "/*",
            reaction,
            "*/",
            "let example = r#\"",
            reaction,
            "\"#;",
            "#[cfg(any())]",
            reaction,
            "if branch(agent) { // a nested native call is not a root statement",
            reaction,
            "}",
            reaction,
        ];
        let heads = direct_native_statement_lines(&body);
        assert!(heads.contains(&11));
        for opaque in [1, 4, 7, 9] {
            assert!(
                !heads.contains(&opaque),
                "non-executable or guarded line {opaque}"
            );
        }
        let parsed = parse_excute_block(&body, ParseDomain::Game, true);
        assert_eq!(
            parsed
                .iter()
                .filter(|command| matches!(
                    command,
                    ExcuteStmt::AttackModuleAddReactionFrame { .. }
                ))
                .count(),
            1
        );

        for (prefix, suffix) in [("/*", "*/"), ("let example = r#\"", "\"#;")] {
            let mut body = vec![prefix];
            body.extend(ATTACK_ABS_APPLY_THROW_SOURCE);
            body.push(suffix);
            let parsed = parse_excute_block(&body, ParseDomain::Game, true);
            assert!(parsed
                .iter()
                .all(|command| !matches!(command, ExcuteStmt::AttackAbsApplyThrow)));
        }
        let invalid = ["let invalid = ;", reaction];
        assert!(direct_native_statement_lines(&invalid).is_empty());
        let parsed = parse_excute_block(&invalid, ParseDomain::Game, true);
        assert!(parsed
            .iter()
            .all(|command| !matches!(command, ExcuteStmt::AttackModuleAddReactionFrame { .. })));
    }

    #[test]
    fn finish_camera_accepts_only_measured_direct_game_tuples() {
        let valid = "macros::CHECK_FINISH_CAMERA(agent, 15, 5);";
        let parsed = parse_excute_block(&[valid], ParseDomain::Game, true);
        let [ExcuteStmt::CheckFinishCamera(call)] = parsed.as_slice() else {
            panic!("expected the measured robot donor tuple, got {parsed:?}");
        };
        assert_eq!(call.arg1, "15");
        assert_eq!(call.arg2, "5");
        let json = serde_json::to_value(&parsed).unwrap();
        let restored: Vec<ExcuteStmt> = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), json);
        assert_eq!(
            emit_excute_stmts(&parsed, "").join("\n"),
            valid,
            "the exact literal tokens and call spelling are preserved"
        );

        for malformed in [
            "macros::CHECK_FINISH_CAMERA(boma, 15, 5);",
            "macros::CHECK_FINISH_CAMERA(agent, 15);",
            "macros::CHECK_FINISH_CAMERA(agent, 15, 5, 0);",
            "macros::CHECK_FINISH_CAMERA(agent, 15, 5)",
            "macros::CHECK_FINISH_CAMERA(agent, 999, 999);",
            "macros::CHECK_FINISH_CAMERA(agent, 15, 5); extra(agent);",
        ] {
            let rejected = parse_excute_block(&[malformed], ParseDomain::Game, true);
            assert!(
                matches!(
                    rejected.as_slice(),
                    [ExcuteStmt::Raw(raw)] if raw.trim() == malformed
                ),
                "unmeasured syntax must remain raw: {malformed}"
            );
        }

        let attributed = ["#[cfg(any())]", valid];
        let rejected = parse_excute_block(&attributed, ParseDomain::Game, true);
        assert!(rejected
            .iter()
            .all(|command| !matches!(command, ExcuteStmt::CheckFinishCamera(_))));

        let nested = [
            "if branch(agent) {",
            "    macros::CHECK_FINISH_CAMERA(agent, 15, 5);",
            "}",
        ];
        let rejected = parse_excute_block(&nested, ParseDomain::Game, true);
        assert!(rejected
            .iter()
            .all(|command| !matches!(command, ExcuteStmt::CheckFinishCamera(_))));
        for domain in [ParseDomain::Sound, ParseDomain::Expression] {
            let rejected = parse_excute_block(&[valid], domain, true);
            assert!(matches!(
                rejected.as_slice(),
                [ExcuteStmt::Raw(raw)] if raw.trim() == valid
            ));
        }
        let nested_context = parse_excute_block(&[valid], ParseDomain::Game, false);
        assert!(matches!(
            nested_context.as_slice(),
            [ExcuteStmt::Raw(raw)] if raw.trim() == valid
        ));
    }

    #[test]
    fn final_start_camera_accepts_only_closed_root_execute_tuples() {
        use crate::data::FinalStartCameraCall;

        let valid = "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 7, 20, 0, 0, 0);";
        let parsed = parse_excute_block(&[valid], ParseDomain::Game, true);
        let [ExcuteStmt::CheckFinalStartCamera(call)] = parsed.as_slice() else {
            panic!("expected closed final-start-camera tuple, got {parsed:?}");
        };
        assert_eq!(
            call.args.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["0", "7", "20", "0", "0", "0"]
        );
        let json = serde_json::to_value(&parsed).unwrap();
        let restored: Vec<ExcuteStmt> = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), json);
        assert_eq!(emit_excute_stmts(&parsed, "").join("\n"), valid);

        let measured_fraction =
            "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 6.5, 20, 0, 0, 0);";
        let fractional = parse_excute_block(&[measured_fraction], ParseDomain::Game, true);
        assert!(matches!(
            fractional.as_slice(),
            [ExcuteStmt::CheckFinalStartCamera(call)] if call.args[1] == "6.5"
        ));
        let tuples = crate::data::FINAL_START_CAMERA_PRESETS
            .iter()
            .map(|preset| preset.args)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(tuples.len(), 20);
        for tuple in tuples {
            let text = format!(
                "macros::CHECK_VALID_FINAL_START_CAMERA(agent, {});",
                tuple.join(", ")
            );
            let parsed = parse_excute_block(&[text.as_str()], ParseDomain::Game, true);
            assert!(matches!(parsed.as_slice(),
                [ExcuteStmt::CheckFinalStartCamera(call)] if call.args == tuple.map(str::to_owned)));
            assert_eq!(emit_excute_stmts(&parsed, "").join("\n"), text);
        }

        for malformed in [
            "macros::CHECK_VALID_FINAL_START_CAMERA(boma, 0, 7, 20, 0, 0, 0);",
            "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 7, 20, 0, 0);",
            "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 7, 20, 0, 0, 0, 1);",
            "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 7, 20, 0, 0, 0)",
            "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 7, 20, 0, 0, 1);",
            "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 7, 20, 0, 0, 6.4);",
            "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 6.50, 20, 0, 0, 0);",
            "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 6.5f32, 20, 0, 0, 0);",
            "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 7, 20, 0, 0, 0,);",
            "macros::CHECK_VALID_FINAL_START_CAMERA(agent, 0, 7, 20, 0, 0, 0); extra(agent);",
        ] {
            let rejected = parse_excute_block(&[malformed], ParseDomain::Game, true);
            assert!(
                matches!(rejected.as_slice(), [ExcuteStmt::Raw(raw)] if raw.trim() == malformed),
                "unmeasured or malformed source must remain raw: {malformed}"
            );
        }

        let attributed = ["#[cfg(any())]", valid];
        assert!(parse_excute_block(&attributed, ParseDomain::Game, true)
            .iter()
            .all(|command| !matches!(command, ExcuteStmt::CheckFinalStartCamera(_))));
        let nested = ["if outer(agent) {", valid, "}"];
        assert!(parse_excute_block(&nested, ParseDomain::Game, true)
            .iter()
            .all(|command| !matches!(command, ExcuteStmt::CheckFinalStartCamera(_))));
        assert!(matches!(
            parse_excute_block(&[valid], ParseDomain::Game, false).as_slice(),
            [ExcuteStmt::Raw(_)]
        ));
        for domain in [ParseDomain::Sound, ParseDomain::Expression] {
            assert!(matches!(
                parse_excute_block(&[valid], domain, true).as_slice(),
                [ExcuteStmt::Raw(_)]
            ));
        }

        let bare_function = format!(
            r#"unsafe extern "C" fn game_bare(agent: &mut L2CAgentBase) {{
    {valid}
}}
"#
        );
        let bare_script = parse_acmd_script(&bare_function);
        assert!(bare_script.stmts.iter().all(|stmt| match stmt {
            crate::data::AcmdStmt::Excute(commands) => commands
                .iter()
                .all(|command| !matches!(command, ExcuteStmt::CheckFinalStartCamera(_))),
            _ => true,
        }));
        assert!(preview_game_fn(&bare_script, "bare").contains(valid));

        let unknown_token = FinalStartCameraCall {
            args: ["0", "7", "20", "0", "0", "6.4"].map(str::to_owned),
        };
        assert!(!unknown_token.is_measured_tuple());
        for invalid in [
            serde_json::json!({"args": ["0", "7", "20", "0", "0"]}),
            serde_json::json!({"args": ["0", "7", "20", "0", "0", "0"], "extra": true}),
            serde_json::json!({"args": [0, "7", "20", "0", "0", "0"]}),
        ] {
            assert!(serde_json::from_value::<FinalStartCameraCall>(invalid).is_err());
        }
    }

    #[test]
    fn final_fear_face_is_closed_and_only_admitted_in_game_execute_contexts() {
        use crate::data::{FinalFearFaceCall, FinalFearFacePreset};

        let valid = "macros::FT_SET_FINAL_FEAR_FACE(agent, 60);";
        let parsed = parse_excute_block(&[valid], ParseDomain::Game, true);
        let [ExcuteStmt::FinalFearFace(call)] = parsed.as_slice() else {
            panic!("expected exact measured fear-face token, got {parsed:?}");
        };
        assert_eq!(call.argument, "60");
        assert!(call.is_measured_token());
        assert_eq!(emit_excute_stmts(&parsed, "").join("\n"), valid);
        let encoded = serde_json::to_value(call).unwrap();
        let restored: FinalFearFaceCall = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(&restored, call);
        assert_eq!(serde_json::to_value(restored).unwrap(), encoded);

        let catalog = crate::data::FINAL_FEAR_FACE_PRESETS
            .iter()
            .map(|preset: &FinalFearFacePreset| (preset.fighter, preset.argument))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(catalog.len(), 94);
        assert_eq!(
            catalog
                .iter()
                .map(|(fighter, _)| *fighter)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            90
        );
        assert_eq!(
            catalog
                .iter()
                .map(|(_, value)| *value)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            13
        );
        for (fighter, argument) in catalog {
            let source = format!("macros::FT_SET_FINAL_FEAR_FACE(agent, {argument});");
            let parsed = parse_excute_block(&[source.as_str()], ParseDomain::Game, true);
            assert!(
                matches!(parsed.as_slice(), [ExcuteStmt::FinalFearFace(call)] if call.argument == argument)
            );
            assert_eq!(emit_excute_stmts(&parsed, "").join("\n"), source);
            assert!(crate::data::FINAL_FEAR_FACE_PRESETS
                .iter()
                .any(|preset| preset.fighter == fighter && preset.argument == argument));
        }

        for malformed in [
            "macros::FT_SET_FINAL_FEAR_FACE(boma, 60);",
            "macros::FT_SET_FINAL_FEAR_FACE(agent);",
            "macros::FT_SET_FINAL_FEAR_FACE(agent, 60, 1);",
            "macros::FT_SET_FINAL_FEAR_FACE(agent, 060);",
            "macros::FT_SET_FINAL_FEAR_FACE(agent, 60.0);",
            "macros::FT_SET_FINAL_FEAR_FACE(agent, 60,);",
            "macros::FT_SET_FINAL_FEAR_FACE(agent, 999);",
            "macros::FT_SET_FINAL_FEAR_FACE(agent, 60)",
            "macros::FT_SET_FINAL_FEAR_FACE(agent, 60); extra(agent);",
        ] {
            let rejected = parse_excute_block(&[malformed], ParseDomain::Game, true);
            assert!(
                matches!(rejected.as_slice(), [ExcuteStmt::Raw(raw)] if raw.trim() == malformed),
                "noncanonical or unmeasured command must remain raw: {malformed}"
            );
        }
        assert!(matches!(
            parse_excute_block_with_fear_face_guard(
                &[valid],
                ParseDomain::Game,
                false,
                false,
                false,
                false,
                false,
            )
            .as_slice(),
            [ExcuteStmt::Raw(_)]
        ));
        for domain in [ParseDomain::Sound, ParseDomain::Expression] {
            assert!(matches!(
                parse_excute_block_with_fear_face_guard(
                    &[valid],
                    domain,
                    false,
                    true,
                    false,
                    false,
                    false
                )
                .as_slice(),
                [ExcuteStmt::Raw(_)]
            ));
        }
        let attributed = ["#[cfg(any())]", valid];
        assert!(parse_excute_block_with_fear_face_guard(
            &attributed,
            ParseDomain::Game,
            false,
            true,
            false,
            false,
            false,
        )
        .iter()
        .all(|command| !matches!(command, ExcuteStmt::FinalFearFace(_))));

        for invalid in [
            serde_json::json!({"argument": "60", "extra": true}),
            serde_json::json!({"argument": 60}),
        ] {
            assert!(serde_json::from_value::<FinalFearFaceCall>(invalid).is_err());
        }
        assert!(!FinalFearFaceCall {
            argument: "060".into()
        }
        .is_measured_token());
        assert!(!FinalFearFaceCall {
            argument: "999".into()
        }
        .is_measured_token());
    }

    #[test]
    fn final_fear_face_read_only_guard_permission_is_non_transitive() {
        fn typed_fear_face_count(stmts: &[AcmdStmt]) -> usize {
            stmts
                .iter()
                .map(|stmt| match stmt {
                    AcmdStmt::Excute(commands) => commands
                        .iter()
                        .filter(|command| matches!(command, ExcuteStmt::FinalFearFace(_)))
                        .count(),
                    AcmdStmt::RawBlock { body, .. } => typed_fear_face_count(body),
                    AcmdStmt::Loop { body, .. } => typed_fear_face_count(body),
                    _ => 0,
                })
                .sum()
        }

        let exact_guard_with_else_loop_and_sibling = r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {
        if macros::is_excute(agent) {
            macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
            macros::CHECK_FINISH_CAMERA(agent, 0, 0);
        }
        for _ in 0..1 {
            if macros::is_excute(agent) {
                macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
            }
        }
    } else {
        if macros::is_excute(agent) {
            macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
        }
    }
    if agent.is_some() {
        if macros::is_excute(agent) {
            macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
        }
    }
}
"#;
        let exact = parse_acmd_script(exact_guard_with_else_loop_and_sibling);
        assert_eq!(
            typed_fear_face_count(&exact.stmts),
            1,
            "only the immediate Execute under the exact Game guard receives the scoped exception"
        );
        let AcmdStmt::RawBlock { body, .. } = &exact.stmts[0] else {
            panic!("expected exact guard block");
        };
        let [AcmdStmt::Excute(commands), ..] = body.as_slice() else {
            panic!("expected immediate Execute child");
        };
        assert!(
            matches!(
                commands.as_slice(),
                [ExcuteStmt::FinalFearFace(_), ExcuteStmt::Raw(_)]
            ),
            "the read-only condition exception must not admit sibling native commands"
        );
        assert!(
            serde_json::from_value::<AcmdScript>(serde_json::to_value(&exact).unwrap()).is_ok()
        );

        let attributed_guard = r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {
    #[cfg(any())]
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {
        if macros::is_excute(agent) {
            macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
        }
    }
}
"#;
        let attributed = parse_acmd_script(attributed_guard);
        assert_eq!(
            typed_fear_face_count(&attributed.stmts),
            0,
            "an attribute attached to the guard must not grant the read-only exception"
        );
        assert!(emit_stmts(&attributed.stmts, "    ")
            .join("\n")
            .contains("#[cfg(any())]"));
        for prefix in [
            "#[cfg(any())]\n\n    // attached despite this comment",
            "#[cfg(\n        any()\n    )]",
        ] {
            let source = attributed_guard.replace("#[cfg(any())]", prefix);
            let parsed = parse_acmd_script(&source);
            assert_eq!(
                typed_fear_face_count(&parsed.stmts),
                0,
                "attributes separated by trivia must not authorize a child: {prefix}"
            );
        }

        let exact_guard_nested_under_unknown = r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {
    if(methodlib::L2CValue::operator==(lib::L2CValueconst&)const(false, false)){
        if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {
            if macros::is_excute(agent) {
                macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
            }
        }
    }
}
"#;
        let nested = parse_acmd_script(exact_guard_nested_under_unknown);
        assert_eq!(
            typed_fear_face_count(&nested.stmts),
            1,
            "the exact guard can type its child without escaping the unknown outer block"
        );
        assert_eq!(
            preview_game_fn(&nested, "finalstart")
                .matches("FT_SET_FINAL_FEAR_FACE")
                .count(),
            1
        );
    }

    #[test]
    fn two_absolute_hits_sharing_an_id_are_told_apart_by_their_kind() {
        let hitboxes = parse_acmd_script(THROW_ABS).to_hitboxes();
        let [throw, catch] = &hitboxes[..] else {
            panic!("expected two rows, got {}", hitboxes.len());
        };
        assert_eq!(throw.category, crate::data::CAT_ABS);
        assert_eq!(
            throw.abs.as_ref().unwrap().kind,
            "FIGHTER_ATTACK_ABSOLUTE_KIND_THROW"
        );
        assert_eq!(
            catch.abs.as_ref().unwrap().kind,
            "FIGHTER_ATTACK_ABSOLUTE_KIND_CATCH"
        );
        // Neither ended the other.
        assert_eq!((throw.active_end, catch.active_end), (9999, 9999));
        // Slot order is not ATTACK's; these are the values that shift if it were read as one.
        assert_eq!((throw.damage, throw.angle), (5.0, 75));
        assert_eq!((throw.kb_scaling, throw.fkb, throw.kb_base), (125, 0, 40));
        assert_eq!(catch.damage, 3.0);
        // No volume anywhere in the call — the panel and viewport read these as "not
        // applicable", so they must not come back as a plausible-looking hitbox at the origin.
        assert_eq!((throw.size, throw.bone_name.as_str()), (0.0, ""));
    }

    #[test]
    fn the_absolute_family_round_trips_through_the_emitter() {
        let script = parse_acmd_script(THROW_ABS);
        let exported = export_acmd_source(&script, "kirby", "throw_f");
        for line in [
            "*FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, 0, 5.0, 75, 125, 0, 40, 0.0, 1.0, \
             *ATTACK_LR_CHECK_F, 0.0, true,",
            "*FIGHTER_ATTACK_ABSOLUTE_KIND_CATCH, 0, 3.0, 361, 100, 0, 60, 0.0, 1.0,",
        ] {
            assert!(exported.contains(line), "missing `{line}` in\n{exported}");
        }
        assert_eq!(
            parse_acmd_script(&exported).to_hitboxes().len(),
            2,
            "the export must read back as the same two rows"
        );
    }

    #[test]
    fn a_fighter_specific_absolute_kind_survives_the_round_trip() {
        let source = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::ATTACK_ABS(agent, *FIGHTER_DOLLY_ATTACK_ABSOLUTE_KIND_FINAL, 0, 12.0, 361, 100, 0, 30, 0.5, 1.0, *ATTACK_LR_CHECK_F, 0.0, true, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_L, *COLLISION_SOUND_ATTR_DOLLY_CRITICAL, *ATTACK_REGION_NONE);
    }
}
"#;
        let script = parse_acmd_script(source);
        assert_eq!(
            script.to_hitboxes()[0].abs.as_ref().unwrap().kind,
            "FIGHTER_DOLLY_ATTACK_ABSOLUTE_KIND_FINAL"
        );
        assert!(
            export_acmd_source(&script, "dolly", "final_air_start")
                .contains("*FIGHTER_DOLLY_ATTACK_ABSOLUTE_KIND_FINAL"),
            "a fighter-specific kind must not be normalised away"
        );
    }

    /// dolly/SpecialAirHiCommand, verbatim and trimmed to the hurtbox lines — the knee and leg
    const HURT_INTANGIBLE: &str = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_XLU);
        macros::HIT_NODE(agent, Hash40::new("legl"), *HIT_STATUS_XLU);
    }
    frame(agent.lua_state_agent, 20.0);
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_NORMAL);
        macros::HIT_NODE(agent, Hash40::new("legl"), *HIT_STATUS_NORMAL);
    }
}
"#;

    const HURT_ARMOR: &str = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        damage!(agent, *MA_MSC_DAMAGE_DAMAGE_NO_REACTION, *DAMAGE_NO_REACTION_MODE_ALWAYS, 0);
    }
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        damage!(agent, *MA_MSC_DAMAGE_DAMAGE_NO_REACTION, *DAMAGE_NO_REACTION_MODE_DAMAGE_POWER, 25);
    }
    frame(agent.lua_state_agent, 18.0);
    if macros::is_excute(agent) {
        damage!(agent, *MA_MSC_DAMAGE_DAMAGE_NO_REACTION, *DAMAGE_NO_REACTION_MODE_NORMAL, 0);
    }
}
"#;

    #[test]
    fn damage_no_reaction_calls_are_typed_and_round_trip_with_condition_spans() {
        let script = parse_acmd_script(HURT_ARMOR);
        let events = script.to_hurtbox_condition_events();
        assert_eq!(
            events.iter().map(|event| event.frame).collect::<Vec<_>>(),
            vec![3, 12, 18]
        );
        assert!(matches!(
            events[0].condition,
            crate::data::HurtboxCondition::SuperArmor
        ));
        assert!(matches!(
            events[1].condition,
            crate::data::HurtboxCondition::DamageBasedArmor { threshold } if threshold == 25.0
        ));
        assert!(matches!(
            events[2].condition,
            crate::data::HurtboxCondition::Normal
        ));
        let spans = script.to_hurtbox_conditions();
        assert_eq!(spans.len(), 3);
        assert_eq!((spans[0].active_start, spans[0].active_end), (3, 11));
        assert_eq!((spans[1].active_start, spans[1].active_end), (12, 17));
        assert_eq!((spans[2].active_start, spans[2].active_end), (18, 18));
        assert!(spans[2].condition.is_normal());
        let exported = export_acmd_source(&script, "kirby", "club_swing_dash");
        assert!(exported.contains(
            "damage!(agent, *MA_MSC_DAMAGE_DAMAGE_NO_REACTION, *DAMAGE_NO_REACTION_MODE_ALWAYS, 0);"
        ));
        assert_eq!(
            parse_acmd_script(&exported).to_hurtbox_condition_events(),
            events
        );
    }

    #[test]
    fn a_hurtbox_state_spans_from_its_call_to_the_one_that_takes_it_back() {
        let (states, _) = parse_acmd_script(HURT_INTANGIBLE).to_hurtboxes();
        let knee: Vec<_> = states
            .iter()
            .filter(|s| s.target == crate::data::HurtTarget::Bone("kneer".into()))
            .collect();
        let [xlu, normal] = &knee[..] else {
            panic!("expected two states for kneer, got {}", knee.len());
        };
        assert_eq!(xlu.status, "HIT_STATUS_XLU");
        assert_eq!(
            (xlu.active_start, xlu.active_end),
            (9, 19),
            "intangible from its own frame until the frame before it is taken back"
        );
        // The closing call is a span too, and runs to the end of the move: nothing takes it
        // back, and inventing an end for it would claim the bone stops being normal.
        assert_eq!(normal.status, "HIT_STATUS_NORMAL");
        assert_eq!((normal.active_start, normal.active_end), (20, 9999));
    }

    /// A runtime `if` around a hurtbox call, with a second call after it.
    const HURT_IN_RAW_BLOCK: &str = r#"unsafe extern "C" fn game_rawblockhurt(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 2.0);
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_SQUAT_FLAG_REQUEST_SQUAT_SE) {
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("shoulderl"), *HIT_STATUS_XLU);
    }
    }
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_NORMAL);
    }
}
"#;

    #[test]
    fn a_hurtbox_inside_a_runtime_branch_resolves_to_its_own_call() {
        let mut script = parse_acmd_script(HURT_IN_RAW_BLOCK);
        let (states, _) = script.to_hurtboxes();
        let expected: Vec<(usize, String)> = states
            .iter()
            .map(|s| {
                let crate::data::HurtTarget::Bone(bone) = &s.target else {
                    panic!("fixture uses HIT_NODE, which targets a bone");
                };
                (s.site, bone.clone())
            })
            .collect();
        assert_eq!(
            expected.len(),
            2,
            "the walk should see the branch's call and the one after it"
        );
        for (site, bone) in expected {
            let stmt = script
                .hurt_stmt_mut(site)
                .unwrap_or_else(|| panic!("site {site} ({bone}) resolved to nothing"));
            let crate::data::ExcuteStmt::HitStatus { target, .. } = stmt else {
                panic!("site {site} resolved to {stmt:?}, not a hurtbox statement");
            };
            assert_eq!(
                target,
                &crate::data::HurtTarget::Bone(bone.clone()),
                "site {site} should be the `{bone}` call the walk took it from"
            );
        }
    }

    #[test]
    fn a_branch_inside_a_loop_that_never_runs_still_advances_both_cursors() {
        const EMPTY_LOOP_BRANCH: &str = r#"unsafe extern "C" fn game_emptyloopbranch(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 2.0);
    for _ in 0..0 {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_SQUAT_FLAG_REQUEST_SQUAT_SE) {
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("shoulderl"), *HIT_STATUS_XLU);
        macros::ATK_POWER(agent, 0, 12.0);
    }
    }
    }
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_NORMAL);
        macros::ATK_POWER(agent, 1, 3.5);
    }
}
"#;
        let script = parse_acmd_script(EMPTY_LOOP_BRANCH);
        let (states, _) = script.to_hurtboxes();
        let mods = script.to_attack_mods();
        assert_eq!(
            states.len(),
            1,
            "a loop with no iterations set a hurtbox anyway"
        );
        assert_eq!(
            mods.len(),
            1,
            "a loop with no iterations ran a modifier anyway"
        );
        // Site 1, not 0: the skipped branch's own call is site 0 and keeps it. Without the
        // `RawBlock` arm in the counters the cursor never steps over that call, and the
        // surviving statement claims site 0 — the line inside the loop that never ran.
        assert_eq!(
            states[0].site, 1,
            "the surviving `kneer` call took the site of the `shoulderl` call inside the skipped loop"
        );
        assert_eq!(
            mods[0].site, 1,
            "the surviving id-1 modifier took the site of the id-0 call inside the skipped loop"
        );
    }

    #[test]
    fn an_attack_modifier_inside_a_runtime_branch_resolves_to_its_own_call() {
        const MOD_IN_RAW_BLOCK: &str = r#"unsafe extern "C" fn game_rawblockmod(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_SMASH_SMASH_HOLD_TO_ATTACK) {
    if macros::is_excute(agent) {
        macros::ATK_POWER(agent, 0, 12.0);
    }
    }
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::ATK_POWER(agent, 1, 3.5);
    }
}
"#;
        let mut script = parse_acmd_script(MOD_IN_RAW_BLOCK);
        let expected: Vec<(usize, i64)> = script
            .to_attack_mods()
            .iter()
            .map(|m| (m.site, m.id))
            .collect();
        assert_eq!(expected.len(), 2, "the walk should see both modifiers");
        for (site, id) in expected {
            let stmt = script
                .attack_mod_stmt_mut(site)
                .unwrap_or_else(|| panic!("site {site} (id {id}) resolved to nothing"));
            let crate::data::ExcuteStmt::AttackMod { id: got, .. } = stmt else {
                panic!("site {site} resolved to {stmt:?}, not an attack modifier");
            };
            assert_eq!(
                *got, id,
                "site {site} should be the id-{id} call the walk took it from"
            );
        }
    }

    #[test]
    fn raw_hurtbox_bone_hash_round_trips_without_a_skeleton_name() {
        let source = r#"
unsafe extern "C" fn game_specialairhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new_raw(0xdead_beef), *HIT_STATUS_XLU);
    }
}
"#;
        let script = parse_acmd_script(source);
        let (states, _) = script.to_hurtboxes();
        assert_eq!(
            states[0].target,
            crate::data::HurtTarget::Bone("0xdeadbeef".into())
        );
        let exported = export_acmd_source(&script, "mario", "special_air_hi");
        assert!(exported.contains("Hash40::new_raw(0xdeadbeef)"));
        assert_eq!(
            parse_acmd_script(&exported).to_hurtboxes(),
            script.to_hurtboxes()
        );
    }

    /// kirby/FinalStart and kirby/ThrownHi, trimmed to the hurtbox lines. Kirby's final smash
    const HURT_WHOLE_BODY: &str = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        macros::WHOLE_HIT(agent, *HIT_STATUS_XLU);
    }
    frame(agent.lua_state_agent, 46.0);
    if macros::is_excute(agent) {
        macros::WHOLE_HIT(agent, *HIT_STATUS_NORMAL);
    }
}
"#;

    #[test]
    fn a_whole_body_status_is_emitted_into_the_slot_the_target_vacated() {
        let script = parse_acmd_script(HURT_WHOLE_BODY);
        let exported = export_acmd_source(&script, "kirby", "final_start");
        assert!(
            exported.contains("macros::WHOLE_HIT(agent, *HIT_STATUS_XLU);"),
            "{exported}"
        );
        assert!(
            !exported.contains("WHOLE_HIT(agent, Hash40"),
            "the target slot must not come back: {exported}"
        );
        assert_eq!(
            parse_acmd_script(&exported).to_hurtboxes(),
            script.to_hurtboxes(),
            "the export must resolve to the same spans as the source"
        );
    }

    #[test]
    fn a_whole_hit_written_with_the_targeted_arity_is_left_alone() {
        let two_args =
            r#"        macros::WHOLE_HIT(agent, Hash40::new("kneer"), *HIT_STATUS_XLU);"#;
        assert!(
            parse_hurtbox_call(two_args).is_none(),
            "a WHOLE_HIT with a target slot is not a form this parser has seen"
        );
        let one_arg = r#"        macros::WHOLE_HIT(agent, *HIT_STATUS_XLU);"#;
        assert!(matches!(
            parse_hurtbox_call(one_arg),
            Some(crate::data::ExcuteStmt::HitStatus {
                target: crate::data::HurtTarget::Whole,
                ..
            })
        ));
    }

    /// kirby/AttackLw4 and kirby/Attack100Sub, spliced: the two placements the corpus uses.
    const ATTACK_MODS: &str = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::ATK_SET_SHIELD_SETOFF_MUL(agent, 0, 7);
    }
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        macros::ATK_POWER(agent, 0, 10);
        macros::ATK_POWER(agent, 1, 10);
    }
}
"#;

    #[test]
    fn attack_modifiers_resolve_against_the_hitbox_id_they_name() {
        use crate::data::AttackModKind;
        let mods = parse_acmd_script(ATTACK_MODS).to_attack_mods();
        let [setoff, power_0, power_1] = &mods[..] else {
            panic!("expected three modifiers, got {}", mods.len());
        };
        assert_eq!(setoff.kind, AttackModKind::ShieldSetoffMul);
        assert_eq!((setoff.id, setoff.value, setoff.frame), (0, 7.0, 1));
        // Five frames after the ATTACK they retune, and telling the two ids apart is the whole
        // point of reading slot 0 as the id: both calls carry the same value.
        assert_eq!(power_0.kind, AttackModKind::Power);
        assert_eq!((power_0.id, power_0.value, power_0.frame), (0, 10.0, 10));
        assert_eq!((power_1.id, power_1.value, power_1.frame), (1, 10.0, 10));
        // Own numbering space: these must not consume hurtbox sites, or every hurtbox edit in a
        // script that also tunes a hitbox would land on the wrong line.
        assert_eq!([setoff.site, power_0.site, power_1.site], [0, 1, 2]);
    }

    #[test]
    fn an_attack_modifier_does_not_become_a_hitbox_of_its_own() {
        assert!(parse_acmd_script(ATTACK_MODS).to_hitboxes().is_empty());
    }

    #[test]
    fn a_whole_modifier_value_is_emitted_as_the_integer_the_corpus_writes() {
        let script = parse_acmd_script(ATTACK_MODS);
        let exported = export_acmd_source(&script, "kirby", "attack_lw4");
        assert!(
            exported.contains("macros::ATK_SET_SHIELD_SETOFF_MUL(agent, 0, 7);"),
            "{exported}"
        );
        assert!(
            exported.contains("macros::ATK_POWER(agent, 0, 10);"),
            "{exported}"
        );
        // Scoped to the modifier lines: the surrounding `frame(agent.lua_state_agent, 10.0);`
        // is a slot declared `f32`, where `num`'s decimal point is exactly right.
        let emitted: Vec<&str> = exported
            .lines()
            .filter(|l| l.contains("ATK_POWER(") || l.contains("ATK_SET_SHIELD_SETOFF_MUL("))
            .collect();
        assert!(
            emitted.iter().all(|l| !l.contains('.')),
            "a whole value must not gain a decimal point: {emitted:?}"
        );
        assert_eq!(attack_mod_num(2.5), "2.5", "a fraction keeps its point");
        assert_eq!(
            parse_acmd_script(&exported).to_attack_mods(),
            script.to_attack_mods(),
            "the export must resolve to the same modifiers as the source"
        );
    }

    #[test]
    fn the_id_slot_is_read_as_the_id_and_a_wrong_arity_call_falls_through() {
        use crate::data::{AttackModKind, ExcuteStmt};
        let asymmetric = "        macros::ATK_POWER(agent, 3, 12);";
        assert!(matches!(
            parse_attack_mod_call(asymmetric),
            Some(ExcuteStmt::AttackMod {
                kind: AttackModKind::Power,
                id: 3,
                value,
            }) if value == 12.0
        ));
        // One argument short: the signature has two, so this is not a form the parser has seen
        // and it stays `Raw` rather than being reinterpreted.
        assert!(parse_attack_mod_call("        macros::ATK_POWER(agent, 3);").is_none());
        // `ATK_HIT_ABS` takes no id and passes local variables. It must not be dragged in here
        // by its shared prefix — it is outside the hitbox-modifier family for that reason.
        let hit_abs = r#"        macros::ATK_HIT_ABS(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, Hash40::new("throw"), target, target_group, target_no);"#;
        assert!(parse_attack_mod_call(hit_abs).is_none());
    }

    #[test]
    fn the_argument_less_members_and_the_numbered_ones_keep_their_own_families() {
        let source = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::HIT_NO(agent, 8, *HIT_STATUS_OFF);
        macros::COL_PRI(agent, 200);
    }
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        macros::HIT_RESET_ALL(agent);
    }
    frame(agent.lua_state_agent, 16.0);
    if macros::is_excute(agent) {
        macros::COL_NORMAL(agent);
    }
}
"#;
        let script = parse_acmd_script(source);
        let (states, pris) = script.to_hurtboxes();
        let [group] = &states[..] else {
            panic!("expected one hurtbox state, got {}", states.len());
        };
        assert_eq!(group.target, crate::data::HurtTarget::Group(8));
        assert_eq!(group.status, "HIT_STATUS_OFF");
        assert_eq!(
            (group.active_start, group.active_end),
            (4, 11),
            "HIT_RESET_ALL ends it"
        );

        let [pri] = &pris[..] else {
            panic!("expected one priority span, got {}", pris.len());
        };
        assert_eq!(pri.pri, 200);
        assert_eq!(
            (pri.active_start, pri.active_end),
            (4, 15),
            "COL_NORMAL ends the priority, and HIT_RESET_ALL at frame 12 does not"
        );

        let exported = export_acmd_source(&script, "kirby", "guard_off");
        for line in [
            "macros::HIT_NO(agent, 8, *HIT_STATUS_OFF);",
            "macros::COL_PRI(agent, 200);",
            "macros::HIT_RESET_ALL(agent);",
            "macros::COL_NORMAL(agent);",
        ] {
            assert!(exported.contains(line), "missing {line} in {exported}");
        }
    }

    #[test]
    fn a_hurtbox_call_of_the_wrong_arity_is_left_alone_rather_than_padded() {
        for line in [
            "macros::HIT_NODE(agent, Hash40::new(\"legr\"));",
            "macros::COL_PRI(agent);",
            "macros::HIT_RESET_ALL(agent, 3);",
        ] {
            let source = format!(
                "unsafe extern \"C\" fn game_test(agent: &mut L2CAgentBase) {{\n    \
                 if macros::is_excute(agent) {{\n        {line}\n    }}\n}}\n"
            );
            let script = parse_acmd_script(&source);
            let (states, pris) = script.to_hurtboxes();
            assert!(
                states.is_empty() && pris.is_empty(),
                "{line} should not have parsed into a span"
            );
            // …and it must still come back out, which is what `Raw` buys.
            assert!(
                export_acmd_source(&script, "kirby", "attack_11").contains(line),
                "{line} was dropped by the export"
            );
        }
    }

    /// kirby/ThrowHi, verbatim — the only `ATTACK_IGNORE_THROW` in the vanilla corpus, and
    const IGNORE_THROW: &str = r#"macros::ATTACK_IGNORE_THROW(agent, 0, 0, Hash40::new("top"), 7.0, 65, 95, 0, 85, 9.5, 0.0, 6.5, 2.0, 1.0, 1.0, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_BODY);"#;

    #[test]
    fn a_capsule_less_attack_call_reads_every_slot_after_the_transform() {
        let source = format!(
            "unsafe extern \"C\" fn game_test(agent: &mut L2CAgentBase) {{\n    \
             frame(agent.lua_state_agent, 12.0);\n    if macros::is_excute(agent) {{\n        \
             {IGNORE_THROW}\n    }}\n}}\n"
        );
        let hitboxes = parse_acmd_script(&source).to_hitboxes();
        let [hb] = &hitboxes[..] else {
            panic!("expected one hitbox, got {}", hitboxes.len());
        };

        assert_eq!(hb.func, "ATTACK_IGNORE_THROW");
        assert_eq!(hb.capsule_end, None, "this call has no capsule triple");
        // The transform is ahead of the optional arguments and cannot shift.
        assert_eq!((hb.damage, hb.angle, hb.size), (7.0, 65, 9.5));
        assert_eq!((hb.offset_x, hb.offset_y, hb.offset_z), (0.0, 6.5, 2.0));
        // Everything below here is what the shift used to corrupt.
        assert_eq!(hb.hitlag_mult, 1.0, "hitlag must not read the capsule slot");
        assert_eq!(hb.sdi_mult, 1.0);
        assert_eq!(hb.setoff_kind, "ATTACK_SETOFF_KIND_OFF");
        assert_eq!(hb.lr_check, "ATTACK_LR_CHECK_POS");
        assert!(!hb.is_clang);
        assert_eq!(hb.hitbox_attr, 0.0);
        assert!(hb.is_landing_attack);
        assert_eq!(hb.situation_mask, "COLLISION_SITUATION_MASK_GA");
        assert_eq!(hb.collision_attr, "collision_attr_normal");
        assert_eq!(hb.sound_level, "ATTACK_SOUND_LEVEL_M");
        assert_eq!(hb.sound_attr, "COLLISION_SOUND_ATTR_KICK");
        assert_eq!(hb.attack_region, "ATTACK_REGION_BODY");
    }

    #[test]
    fn an_attack_family_call_is_exported_under_its_own_macro() {
        let source = format!(
            "unsafe extern \"C\" fn game_test(agent: &mut L2CAgentBase) {{\n    \
             frame(agent.lua_state_agent, 12.0);\n    if macros::is_excute(agent) {{\n        \
             {IGNORE_THROW}\n    }}\n}}\n"
        );
        let parsed = parse_acmd_script(&source);
        let emitted = preview_game_fn(&parsed, "throwhi");
        assert!(
            emitted.contains("macros::ATTACK_IGNORE_THROW(agent, 0, 0,"),
            "the export dropped the family member:\n{emitted}"
        );
        // Written back out in the long form, because that is the one smash-script declares
        // and therefore the only one that builds.
        assert!(
            emitted.contains("2.0, None, None, None, 1.0, 1.0,"),
            "the export must supply the capsule options the source omitted:\n{emitted}"
        );
        // The long form re-reads as the same hitbox: the shift is a source-text detail, not
        // a property of the move.
        assert_eq!(
            parse_acmd_script(&emitted).to_hitboxes(),
            parsed.to_hitboxes(),
            "a capsule-less call must survive a round trip through the emitter"
        );
    }

    #[test]
    fn ordinary_attack_and_ignore_throw_keep_their_native_property_slots() {
        const FULL: &str = r#"macros::ATTACK(agent, 1, 2, Hash40::new("top"), 3.25, 45, 101, 7, 89, 4.5, 1.0, 2.0, 3.0, None, None, None, 1.25, 1.5, *ATTACK_SETOFF_KIND_THRU, *ATTACK_LR_CHECK_LEFT, true, 12, 0.35, 4, false, true, false, true, false, *COLLISION_SITUATION_MASK_G, *COLLISION_CATEGORY_MASK_FIGHTER, *COLLISION_PART_MASK_HEAD, true, Hash40::new("collision_attr_fire"), *ATTACK_SOUND_LEVEL_L, *COLLISION_SOUND_ATTR_CUTUP, *ATTACK_REGION_SWORD);"#;
        const SHORT: &str = r#"macros::ATTACK_IGNORE_THROW(agent, 1, 2, Hash40::new("top"), 3.25, 45, 101, 7, 89, 4.5, 1.0, 2.0, 3.0, 1.25, 1.5, *ATTACK_SETOFF_KIND_THRU, *ATTACK_LR_CHECK_LEFT, true, 12, 0.35, 4, false, true, false, true, false, *COLLISION_SITUATION_MASK_G, *COLLISION_CATEGORY_MASK_FIGHTER, *COLLISION_PART_MASK_HEAD, true, Hash40::new("collision_attr_fire"), *ATTACK_SOUND_LEVEL_L, *COLLISION_SOUND_ATTR_CUTUP, *ATTACK_REGION_SWORD);"#;

        for (func, line) in [("ATTACK", FULL), ("ATTACK_IGNORE_THROW", SHORT)] {
            let source = format!(
                "unsafe extern \"C\" fn game_test(agent: &mut L2CAgentBase) {{\n    \
                 frame(agent.lua_state_agent, 8.0);\n    if macros::is_excute(agent) {{\n        \
                 {line}\n    }}\n}}\n"
            );
            let parsed = parse_acmd_script(&source);
            let hitboxes = parsed.to_hitboxes();
            let [hb] = &hitboxes[..] else {
                panic!("{func} should parse as one hitbox");
            };
            assert_eq!(hb.func, func);
            assert_eq!((hb.id, hb.part, hb.damage, hb.angle), (1, 2, 3.25, 45));
            assert_eq!(hb.capsule_end, None);
            assert_eq!((hb.hitlag_mult, hb.sdi_mult), (1.25, 1.5));
            assert_eq!(hb.setoff_kind, "ATTACK_SETOFF_KIND_THRU");
            assert_eq!(hb.lr_check, "ATTACK_LR_CHECK_LEFT");
            assert!(hb.is_clang);
            assert_eq!(hb.is_add_attack, 12);
            assert_eq!(hb.hitbox_attr, 0.35);
            assert_eq!(hb.ground_or_air, 4);
            assert!(!hb.is_mtk);
            assert!(hb.is_shield_disable);
            assert!(!hb.is_reflectable);
            assert!(hb.is_absorbable);
            assert!(!hb.is_landing_attack);
            assert_eq!(hb.situation_mask, "COLLISION_SITUATION_MASK_G");
            assert_eq!(hb.category_mask, "COLLISION_CATEGORY_MASK_FIGHTER");
            assert_eq!(hb.part_mask, "COLLISION_PART_MASK_HEAD");
            assert!(hb.no_finish_camera);
            assert_eq!(hb.collision_attr, "collision_attr_fire");
            assert_eq!(hb.sound_level, "ATTACK_SOUND_LEVEL_L");
            assert_eq!(hb.sound_attr, "COLLISION_SOUND_ATTR_CUTUP");
            assert_eq!(hb.attack_region, "ATTACK_REGION_SWORD");

            let emitted = preview_game_fn(&parsed, "test");
            let emitted_line = emitted
                .lines()
                .find(|line| line.contains(&format!("macros::{func}(")));
            let emitted_line = emitted_line.unwrap_or_else(|| panic!("missing {func}: {emitted}"));
            let open = emitted_line.find('(').unwrap();
            let close = emitted_line.rfind(')').unwrap();
            let args = tokenize_args(&emitted_line[open + 1..close]);
            // The emitter normalizes both source spellings to the full 36-argument binding ABI.
            assert_eq!(
                args.len(),
                37,
                "agent plus 36 native parameters: {emitted_line}"
            );
            for (slot, expected) in [
                (18, "*ATTACK_SETOFF_KIND_THRU"),
                (19, "*ATTACK_LR_CHECK_LEFT"),
                (20, "true"), // pinned parameter: set_weight
                (21, "12"),   // shield_damage
                (22, "0.35"), // trip
                (23, "4"),    // rehit
                (24, "false"),
                (25, "true"),
                (26, "false"),
                (27, "true"),
                (28, "false"),
                (29, "*COLLISION_SITUATION_MASK_G"),
                (30, "*COLLISION_CATEGORY_MASK_FIGHTER"),
                (31, "*COLLISION_PART_MASK_HEAD"),
                (32, "true"), // friendly_fire
                (33, "Hash40::new(\"collision_attr_fire\")"),
                (34, "*ATTACK_SOUND_LEVEL_L"),
                (35, "*COLLISION_SOUND_ATTR_CUTUP"),
                (36, "*ATTACK_REGION_SWORD"),
            ] {
                assert_eq!(
                    args[slot], expected,
                    "native argument slot {slot}: {emitted_line}"
                );
            }
            assert!(
                emitted_line.contains(&format!("macros::{func}(")),
                "family changed: {emitted}"
            );
            let round_trip = parse_acmd_script(&emitted).to_hitboxes();
            assert_eq!(round_trip.len(), 1);
            let reparsed = &round_trip[0];
            assert_eq!(reparsed.func, func);
            assert_eq!(reparsed.is_clang, hb.is_clang);
            assert_eq!(reparsed.setoff_kind, hb.setoff_kind);
            assert_eq!(reparsed.lr_check, hb.lr_check);
            assert_eq!(reparsed.is_add_attack, hb.is_add_attack);
            assert_eq!(reparsed.hitbox_attr, hb.hitbox_attr);
            assert_eq!(reparsed.ground_or_air, hb.ground_or_air);
            assert_eq!(reparsed.is_mtk, hb.is_mtk);
            assert_eq!(reparsed.is_shield_disable, hb.is_shield_disable);
            assert_eq!(reparsed.is_reflectable, hb.is_reflectable);
            assert_eq!(reparsed.is_absorbable, hb.is_absorbable);
            assert_eq!(reparsed.is_landing_attack, hb.is_landing_attack);
            assert_eq!(reparsed.situation_mask, hb.situation_mask);
            assert_eq!(reparsed.category_mask, hb.category_mask);
            assert_eq!(reparsed.part_mask, hb.part_mask);
            assert_eq!(reparsed.no_finish_camera, hb.no_finish_camera);
            assert_eq!(reparsed.collision_attr, hb.collision_attr);
            assert_eq!(reparsed.sound_level, hb.sound_level);
            assert_eq!(reparsed.sound_attr, hb.sound_attr);
            assert_eq!(reparsed.attack_region, hb.attack_region);
        }
    }

    #[test]
    fn wind_commands_round_trip_with_exact_shapes_and_independent_erases() {
        let source = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::AREA_WIND_2ND_arg10(agent, 3, 1, 80, 300, 0.8, 4, 12, 24, 16, 50);
    }
    frame(agent.lua_state_agent, 7.0);
    if macros::is_excute(agent) {
        macros::AREA_WIND_2ND_RAD(agent, 4, 0.5, 0.02, 1000, 1, -2, 6, 18);
    }
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
        AreaModule::erase_wind(agent.module_accessor, 4);
    }
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        AreaModule::erase_wind(agent.module_accessor, 3);
    }
}
"#;
        let script = parse_acmd_script(source);
        let hitboxes = script.to_hitboxes();
        assert_eq!(hitboxes.len(), 2);

        let rect = &hitboxes[0];
        let rect_wind = rect.wind.as_ref().unwrap();
        assert_eq!(rect.id, 3);
        assert_eq!(rect_wind.command, "AREA_WIND_2ND_arg10");
        assert_eq!(rect_wind.offset(), [4.0, 12.0]);
        assert_eq!(rect_wind.dimensions(), [24.0, 16.0]);
        assert_eq!((rect.active_start, rect.active_end), (5, 11));

        let radial = &hitboxes[1];
        let radial_wind = radial.wind.as_ref().unwrap();
        assert_eq!(radial.id, 4);
        assert_eq!(radial_wind.command, "AREA_WIND_2ND_RAD");
        assert_eq!(radial_wind.offset(), [-2.0, 6.0]);
        assert_eq!(radial_wind.radius(), 18.0);
        assert_eq!((radial.active_start, radial.active_end), (7, 8));

        let emitted = export_acmd_source(&script, "mario", "test");
        assert!(emitted.contains(
            "macros::AREA_WIND_2ND_arg10(agent, 3, 1, 80, 300, 0.8, 4, 12, 24, 16, 50);"
        ));
        assert!(
            emitted.contains("macros::AREA_WIND_2ND_RAD(agent, 4, 0.5, 0.02, 1000, 1, -2, 6, 18);")
        );
        assert!(emitted.contains("AreaModule::erase_wind(agent.module_accessor, 4);"));
    }

    #[test]
    fn catch_calls_round_trip_through_the_editor() {
        let src = r#"unsafe extern "C" fn game_catch(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::CATCH(agent, 0, Hash40::new("top"), 5.5, 0.0, 6.4, 10.2, Some(1.0), Some(2.0), Some(3.0), *FIGHTER_STATUS_KIND_SWALLOWED, *COLLISION_SITUATION_MASK_A);
    }
    frame(agent.lua_state_agent, 13.0);
    if macros::is_excute(agent) {
        GrabModule::clear_all(agent.module_accessor);
    }
}
"#;
        let script = parse_acmd_script(src);
        let boxes = script.to_hitboxes();
        assert_eq!(boxes.len(), 1, "{boxes:#?}");
        let grab = &boxes[0];
        assert_eq!(grab.category, 1, "a CATCH is a grab box, not an attack");
        assert_eq!((grab.active_start, grab.active_end), (8, 12));
        assert_eq!(grab.size, 5.5);
        assert_eq!(grab.capsule_end, Some([1.0, 2.0, 3.0]));
        // Grabs deal no damage — the attack-only fields must not inherit ATTACK's defaults.
        assert_eq!(grab.damage, 0.0);

        // The author's own status and situation survive, rather than being replaced by the
        // stand-ins used for a grab that never came from a script.
        let exported = preview_game_fn(&script, "catch");
        assert!(
            exported.contains("*FIGHTER_STATUS_KIND_SWALLOWED"),
            "{exported}"
        );
        assert!(
            exported.contains("*COLLISION_SITUATION_MASK_A"),
            "{exported}"
        );
        assert!(exported.contains("GrabModule::clear_all"), "{exported}");
        assert!(!exported.contains("AttackModule::clear_all"), "{exported}");

        // And the whole thing round-trips: re-parsing the export gives the same grab back.
        assert_eq!(
            parse_acmd_script(&exported).to_hitboxes(),
            boxes,
            "{exported}"
        );
    }

    #[test]
    fn search_boxes_parse_in_both_of_the_shapes_the_corpus_writes_them() {
        let src = r#"unsafe extern "C" fn game_search(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::SEARCH(agent, 0, 0, Hash40::new("top"), 4.0, 0.0, 7.0, 8.0, *COLLISION_KIND_MASK_ATTACK, *HIT_STATUS_MASK_ALL, 0, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_FIGHTER, *COLLISION_PART_MASK_ALL, false);
    }
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::SEARCH(agent, 1, 0, Hash40::new("top"), 7.5, 0.0, 7.0, 4.0, 0.0, 7.0, 13.0, *COLLISION_KIND_MASK_ATTACK, *HIT_STATUS_MASK_NORMAL, 60, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false);
    }
}
"#;
        let script = parse_acmd_script(src);
        let boxes = script.to_hitboxes();
        assert_eq!(boxes.len(), 2, "{boxes:#?}");

        let short = &boxes[0];
        assert_eq!(short.category, crate::data::CAT_SEARCH);
        assert_eq!(short.size, 4.0);
        assert_eq!(
            (short.offset_x, short.offset_y, short.offset_z),
            (0.0, 7.0, 8.0)
        );
        assert_eq!(short.capsule_end, None);
        // The three masks land on the fields that already existed, not in the extras.
        assert_eq!(short.situation_mask, "COLLISION_SITUATION_MASK_GA");
        assert_eq!(short.category_mask, "COLLISION_CATEGORY_MASK_FIGHTER");
        assert_eq!(short.part_mask, "COLLISION_PART_MASK_ALL");
        let extras = short.search.as_ref().expect("a SEARCH carries its extras");
        assert_eq!(extras.collision_kind, "COLLISION_KIND_MASK_ATTACK");
        assert_eq!(extras.hit_status, "HIT_STATUS_MASK_ALL");
        assert_eq!(extras.unk, 0);
        assert!(!extras.unk2);
        // A detection box deals nothing — the attack defaults must not leak in.
        assert_eq!(short.damage, 0.0);

        let long = &boxes[1];
        assert_eq!(long.capsule_end, Some([0.0, 7.0, 13.0]));
        assert_eq!(long.size, 7.5);
        assert_eq!(long.category_mask, "COLLISION_CATEGORY_MASK_ALL");
        let extras = long.search.as_ref().expect("a SEARCH carries its extras");
        assert_eq!(extras.hit_status, "HIT_STATUS_MASK_NORMAL");
        // Not invariant across the corpus, so it has to survive rather than be defaulted.
        assert_eq!(extras.unk, 60);

        // Nothing closes a search volume, so both run to the end of the move.
        assert_eq!(short.active_end, 9999);
        assert_eq!(long.active_end, 9999);

        // And the whole thing round-trips.
        let exported = preview_game_fn(&script, "search");
        assert_eq!(
            parse_acmd_script(&exported).to_hitboxes(),
            boxes,
            "{exported}"
        );
    }

    #[test]
    fn a_search_a_grab_and_a_throw_hit_sharing_id_zero_do_not_close_each_other() {
        let src = r#"unsafe extern "C" fn game_specialnstart(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 18.0);
    if macros::is_excute(agent) {
        macros::CATCH(agent, 0, Hash40::new("top"), 6.0, 0.0, 6.0, 5.0, *FIGHTER_STATUS_KIND_SWALLOWED, *COLLISION_SITUATION_MASK_GA);
        macros::SEARCH(agent, 0, 0, Hash40::new("top"), 4.0, 0.0, 7.0, 8.0, *COLLISION_KIND_MASK_ATTACK, *HIT_STATUS_MASK_ALL, 0, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_FIGHTER, *COLLISION_PART_MASK_ALL, false);
        macros::ATTACK_ABS(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_CATCH, 0, 5.0, 361, 100, 0, 0, 0.0, 1.0, *ATTACK_LR_CHECK_F, 0.0, true, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_NONE, *ATTACK_REGION_NONE);
    }
}
"#;
        let script = parse_acmd_script(src);
        let boxes = script.to_hitboxes();
        assert_eq!(boxes.len(), 3, "all three families stay open\n{boxes:#?}");

        let mut categories: Vec<u8> = boxes.iter().map(|b| b.category).collect();
        categories.sort_unstable();
        assert_eq!(
            categories,
            vec![1, crate::data::CAT_ABS, crate::data::CAT_SEARCH]
        );
        assert!(
            boxes.iter().all(|b| b.id == 0 && b.active_start == 18),
            "{boxes:#?}"
        );
        // None of them was ended by another's arrival.
        assert!(boxes.iter().all(|b| b.active_end == 9999), "{boxes:#?}");

        // The inhale's own arguments all survive the export, each through its own layout.
        let exported = preview_game_fn(&script, "specialnstart");
        assert!(
            exported.contains("*FIGHTER_STATUS_KIND_SWALLOWED"),
            "{exported}"
        );
        assert!(
            exported.contains("*COLLISION_CATEGORY_MASK_FIGHTER"),
            "{exported}"
        );
        assert!(
            exported.contains("*FIGHTER_ATTACK_ABSOLUTE_KIND_CATCH"),
            "{exported}"
        );
    }

    #[test]
    fn the_search_size_modifier_is_not_parsed_as_a_search_box() {
        let src = r#"unsafe extern "C" fn game_search(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::SET_SEARCH_SIZE_EXIST(agent, 0, 7);
    }
}
"#;
        let script = parse_acmd_script(src);
        assert!(
            script.to_hitboxes().is_empty(),
            "a size modifier is not a box of its own"
        );
        // Unmodelled, so it rides through verbatim rather than being dropped.
        let exported = preview_game_fn(&script, "search");
        assert!(
            exported.contains("macros::SET_SEARCH_SIZE_EXIST(agent, 0, 7);"),
            "{exported}"
        );
    }

    #[test]
    fn a_grab_written_without_capsule_slots_keeps_its_status() {
        // Verbatim from kirby/SpecialNStart.
        let src = r#"unsafe extern "C" fn game_catch(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::CATCH(agent, 0, Hash40::new("top"), 6.0, 0.0, 6.0, 5.0, *FIGHTER_STATUS_KIND_SWALLOWED, *COLLISION_SITUATION_MASK_GA);
    }
}
"#;
        let script = parse_acmd_script(src);
        let boxes = script.to_hitboxes();
        assert_eq!(boxes.len(), 1, "{boxes:#?}");
        let grab = &boxes[0];
        let extras = grab.catch.as_ref().expect("a CATCH carries its extras");
        assert_eq!(extras.status, "FIGHTER_STATUS_KIND_SWALLOWED");
        assert_eq!(extras.situation, "COLLISION_SITUATION_MASK_GA");

        // The geometry is still read from the slots it really occupies, and the absent capsule
        // is absent rather than being assembled out of the constants that follow it.
        assert_eq!(grab.size, 6.0);
        assert_eq!(
            (grab.offset_x, grab.offset_y, grab.offset_z),
            (0.0, 6.0, 5.0)
        );
        assert_eq!(grab.capsule_end, None);

        let exported = preview_game_fn(&script, "catch");
        assert!(
            exported.contains("*FIGHTER_STATUS_KIND_SWALLOWED"),
            "{exported}"
        );
    }

    #[test]
    fn attack_and_grab_clears_do_not_close_each_other() {
        let src = r#"unsafe extern "C" fn game_catch(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 10.0, 361, 100, 0, 30, 4.0, 0.0, 8.0, 0.0, None, None, None, 1.0, 1.0, 0, 1, false, 0, 0.0, 0, false, false, true, true, false, 0, 0, 0, false, Hash40::new("collision_attr_normal"), 0, 0, 0);
        macros::CATCH(agent, 0, Hash40::new("top"), 5.5, 0.0, 6.4, 10.2, None, None, None, 0, 0);
    }
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
    }
    frame(agent.lua_state_agent, 20.0);
    if macros::is_excute(agent) {
        GrabModule::clear_all(agent.module_accessor);
    }
}
"#;
        let boxes = parse_acmd_script(src).to_hitboxes();
        let attack = boxes.iter().find(|h| h.category == 0).expect("an attack");
        let grab = boxes.iter().find(|h| h.category == 1).expect("a grab");
        assert_eq!(
            attack.active_end, 9,
            "AttackModule::clear_all ends the attack"
        );
        assert_eq!(
            grab.active_end, 19,
            "and leaves the grab running to its own clear"
        );
    }

    #[test]
    fn attack_fp_uses_its_own_slot_table_and_preserves_unknowns() {
        let src = r#"unsafe extern "C" fn game_fp(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK_FP(agent, 2, 3, Hash40::new("top"), 9.5, 45, 100, 2, 30, 4.0, 1.0, 2.0, 3.0, Hash40::new_raw(0x123456), 0.25, 0.5, 0.75, true, false, 0, 3, 4, 0, true, 7, 8, false, 9, false, true, false, false, 10, true, false, *ATTACK_LR_CHECK_POS, false, true, false, false, false, 12);
    }
}
"#;
        let script = parse_acmd_script(src);
        let boxes = script.to_hitboxes();
        assert_eq!(boxes.len(), 1, "{boxes:#?}");
        let fp = &boxes[0];
        assert_eq!(fp.category, crate::data::CAT_ATTACK_FP);
        assert_eq!(fp.func, "ATTACK_FP");
        assert_eq!((fp.id, fp.part, fp.damage, fp.angle), (2, 3, 9.5, 45));
        assert_eq!(fp.ground_or_air, 0, "ATTACK_FP situation is slot 21");
        assert_eq!(fp.collision_attr, "0x123456");
        assert_eq!(fp.lr_check, "ATTACK_LR_CHECK_POS");
        assert_eq!(
            fp.fp.as_ref().unwrap().args.len(),
            crate::data::ATTACK_FP_ARGC
        );
        // Slot 31 is undocumented `rehit`; it must not disappear just because the editor only
        // exposes the established shared fields.
        assert_eq!(fp.fp.as_ref().unwrap().args[31].source(), "10");
        assert_eq!(fp.fp.as_ref().unwrap().args[21].source(), "0");

        let emitted = preview_game_fn(&script, "fp");
        assert!(
            emitted.contains("macros::ATTACK_FP(agent, 2, 3"),
            "{emitted}"
        );
        let round_trip = parse_acmd_script(&emitted).to_hitboxes();
        assert_eq!(round_trip.len(), 1);
        assert_eq!(round_trip[0].category, crate::data::CAT_ATTACK_FP);
        assert_eq!(round_trip[0].damage, fp.damage);
        assert_eq!(round_trip[0].ground_or_air, 0);
        assert_eq!(round_trip[0].fp.as_ref().unwrap().args[21].source(), "0");
        assert_eq!(round_trip[0].fp.as_ref().unwrap().args[31].source(), "10");
        assert_eq!(round_trip[0].collision_attr, "0x123456");
        for slot in [
            2usize, 8, 9, 10, 11, 13, 17, 18, 22, 24, 25, 26, 27, 28, 31, 32, 33, 35, 36, 37, 38,
            39, 40,
        ] {
            assert_eq!(
                round_trip[0].fp.as_ref().unwrap().args[slot],
                fp.fp.as_ref().unwrap().args[slot],
                "unmodeled ATTACK_FP slot {slot}"
            );
        }
    }

    #[test]
    fn id_scoped_attack_clear_round_trips() {
        let source = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        AttackModule::clear(agent.module_accessor, 3, false);
    }
}
"#;
        let script = parse_acmd_script(source);
        assert!(matches!(
            script.stmts.as_slice(),
            [crate::data::AcmdStmt::Frame(frame), crate::data::AcmdStmt::Excute(inner)]
                if *frame == 9.0 && matches!(inner.as_slice(), [crate::data::ExcuteStmt::Clear(3)])
        ));
        let emitted = export_acmd_source(&script, "mario", "test");
        assert!(emitted.contains("AttackModule::clear(agent.module_accessor, 3, false);"));
    }

    #[test]
    fn measured_expression_calls_round_trip_with_raw_neighbors() {
        let source = r#"
unsafe extern "C" fn expression_throwhi(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_LR);
        macros::FT_ATTACK_ABS_CAMERA_QUAKE(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, *CAMERA_QUAKE_KIND_NONE);
        unknown_expression_extension(agent);
    }
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::RUMBLE_HIT(agent, Hash40::new("rbkind_attackm"), 0);
        macros::QUAKE(agent, *CAMERA_QUAKE_KIND_M);
        ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_attackm"), 3, false, *BATTLE_OBJECT_ID_INVALID as u32);
    }
    frame(agent.lua_state_agent, 7.0);
    ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_walk"), 0, true, *BATTLE_OBJECT_ID_INVALID as u32);
}
"#;
        let script = parse_expression_script(source);
        let events = script.to_expression_events();
        assert_eq!(events.len(), 5);
        assert_eq!(events[0].frame, 1);
        assert_eq!(events[1].frame, 6);
        assert_eq!(events[0].call.func(), "FT_ATTACK_ABS_CAMERA_QUAKE");
        assert_eq!(events[1].call.func(), "RUMBLE_HIT");
        assert_eq!(events[2].call.func(), "QUAKE");
        assert_eq!(events[3].call.func(), "ControlModule::set_rumble");
        assert_eq!(events[4].frame, 7);
        assert!(matches!(
            script.stmts.first(),
            Some(AcmdStmt::Excute(commands))
                if matches!(commands.first(), Some(ExcuteStmt::Slope(_)))
                    && matches!(commands.get(2), Some(ExcuteStmt::Raw(line)) if line == "unknown_expression_extension(agent);")
        ));

        let emitted = preview_expression_fn(&script, "throw_hi");
        assert!(emitted.contains("slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_LR);"));
        assert!(emitted.contains(
            "macros::FT_ATTACK_ABS_CAMERA_QUAKE(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, *CAMERA_QUAKE_KIND_NONE);"
        ));
        assert!(emitted.contains(
            "ControlModule::set_rumble(agent.module_accessor, Hash40::new(\"rbkind_walk\"), 0, true, *BATTLE_OBJECT_ID_INVALID as u32);"
        ));
        assert_eq!(
            parse_expression_script(&emitted).to_expression_events(),
            events
        );
        assert!(emitted.contains("unknown_expression_extension(agent);"));
    }

    #[test]
    fn malformed_direct_rumble_shape_stays_raw() {
        let source = r#"
unsafe extern "C" fn expression_bad(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        ControlModule::set_rumble(agent.module_accessor);
    }
}
"#;
        let script = parse_expression_script(source);
        assert!(script.to_expression_events().is_empty());
        let emitted = preview_expression_fn(&script, "bad");
        assert!(emitted.contains("ControlModule::set_rumble(agent.module_accessor);"));
    }

    /// The inputs `sample_project` is built from, so a test can check the preview against
    type SampleEdits = (
        crate::data::AcmdScript,
        Vec<crate::data::EffectCall>,
        crate::data::AcmdScript,
        Vec<crate::mod_project::LiveTweak>,
    );

    fn sample_project() -> ModProject {
        let (script, fx, sfx, tweaks) = sample_edits();
        build_mod_project_full(
            &[("mario".into(), "attack_air_n".into(), script)],
            &[(
                "mario".into(),
                "attack_air_n".into(),
                fx,
                Default::default(),
            )],
            &[("mario".into(), "attack_air_n".into(), sfx)],
            &tweaks,
            "sample_plugin",
        )
    }

    fn sample_edits() -> SampleEdits {
        let atk = crate::data::AttackCall {
            func: "ATTACK".into(),
            id: 0,
            part: 0,
            bone_name: "Top".into(),
            damage: 8.0,
            angle: 361,
            kb_scaling: 100,
            fkb: 0,
            kb_base: 40,
            size: 4.0,
            offset_x: 0.0,
            offset_y: 8.0,
            offset_z: 6.0,
            capsule_end: Some([0.0, 8.0, -2.0]),
            hitlag_mult: 1.0,
            sdi_mult: 1.0,
            setoff_kind: "ATTACK_SETOFF_KIND_ON".into(),
            lr_check: "ATTACK_LR_CHECK_POS".into(),
            is_clang: true,
            is_add_attack: 1,
            hitbox_attr: 0.0,
            ground_or_air: 0,
            is_mtk: false,
            is_shield_disable: false,
            is_reflectable: false,
            is_absorbable: false,
            is_landing_attack: false,
            situation_mask: "COLLISION_SITUATION_MASK_GA".into(),
            category_mask: "COLLISION_CATEGORY_MASK_ALL".into(),
            part_mask: "COLLISION_PART_MASK_ALL".into(),
            no_finish_camera: false,
            collision_attr: "collision_attr_normal".into(),
            sound_level: "ATTACK_SOUND_LEVEL_S".into(),
            sound_attr: "COLLISION_SOUND_ATTR_KICK".into(),
            attack_region: "ATTACK_REGION_KICK".into(),
        };
        let script = crate::data::AcmdScript {
            stmts: vec![
                crate::data::AcmdStmt::Frame(3.0),
                crate::data::AcmdStmt::Excute(vec![crate::data::ExcuteStmt::Attack(atk)]),
                crate::data::AcmdStmt::Wait(2.0),
                crate::data::AcmdStmt::Excute(vec![crate::data::ExcuteStmt::ClearAll]),
            ],
        };
        let fx = vec![
            crate::data::EffectCall {
                effect_name: "sys_attack_arc".into(),
                effect_name_alt: None,
                spawn_func: "EFFECT".into(),
                bone_name: "Top".into(),
                offset: [0.0, 8.0, 0.0],
                rotation: [0.0, 90.0, 0.0],
                scale: 1.2,
                follows_bone: false,
                active_start: 3,
                active_end: 3,
                disabled: false,
                extra_args: Some(
                    vec!["0".into(); 6]
                        .into_iter()
                        .chain(["false".into()])
                        .collect(),
                ),
                flip_axis: None,
                raw_line: None,
                trail_command: None,
                trail_off: None,
                off_fade: None,
                off_detach: None,
                trail_bone2: None,
                rate: None,
                work_int: None,
                camera_offset: None,
                tint: None,
                particle_tint: None,
                alpha: None,
                scale_w: None,
                color: None,
                control: None,
                guard: None,
                leading: Vec::new(),
                trailing: Vec::new(),
            },
            crate::data::EffectCall {
                effect_name: "sys_flash".into(),
                effect_name_alt: None,
                spawn_func: "EFFECT_FOLLOW".into(),
                bone_name: "HaveR".into(),
                offset: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0],
                scale: 0.8,
                follows_bone: true,
                active_start: 5,
                active_end: 20,
                disabled: false,
                extra_args: Some(vec!["true".into()]),
                flip_axis: None,
                raw_line: None,
                trail_command: None,
                trail_off: None,
                off_fade: None,
                off_detach: None,
                trail_bone2: None,
                rate: None,
                work_int: None,
                camera_offset: None,
                tint: None,
                particle_tint: None,
                alpha: None,
                scale_w: None,
                color: None,
                control: None,
                guard: None,
                leading: Vec::new(),
                trailing: Vec::new(),
            },
        ];
        let tweaks = vec![crate::mod_project::LiveTweak {
            effect_name: "sys_attack_arc".into(),
            color: Some([0.2, 0.4, 2.0, 1.0]),
            speed: Some(1.5),
        }];
        // `kirby/TurnDash` verbatim, parsed rather than hand-built, so the sound half of the
        // compile golden is a script the game actually ships. It carries a wrapped call, a
        // two-hash member, and the one member with a trailing non-hash argument — the three
        // shapes the emitter can get wrong.
        let sfx = parse_sound_script(
            r#"unsafe extern "C" fn sound_turndash(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::PLAY_SE(agent, Hash40::new("se_kirby_dash_start"));
        macros::SET_PLAY_INHIVIT(agent, Hash40::new("se_kirby_dash_start"), 20);
    }
    wait(agent.lua_state_agent, 13.0);
    if macros::is_excute(agent) {
        macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_left_m"), Hash40::new("se_kirby_step_right_m"));
    }
}
"#,
        );
        (script, fx, sfx, tweaks)
    }

    #[test]
    fn generated_source_project_compiles() {
        let project = sample_project();
        assert!(project.files.iter().any(|f| f.rel_path == "Cargo.toml"));
        assert!(project.files.iter().any(|f| f.rel_path == "src/lib.rs"));
        assert!(project
            .files
            .iter()
            .any(|f| f.rel_path == "src/mario/acmd.rs"));
        let generated_acmd = project
            .files
            .iter()
            .find(|f| f.rel_path == "src/mario/acmd.rs")
            .map(|f| f.contents.as_str())
            .expect("generated fighter ACMD");
        assert!(
            generated_acmd.contains("Hash40::new(\"top\")")
                && generated_acmd.contains("Hash40::new(\"haver\")")
                && !generated_acmd.contains("Hash40::new(\"Top\")")
                && !generated_acmd.contains("Hash40::new(\"HaveR\")"),
            "display-case skeleton names must export as lowercase ACMD Hash40 names"
        );
        assert!(
            generated_acmd.contains("frame(agent.lua_state_agent, 20.0);")
                && generated_acmd.contains(
                    "macros::EFFECT_OFF_KIND(agent, Hash40::new(\"sys_flash\"), false, true);"
                ),
            "a finite follow-effect end frame must emit EFFECT_OFF_KIND"
        );
        assert!(
            !generated_acmd
                .contains("macros::EFFECT_OFF_KIND(agent, Hash40::new(\"sys_attack_arc\")"),
            "one-shot effects must retain their intrinsic lifetime"
        );

        let root = crate::scratch_dirs::app_storage_root().join("source-golden");
        let proj_root = root.join(&project.name);
        for f in &project.files {
            let dest = proj_root.join(&f.rel_path);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(&dest, &f.contents).unwrap();
        }
        if std::env::var("VISIONARY_COMPILE_GOLDEN").is_err() {
            return;
        }
        let out = std::process::Command::new("cargo")
            .args(["skyline", "build", "--release"])
            .current_dir(&proj_root)
            .output()
            .expect("cargo-skyline not runnable");
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "generated project failed to build in {}:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            proj_root.display(),
            &stdout[stdout.len().saturating_sub(2000)..],
            &stderr[stderr.len().saturating_sub(6000)..],
        );
    }

    fn wrap_effect_fn(body: &str) -> String {
        format!("unsafe extern \"C\" fn effect_test(agent: &mut L2CAgentBase) {{\n    {body}\n}}\n")
    }

    #[test]
    fn test_bug_condition_bare_effect_produces_effect_call() {
        let src = wrap_effect_fn(
            r#"macros::EFFECT(agent, Hash40::new("test_effect"), Hash40::new("top"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, true);"#,
        );
        let script = parse_effect_script(&src);
        let calls = script.to_effect_calls();
        assert!(
            !calls.is_empty(),
            "bare EFFECT should produce at least one EffectCall, got 0"
        );
        assert_eq!(
            calls[0].effect_name, "test_effect",
            "effect_name should be 'test_effect', got '{}'",
            calls[0].effect_name
        );
    }

    #[test]
    fn test_bug_condition_bare_effect_follow_follows_bone() {
        let src = wrap_effect_fn(
            r#"macros::EFFECT_FOLLOW(agent, Hash40::new("follow_eff"), Hash40::new("hip"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, true);"#,
        );
        let script = parse_effect_script(&src);
        let calls = script.to_effect_calls();
        assert!(
            !calls.is_empty(),
            "bare EFFECT_FOLLOW should produce EffectCall"
        );
        assert!(
            calls[0].follows_bone,
            "EFFECT_FOLLOW should set follows_bone=true"
        );
        assert_eq!(calls[0].effect_name, "follow_eff");
    }

    #[test]
    fn rustfmt_multiline_effect_calls_import_as_single_statements() {
        let src = r#"
unsafe extern "C" fn effect_attackairf(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 25.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(
            agent,
            Hash40::new("sys_attack_arc_c"),
            Hash40::new("top"),
            3.8,
            3.0,
            17.4,
            25.0,
            90.0,
            0.0,
            1,
            true,
        );
        macros::LAST_EFFECT_SET_COLOR(agent, 1.0, 0.0, 0.0);
    }
    frame(agent.lua_state_agent, 37.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(
            agent,
            Hash40::new("sys_attack_arc_c"),
            false,
            true,
        );
    }
}
"#;

        let calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(
            calls.len(),
            1,
            "the multiline spawn must be imported: {calls:#?}"
        );
        let call = &calls[0];
        assert_eq!(call.effect_name, "sys_attack_arc_c");
        assert_eq!(call.bone_name, "top");
        assert_eq!(call.offset, [3.8, 3.0, 17.4]);
        assert_eq!(call.rotation, [0.0, 90.0, 25.0]);
        assert_eq!(call.scale, 1.0);
        assert!(call.follows_bone);
        assert_eq!(call.tint, Some([1.0, 0.0, 0.0]));
        assert_eq!(call.active_start, 25);
        assert_eq!(call.active_end, 37);
    }

    #[test]
    fn dumped_effect_variants_and_kill_kind_are_fully_timed() {
        let src = wrap_effect_fn(
            r#"
frame(agent.lua_state_agent, 3.0);
if macros::is_excute(agent) {
    macros::EFFECT_FOLLOW_NO_STOP_FLIP(agent, Hash40::new("moon_explosion"), Hash40::new("moon_explosion"), Hash40::new("top"), 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 0.7, true, *EF_FLIP_YZ);
    macros::EFFECT_FOLLOW_ALPHA(agent, Hash40::new("moon_explosion"), Hash40::new("top"), 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 0.8, true, 0.5);
    macros::EFFECT_ATTR(agent, Hash40::new("trace"), Hash40::new("rot"), 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 1.2, 0, 0, 0, 0, 0, 0, true, *EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE);
    macros::LANDING_EFFECT_FLIP(agent, Hash40::new("smoke_l"), Hash40::new("smoke_r"), Hash40::new("top"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0, 0, 0, 0, 0, 0, false, *EF_FLIP_NONE);
}
frame(agent.lua_state_agent, 18.0);
if macros::is_excute(agent) {
    macros::EFFECT_OFF_KIND(agent, Hash40::new("moon_explosion"), false, true);
}"#,
        );
        let calls = parse_effect_script(&src).to_effect_calls();
        assert_eq!(calls.len(), 4);
        let moon: Vec<_> = calls
            .iter()
            .filter(|call| call.effect_name == "moon_explosion")
            .collect();
        assert_eq!(moon.len(), 2);
        assert!(moon.iter().all(|call| call.active_end == 18));
        assert!(calls
            .iter()
            .any(|call| call.effect_name == "trace" && call.active_end == call.active_start));
        assert!(calls
            .iter()
            .any(|call| call.effect_name == "smoke_l" && call.active_end == call.active_start));
        let landing = calls
            .iter()
            .find(|call| call.spawn_func == "LANDING_EFFECT_FLIP")
            .unwrap();
        assert_eq!(landing.effect_name, "smoke_l");
        assert_eq!(landing.effect_name_alt.as_deref(), Some("smoke_r"));
    }

    #[test]
    fn test_bug_condition_bare_after_image_produces_effect_call() {
        // AFTER_IMAGE4_ON_arg29: args[1]=tex1, args[4]=bone
        let src = wrap_effect_fn(
            r#"macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new("sword_trail"), Hash40::new("tex2"), 4, Hash40::new("sword"), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);"#,
        );
        let script = parse_effect_script(&src);
        let calls = script.to_effect_calls();
        assert!(
            !calls.is_empty(),
            "bare AFTER_IMAGE4_ON_arg29 should produce EffectCall"
        );
        assert_eq!(calls[0].effect_name, "sword_trail");
    }

    #[test]
    fn test_preservation_is_excute_wrapped_effect_unchanged() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("wrapped_eff"), Hash40::new("top"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, true);
    }
}
"#;
        let script = parse_effect_script(src);
        let calls = script.to_effect_calls();
        assert!(
            !calls.is_empty(),
            "is_excute-wrapped EFFECT should produce EffectCall"
        );
        assert_eq!(calls[0].effect_name, "wrapped_eff");
    }

    #[test]
    fn test_preservation_non_effect_bare_line_stays_raw() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_ATTACK);
}
"#;
        let script = parse_effect_script(src);
        let calls = script.to_effect_calls();
        assert!(
            calls.is_empty(),
            "non-EFFECT bare line should produce no EffectCall, got {}",
            calls.len()
        );
    }

    #[test]
    fn test_preservation_for_loop_with_is_excute_unchanged() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    for _ in 0..2 {
        wait(agent.lua_state_agent, 4.0);
        if macros::is_excute(agent) {
            macros::EFFECT(agent, Hash40::new("loop_eff"), Hash40::new("top"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, true);
        }
    }
}
"#;
        let script = parse_effect_script(src);
        let calls = script.to_effect_calls();
        // Loop runs 2 times, each spawning 1 effect at frames 4 and 8
        assert_eq!(
            calls.len(),
            2,
            "for loop should produce 2 EffectCalls, got {}",
            calls.len()
        );
        assert_eq!(calls[0].active_start, 4);
        assert_eq!(calls[1].active_start, 8);
    }

    #[test]
    fn native_last_effect_controls_type_only_exact_direct_one_argument_calls() {
        use crate::data::{EffectControl, EffectMacro, EffectStmt};

        let source = r#"unsafe extern "C" fn effect_native_control(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        EffectModule::set_disable_render_offset_last(agent.module_accessor);
        EffectModule::enable_sync_init_pos_last(agent.module_accessor);
        EffectModule::enable_sync_init_pos_last(agent.module_accessor, 0);
        EffectModule::set_disable_render_offset_last(boma);
        EffectModule::set_disable_render_offset_last(agent.module_accessor, 0);
        EffectModule::set_disable_render_offset_last(agent.module_accessor );
        if custom_effect_guard(agent) {
            EffectModule::enable_sync_init_pos_last(agent.module_accessor);
        }
    }
    EffectModule::set_disable_render_offset_last(agent.module_accessor);
}
"#;
        let parsed = parse_effect_script(source);
        let Some(EffectStmt::Excute(commands)) = parsed.stmts.first() else {
            panic!("expected a single execute statement: {:#?}", parsed.stmts);
        };
        assert!(matches!(
            commands.first(),
            Some(EffectMacro::Control(
                EffectControl::SetDisableRenderOffsetLast
            ))
        ));
        assert!(matches!(
            commands.get(1),
            Some(EffectMacro::Control(EffectControl::EnableSyncInitPosLast))
        ));
        for raw in [
            "EffectModule::enable_sync_init_pos_last(agent.module_accessor, 0);",
            "EffectModule::set_disable_render_offset_last(boma);",
            "EffectModule::set_disable_render_offset_last(agent.module_accessor, 0);",
            "EffectModule::set_disable_render_offset_last(agent.module_accessor );",
            "if custom_effect_guard(agent) {",
            "EffectModule::enable_sync_init_pos_last(agent.module_accessor);",
            "}",
        ] {
            assert!(
                commands
                    .iter()
                    .any(|command| matches!(command, EffectMacro::Raw(line) if line == raw)),
                "unsupported/nested native syntax should remain raw: {raw:?}; {commands:#?}"
            );
        }
        assert!(matches!(
            parsed.stmts.get(1),
            Some(EffectStmt::Raw(line))
                if line == "EffectModule::set_disable_render_offset_last(agent.module_accessor);"
        ));

        let controls = parsed.to_effect_calls();
        assert_eq!(controls.len(), 2);
        assert!(matches!(
            controls[0].control.as_ref(),
            Some(EffectControl::SetDisableRenderOffsetLast)
        ));
        assert!(matches!(
            controls[1].control.as_ref(),
            Some(EffectControl::EnableSyncInitPosLast)
        ));
    }

    #[test]
    fn measured_native_screen_and_camera_controls_are_closed_and_contextual() {
        use crate::data::{EffectControl, EffectMacro, EffectReqScreenPreset, EffectStmt};

        assert_eq!(EffectReqScreenPreset::all().len(), 110);
        let ids: std::collections::BTreeSet<_> = EffectReqScreenPreset::all()
            .iter()
            .map(|preset| preset.id)
            .collect();
        assert_eq!(ids.len(), 110);
        assert_eq!(EffectReqScreenPreset::all()[0].id, "bg_bayonetta_final");
        assert_eq!(EffectReqScreenPreset::all()[0].flags, [false; 3]);
        assert_eq!(
            EffectReqScreenPreset::from_source_parts("demon_ragedrive_bg", [true, false, false])
                .map(|preset| preset.id),
            Some("demon_ragedrive_bg")
        );
        assert!(EffectReqScreenPreset::from_source_parts(
            "bg_bayonetta_final",
            [true, false, false]
        )
        .is_none());
        assert!(EffectReqScreenPreset::from_source_parts("arbitrary_effect", [false; 3]).is_none());
        for preset in EffectReqScreenPreset::all() {
            let command = EffectMacro::Control(EffectControl::ReqScreen {
                preset_id: preset.id.to_string(),
            });
            let emitted = crate::move_nodes::effects::command_lines(&command).unwrap();
            let source = format!(
                "unsafe extern \"C\" fn effect_preset(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {}\n    }}\n}}\n",
                emitted.join("\n")
            );
            let parsed = parse_effect_script(&source);
            let Some(EffectStmt::Excute(commands)) = parsed.stmts.first() else {
                panic!(
                    "preset did not parse inside direct execute context: {}",
                    preset.id
                );
            };
            assert_json_eq!(
                commands.first().unwrap(),
                command,
                "preset did not preserve stable ID/source tuple: {}",
                preset.id
            );
        }

        let source = r#"unsafe extern "C" fn effect_native_screen(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        EffectModule::req_screen(agent.module_accessor, Hash40::new("bg_bayonetta_final"), false, false, false);
        EffectModule::req_screen(agent.module_accessor, Hash40::new("demon_ragedrive_bg"), true, false, false);
        ColorBlendModule::set_disable_camera_depth_influence(agent.module_accessor, true);
        ColorBlendModule::set_disable_camera_depth_influence(agent.module_accessor, false);
        EffectModule::req_screen(agent.module_accessor, Hash40::new_raw(0x1234), false, false, false);
        EffectModule::req_screen(agent.module_accessor, Hash40::new_raw(Hash40::new("bg_demon_final_after_l)")), false, false, false);
        EffectModule::req_screen(agent.module_accessor, Hash40::new("bg_bayonetta_final"), false, false);
        EffectModule::req_screen(agent.module_accessor, Hash40::new("bg_bayonetta_final"), false, true, false);
        ColorBlendModule::set_disable_camera_depth_influence(boma, true);
        ColorBlendModule::set_disable_camera_depth_influence(agent.module_accessor, enabled);
        #[cfg(target_os = "horizon")]
        EffectModule::req_screen(agent.module_accessor, Hash40::new("bg_bayonetta_final"), false, false, false);
        if custom_effect_guard(agent) {
            ColorBlendModule::set_disable_camera_depth_influence(agent.module_accessor, true);
        }
    }

    EffectModule::req_screen(agent.module_accessor, Hash40::new("bg_bayonetta_final"), false, false, false);
}
"#;
        let parsed = parse_effect_script(source);
        let Some(EffectStmt::Excute(commands)) = parsed.stmts.first() else {
            panic!("expected execute node: {:#?}", parsed.stmts);
        };
        assert!(matches!(
            commands.first(),
            Some(EffectMacro::Control(EffectControl::ReqScreen { preset_id }))
                if preset_id == "bg_bayonetta_final"
        ));
        assert!(matches!(
            commands.get(1),
            Some(EffectMacro::Control(EffectControl::ReqScreen { preset_id }))
                if preset_id == "demon_ragedrive_bg"
        ));
        assert!(matches!(
            commands.get(2),
            Some(EffectMacro::Control(
                EffectControl::SetDisableCameraDepthInfluence { value: true }
            ))
        ));
        assert!(matches!(
            commands.get(3),
            Some(EffectMacro::Control(
                EffectControl::SetDisableCameraDepthInfluence { value: false }
            ))
        ));
        for raw in [
            "EffectModule::req_screen(agent.module_accessor, Hash40::new_raw(0x1234), false, false, false);",
            "EffectModule::req_screen(agent.module_accessor, Hash40::new_raw(Hash40::new(\"bg_demon_final_after_l)\")), false, false, false);",
            "EffectModule::req_screen(agent.module_accessor, Hash40::new(\"bg_bayonetta_final\"), false, false);",
            "EffectModule::req_screen(agent.module_accessor, Hash40::new(\"bg_bayonetta_final\"), false, true, false);",
            "ColorBlendModule::set_disable_camera_depth_influence(boma, true);",
            "ColorBlendModule::set_disable_camera_depth_influence(agent.module_accessor, enabled);",
            "#[cfg(target_os = \"horizon\")]",
            "EffectModule::req_screen(agent.module_accessor, Hash40::new(\"bg_bayonetta_final\"), false, false, false);",
            "if custom_effect_guard(agent) {",
            "ColorBlendModule::set_disable_camera_depth_influence(agent.module_accessor, true);",
            "}",
        ] {
            assert!(
                commands.iter().any(|command| matches!(command, EffectMacro::Raw(line) if line == raw)),
                "unsupported context/shape should remain Raw: {raw:?}; {commands:#?}"
            );
        }
        assert!(matches!(
            parsed.stmts.get(1),
            Some(EffectStmt::Raw(line)) if line.contains("EffectModule::req_screen(")
        ));
        let wrong_track = r#"unsafe extern "C" fn game_native_screen(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        EffectModule::req_screen(agent.module_accessor, Hash40::new("bg_bayonetta_final"), false, false, false);
        ColorBlendModule::set_disable_camera_depth_influence(agent.module_accessor, true);
    }
}
"#;
        let game = parse_acmd_script(wrong_track);
        assert!(matches!(
            game.stmts.first(),
            Some(crate::data::AcmdStmt::Excute(commands))
                if commands.len() == 2 && commands.iter().all(|command| matches!(command, crate::data::ExcuteStmt::Raw(line) if line.contains("req_screen(") || line.contains("set_disable_camera_depth_influence(")))
        ));

        let json = serde_json::to_value(&parsed).unwrap();
        let restored: crate::data::EffectScript = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), json);
    }

    #[test]
    fn measured_off_kind_work_presets_are_closed_owner_scoped_and_roundtrip() {
        use crate::data::{EffectControl, EffectMacro, EffectOffKindWorkPreset, EffectStmt};
        let presets = EffectOffKindWorkPreset::all();
        assert_eq!(presets.len(), 13);
        let ids = presets
            .iter()
            .map(|preset| preset.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), presets.len());

        for preset in presets {
            let entry = crate::gimmicks::registry::resolve(
                preset.fighter,
                preset.work_symbol,
                crate::gimmicks::registry::IdentifierKind::WorkInt,
            )
            .unwrap_or_else(|error| panic!("{} registry slot mismatch: {error}", preset.id));
            assert!(
                entry.group.is_empty(),
                "{} is not instance-scoped",
                preset.id
            );
            assert_eq!(
                EffectOffKindWorkPreset::from_source_parts(
                    preset.fighter,
                    preset.work_symbol,
                    preset.flags,
                )
                .map(|row| row.id),
                Some(preset.id)
            );
            let command = EffectMacro::Control(EffectControl::OffKindWork {
                preset_id: preset.id.to_owned(),
            });
            let lines = crate::move_nodes::effects::command_lines(&command).unwrap();
            assert_eq!(
                lines,
                [format!(
                    "macros::EFFECT_OFF_KIND_WORK(agent, *{}, {}, {});",
                    preset.work_symbol, preset.flags[0], preset.flags[1]
                )]
            );
            assert!(crate::move_nodes::effects::command_lines_for_fighter(
                Some(preset.fighter),
                &command
            )
            .is_ok());
            let ownerless =
                crate::move_nodes::effects::command_lines_for_fighter(None, &command).unwrap_err();
            assert!(ownerless.contains("<ownerless>") && ownerless.contains(preset.id));
            let wrong_owner =
                crate::move_nodes::effects::command_lines_for_fighter(Some("mario"), &command)
                    .unwrap_err();
            assert!(wrong_owner.contains("mario"));
            assert!(wrong_owner.contains(preset.id));
            assert!(wrong_owner.contains(preset.fighter));
            assert!(wrong_owner.contains(preset.work_symbol));

            let source = format!(
                "unsafe extern \"C\" fn effect_preset(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {}\n    }}\n}}\n",
                lines.join("\n")
            );
            let parsed = parse_effect_script(&source);
            let Some(EffectStmt::Excute(commands)) = parsed.stmts.first() else {
                panic!(
                    "preset did not parse in direct execute context: {}",
                    preset.id
                );
            };
            assert_json_eq!(
                commands.first().unwrap(),
                &command,
                "preset did not parse: {}",
                preset.id
            );
            let imported = crate::move_nodes::effects::import_for_fighter(preset.fighter, &parsed);
            let graph = crate::move_nodes::model::MoveGraph {
                tracks: vec![imported],
                ..Default::default()
            };
            assert!(crate::move_nodes::live_support::source_only_tracks(&graph)
                .any(|track| track == crate::move_nodes::model::Track::Effect));

            let legacy_calls = parsed.to_effect_calls();
            assert_eq!(
                legacy_calls.len(),
                1,
                "legacy conversion lost {}",
                preset.id
            );
            let legacy_source = preview_effect_fn(
                &legacy_calls,
                "preset",
                &[],
                &std::collections::BTreeMap::new(),
            );
            assert!(
                legacy_source.contains(&lines[0]),
                "legacy emitter lost {}",
                preset.id
            );
            let legacy_roundtrip = parse_effect_script(&legacy_source).to_effect_calls();
            assert_eq!(legacy_roundtrip.len(), 1);
            assert_json_eq!(
                legacy_roundtrip[0].control.as_ref().unwrap(),
                &EffectControl::OffKindWork {
                    preset_id: preset.id.to_owned()
                },
                "legacy source roundtrip changed {}",
                preset.id
            );
            let json = serde_json::to_value(&command).unwrap();
            assert_eq!(
                serde_json::to_value(serde_json::from_value::<EffectMacro>(json.clone()).unwrap())
                    .unwrap(),
                json
            );
        }
        assert!(EffectOffKindWorkPreset::by_id("not_a_preset").is_none());
        assert!(EffectOffKindWorkPreset::from_source_parts(
            "cloud",
            "FIGHTER_BRAVE_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE",
            [false, true],
        )
        .is_none());
    }

    #[test]
    fn off_kind_work_parser_fails_closed_on_wrong_shapes_and_contexts() {
        use crate::data::{EffectControl, EffectMacro, EffectStmt};
        let valid = "macros::EFFECT_OFF_KIND_WORK(agent, *FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE, false, true);";
        for malformed in [
            valid.replace("agent,", "boma,"),
            valid.replace(
                "*FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE",
                "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE",
            ),
            valid.replace("false, true", "enabled, true"),
            valid.replace("false, true", "false, maybe"),
            valid.replace(", true);", ");"),
            valid.replace(", false, true);", ", false, true, false);"),
            valid.replace(
                "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE",
                "FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_UNKNOWN_SLOT",
            ),
            valid.replace("macros::", "custom::"),
        ] {
            let source = format!(
                "unsafe extern \"C\" fn effect_bad(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {malformed}\n    }}\n}}\n"
            );
            let parsed = parse_effect_script(&source);
            assert!(
                matches!(parsed.stmts.first(), Some(EffectStmt::Excute(commands)) if commands.iter().any(|command| matches!(command, EffectMacro::Raw(_)))),
                "malformed source was admitted: {malformed}"
            );
            assert_eq!(
                conserved_off_kind_count(&parsed.stmts),
                source.matches("macros::EFFECT_OFF_KIND_WORK(").count(),
                "malformed call was not preserved: {malformed}"
            );
        }

        let nested_source = format!(
            "unsafe extern \"C\" fn effect_nested(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        if custom_effect_guard(agent) {{\n            {valid}\n        }}\n    }}\n}}\n"
        );
        let nested = parse_effect_script(&nested_source);
        fn contains_typed_off_kind(stmts: &[EffectStmt]) -> bool {
            stmts.iter().any(|stmt| match stmt {
                EffectStmt::Excute(commands) => commands.iter().any(|command| {
                    matches!(
                        command,
                        EffectMacro::Control(EffectControl::OffKindWork { .. })
                    )
                }),
                EffectStmt::Loop { body, .. } | EffectStmt::Cond { body, .. } => {
                    contains_typed_off_kind(body)
                }
                _ => false,
            })
        }
        fn conserved_off_kind_count(stmts: &[EffectStmt]) -> usize {
            let mut count = 0;
            for stmt in stmts {
                match stmt {
                    EffectStmt::Excute(commands) => {
                        for command in commands {
                            match command {
                                EffectMacro::Control(EffectControl::OffKindWork { .. }) => {
                                    count += 1
                                }
                                EffectMacro::Raw(raw) => {
                                    count += raw.matches("macros::EFFECT_OFF_KIND_WORK(").count()
                                }
                                _ => {}
                            }
                        }
                    }
                    EffectStmt::Raw(raw) => {
                        count += raw.matches("macros::EFFECT_OFF_KIND_WORK(").count()
                    }
                    EffectStmt::Loop { body, .. } | EffectStmt::Cond { body, .. } => {
                        count += conserved_off_kind_count(body)
                    }
                    _ => {}
                }
            }
            count
        }
        assert!(
            !contains_typed_off_kind(&nested.stmts),
            "nested off-kind control was typed"
        );
        assert_eq!(
            conserved_off_kind_count(&nested.stmts),
            nested_source
                .matches("macros::EFFECT_OFF_KIND_WORK(")
                .count(),
            "nested call was not preserved"
        );

        let multiline = valid.replace(
            "(agent, *FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE, false, true)",
            "(agent,\n            *FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE,\n            false, true)",
        );
        let attributed = format!("#[cfg(feature = \"fixture\")]\n{valid}");
        let wrong_track = format!(
            "unsafe extern \"C\" fn game_wrong_track(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {valid}\n    }}\n}}\n"
        );
        let bare = format!(
            "unsafe extern \"C\" fn effect_bare(agent: &mut L2CAgentBase) {{\n    {valid}\n}}\n"
        );
        for (source, effect_domain) in [
            (format!("unsafe extern \"C\" fn effect_multiline(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {multiline}\n    }}\n}}\n"), true),
            (format!("unsafe extern \"C\" fn effect_attributed(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {attributed}\n    }}\n}}\n"), true),
            (wrong_track, false),
            (bare, true),
        ] {
            if effect_domain {
                let parsed = parse_effect_script(&source);
                assert!(!contains_typed_off_kind(&parsed.stmts), "unsupported placement was typed: {source}");
                assert_eq!(
                    conserved_off_kind_count(&parsed.stmts),
                    source.matches("macros::EFFECT_OFF_KIND_WORK(").count(),
                    "unsupported effect placement was not preserved: {source}"
                );
            } else {
                let parsed = parse_acmd_script(&source);
                assert!(matches!(parsed.stmts.first(), Some(crate::data::AcmdStmt::Excute(commands)) if commands.iter().any(|command| matches!(command, crate::data::ExcuteStmt::Raw(raw) if raw.contains("EFFECT_OFF_KIND_WORK")))));
                let preserved = parsed
                    .stmts
                    .iter()
                    .filter_map(|stmt| match stmt {
                        crate::data::AcmdStmt::Excute(commands) => Some(commands.iter().filter(|command| matches!(command, crate::data::ExcuteStmt::Raw(raw) if raw.contains("EFFECT_OFF_KIND_WORK"))).count()),
                        _ => None,
                    })
                    .sum::<usize>();
                assert_eq!(preserved, 1, "wrong-track call was not preserved: {source}");
            }
        }

        let malformed_neighbor = format!(
            "unsafe extern \"C\" fn effect_neighbor(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {valid}\n        macros::EFFECT_OFF_KIND_WORK(agent, *FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_UNKNOWN_SLOT, false, true);\n    }}\n}}\n"
        );
        let parsed = parse_effect_script(&malformed_neighbor);
        let Some(EffectStmt::Excute(commands)) = parsed.stmts.first() else {
            panic!()
        };
        assert!(matches!(
            commands.first(),
            Some(EffectMacro::Control(EffectControl::OffKindWork { .. }))
        ));
        assert!(
            matches!(commands.get(1), Some(EffectMacro::Raw(raw)) if raw.contains("UNKNOWN_SLOT"))
        );
        let lexical = malformed_neighbor
            .matches("macros::EFFECT_OFF_KIND_WORK(")
            .count();
        assert_eq!(
            commands
                .iter()
                .map(|command| match command {
                    EffectMacro::Control(EffectControl::OffKindWork { .. }) => 1,
                    EffectMacro::Raw(raw) => raw.matches("macros::EFFECT_OFF_KIND_WORK(").count(),
                    _ => 0,
                })
                .sum::<usize>(),
            lexical
        );
        // A syntactically invalid sibling invalidates the complete direct-statement guard.
        // Do not partly lift an otherwise well-shaped native control out of that body.
        let invalid_body = format!(
            "unsafe extern \"C\" fn effect_invalid_body(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        let broken = ;\n        {valid}\n    }}\n}}\n"
        );
        let invalid = parse_effect_script(&invalid_body);
        assert!(!contains_typed_off_kind(&invalid.stmts));
        assert_eq!(conserved_off_kind_count(&invalid.stmts), 1);
    }

    #[test]
    fn spawn_macros_survive_a_source_round_trip() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("sys_hit_l"), Hash40::new("sys_hit_r"), Hash40::new("haver"), 1.0, 2.0, 3.0, 0.0, 90.0, 45.0, 1.5, true, *EF_FLIP_YZ);
    }
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_NO_STOP(agent, Hash40::new("sys_smoke"), Hash40::new("top"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].spawn_func, "EFFECT_FOLLOW_FLIP");
        assert_eq!(calls[0].effect_name_alt.as_deref(), Some("sys_hit_r"));
        assert_eq!(
            calls[0].extra_args.as_deref(),
            Some(&["true".to_string(), "*EF_FLIP_YZ".to_string()][..])
        );
        assert_eq!(calls[0].flip_axis.as_deref(), Some("*EF_FLIP_YZ"));
        assert_eq!(calls[1].spawn_func, "EFFECT_FOLLOW_NO_STOP");

        let (_, emitted) =
            emit_effect_move_fn(&calls, "test", &Default::default(), &Default::default());
        assert!(
            emitted.contains(
                "macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new(\"sys_hit_l\"), Hash40::new(\"sys_hit_r\"), Hash40::new(\"haver\"), 1.0, 2.0, 3.0, 0.0, 90.0, 45.0, 1.5, true, *EF_FLIP_YZ);"
            ),
            "flipped follow spawn must come back out whole:\n{emitted}"
        );
        assert!(
            emitted.contains("macros::EFFECT_FOLLOW_NO_STOP(agent, Hash40::new(\"sys_smoke\")"),
            "a NO_STOP follow must not be swapped for plain EFFECT_FOLLOW:\n{emitted}"
        );
    }

    #[test]
    fn an_effect_command_swap_exports_the_target_family_and_abi() {
        let mut calls = sample_edits().1;
        let call = &mut calls[0];
        call.spawn_func = "EFFECT_FOLLOW_FLIP_ALPHA".into();
        call.effect_name_alt = Some("sys_attack_arc_alt".into());
        call.follows_bone = true;
        call.active_end = crate::data::OPEN_ENDED_EFFECT_FRAME;
        call.extra_args = Some(vec!["false".into(), "0".into(), "1.0".into()]);
        call.flip_axis = Some("*EF_FLIP_XY".into());

        let (_, emitted) = emit_effect_move_fn(
            &calls,
            "attack_air_n",
            &Default::default(),
            &Default::default(),
        );
        assert!(
            emitted.contains(
                "macros::EFFECT_FOLLOW_FLIP_ALPHA(agent, Hash40::new(\"sys_attack_arc\"), Hash40::new(\"sys_attack_arc_alt\"), Hash40::new(\"top\")"
            ),
            "the selected command and both graphic slots must be exported:\n{emitted}"
        );
        assert!(
            emitted.contains(", 1.2, false, *EF_FLIP_XY, 1.0);"),
            "the target command must receive its own bool/axis/alpha tail:\n{emitted}"
        );

        let reparsed = parse_effect_script(&emitted).to_effect_calls();
        assert_eq!(reparsed[0].spawn_func, "EFFECT_FOLLOW_FLIP_ALPHA");
        assert_eq!(
            reparsed[0].effect_name_alt.as_deref(),
            Some("sys_attack_arc_alt")
        );
        assert_eq!(
            reparsed[0].extra_args.as_deref(),
            Some(&["false", "*EF_FLIP_XY", "1.0"].map(str::to_string)[..])
        );
        assert_eq!(reparsed[0].flip_axis.as_deref(), Some("*EF_FLIP_XY"));
    }

    #[test]
    fn a_rate_binds_to_the_spawn_above_it_and_survives_an_export() {
        let src = r#"
unsafe extern "C" fn effect_downattackd(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 15.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_atk_smoke"), Hash40::new("top"), 0, 0, 0, 0, 180, 0, 0.4, 0, 0, 0, 0, 0, 0, false);
        macros::LAST_EFFECT_SET_RATE(agent, 2);
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("sys_attack_line"), Hash40::new("sys_attack_line"), Hash40::new("top"), -8, 4, 2.5, 0, 160, 0, 1.1, true, *EF_FLIP_YZ);
        macros::LAST_EFFECT_SET_RATE(agent, 1.5);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].effect_name, "sys_atk_smoke");
        assert_eq!(calls[0].rate, Some(2.0));
        assert_eq!(calls[1].effect_name, "sys_attack_line");
        assert_eq!(calls[1].rate, Some(1.5));

        let (_, emitted) = emit_effect_move_fn(
            &calls,
            "downattackd",
            &Default::default(),
            &Default::default(),
        );
        // `2`, not `2.0`: the macro is generic over `ToF32`, and this is the spelling the
        // archive uses and the source write-back produces.
        assert!(
            emitted.contains("macros::LAST_EFFECT_SET_RATE(agent, 2);"),
            "a whole rate must not sprout a decimal point:\n{emitted}"
        );
        assert!(
            emitted.contains("macros::LAST_EFFECT_SET_RATE(agent, 1.5);"),
            "the second spawn's own rate must be written too:\n{emitted}"
        );
        let round_tripped = parse_effect_script(&emitted).to_effect_calls();
        assert_eq!(
            round_tripped.iter().map(|c| c.rate).collect::<Vec<_>>(),
            vec![Some(2.0), Some(1.5)],
            "reading the export back must find the same rates on the same spawns"
        );
    }

    #[test]
    fn camera_flat_offset_binds_to_its_spawn_and_survives_an_export() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_hit"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, -5);
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_hit_2"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, 0.4);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(
            calls.iter().map(|c| c.camera_offset).collect::<Vec<_>>(),
            vec![Some(-5.0), Some(0.4)]
        );

        let (_, emitted) =
            emit_effect_move_fn(&calls, "test", &Default::default(), &Default::default());
        assert!(
            emitted.contains("macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, -5);"),
            "the first camera-flat offset must be exported:\n{emitted}"
        );
        assert!(
            emitted.contains("macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, 0.4);"),
            "the second camera-flat offset must be exported:\n{emitted}"
        );
        let round_tripped = parse_effect_script(&emitted).to_effect_calls();
        assert_eq!(
            round_tripped
                .iter()
                .map(|c| c.camera_offset)
                .collect::<Vec<_>>(),
            vec![Some(-5.0), Some(0.4)]
        );
    }

    #[test]
    fn particle_colour_is_typed_separately_and_malformed_calls_are_carried() {
        let source = r#"unsafe extern "C" fn effect_particle(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 7.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_hit"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::LAST_PARTICLE_SET_COLOR(agent, 0.1, 1.2, 0.3);
    }
}
"#;
        let calls = parse_effect_script(source).to_effect_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tint, None);
        assert_eq!(calls[0].particle_tint, Some([0.1, 1.2, 0.3]));
        let (_, emitted) =
            emit_effect_move_fn(&calls, "particle", &Default::default(), &Default::default());
        assert!(emitted.contains("macros::LAST_PARTICLE_SET_COLOR(agent, 0.1, 1.2, 0.3);"));

        let malformed = source.replace(
            "macros::LAST_PARTICLE_SET_COLOR(agent, 0.1, 1.2, 0.3);",
            "macros::LAST_PARTICLE_SET_COLOR(agent);",
        );
        let (calls, residue) = parse_effect_script(&malformed).to_effect_calls_and_residue();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].particle_tint, None);
        assert!(calls[0]
            .trailing
            .iter()
            .any(|line| line.contains("LAST_PARTICLE_SET_COLOR(agent);")));
        assert!(residue.is_empty());

        let over_arity = source.replace(
            "macros::LAST_PARTICLE_SET_COLOR(agent, 0.1, 1.2, 0.3);",
            "macros::LAST_PARTICLE_SET_COLOR(agent, 0.1, 1.2, 0.3, 4.0);",
        );
        let (calls, residue) = parse_effect_script(&over_arity).to_effect_calls_and_residue();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].particle_tint, None);
        assert!(calls[0]
            .trailing
            .iter()
            .any(|line| line.contains("LAST_PARTICLE_SET_COLOR(agent, 0.1, 1.2, 0.3, 4.0);")));
        assert!(residue.is_empty());
    }

    #[test]
    fn the_work_int_effect_line_is_typed_and_exported_through_a_local_helper() {
        let src = r#"unsafe extern "C" fn effect_tornadostart(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("metaknight_tornado"), Hash40::new("trans"), 0, 0, 0, 0, 0, 0, 1, false);
        macros::LAST_EFFECT_SET_WORK_INT(agent, *FIGHTER_METAKNIGHT_STATUS_SPECIAL_N_SPIN_WORK_INT_EFFECT_HANDLE);
    }
}
"#;
        let source = parse_effect_script(src);
        let (calls, residue) = source.to_effect_calls_and_residue();
        assert_eq!(calls.len(), 1);
        assert!(
            residue.is_empty(),
            "the line shares the spawn's frame: {residue:?}"
        );
        assert!(calls[0].trailing.is_empty());
        assert_eq!(
            calls[0].work_int.as_deref(),
            Some("FIGHTER_METAKNIGHT_STATUS_SPECIAL_N_SPIN_WORK_INT_EFFECT_HANDLE")
        );

        let emitted = preview_effect_fn(&calls, "tornadostart", &[], &residue);
        assert_eq!(
            emitted
                .matches("visionary_last_effect_set_work_int(agent,")
                .count(),
            1,
            "the typed Work ID line must reach generated text exactly once:\n{emitted}"
        );
        assert!(unexportable_effect_lines(&source).is_empty());
        assert!(emitted.contains(
            "unsafe fn visionary_last_effect_set_work_int(agent: &mut L2CAgentBase, work: i32)"
        ));
        assert!(emitted.contains(
            "visionary_last_effect_set_work_int(agent, *FIGHTER_METAKNIGHT_STATUS_SPECIAL_N_SPIN_WORK_INT_EFFECT_HANDLE);"
        ));
        let reparsed = parse_effect_script(&emitted).to_effect_calls();
        assert_eq!(reparsed[0].work_int, calls[0].work_int);

        let mut report = crate::acmd_verify::Report::default();
        crate::acmd_verify::verify_effect_move(
            "kirby / tornadostart",
            &calls,
            &emitted,
            &[],
            Some(&unexportable_effect_lines(&source)),
            &residue,
            &mut report,
        );
        assert!(!report.has_blockers(), "{report:?}");
    }

    #[test]
    fn bare_c7_effect_lines_reach_frame_residue_without_becoming_typed_calls() {
        let src = r#"unsafe extern "C" fn effect_c7(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 7.0);
    macros::LAST_PARTICLE_SET_COLOR(agent, 1, 2, 3);
    macros::LAST_EFFECT_SET_WORK_INT(agent, *WORK_INT);
    macros::LAST_EFFECT_SET_SCALE_W(agent, 1, 2, 3);
    macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, 4);
}
"#;
        let source = parse_effect_script(src);
        let (calls, residue) = source.to_effect_calls_and_residue();
        assert!(
            calls.is_empty(),
            "standalone C7 lines are not effect spawns"
        );
        let lines = residue.get(&7).expect("bare calls keep their frame");
        for name in [
            "LAST_PARTICLE_SET_COLOR",
            "LAST_EFFECT_SET_WORK_INT",
            "LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT",
        ] {
            assert!(
                lines.iter().any(|line| line.contains(name)),
                "{name} was not carried: {lines:?}"
            );
        }
        assert!(
            lines
                .iter()
                .any(|line| line.contains("visionary_last_effect_set_scale_w")),
            "LAST_EFFECT_SET_SCALE_W was not carried through its dynamic helper: {lines:?}"
        );
        let emitted = preview_effect_fn(&calls, "c7", &[], &residue);
        for name in [
            "LAST_PARTICLE_SET_COLOR",
            "LAST_EFFECT_SET_WORK_INT",
            "LAST_EFFECT_SET_SCALE_W",
            "LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT",
        ] {
            assert_eq!(
                emitted.matches(name).count(),
                1,
                "{name} must be emitted once:\n{emitted}"
            );
        }
        assert_eq!(
            emitted.matches("LAST_EFFECT_SET_SCALE_W").count(),
            1,
            "the dynamic scale-W primitive must be emitted once:\n{emitted}"
        );
    }

    #[test]
    fn the_dumped_particle_color_stack_form_is_carried_and_marked_unbuildable() {
        let src = r#"unsafe extern "C" fn effect_set_ink_color(agent: &mut L2CAgentBase) {
    WorkModule::get_float(agent.module_accessor, *FIGHTER_KIRBY_INSTANCE_WORK_ID_FLOAT_INKLING_SPECIAL_N_INK_R);
    WorkModule::get_float(agent.module_accessor, -1165490952, *FIGHTER_KIRBY_INSTANCE_WORK_ID_FLOAT_INKLING_SPECIAL_N_INK_G);
    WorkModule::get_float(agent.module_accessor, -1165490952, *FIGHTER_KIRBY_INSTANCE_WORK_ID_FLOAT_INKLING_SPECIAL_N_INK_B);
    if macros::is_excute(agent) {
        macros::LAST_PARTICLE_SET_COLOR(agent);
    }
}
"#;
        let source = parse_effect_script(src);
        let (calls, residue) = source.to_effect_calls_and_residue();
        assert!(calls.is_empty(), "the dump has no typed effect spawn");
        let lines = residue
            .get(&1)
            .expect("the particle call still has a frame");
        assert!(
            lines
                .iter()
                .any(|line| line == "macros::LAST_PARTICLE_SET_COLOR(agent);"),
            "the stack-form macro was dropped: {lines:?}"
        );
        let emitted = preview_effect_fn(&calls, "set_ink_color", &[], &residue);
        assert_eq!(
            emitted
                .matches("macros::LAST_PARTICLE_SET_COLOR(agent);")
                .count(),
            1,
            "the observed particle call must reach generated text once:\n{emitted}"
        );
        let lost = unexportable_effect_lines(&source);
        assert!(
            lost.iter()
                .any(|line| line.contains("WorkModule::get_float")),
            "the stack inputs remain an explicit unmodelled loss: {lost:?}"
        );

        let mut report = crate::acmd_verify::Report::default();
        crate::acmd_verify::verify_effect_move(
            "kirby / set_ink_color",
            &calls,
            &emitted,
            &[],
            Some(&lost),
            &residue,
            &mut report,
        );
        assert!(
            report
                .blockers()
                .any(|finding| finding.message.contains("zero-argument")),
            "the wrapper mismatch must be explicit: {report:?}"
        );
    }

    #[test]
    fn c4_controls_are_typed_points_and_survive_effect_export() {
        let src = r#"unsafe extern "C" fn effect_c4(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_smoke"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::EFFECT_DETACH_KIND(agent, Hash40::new("sys_smoke"), 0);
        macros::EFFECT_DETACH_KIND_WORK(agent, *WORK_INT, 0);
        macros::ENABLE_AREA(agent, 2);
        macros::UNABLE_AREA(agent, 2);
    }
    frame(agent.lua_state_agent, 13.0);
    macros::ENABLE_AREA(agent, 2);
        }
"#;
        let source = parse_effect_script(src);
        let (calls, residue) = source.to_effect_calls_and_residue();
        assert_eq!(calls.len(), 6);
        assert_eq!(
            calls.iter().map(|c| c.active_start).collect::<Vec<_>>(),
            [9, 9, 9, 9, 9, 13]
        );
        assert_eq!(calls[0].effect_name, "sys_smoke");
        assert_eq!(
            calls[0].active_end,
            crate::data::OPEN_ENDED_EFFECT_FRAME,
            "detach must not end the follow effect"
        );
        assert!(matches!(
            calls[1].control,
            Some(crate::data::EffectControl::DetachKind { ref effect_name, unk: 0 })
                if effect_name == "sys_smoke"
        ));
        assert!(matches!(
            calls[2].control,
            Some(crate::data::EffectControl::DetachKindWork { ref work, unk: 0 })
                if work == "WORK_INT"
        ));
        assert!(matches!(
            calls[3].control,
            Some(crate::data::EffectControl::EnableArea { ref kind }) if kind == "2"
        ));
        assert!(matches!(
            calls[4].control,
            Some(crate::data::EffectControl::UnableArea { ref kind }) if kind == "2"
        ));
        assert!(matches!(
            calls[5].control,
            Some(crate::data::EffectControl::EnableArea { ref kind }) if kind == "2"
        ));
        assert!(
            residue.is_empty(),
            "typed controls no longer need residue: {residue:?}"
        );
        let lost = unexportable_effect_lines(&source);
        assert!(
            !lost.iter().any(|line| {
                [
                    "EFFECT_DETACH_KIND",
                    "EFFECT_DETACH_KIND_WORK",
                    "ENABLE_AREA",
                    "UNABLE_AREA",
                ]
                .iter()
                .any(|name| line.contains(name))
            }),
            "typed C4 lines must not be reported as deleted: {lost:?}"
        );

        let emitted = preview_effect_fn(&calls, "c4", &[], &residue);
        assert_eq!(
            emitted.matches("EFFECT_DETACH_KIND(agent,").count(),
            1,
            "detach-by-kind must be emitted once:\n{emitted}"
        );
        assert_eq!(
            emitted.matches("EFFECT_DETACH_KIND_WORK(agent,").count(),
            1,
            "detach-by-work must be emitted once:\n{emitted}"
        );
        assert_eq!(
            emitted.matches("ENABLE_AREA(agent, 2);").count(),
            2,
            "both area toggles must keep their separate frames:\n{emitted}"
        );
        assert_eq!(
            emitted.matches("UNABLE_AREA(agent, 2);").count(),
            1,
            "the disable must be emitted once:\n{emitted}"
        );

        let mut report = crate::acmd_verify::Report::default();
        crate::acmd_verify::verify_effect_move(
            "test / c4",
            &calls,
            &emitted,
            &[],
            Some(&lost),
            &residue,
            &mut report,
        );
        assert!(
            !report.has_blockers(),
            "opaque C4 lines are not blockers: {report:?}"
        );
    }

    #[test]
    fn a_tint_and_an_opacity_bind_to_the_spawn_above_and_survive_an_export() {
        let src = r#"
unsafe extern "C" fn effect_attackairhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("kirby_attack_arc"), Hash40::new("kirby_attack_arc"), Hash40::new("top"), -3, 7, 0, 0, 90, 90, 1, true, *EF_FLIP_YZ);
        macros::LAST_EFFECT_SET_COLOR(agent, 0.25, 1.3, 2.5);
        macros::EFFECT(agent, Hash40::new("sys_attack_impact"), Hash40::new("top"), 13, 6, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0, 360, false);
        macros::LAST_EFFECT_SET_ALPHA(agent, 0.7);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tint, Some([0.25, 1.3, 2.5]));
        // Each modifier lands on its own spawn and nowhere else. Sharing one field, or reaching
        // past the second spawn, would put the arc's colour on the impact or vice versa.
        assert_eq!(calls[0].alpha, None);
        assert_eq!(calls[1].tint, None);
        assert_eq!(calls[1].alpha, Some(0.7));

        let (_, emitted) = emit_effect_move_fn(
            &calls,
            "attackairhi",
            &Default::default(),
            &Default::default(),
        );
        assert!(
            emitted.contains("macros::LAST_EFFECT_SET_COLOR(agent, 0.25, 1.3, 2.5);"),
            "the script's own tint must be written back out:\n{emitted}"
        );
        assert!(
            emitted.contains("macros::LAST_EFFECT_SET_ALPHA(agent, 0.7);"),
            "the script's own opacity must be written back out:\n{emitted}"
        );
        let round_tripped = parse_effect_script(&emitted).to_effect_calls();
        assert_eq!(
            round_tripped.iter().map(|c| c.tint).collect::<Vec<_>>(),
            vec![Some([0.25, 1.3, 2.5]), None]
        );
        assert_eq!(
            round_tripped.iter().map(|c| c.alpha).collect::<Vec<_>>(),
            vec![None, Some(0.7)]
        );
    }

    #[test]
    fn a_tint_cut_off_from_its_spawn_by_a_branch_is_carried_with_its_costume_check() {
        let src = r#"
unsafe extern "C" fn effect_specialhicommand(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_ALPHA(agent, Hash40::new("dolly_roll_l_color1"), Hash40::new("throw"), 0, 2.5, 0, 0, 0, 0, 1, true, 0.8);
    }
    if(0x2508e0(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 0)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.146, 0.205, 0.333);
        }
    }
}
"#;
        let (calls, residue) = parse_effect_script(src).to_effect_calls_and_residue();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].tint, None,
            "a costume-gated tint must not be exported onto every costume"
        );
        assert!(
            residue.is_empty(),
            "the line rides on the spawn above it, so no frame owns it: {residue:?}"
        );
        assert_eq!(
            calls[0].trailing,
            vec![
                "if(0x2508e0(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 0)){",
                "if macros::is_excute(agent) {",
                "macros::LAST_EFFECT_SET_COLOR(agent, 0.146, 0.205, 0.333);",
                "}",
                "}",
            ],
            "the line rides on the spawn it followed, keeping its costume check"
        );

        let (_, emitted) = emit_effect_move_fn(
            &calls,
            "specialhicommand",
            &Default::default(),
            &Default::default(),
        );
        // Both halves matter and they pull against each other. The tint must come back — that
        // is C6 — but only inside the costume check it was written in. Emitting the bare macro
        // would recolour all eight costumes, which is exactly what C1 refused to do and the
        // reason the line went unbound to begin with.
        assert!(
            emitted.contains("if(0x2508e0(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 0)){"),
            "the costume check must survive the export:\n{emitted}"
        );
        assert!(
            emitted.contains("macros::LAST_EFFECT_SET_COLOR(agent, 0.146, 0.205, 0.333);"),
            "the tint must survive the export:\n{emitted}"
        );
        let spawn_at = emitted.find("EFFECT_FOLLOW_ALPHA").unwrap();
        let tint_at = emitted.find("LAST_EFFECT_SET_COLOR").unwrap();
        assert!(
            spawn_at < tint_at,
            "LAST_EFFECT_SET_COLOR recolours whatever spawned last, so carrying it above its \
             own spawn would land it on someone else's:\n{emitted}"
        );
    }

    #[test]
    fn a_frame_whose_block_holds_no_spawn_still_reaches_the_export() {
        let src = r#"unsafe extern "C" fn effect_finalairend(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("dolly_buster_ground"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);
    }
    frame(agent.lua_state_agent, 40.0);
    if macros::is_excute(agent) {
        macros::CANCEL_FILL_SCREEN(agent, 1, 30);
        macros::CANCEL_FILL_SCREEN(agent, 0, 30);
    }
}
"#;
        let (calls, residue) = parse_effect_script(src).to_effect_calls_and_residue();
        assert_eq!(calls.len(), 1, "only the EFFECT is a call");

        // Keyed by frame 40, not by frame 1. `end_frame` is handed the cursor it is *leaving*,
        // and handing it the one being entered would put these lines on the following block —
        // still exported, still well-formed, and playing at the wrong time. There is no later
        // frame in this fixture, so the wrong answer here is the script's first frame.
        assert_eq!(
            residue.keys().copied().collect::<Vec<_>>(),
            vec![40],
            "{residue:?}"
        );
        assert!(
            residue[&40]
                .iter()
                .any(|l| l.contains("CANCEL_FILL_SCREEN(agent, 1, 30)"))
                && residue[&40]
                    .iter()
                    .any(|l| l.contains("CANCEL_FILL_SCREEN(agent, 0, 30)")),
            "both calls, in their own is_excute wrapper: {:?}",
            residue[&40]
        );

        let (_, emitted) =
            emit_effect_move_fn(&calls, "finalairend", &Default::default(), &residue);
        let frame_at = emitted
            .find("frame(agent.lua_state_agent, 40.0);")
            .unwrap_or_else(|| panic!("frame 40 must exist in its own right:\n{emitted}"));
        let first = emitted
            .find("CANCEL_FILL_SCREEN(agent, 1, 30)")
            .unwrap_or_else(|| panic!("the line the export used to delete:\n{emitted}"));
        assert!(
            frame_at < first,
            "the lines must land after their frame, not before it:\n{emitted}"
        );
        assert!(
            emitted.contains("CANCEL_FILL_SCREEN(agent, 0, 30)"),
            "both of them, in source order:\n{emitted}"
        );
        assert_eq!(
            emitted.matches("CANCEL_FILL_SCREEN").count(),
            2,
            "and each exactly once — a residue block emitted per call would duplicate them on a \
             frame that had two:\n{emitted}"
        );
        assert_eq!(
            emitted.matches('{').count(),
            emitted.matches('}').count(),
            "the carried block brings its own braces:\n{emitted}"
        );
        assert!(
            !emitted.contains("if macros::is_excute(agent) {\n    }"),
            "a frame that exists only for residue must not also open an empty block:\n{emitted}"
        );

        // The export no longer deletes them, so the report must no longer say it does.
        assert!(
            !unexportable_effect_lines(&parse_effect_script(src))
                .iter()
                .any(|l| l.contains("CANCEL_FILL_SCREEN")),
            "a line the export writes must not be reported as dropped"
        );
    }

    #[test]
    fn residue_is_keyed_to_the_frame_it_was_written_at_not_the_next_one() {
        let src = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 40.0);
    if macros::is_excute(agent) {
        macros::CANCEL_FILL_SCREEN(agent, 1, 30);
    }
    frame(agent.lua_state_agent, 60.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_atk_smoke"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
    }
}
"#;
        let (calls, residue) = parse_effect_script(src).to_effect_calls_and_residue();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].active_start, 60, "the spawn is the later frame's");
        assert_eq!(
            residue.keys().copied().collect::<Vec<_>>(),
            vec![40],
            "the line was written under frame 40 and belongs to it: {residue:?}"
        );
        assert!(
            calls[0].leading.is_empty(),
            "and it must not have been handed to the next frame's spawn either, which would \
             retime it just as surely: {:?}",
            calls[0].leading
        );

        let (_, emitted) = emit_effect_move_fn(&calls, "test", &Default::default(), &residue);
        let at_40 = emitted.find("frame(agent.lua_state_agent, 40.0);").unwrap();
        let at_60 = emitted.find("frame(agent.lua_state_agent, 60.0);").unwrap();
        let line = emitted.find("CANCEL_FILL_SCREEN").unwrap();
        assert!(
            at_40 < line && line < at_60,
            "the line must sit between its own frame and the next:\n{emitted}"
        );
    }

    #[test]
    fn each_costume_tint_stays_with_the_spawn_it_recolours() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_ALPHA(agent, Hash40::new("dolly_roll_l_color1"), Hash40::new("throw"), 0, 2.5, 0, 0, 0, 0, 1, true, 0.8);
    }
    if(0x2508e0(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 0)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.146, 0.205, 0.333);
        }
    }
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_ALPHA(agent, Hash40::new("dolly_roll_l_color2"), Hash40::new("throw"), 0, 2.5, 0, 0, 0, 0, 1, true, 0.8);
    }
    if(0x2508e0(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 0)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.587, 0.126, 0.169);
        }
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(calls.len(), 2);
        let (_, emitted) =
            emit_effect_move_fn(&calls, "test", &Default::default(), &Default::default());

        let first_spawn = emitted.find("dolly_roll_l_color1").unwrap();
        let first_tint = emitted.find("0.146").unwrap();
        let second_spawn = emitted.find("dolly_roll_l_color2").unwrap();
        let second_tint = emitted.find("0.587").unwrap();
        assert!(
            first_spawn < first_tint && first_tint < second_spawn && second_spawn < second_tint,
            "each tint must sit between its own spawn and the next one:\n{emitted}"
        );
        // The two spawns cannot share one `is_excute` block, because a tint has to come between
        // them — so the emitter must have split the frame rather than merging the run.
        assert_eq!(
            emitted.matches("if macros::is_excute(agent) {").count(),
            4,
            "one block per spawn and one per carried tint:\n{emitted}"
        );
    }

    #[test]
    fn a_live_colour_override_replaces_the_scripts_tint_rather_than_stacking_with_it() {
        let src = r#"
unsafe extern "C" fn effect_attackairhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("kirby_attack_arc"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        macros::LAST_EFFECT_SET_COLOR(agent, 0.25, 1.3, 2.5);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(calls[0].tint, Some([0.25, 1.3, 2.5]));
        let tweaks = std::collections::HashMap::from([(
            tweak_hash("kirby_attack_arc"),
            crate::mod_project::LiveTweak {
                effect_name: "kirby_attack_arc".into(),
                color: Some([1.0, 0.0, 0.0, 1.0]),
                speed: None,
            },
        )]);
        let (_, emitted) = emit_effect_move_fn(&calls, "attackairhi", &tweaks, &Default::default());
        assert_eq!(
            emitted.matches("LAST_EFFECT_SET_COLOR").count(),
            1,
            "exactly one tint line, or the export says two different things:\n{emitted}"
        );
        assert!(
            emitted.contains("macros::LAST_EFFECT_SET_COLOR(agent, 1.0, 0.0, 0.0);"),
            "the live override is the one that must survive:\n{emitted}"
        );
    }

    #[test]
    fn a_rate_with_no_spawn_directly_above_it_attaches_to_nothing() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_atk_smoke"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        macros::SOME_UNMODELLED_SPAWN(agent, Hash40::new("whatever"));
        macros::LAST_EFFECT_SET_RATE(agent, 2);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].rate, None,
            "the rate belongs to the unmodelled line above it, not to sys_atk_smoke"
        );
        let (_, emitted) =
            emit_effect_move_fn(&calls, "test", &Default::default(), &Default::default());
        assert_eq!(
            emitted.matches("SOME_UNMODELLED_SPAWN").count(),
            1,
            "a carried line must not be emitted alongside a call regenerated from it:\n{emitted}"
        );
        let unmodelled_at = emitted.find("SOME_UNMODELLED_SPAWN").unwrap();
        let rate_at = emitted.find("LAST_EFFECT_SET_RATE").unwrap();
        let smoke_at = emitted.find("sys_atk_smoke").unwrap();
        assert!(
            smoke_at < unmodelled_at && unmodelled_at < rate_at,
            "the rate must stay below the unmodelled spawn it names, and both below \
             sys_atk_smoke:\n{emitted}"
        );
    }

    #[test]
    fn colour_commands_survive_an_export_instead_of_being_dropped() {
        let src = r#"
unsafe extern "C" fn effect_attackdash(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_NO_STOP(agent, Hash40::new("kirby_dash"), Hash40::new("top"), 0, 6, 5, -90, 0, 160, 0.7, true);
    }
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::BURN_COLOR(agent, 2, 0.059, 0.008, 0);
        macros::BURN_COLOR_FRAME(agent, 4, 2, 0.059, 0.008, 0.9);
    }
    frame(agent.lua_state_agent, 30.0);
    if macros::is_excute(agent) {
        macros::BURN_COLOR(agent, 2, 0.059, 0.008, 0.9);
        macros::BURN_COLOR_FRAME(agent, 12, 2, 0.059, 0.008, 0);
        macros::EFFECT_OFF_KIND(agent, Hash40::new("kirby_dash"), false, true);
    }
    frame(agent.lua_state_agent, 42.0);
    if macros::is_excute(agent) {
        macros::BURN_COLOR_NORMAL(agent);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        // One spawn plus five colour commands, in script order.
        assert_eq!(calls.len(), 6);
        assert_eq!(calls[0].effect_name, "kirby_dash");
        assert!(calls[0].color.is_none());
        let colors: Vec<_> = calls[1..]
            .iter()
            .map(|call| (call.spawn_func.as_str(), call.color.clone().unwrap()))
            .collect();
        assert_eq!(
            colors,
            vec![
                (
                    "BURN_COLOR",
                    crate::data::ColorCall {
                        transition: None,
                        rgba: Some([2.0, 0.059, 0.008, 0.0])
                    }
                ),
                (
                    "BURN_COLOR_FRAME",
                    crate::data::ColorCall {
                        transition: Some(4.0),
                        rgba: Some([2.0, 0.059, 0.008, 0.9])
                    }
                ),
                (
                    "BURN_COLOR",
                    crate::data::ColorCall {
                        transition: None,
                        rgba: Some([2.0, 0.059, 0.008, 0.9])
                    }
                ),
                (
                    "BURN_COLOR_FRAME",
                    crate::data::ColorCall {
                        transition: Some(12.0),
                        rgba: Some([2.0, 0.059, 0.008, 0.0])
                    }
                ),
                (
                    "BURN_COLOR_NORMAL",
                    crate::data::ColorCall {
                        transition: None,
                        rgba: None
                    }
                ),
            ],
            "the interpolation length is the first argument, and the four after it are the colour"
        );
        // A colour command is one instant event: nothing closes it, so it must not be given a
        // window that an EFFECT_OFF_KIND would then be emitted to end.
        assert!(calls[1..]
            .iter()
            .all(|call| call.active_end == call.active_start));

        let (_, emitted) = emit_effect_move_fn(
            &calls,
            "attackdash",
            &Default::default(),
            &Default::default(),
        );
        // Whole numbers stay whole: every slot in this family is generic over `ToF32`, so this
        // is the spelling the archive uses and the one the source write-back produces.
        for line in [
            "macros::BURN_COLOR(agent, 2, 0.059, 0.008, 0);",
            "macros::BURN_COLOR_FRAME(agent, 4, 2, 0.059, 0.008, 0.9);",
            "macros::BURN_COLOR(agent, 2, 0.059, 0.008, 0.9);",
            "macros::BURN_COLOR_FRAME(agent, 12, 2, 0.059, 0.008, 0);",
            "macros::BURN_COLOR_NORMAL(agent);",
        ] {
            assert!(emitted.contains(line), "missing {line} from:\n{emitted}");
        }
        assert_eq!(
            parse_effect_script(&emitted).to_effect_calls(),
            calls,
            "reading the export back must produce the very same calls"
        );
    }

    #[test]
    fn a_burn_color_normal_is_not_read_as_a_col_normal() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::BURN_COLOR_NORMAL(agent);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].spawn_func, "BURN_COLOR_NORMAL");
    }

    #[test]
    fn a_game_script_still_reads_col_normal_as_its_own_statement() {
        let src = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::COL_PRI(agent, 200);
    }
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::COL_NORMAL(agent);
    }
}
"#;
        let script = parse_acmd_script(src);
        let stmts: Vec<_> = script
            .stmts
            .iter()
            .filter_map(|stmt| match stmt {
                crate::data::AcmdStmt::Excute(inner) => Some(inner),
                _ => None,
            })
            .flatten()
            .map(|stmt| format!("{stmt:?}"))
            .collect();
        assert_eq!(stmts, vec!["ColPri(200)", "ColNormal"]);
    }

    #[test]
    fn a_colour_command_takes_an_ordinal_but_never_anchors_a_rate() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_atk_smoke"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        macros::FLASH(agent, 0.314, 0.235, 0.157, 0.039);
        macros::LAST_EFFECT_SET_RATE(agent, 2);
    }
}
"#;
        let script = parse_effect_script(src);
        let calls = script.to_effect_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].rate, None,
            "the rate sits under the FLASH, not under the spawn — attaching it anyway would \
             retune an effect the script never asked to retune"
        );
        assert_eq!(
            script.call_macro_ordinals().len(),
            calls.len(),
            "every call needs an ordinal or write-back writes into the wrong line"
        );
    }

    #[test]
    fn a_live_speed_override_replaces_the_spawns_own_rate_rather_than_joining_it() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_atk_smoke"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        macros::LAST_EFFECT_SET_RATE(agent, 2);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        let mut tweaks = std::collections::HashMap::new();
        tweaks.insert(
            tweak_hash("sys_atk_smoke"),
            crate::mod_project::LiveTweak {
                effect_name: "sys_atk_smoke".into(),
                color: None,
                speed: Some(0.5),
            },
        );
        let (_, emitted) = emit_effect_move_fn(&calls, "test", &tweaks, &Default::default());
        assert_eq!(
            emitted.matches("LAST_EFFECT_SET_RATE").count(),
            1,
            "exactly one rate line, or the export says two different things:\n{emitted}"
        );
        assert!(
            emitted.contains("macros::LAST_EFFECT_SET_RATE(agent, 0.5);"),
            "the override is the one that must survive:\n{emitted}"
        );
    }

    #[test]
    fn rotation_arguments_keep_their_zyx_slot_order() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("g"), Hash40::new("top"), 0.0, 0.0, 0.0, 10.0, 20.0, 30.0, 1.0, 0, 0, 0, 0, 0, 0, false);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        // zr=10, yr=20, xr=30 → the editor's [x, y, z].
        assert_eq!(calls[0].rotation, [30.0, 20.0, 10.0]);

        let (_, emitted) =
            emit_effect_move_fn(&calls, "test", &Default::default(), &Default::default());
        assert!(
            emitted.contains("0.0, 0.0, 0.0, 10.0, 20.0, 30.0, 1.0"),
            "the angles must land back in the slots they came from:\n{emitted}"
        );
    }

    #[test]
    fn renaming_a_trail_reaches_the_exported_source() {
        let src = wrap_effect_fn(
            r#"        macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new("tex1"), Hash40::new("tex2"), 4, Hash40::new("sword1"), Hash40::new("sword2"), 3, 8, 0.75);"#,
        );
        let calls = parse_effect_script(&src).to_effect_calls();
        assert_eq!(calls.len(), 1, "{calls:#?}");

        let mut edited = calls.clone();
        edited[0].effect_name = "my_trail".into();
        edited[0].bone_name = "HaveL".into();
        let out = preview_effect_fn(&edited, "attacks4", &[], &Default::default());

        assert!(out.contains(r#"Hash40::new("my_trail")"#), "{out}");
        // Joints are hashed lowercase, as everywhere else.
        assert!(out.contains(r#"Hash40::new("havel")"#), "{out}");
        // Everything the editor has no field for is left exactly as the user wrote it.
        assert!(out.contains(r#"Hash40::new("tex2"), 4,"#), "{out}");
        assert!(
            out.contains(r#"Hash40::new("sword2"), 3, 8, 0.75)"#),
            "{out}"
        );
        assert!(out.contains("macros::AFTER_IMAGE4_ON_arg29("), "{out}");

        // And an untouched trail still comes back byte-identical.
        let untouched = preview_effect_fn(&calls, "attacks4", &[], &Default::default());
        assert!(
            untouched.contains(
                r#"macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new("tex1"), Hash40::new("tex2"), 4, Hash40::new("sword1"), Hash40::new("sword2"), 3, 8, 0.75);"#
            ),
            "{untouched}"
        );
    }

    /// The first trail-ON call of kirby `SpecialHi2`, verbatim.
    pub(crate) const CORPUS_TRAIL: &str = r#"        effect(*MA_MSC_CMD_EFFECT_AFTER_IMAGE3_ON, Hash40::new("tex_kirby_cutter"), Hash40::new("tex_kirby_cutter"), 12, Hash40::new("haver"), 0, 3, 0.25, Hash40::new("haver"), 0, 26, 0.5, true, Hash40::new("null"), Hash40::new("haver"), 0, 0, 0, 0, 0, 0, 1, 0, *EFFECT_AXIS_X, 0, *TRAIL_BLEND_BLEND_SRC_ONE, 1);"#;

    pub(crate) fn corpus_trail_script() -> String {
        format!(
            r#"
unsafe extern "C" fn effect_specialhi2(agent: &mut L2CAgentBase) {{
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {{
{CORPUS_TRAIL}
        macros::EFFECT_FOLLOW(agent, Hash40::new("kirby_fcut_rise"), Hash40::new("haver"), 0, 3, 0.3, 0, 0, 0, 1, true);
    }}
    frame(agent.lua_state_agent, 7.0);
    if macros::is_excute(agent) {{
        macros::AFTER_IMAGE_OFF(agent, 3);
    }}
}}
"#
        )
    }

    #[test]
    fn the_raw_command_trail_the_corpus_writes_round_trips_byte_identically() {
        let calls = parse_effect_script(&corpus_trail_script()).to_effect_calls();
        let trail = calls
            .iter()
            .find(|call| call.spawn_func == "AFTER_IMAGE_ON")
            .expect("the raw command form must produce a trail call");
        // Slot 1 and slot 4, read through the same constants the wrapper form uses.
        assert_eq!(trail.effect_name, "tex_kirby_cutter");
        assert_eq!(trail.bone_name, "haver");

        let (_, emitted) = emit_effect_move_fn(
            &calls,
            "specialhi2",
            &Default::default(),
            &Default::default(),
        );
        assert!(
            emitted.contains(CORPUS_TRAIL.trim()),
            "the trail must be re-emitted exactly as written — there is no \
             `macros::AFTER_IMAGE3_ON` to fall back to:\n{emitted}"
        );
    }

    #[test]
    fn a_raw_trails_graphic_and_joints_are_read_from_the_slots_the_declaration_names() {
        // Slot 2 is the second texture; slot 8 the trail's other edge.
        let line = CORPUS_TRAIL
            .trim()
            .replacen(
                r#"tex_kirby_cutter"), Hash40::new("tex_kirby_cutter")"#,
                r#"tex_kirby_cutter"), Hash40::new("tex_other")"#,
                1,
            )
            .replacen(
                r#"0.25, Hash40::new("haver")"#,
                r#"0.25, Hash40::new("sword")"#,
                1,
            );
        assert!(
            line.contains(r#"Hash40::new("tex_other")"#)
                && line.contains(r#"Hash40::new("sword")"#),
            "the fixture must actually differ at slots 2 and 8, or this proves nothing:\n{line}"
        );
        let src = format!(
            "unsafe extern \"C\" fn effect_t(agent: &mut L2CAgentBase) {{\n    \
             if macros::is_excute(agent) {{\n{line}\n    }}\n}}\n"
        );
        let calls = parse_effect_script(&src).to_effect_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].effect_name, "tex_kirby_cutter",
            "slot 1 is `trail1`; slot 2 is the second texture"
        );
        assert_eq!(
            calls[0].bone_name, "haver",
            "slot 4 is `trail_bone1`; slot 8 is the trail's other edge and slot 14 the flare's"
        );
        // The same fixture, read the other way round. Asserting both directions is what makes
        // the pair a statement about *which* slot each field comes from: either assertion alone
        // still passes with the two reads swapped, because on any vanilla call they agree.
        assert_eq!(
            calls[0].trail_bone2.as_deref(),
            Some("sword"),
            "slot 8 is `trail_bone2`; slot 14 is the flare's bone and is not a trail edge"
        );
    }

    #[test]
    fn editing_a_raw_trails_joint_rewrites_only_that_argument() {
        let mut calls = parse_effect_script(&corpus_trail_script()).to_effect_calls();
        let trail = calls
            .iter_mut()
            .find(|call| call.spawn_func == "AFTER_IMAGE_ON")
            .expect("trail call");
        trail.bone_name = "top".into();

        let (_, emitted) = emit_effect_move_fn(
            &calls,
            "specialhi2",
            &Default::default(),
            &Default::default(),
        );
        let line = emitted
            .lines()
            .find(|line| line.contains("AFTER_IMAGE3_ON"))
            .expect("the trail line must still be emitted");

        let expected = CORPUS_TRAIL.trim().replacen(
            r#"12, Hash40::new("haver")"#,
            r#"12, Hash40::new("top")"#,
            1,
        );
        assert_eq!(
            line.trim(),
            expected,
            "only the joint slot may change; slots 8 and 14 are also `haver` and must not"
        );
    }

    #[test]
    fn editing_a_raw_trails_second_joint_rewrites_only_that_argument() {
        let mut calls = parse_effect_script(&corpus_trail_script()).to_effect_calls();
        let trail = calls
            .iter_mut()
            .find(|call| call.spawn_func == "AFTER_IMAGE_ON")
            .expect("trail call");
        assert_eq!(
            trail.trail_bone2.as_deref(),
            Some("haver"),
            "the corpus call must carry a second joint, or this edits nothing"
        );
        trail.trail_bone2 = Some("blade".into());

        let (_, emitted) = emit_effect_move_fn(
            &calls,
            "specialhi2",
            &Default::default(),
            &Default::default(),
        );
        let line = emitted
            .lines()
            .find(|line| line.contains("AFTER_IMAGE3_ON"))
            .expect("the trail line must still be emitted");

        // Anchored on the preceding `trail_z1` value, which is what distinguishes slot 8 from
        // the identical hashes at 4 and 14.
        let expected = CORPUS_TRAIL.trim().replacen(
            r#"0.25, Hash40::new("haver")"#,
            r#"0.25, Hash40::new("blade")"#,
            1,
        );
        assert_eq!(
            line.trim(),
            expected,
            "only `trail_bone2` may change; `trail_bone1` and `flare_bone` hold the same hash"
        );
    }

    #[test]
    fn a_trail_too_short_to_reach_slot_eight_offers_no_second_joint() {
        let short = r#"        effect(*MA_MSC_CMD_EFFECT_AFTER_IMAGE3_ON, Hash40::new("tex_kirby_cutter"), Hash40::new("tex_kirby_cutter"), 12, Hash40::new("haver"));"#;
        let src = format!(
            "unsafe extern \"C\" fn effect_t(agent: &mut L2CAgentBase) {{\n    \
             if macros::is_excute(agent) {{\n{short}\n    }}\n}}\n"
        );
        let calls = parse_effect_script(&src).to_effect_calls();
        assert_eq!(calls.len(), 1, "the call must still parse as a trail");
        assert_eq!(calls[0].bone_name, "haver", "slot 4 is present and is read");
        assert_eq!(
            calls[0].trail_bone2, None,
            "there is no slot 8 in this call, so there is no second joint to offer"
        );

        let (_, emitted) =
            emit_effect_move_fn(&calls, "t", &Default::default(), &Default::default());
        assert!(
            emitted.contains(short.trim()),
            "a short trail must still ride through verbatim:\n{emitted}"
        );
    }

    #[test]
    fn an_undeclared_trail_spelling_offers_no_second_joint() {
        let undeclared = r#"        macros::AFTER_IMAGE_ON(agent, Hash40::new("tex_a"), Hash40::new("tex_b"), 12, Hash40::new("haver"), 0, 3, 0.25, Hash40::new("sword"), 0, 26, 0.5);"#;
        let src = format!(
            "unsafe extern \"C\" fn effect_t(agent: &mut L2CAgentBase) {{\n    \
             if macros::is_excute(agent) {{\n{undeclared}\n    }}\n}}\n"
        );
        let calls = parse_effect_script(&src).to_effect_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].bone_name, "haver",
            "the first joint is read from these spellings, as it always was"
        );
        assert_eq!(
            calls[0].trail_bone2, None,
            "slot 8 holds `sword` here, but nothing declares this call's layout"
        );

        // And the line still round-trips, which is the only reason the branch exists.
        let (_, emitted) =
            emit_effect_move_fn(&calls, "t", &Default::default(), &Default::default());
        assert!(
            emitted.contains(undeclared.trim()),
            "an undeclared spelling must ride through verbatim:\n{emitted}"
        );
    }

    #[test]
    fn a_trail_off_closes_the_trail_and_not_a_spawn_still_running() {
        let calls = parse_effect_script(&corpus_trail_script()).to_effect_calls();
        let trail = calls
            .iter()
            .find(|call| call.spawn_func == "AFTER_IMAGE_ON")
            .expect("trail call");
        assert_eq!(trail.active_end, 7, "the trail is what frame 7 closes");
        assert_eq!(
            trail.trail_off,
            Some(3.0),
            "and it keeps the written argument"
        );

        let follow = calls
            .iter()
            .find(|call| call.effect_name == "kirby_fcut_rise")
            .expect("follow call");
        assert_eq!(
            follow.active_end,
            crate::data::OPEN_ENDED_EFFECT_FRAME,
            "the follow is never closed by this script and must stay open"
        );

        let (_, emitted) = emit_effect_move_fn(
            &calls,
            "specialhi2",
            &Default::default(),
            &Default::default(),
        );
        assert!(
            emitted.contains("macros::AFTER_IMAGE_OFF(agent, 3);"),
            "the close must survive to the export:\n{emitted}"
        );
        assert!(
            !emitted.contains("EFFECT_OFF_KIND"),
            "no end was written for the follow, so none may be invented:\n{emitted}"
        );
    }

    #[test]
    fn a_trail_off_with_no_trail_open_is_carried_rather_than_dropped() {
        let src = r#"
unsafe extern "C" fn effect_specialhi4(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("kirby_fcut_rise"), Hash40::new("haver"), 0, 3, 0.3, 0, 0, 0, 1, true);
        macros::AFTER_IMAGE_OFF(agent, 0);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        let (_, emitted) = emit_effect_move_fn(
            &calls,
            "specialhi4",
            &Default::default(),
            &Default::default(),
        );
        assert!(
            emitted.contains("macros::AFTER_IMAGE_OFF(agent, 0);"),
            "a close with nothing local to close still has to reach the export:\n{emitted}"
        );
        assert!(
            !emitted.contains("EFFECT_OFF_KIND"),
            "and it must not be resolved against the follow instead:\n{emitted}"
        );
    }

    #[test]
    fn only_a_whole_effect_identifier_starts_a_raw_trail_call() {
        let real = CORPUS_TRAIL.trim();
        let embedded = real.replacen("effect(", "sub_effect(", 1);

        assert!(
            crate::acmd_src::raw_trail_line(real).is_some(),
            "the corpus line must be recognised, or the negative below proves nothing"
        );
        assert!(
            crate::acmd_src::raw_trail_line(&embedded).is_none(),
            "`sub_effect(` is a different function and must not be read as a trail"
        );

        // And the parser agrees with the scanner on both, which is the invariant that matters.
        let wrap = |line: &str| {
            format!(
                "unsafe extern \"C\" fn effect_t(agent: &mut L2CAgentBase) {{\n    \
                 if macros::is_excute(agent) {{\n{line}\n    }}\n}}\n"
            )
        };
        assert_eq!(
            parse_effect_script(&wrap(real)).to_effect_calls().len(),
            1,
            "the real line must produce exactly one call"
        );
        assert_eq!(
            parse_effect_script(&wrap(&embedded))
                .to_effect_calls()
                .len(),
            0,
            "and the embedded one none, matching the scanner"
        );
    }

    #[test]
    fn a_trail_with_no_closing_line_still_exports_the_required_argument() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new("tex1"), Hash40::new("tex2"), 4, Hash40::new("sword1"), 0, 0, 0, Hash40::new("sword2"), 0, 0, 0);
    }
}
"#;
        let mut calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].trail_off, None, "nothing closed it");
        // The user drags the trail's end onto frame 12.
        calls[0].active_end = 12;

        let (_, emitted) =
            emit_effect_move_fn(&calls, "test", &Default::default(), &Default::default());
        assert!(
            emitted.contains(&format!(
                "macros::AFTER_IMAGE_OFF(agent, {});",
                crate::data::TRAIL_OFF_DEFAULT as i64
            )),
            "{emitted}"
        );
        assert!(
            !emitted.contains("AFTER_IMAGE_OFF(agent);"),
            "the arity-less form does not compile:\n{emitted}"
        );
    }

    #[test]
    fn after_image_trails_export_as_trails() {
        let src = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new("tex1"), Hash40::new("tex2"), 4, Hash40::new("sword1"), Hash40::new("sword2"), 0, 0, 0, 3, 8, 0.75);
    }
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::AFTER_IMAGE_OFF(agent, 3);
    }
}
"#;
        let calls = parse_effect_script(src).to_effect_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].spawn_func, "AFTER_IMAGE_ON");
        assert_eq!((calls[0].active_start, calls[0].active_end), (3, 9));
        // The closing call's argument is kept: the corpus does not agree on it, so replacing
        // it with a house value would change what the author wrote.
        assert_eq!(calls[0].trail_off, Some(3.0));

        let (_, emitted) =
            emit_effect_move_fn(&calls, "test", &Default::default(), &Default::default());
        assert!(
            emitted.contains("macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new(\"tex1\")"),
            "the trail call must be replayed verbatim:\n{emitted}"
        );
        // With its argument, and as the bare integer the corpus writes. `macros::AFTER_IMAGE_OFF`
        // is declared `<F: ToF32>(agent, unk: F)` — the bare call this used to emit is a compile
        // error, so every exported move with a trail was an unbuildable project.
        assert!(
            emitted.contains("macros::AFTER_IMAGE_OFF(agent, 3);"),
            "a trail is closed by AFTER_IMAGE_OFF, which takes an argument:\n{emitted}"
        );
        assert!(
            !emitted.contains("AFTER_IMAGE_OFF(agent);"),
            "the arity-less form does not compile:\n{emitted}"
        );
        assert!(
            !emitted.contains("EFFECT_OFF_KIND") && !emitted.contains("EFFECT_FOLLOW"),
            "a trail is not an effect spawn:\n{emitted}"
        );
    }

    #[test]
    fn a_spawn_with_no_recorded_tail_falls_back_to_the_plain_pair() {
        let call = crate::data::EffectCall {
            effect_name: "sys_hit".into(),
            effect_name_alt: Some("sys_hit_alt".into()),
            spawn_func: "EFFECT_FOLLOW_FLIP".into(),
            bone_name: "HaveR".into(),
            offset: [0.0; 3],
            rotation: [0.0; 3],
            scale: 1.0,
            follows_bone: true,
            active_start: 2,
            active_end: crate::data::OPEN_ENDED_EFFECT_FRAME,
            disabled: false,
            extra_args: None,
            flip_axis: None,
            raw_line: None,
            trail_command: None,
            trail_off: None,
            off_fade: None,
            off_detach: None,
            trail_bone2: None,
            rate: None,
            work_int: None,
            camera_offset: None,
            tint: None,
            particle_tint: None,
            alpha: None,
            scale_w: None,
            color: None,
            control: None,
            guard: None,
            leading: Vec::new(),
            trailing: Vec::new(),
        };
        let (_, emitted) =
            emit_effect_move_fn(&[call], "test", &Default::default(), &Default::default());
        assert!(
            emitted.contains(
                "macros::EFFECT_FOLLOW(agent, Hash40::new(\"sys_hit\"), Hash40::new(\"haver\"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, true);"
            ),
            "an unrecoverable tail must fall back to a single-graphic EFFECT_FOLLOW:\n{emitted}"
        );
    }

    #[test]
    fn a_kill_with_no_spawn_to_close_is_carried_into_the_export() {
        let source = r#"unsafe extern "C" fn effect_specialhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(agent, Hash40::new("ganon_sword_flare"), true, false);
    }
}
"#;
        let script = parse_effect_script(source);
        let (calls, residue) = script.to_effect_calls_and_residue();
        let emitted = preview_effect_fn(&calls, "special_hi", &[], &residue);
        assert!(
            emitted.contains(
                "macros::EFFECT_OFF_KIND(agent, Hash40::new(\"ganon_sword_flare\"), true, false);"
            ),
            "the kill must survive with its own arguments:\n{emitted}"
        );
        assert!(
            emitted.contains("frame(agent.lua_state_agent, 4.0);"),
            "and at the frame it was written at:\n{emitted}"
        );
    }

    #[test]
    fn a_kill_keeps_the_booleans_its_script_wrote() {
        let source = r#"unsafe extern "C" fn effect_attacks4(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 28.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("ganon_sword_flare"), Hash40::new("haver"), 0, 0, 0, 0, 0, 0, 1, true);
    }
    frame(agent.lua_state_agent, 35.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(agent, Hash40::new("ganon_sword_flare"), false, false);
    }
}
"#;
        let calls = parse_effect_script(source).to_effect_calls();
        assert_eq!(
            (calls[0].off_fade, calls[0].off_detach),
            (Some(false), Some(false)),
            "the closing kill's arguments belong to the call it closes: {:?}",
            calls[0]
        );
        let emitted = preview_effect_fn(&calls, "attack_s4", &[], &Default::default());
        assert!(
            emitted.contains(
                "macros::EFFECT_OFF_KIND(agent, Hash40::new(\"ganon_sword_flare\"), false, false);"
            ),
            "the export must not substitute its own pair:\n{emitted}"
        );

        // An effect the editor ends itself has no authored pair, and the documented default is
        // what it gets. Without this the assertion above passes on a build that simply never
        // writes the default, and the fallback would go untested.
        let mut invented = calls.clone();
        invented[0].off_fade = None;
        invented[0].off_detach = None;
        let emitted = preview_effect_fn(&invented, "attack_s4", &[], &Default::default());
        assert!(
            emitted.contains(
                "macros::EFFECT_OFF_KIND(agent, Hash40::new(\"ganon_sword_flare\"), false, true);"
            ),
            "an editor-created stop uses EFFECT_OFF_KIND_DEFAULT:\n{emitted}"
        );
    }

    #[test]
    fn a_kill_with_a_non_literal_argument_is_kept_verbatim() {
        let source = r#"unsafe extern "C" fn effect_attacks4(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 28.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("ganon_sword_flare"), Hash40::new("haver"), 0, 0, 0, 0, 0, 0, 1, true);
    }
    frame(agent.lua_state_agent, 35.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(agent, Hash40::new("ganon_sword_flare"), false, hidden);
    }
}
"#;
        let script = parse_effect_script(source);
        let (calls, residue) = script.to_effect_calls_and_residue();
        let emitted = preview_effect_fn(&calls, "attack_s4", &[], &residue);
        assert_eq!(
            emitted.matches("EFFECT_OFF_KIND").count(),
            1,
            "exactly one kill, and it is the author's own:\n{emitted}"
        );
        assert!(
            emitted.contains(
                "macros::EFFECT_OFF_KIND(agent, Hash40::new(\"ganon_sword_flare\"), false, hidden);"
            ),
            "the unreadable call is reproduced as written:\n{emitted}"
        );
    }

    #[test]
    fn a_vanilla_sound_script_reads_as_typed_calls_at_the_frames_it_writes() {
        const TURN_DASH: &str = r#"unsafe extern "C" fn sound_turndash(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::PLAY_SE(agent, Hash40::new("se_kirby_dash_start"));
        macros::SET_PLAY_INHIVIT(agent, Hash40::new("se_kirby_dash_start"), 20);
    }
    wait(agent.lua_state_agent, 13.0);
    if macros::is_excute(agent) {
        macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_left_m"), Hash40::new("se_kirby_step_right_m"));
    }
    wait(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_right_m"), Hash40::new("se_kirby_step_left_m"));
    }
}
"#;
        let script = parse_sound_script(TURN_DASH);
        let events = script.to_sound_events();
        let seen: Vec<(u32, &str, Vec<&str>, Option<&str>)> = events
            .iter()
            .map(|e| {
                (
                    e.frame,
                    e.call.func.as_str(),
                    e.call.sounds.iter().map(String::as_str).collect(),
                    e.call.tail.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                (6, "PLAY_SE", vec!["se_kirby_dash_start"], None),
                (
                    6,
                    "SET_PLAY_INHIVIT",
                    vec!["se_kirby_dash_start"],
                    Some("20")
                ),
                (
                    19,
                    "PLAY_STEP_FLIPPABLE",
                    vec!["se_kirby_step_left_m", "se_kirby_step_right_m"],
                    None
                ),
                (
                    23,
                    "PLAY_STEP_FLIPPABLE",
                    vec!["se_kirby_step_right_m", "se_kirby_step_left_m"],
                    None
                ),
            ]
        );

        // Parse → emit → parse reaches the same events, which is the property that lets a
        // typed call be regenerated instead of copied.
        let again = parse_sound_script(&format!(
            "unsafe extern \"C\" fn sound_turndash(agent: &mut L2CAgentBase) {{\n{}}}\n",
            emit_sound_body(&script)
        ));
        assert_eq!(again.to_sound_events(), events);
    }

    #[test]
    fn a_sound_written_outside_an_excute_block_still_fires() {
        const WALK_MIDDLE: &str = r#"unsafe extern "C" fn sound_walkmiddle(agent: &mut L2CAgentBase) {
    wait_loop_sync_mot();
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_left_m"), Hash40::new("se_kirby_step_right_m"));
    }
    frame(agent.lua_state_agent, 30.0);
    macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_right_m"), Hash40::new("se_kirby_step_left_m"));
}
"#;
        let script = parse_sound_script(WALK_MIDDLE);
        let frames: Vec<u32> = script.to_sound_events().iter().map(|e| e.frame).collect();
        assert_eq!(
            frames,
            vec![8, 30],
            "the bare footstep on frame 30 was lost"
        );

        // And it is written back bare. Emitting it inside an `if macros::is_excute(agent) {`
        // it never had would be a behaviour change, not a formatting one: the wrapper decides
        // whether the line runs on this pass at all. The dump's leading `wait_loop_sync_mot`
        // re-rolls the script into its motion loop, so "bare" is the loop body's own indent.
        let emitted = emit_sound_body(&script);
        assert!(
            emitted.contains(
                "\n        frame(agent.lua_state_agent, 30.0);\n        macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new(\"se_kirby_step_right_m\")"
            ),
            "the bare call did not come back at the loop body's own indent:\n{emitted}"
        );

        // And it is *editable*. Resolving a site has to walk the same shapes the site counter
        // counts, `Bare` included — drop it from either one and this move's second footstep is
        // either silently uneditable or, worse, edited by writing to the first.
        let mut edited = script;
        let events = edited.to_sound_events();
        assert_eq!(events[1].site, 1, "the bare call is the second site");
        let mut call = events[1].call.clone();
        call.sounds[0] = "se_common_step_right_m".into();
        *edited
            .sound_stmt_mut(events[1].site)
            .expect("the bare call must resolve to a statement") =
            crate::data::ExcuteStmt::Sound(call);

        let after = emit_sound_body(&edited);
        assert!(
            after.contains(
                "macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new(\"se_common_step_right_m\"), \
                 Hash40::new(\"se_kirby_step_left_m\"));"
            ),
            "the edit did not reach the bare call:\n{after}"
        );
        assert!(
            after.contains(
                "macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new(\"se_kirby_step_left_m\"), \
                 Hash40::new(\"se_kirby_step_right_m\"));"
            ),
            "the edit landed in the wrapped call instead:\n{after}"
        );
    }

    #[test]
    fn a_looped_sound_reports_the_same_site_every_time_round() {
        const LOOPED: &str = r#"unsafe extern "C" fn sound_looped(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    for _ in 0..3 {
    wait(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        macros::PLAY_SE(agent, Hash40::new("se_kirby_dash_start"));
    }
    }
    wait(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_left_m"), Hash40::new("se_kirby_step_right_m"));
    }
}
"#;
        let events = parse_sound_script(LOOPED).to_sound_events();
        let seen: Vec<(u32, usize, &str)> = events
            .iter()
            .map(|e| (e.frame, e.site, e.call.func.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![
                (8, 0, "PLAY_SE"),
                (10, 0, "PLAY_SE"),
                (12, 0, "PLAY_SE"),
                (16, 1, "PLAY_STEP_FLIPPABLE"),
            ],
            "the site cursor did not rewind for each iteration"
        );

        // And the other half of the join: those ordinals index the textual scan write-back uses.
        // Checking only that they are `0, 0, 0, 1` would pass just as well if the scan happened
        // to list the two calls the other way round.
        let sites = crate::acmd_src::sound_sites(LOOPED);
        for event in &events {
            assert_eq!(
                sites[event.site].name, event.call.func,
                "site {} is not the `{}` it was resolved for",
                event.site, event.call.func
            );
        }
    }

    #[test]
    fn a_loop_that_never_runs_still_advances_the_site_cursor() {
        const EMPTY_LOOP: &str = r#"unsafe extern "C" fn sound_emptyloop(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    for _ in 0..0 {
    if macros::is_excute(agent) {
        macros::PLAY_SE(agent, Hash40::new("se_kirby_dash_start"));
    }
    macros::PLAY_SE(agent, Hash40::new("se_kirby_dash_stop"));
    }
    wait(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_left_m"), Hash40::new("se_kirby_step_right_m"));
    }
}
"#;
        let events = parse_sound_script(EMPTY_LOOP).to_sound_events();
        assert_eq!(
            events.len(),
            1,
            "a loop with no iterations played a sound anyway"
        );
        let sites = crate::acmd_src::sound_sites(EMPTY_LOOP);
        assert_eq!(
            sites.len(),
            3,
            "the scan should still see all three calls — the text has them either way"
        );
        assert_eq!(
            events[0].site, 2,
            "the surviving footstep resolved to `{}`, a call inside the loop that never ran",
            sites[events[0].site].name
        );
    }

    #[test]
    fn a_sound_inside_a_runtime_branch_is_counted_and_resolvable() {
        const SOUND_IN_RAW_BLOCK: &str = r#"unsafe extern "C" fn sound_rawblocksound(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 2.0);
    for _ in 0..0 {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_SQUAT_FLAG_REQUEST_SQUAT_SE) {
    if macros::is_excute(agent) {
        macros::PLAY_SE(agent, Hash40::new("se_common_guardoff"));
    }
    }
    }
    frame(agent.lua_state_agent, 6.0);
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_SQUAT_FLAG_REQUEST_SQUAT_SE) {
    if macros::is_excute(agent) {
        macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_left_m"), Hash40::new("se_kirby_step_right_m"));
    }
    }
}
"#;
        let mut script = parse_sound_script(SOUND_IN_RAW_BLOCK);
        let events = script.to_sound_events();
        assert_eq!(events.len(), 1, "the skipped loop played a sound anyway");
        // Site 1 exercises the counter: without its `RawBlock` arm the cursor never steps over
        // the call inside the skipped branch, and this event claims site 0.
        assert_eq!(
            events[0].site, 1,
            "the surviving call took the site of the one inside the skipped loop"
        );
        // And resolving it exercises the walker, which is the other half and the other function.
        let stmt = script
            .sound_stmt_mut(events[0].site)
            .expect("a sound inside a branch must resolve to a statement");
        let crate::data::ExcuteStmt::Sound(call) = stmt else {
            panic!("site {} resolved to {stmt:?}, not a sound", events[0].site);
        };
        assert_eq!(
            call.func, "PLAY_STEP_FLIPPABLE",
            "resolved to the wrong call — an edit here would rewrite a different line"
        );
    }

    #[test]
    fn a_partial_project_override_keeps_every_category_it_does_not_define() {
        let function = |prefix: &str, frame: u32| {
            format!(
            "unsafe extern \"C\" fn {prefix}test(agent: &mut L2CAgentBase) {{\n    frame(agent.lua_state_agent, {frame}.0);\n}}\n"
        )
        };
        let mirror = SCRIPT_PREFIXES
            .iter()
            .map(|prefix| function(prefix, 1))
            .collect::<String>();
        for overridden in SCRIPT_PREFIXES {
            let project = function(overridden, 3);
            let merged = merge_project_over_mirror_blocked(&project, &[overridden], &[], &mirror);
            for prefix in SCRIPT_PREFIXES {
                let expected = if prefix == overridden {
                    &project
                } else {
                    &mirror
                };
                assert_eq!(
                    extract_function(&merged, prefix),
                    extract_function(expected, prefix)
                );
                assert_eq!(
                    merged
                        .lines()
                        .filter(|line| is_function_header(line.trim(), prefix))
                        .count(),
                    1
                );
            }
        }
    }

    /// Kirby's Final Smash start, verbatim: a `frame()` and an `is_excute` inside a runtime
    const RAW_BRANCH: &str = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {
        frame(agent.lua_state_agent, 10.0);
        if macros::is_excute(agent) {
            macros::FT_START_CUTIN(agent);
        }
    }
    else {
        if macros::is_excute(agent) {
            macros::FT_START_CUTIN(agent);
        }
    }
    frame(agent.lua_state_agent, 17.0);
}
"#;

    #[test]
    fn a_branch_keeps_the_brace_that_closes_it() {
        let script = parse_acmd_script(RAW_BRANCH);
        let emitted = emit_stmts(&script.stmts, "    ").join("\n");
        let opens = emitted.matches('{').count();
        let closes = emitted.matches('}').count();
        assert_eq!(
            (opens, closes),
            (4, 4),
            "the branch, its else and the two excute blocks must all close:\n{emitted}"
        );
        // The `else` has to stay attached to something, or it is not Rust at all.
        assert!(
            emitted.contains("    }\n    else {"),
            "the else arm lost the brace it hangs off:\n{emitted}"
        );
        // One issue per arm, not two in a row at the top level.
        assert_eq!(
            emitted.matches("FT_START_CUTIN").count(),
            2,
            "both arms are still there:\n{emitted}"
        );
    }

    #[test]
    fn a_combined_else_after_cut_in_condition_does_not_panic_or_drift() {
        let source = r#"
unsafe extern "C" fn game_throwf(agent: &mut L2CAgentBase) {
    if FighterCutInManager::is_one_on_one() {
        if macros::is_excute(agent) {
            macros::ATTACK_ABS(agent, 0, 0, 2.0, 40, 125, 0, 70, 0.0, 1.0, *ATTACK_LR_CHECK_F, 0.0, true, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_NONE, *ATTACK_REGION_THROW);
        }
    } else {
        if macros::is_excute(agent) {
            macros::ATTACK_ABS(agent, 0, 0, 3.0, 40, 125, 0, 70, 0.0, 1.0, *ATTACK_LR_CHECK_F, 0.0, true, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_NONE, *ATTACK_REGION_THROW);
        }
    }
}
"#;
        let script = parse_acmd_script(source);
        let emitted = preview_game_fn(&script, "throw_f");
        assert_eq!(emitted.matches('{').count(), emitted.matches('}').count());
        assert!(emitted.contains("    }\n    else {"));
        assert_eq!(script.to_hitboxes().len(), 2);

        let reparsed = parse_acmd_script(&emitted);
        let reemitted = preview_game_fn(&reparsed, "throw_f");
        assert_eq!(emitted, reemitted);

        let else_if_source = source.replace(
            "} else {",
            "} else if FighterCutInManager::is_one_on_one() {",
        );
        let else_if = preview_game_fn(&parse_acmd_script(&else_if_source), "throw_f");
        assert_eq!(else_if.matches('{').count(), else_if.matches('}').count());
        assert!(else_if.contains("    else if FighterCutInManager::is_one_on_one() {"));
    }

    #[test]
    fn truncated_blocks_and_combined_else_tails_stay_raw_across_parser_domains() {
        use crate::data::{AcmdStmt, EffectStmt};

        let malformed_cases: &[&[&str]] = &[
            &[
                "if guard(agent) {",
                "    if macros::is_excute(agent) {",
                "        AttackModule::clear_all(agent.module_accessor);",
                "    }",
            ],
            &[
                "for _ in 0..2 {",
                "    if macros::is_excute(agent) {",
                "        AttackModule::clear_all(agent.module_accessor);",
                "    }",
            ],
            &[
                "if macros::is_excute(agent) {",
                "    AttackModule::clear_all(agent.module_accessor);",
            ],
        ];
        for domain in [
            ParseDomain::Game,
            ParseDomain::Sound,
            ParseDomain::Expression,
        ] {
            for lines in malformed_cases {
                let (parsed, consumed) = parse_stmts(lines, 0, domain, true);
                assert_eq!(consumed, lines.len());
                assert!(matches!(
                    parsed.as_slice(),
                    [AcmdStmt::Raw(raw)] if raw == &lines.join("\n")
                ));
            }
        }

        let incomplete_else = [
            "if guard(agent) {",
            "    frame(agent.lua_state_agent, 1.0);",
            "} else {",
            "    if macros::is_excute(agent) {",
            "        AttackModule::clear_all(agent.module_accessor);",
            "    }",
        ];
        let (parsed, consumed) = parse_stmts(&incomplete_else, 0, ParseDomain::Game, true);
        assert_eq!(consumed, incomplete_else.len());
        assert!(matches!(
            parsed.as_slice(),
            [AcmdStmt::Raw(raw)] if raw == &incomplete_else.join("\n")
        ));

        let valid_neighbor = [
            "for _ in 0..2 {",
            "    if macros::is_excute(agent) {",
            "        AttackModule::clear_all(agent.module_accessor);",
            "    }",
            "}",
            "frame(agent.lua_state_agent, 3.0);",
        ];
        let (parsed, consumed) = parse_stmts(&valid_neighbor, 0, ParseDomain::Game, true);
        assert_eq!(consumed, valid_neighbor.len());
        assert!(matches!(
            parsed.as_slice(),
            [AcmdStmt::Loop { body, .. }, AcmdStmt::Frame(frame)]
                if *frame == 3.0
                    && matches!(body.as_slice(), [AcmdStmt::Excute(commands)]
                        if matches!(commands.as_slice(), [crate::data::ExcuteStmt::ClearAll]))
        ));

        let malformed_effect_cases: &[&[&str]] = &[
            &[
                "for _ in 0..2 {",
                "    if macros::is_excute(agent) {",
                "        macros::EFFECT_OFF_KIND(agent, Hash40::new(\"sys_attack_line\"), false, false);",
                "    }",
            ],
            &[
                "if macros::is_excute(agent) {",
                "    macros::EFFECT_OFF_KIND(agent, Hash40::new(\"sys_attack_line\"), false, false);",
            ],
            &[
                "if color_guard(agent) {",
                "    macros::EFFECT_OFF_KIND(agent, Hash40::new(\"sys_attack_line\"), false, false);",
                "} else {",
                "    if macros::is_excute(agent) {",
                "        macros::EFFECT_OFF_KIND(agent, Hash40::new(\"sys_attack_line\"), false, false);",
                "    }",
            ],
        ];
        for lines in malformed_effect_cases {
            let (parsed, consumed) = parse_effect_stmts(lines, 0, false);
            assert_eq!(consumed, lines.len());
            assert!(matches!(
                parsed.as_slice(),
                [EffectStmt::Raw(raw)] if raw == &lines.join("\n")
            ));
        }

        let valid_effect = [
            "if color_guard(agent) {",
            "    frame(agent.lua_state_agent, 1.0);",
            "} else {",
            "    wait(agent.lua_state_agent, 2.0);",
            "}",
        ];
        let (parsed, consumed) = parse_effect_stmts(&valid_effect, 0, false);
        assert_eq!(consumed, valid_effect.len());
        assert!(matches!(
            parsed.as_slice(),
            [
                EffectStmt::Cond { body: yes, .. },
                EffectStmt::Cond { body: no, .. }
            ] if matches!(yes.as_slice(), [EffectStmt::Frame(frame)] if *frame == 1.0)
                && matches!(no.as_slice(), [EffectStmt::Wait(frames)] if *frames == 2.0)
        ));
    }

    #[test]
    fn read_only_getters_and_status_conditions_remain_verbatim_and_balanced() {
        let source = r#"
unsafe extern "C" fn game_status_getters(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD) {
        frame(agent.lua_state_agent, 1.0);
    }
    if WorkModule::get_int(agent.module_accessor, *FIGHTER_STATUS_WORK_INT_NEXT_STEP) > 0 {
        frame(agent.lua_state_agent, 2.0);
    }
    if WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_WORK_INT64) != 0 {
        frame(agent.lua_state_agent, 3.0);
    }
    if WorkModule::get_float(agent.module_accessor, *FIGHTER_STATUS_WORK_FLOAT) > 0.0 {
        frame(agent.lua_state_agent, 4.0);
    }
    if WorkModule::get_param_float(agent.module_accessor, hash40("param"), 0) > 0.0 {
        frame(agent.lua_state_agent, 5.0);
    }
    if KineticModule::get_sum_speed_y(agent.module_accessor, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN) > 0.0 {
        frame(agent.lua_state_agent, 6.0);
    }
    if StatusModule::status_kind(agent.module_accessor) == *FIGHTER_STATUS_KIND_ATTACK {
        frame(agent.lua_state_agent, 7.0);
    }
    if StatusModule::prev_status_kind(agent.module_accessor) == *FIGHTER_STATUS_KIND_ATTACK {
        frame(agent.lua_state_agent, 8.0);
    }
    if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {
        frame(agent.lua_state_agent, 9.0);
    }
}
"#;
        let script = parse_acmd_script(source);
        assert_eq!(script.stmts.len(), 9);
        assert!(script
            .stmts
            .iter()
            .all(|stmt| matches!(stmt, crate::data::AcmdStmt::RawBlock { .. })));

        let emitted = preview_game_fn(&script, "status_getters");
        for line in [
            "if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD) {",
            "if WorkModule::get_int(agent.module_accessor, *FIGHTER_STATUS_WORK_INT_NEXT_STEP) > 0 {",
            "if WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_WORK_INT64) != 0 {",
            "if WorkModule::get_float(agent.module_accessor, *FIGHTER_STATUS_WORK_FLOAT) > 0.0 {",
            "if WorkModule::get_param_float(agent.module_accessor, hash40(\"param\"), 0) > 0.0 {",
            "if KineticModule::get_sum_speed_y(agent.module_accessor, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN) > 0.0 {",
            "if StatusModule::status_kind(agent.module_accessor) == *FIGHTER_STATUS_KIND_ATTACK {",
            "if StatusModule::prev_status_kind(agent.module_accessor) == *FIGHTER_STATUS_KIND_ATTACK {",
            "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {",
        ] {
            assert!(emitted.contains(line), "read-only condition lost: {line}\n{emitted}");
        }
        assert_eq!(emitted.matches('{').count(), emitted.matches('}').count());

        let reparsed = parse_acmd_script(&emitted);
        let reemitted = preview_game_fn(&reparsed, "status_getters");
        assert_eq!(
            reemitted, emitted,
            "raw condition export must be a fixed point"
        );
    }

    #[test]
    fn a_hitbox_inside_a_branch_is_still_seen_by_the_frame_walk() {
        let source = r#"
unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if WorkModule::get_int(agent.module_accessor, *FIGHTER_STATUS_WORK_ID_INT_STRENGTH) == 0 {
        if macros::is_excute(agent) {
            macros::ATTACK(agent, 0, 0, Hash40::new("top"), 12.0, 361, 100, 0, 30, 4.0, 0.0, 8.0, 0.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);
        }
    }
}
"#;
        let hitboxes = parse_acmd_script(source).to_hitboxes();
        assert_eq!(hitboxes.len(), 1, "the conditional hitbox is still read");
        assert_eq!((hitboxes[0].active_start, hitboxes[0].damage), (4, 12.0));
    }

    #[test]
    fn a_body_with_no_game_function_contributes_no_game_statements() {
        let source = r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_attack_impact"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);
    }
}
"#;
        assert!(
            parse_acmd_script(source).stmts.is_empty(),
            "an effect-only body has no game script in it"
        );
        // The headerless case is what the fallback exists for, and still works.
        assert!(
            !parse_acmd_script("    frame(agent.lua_state_agent, 3.0);\n")
                .stmts
                .is_empty(),
            "a bare pasted body is still read whole"
        );
    }

    #[test]
    fn the_preview_is_exactly_what_the_export_writes() {
        let project = sample_project();
        let acmd = project
            .files
            .iter()
            .find(|f| f.rel_path == "src/mario/acmd.rs")
            .map(|f| f.contents.as_str())
            .expect("generated fighter ACMD");

        let sample = sample_edits();
        let game = preview_game_fn(&sample.0, "attack_air_n");
        assert!(
            acmd.contains(&game),
            "the previewed game_* function is not what was exported:\n{game}\n---\n{acmd}"
        );
        let effect = preview_effect_fn(&sample.1, "attack_air_n", &sample.3, &Default::default());
        assert!(
            acmd.contains(&effect),
            "the previewed effect_* function is not what was exported:\n{effect}\n---\n{acmd}"
        );
        // And the preview really is showing the user's macro, not a substitute.
        assert!(effect.contains("macros::EFFECT_FOLLOW(agent, Hash40::new(\"sys_flash\")"));

        let sound = preview_sound_fn(&sample.2, "attack_air_n");
        assert!(
            acmd.contains(&sound),
            "the previewed sound_* function is not what was exported:\n{sound}\n---\n{acmd}"
        );
        // The function is named for the category, not for the move alone. Get this wrong and
        // the plugin installs the move's sounds over its `game_` script, which does compile.
        assert!(sound.starts_with("unsafe extern \"C\" fn sound_attackairn("));
        assert!(
            sound.contains(
                "macros::SET_PLAY_INHIVIT(agent, Hash40::new(\"se_kirby_dash_start\"), 20);"
            ),
            "the one member with a non-hash tail lost it:\n{sound}"
        );
    }

    #[test]
    fn effect_compatibility_wrappers_keep_native_spelling_and_measured_argument_order() {
        use crate::data::{EffectMacro, EffectScript, EffectStmt};
        let commands = EFFECT_SPAWN_LAYOUTS
            .iter()
            .map(|layout| EffectMacro::Effect {
                effect_name: "sys_smash_flash".into(),
                effect_name_alt: layout.flip.then(|| "sys_smash_flash".into()),
                spawn_func: layout.name.into(),
                bone_name: "top".into(),
                offset: [-1.25, 2.5, 0.375],
                rotation: [15.0, -30.0, 45.0],
                scale: 0.75,
                follows_bone: layout.follows_bone,
                extra_args: layout
                    .tail
                    .iter()
                    .map(|(value, _)| (*value).into())
                    .collect(),
                flip_axis: layout.flip.then(|| "*EF_FLIP_XY".into()),
            })
            .collect();
        let script = EffectScript {
            stmts: vec![EffectStmt::Excute(commands)],
        };
        let (calls, residue) = script.to_effect_calls_and_residue();
        // The flat editor infers lifetimes on source import. Check the exact statement tree
        // it exports, rather than comparing that derived metadata to a hand-built call list.
        let expected = parse_effect_script(&preview_effect_fn(&calls, "attack11", &[], &residue));
        let exported = build_mod_project_full(
            &[],
            &[("mario".into(), "attack11".into(), calls.clone(), residue)],
            &[],
            &[],
            "effect_compatibility",
        );
        let source = &exported
            .files
            .iter()
            .find(|file| file.rel_path == "src/mario/acmd.rs")
            .unwrap()
            .contents;
        syn::parse_file(source).unwrap();
        assert_eq!(source.matches("mod macros {").count(), 1);
        assert!(source.contains("pub use smash_script::macros::*;"));
        for name in [
            "EFFECT_FOLLOW_FLIP_COLOR",
            "EFFECT_FOLLOW_FLIP_RND",
            "EFFECT_FOLLOW_NO_SCALE",
            "EFFECT_FOLLOW_COLOR",
            "EFFECT_FLW_POS_UNSYNC_VIS",
            "EFFECT_FLW_POS_NO_STOP",
            "FOOT_EFFECT_FLIP",
            "DOWN_EFFECT",
            "EFFECT_ATTR",
        ] {
            assert_eq!(source.matches(&format!("pub unsafe fn {name}<")).count(), 1);
            assert!(source.contains(&format!(
                "smash::app::sv_animcmd::{name}(agent.lua_state_agent);"
            )));
        }
        assert!(
            !source.contains("pub unsafe fn EFFECT_FOLLOW<"),
            "existing correct library wrappers must retain their owner"
        );
        let compatibility =
            effect_source_compatibility_macros("macros::EFFECT_FOLLOW_FLIP_COLOR(agent);");
        assert!(compatibility.contains("smash_script::lua_args!(agent, g, a, b, n0.to_f32(), n1.to_f32(), n2.to_f32(), n3.to_f32(), n4.to_f32(), n5.to_f32(), n6.to_f32(), t0, t1, t2.to_f32(), t3.to_f32(), t4.to_f32());"));
        assert_eq!(compatibility.matches("agent.clear_lua_stack();").count(), 2);
        assert!(!compatibility.contains("pub unsafe fn EFFECT_ATTR<"));
        assert_json_eq!(parse_effect_script(source), expected);
        for source in [
            "macros::EFFECT_FOLLOW(agent);",
            "// macros::EFFECT_ATTR(agent);",
            "/* outer /* macros::FOOT_EFFECT_FLIP(agent); */ macros::DOWN_EFFECT(agent); */",
            "let text = r#\"macros::EFFECT_ATTR(agent);\"#;",
        ] {
            assert!(
                effect_source_compatibility_macros(source).is_empty(),
                "unexpected compatibility module for {source}"
            );
        }
    }

    #[test]
    fn hurtbox_status_emission_never_dereferences_an_existing_dereference_again() {
        for (stored, expected) in [
            ("HIT_STATUS_NORMAL", "*HIT_STATUS_NORMAL"),
            ("*HIT_STATUS_NORMAL", "*HIT_STATUS_NORMAL"),
            (" *HIT_STATUS_XLU ", "*HIT_STATUS_XLU"),
            ("0", "0"),
            ("2", "2"),
        ] {
            assert_eq!(emit_status(stored), expected);
            let script = crate::data::AcmdScript {
                stmts: vec![crate::data::AcmdStmt::Excute(vec![
                    crate::data::ExcuteStmt::HitStatus {
                        target: crate::data::HurtTarget::Whole,
                        status: stored.into(),
                    },
                ])],
            };
            let emitted = preview_game_fn(&script, "hurtbox_emission");
            assert!(emitted.contains(&format!("macros::WHOLE_HIT(agent, {expected});")));
            assert!(!emitted.contains("**HIT_STATUS"));
            syn::parse_file(&emitted).unwrap();
        }
    }

    #[test]
    fn canonical_export_imports_attack_wrapper_types_only_when_used() {
        use crate::data::{
            AcmdScript, AcmdStmt, AttackHeightAllKind, AttackReferenceJointKind, ExcuteStmt,
        };

        let typed = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![
                ExcuteStmt::AttackModuleSetHeightAll {
                    height: AttackHeightAllKind::High,
                    arg3: false,
                },
                ExcuteStmt::AttackModuleSetAttackReferenceJointId {
                    kind: AttackReferenceJointKind::TopYZX,
                },
                ExcuteStmt::HitModuleSetStatusAllNormal,
            ])],
        };
        let exported = build_mod_project(
            &[("cloud".into(), "attack11".into(), typed)],
            "wrapper_imports",
        );
        let acmd = exported
            .files
            .iter()
            .find(|file| file.rel_path == "src/cloud/acmd.rs")
            .unwrap()
            .contents
            .as_str();
        let header = acmd
            .lines()
            .take_while(|line| !line.starts_with("pub fn install"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(header.contains(
            "app::{sv_animcmd::*, lua_bind::*, ArticleOperationTarget, AttackDirectionAxis, AttackHeight, HitStatus}"
        ));
        assert_eq!(header.matches("AttackDirectionAxis,").count(), 1);
        assert_eq!(header.matches("AttackHeight,").count(), 1);
        assert_eq!(header.matches("HitStatus}").count(), 1);

        let plain = build_mod_project(
            &[(
                "cloud".into(),
                "attack11".into(),
                AcmdScript {
                    stmts: vec![AcmdStmt::Frame(1.0)],
                },
            )],
            "plain_imports",
        );
        let acmd = plain
            .files
            .iter()
            .find(|file| file.rel_path == "src/cloud/acmd.rs")
            .unwrap()
            .contents
            .as_str();
        let header = acmd
            .lines()
            .take_while(|line| !line.starts_with("pub fn install"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!header.contains("AttackDirectionAxis"));
        assert!(!header.contains("AttackHeight"));
        assert!(!header.contains("HitStatus"));
    }

    #[test]
    fn spawns_the_export_cannot_reproduce_are_reported() {
        let mut calls = sample_edits().1;
        // Faithful: both carry their tails.
        assert!(export_spawn_downgrades(&calls).is_empty());

        // A project saved before tails were recorded, on a macro with no plain equivalent.
        calls[1].spawn_func = "EFFECT_FOLLOW_NO_STOP".into();
        calls[1].extra_args = None;
        assert_eq!(
            export_spawn_downgrades(&calls),
            vec![("EFFECT_FOLLOW_NO_STOP".to_string(), "sys_flash".to_string())]
        );

        // A disabled call is not exported at all, so it is not a downgrade.
        calls[1].disabled = true;
        assert!(export_spawn_downgrades(&calls).is_empty());

        // Neither is one whose macro IS what the fallback emits.
        calls[1].disabled = false;
        calls[1].spawn_func = "EFFECT_FOLLOW".into();
        assert!(export_spawn_downgrades(&calls).is_empty());
    }

    #[test]
    fn expression_valued_graphic_arguments_are_not_re_quoted() {
        assert_eq!(hash_arg("sys_hit"), "Hash40::new(\"sys_hit\")");
        assert_eq!(hash_arg("0x1234abcd"), "Hash40::new_raw(0x1234abcd)");
        assert_eq!(hash_arg("*EFFECT_KIND"), "*EFFECT_KIND");
        assert_eq!(hash_arg("Hash40::new(\"x\")"), "Hash40::new(\"x\")");
    }

    #[test]
    fn set_speed_ex_parses_exports_and_preserves_the_kinetic_token() {
        let source = r#"unsafe extern "C" fn game_attackairlw(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::SET_SPEED_EX(agent, 0, -3.8, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_speed_ex_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].frame, 4);
        assert_eq!(events[0].call.speed_x, 0.0);
        assert_eq!(events[0].call.speed_y, -3.8);
        assert_eq!(
            events[0].call.kinetic_kind,
            "*KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN"
        );

        let emitted = preview_game_fn(&script, "attack_air_lw");
        assert!(emitted.contains(
            "macros::SET_SPEED_EX(agent, 0.0, -3.8, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);"
        ));
        assert_eq!(parse_acmd_script(&emitted).to_speed_ex_events(), events);
    }

    #[test]
    fn set_speed_parses_as_a_typed_point_and_generated_helper_round_trips() {
        let source = r#"unsafe extern "C" fn game_attackairlw(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::SET_SPEED(agent, 0, -3.8);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_speed_events();
        assert_eq!(
            events,
            vec![crate::data::SetSpeedEvent {
                frame: 4,
                call: crate::data::SetSpeedCall {
                    speed_x: 0.0,
                    speed_y: -3.8,
                },
                site: 0,
            }]
        );

        let emitted = preview_game_fn(&script, "attack_air_lw");
        assert!(emitted.contains("visionary_set_speed(agent, 0.0, -3.8);"));
        assert!(emitted.contains("sv_animcmd::SET_SPEED(agent.lua_state_agent);"));
        assert_eq!(parse_acmd_script(&emitted).to_speed_events(), events);

        let reparsed = parse_acmd_script(&emitted);
        let reemitted = preview_game_fn(&reparsed, "attack_air_lw");
        assert_eq!(
            reemitted.matches("unsafe fn visionary_set_speed(").count(),
            1,
            "the generated helper is an emitter detail and must not duplicate on re-export"
        );
        assert_eq!(parse_acmd_script(&reemitted).to_speed_events(), events);
    }

    #[test]
    fn generated_movement_helpers_consume_only_their_emitted_attributes_and_bodies() {
        let script = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![
                ExcuteStmt::SetSpeed(crate::data::SetSpeedCall {
                    speed_x: 1.25,
                    speed_y: -3.5,
                }),
                ExcuteStmt::ClrSpeed(crate::data::ClrSpeedCall {
                    kinetic_kind: "*FIGHTER_KINETIC_ENERGY_ID_GRAVITY".into(),
                }),
                ExcuteStmt::SetAir,
            ])],
        };
        let emitted = preview_game_fn(&script, "attackairlw");
        let parsed = parse_acmd_script(&emitted);
        assert_json_eq!(
            &parsed,
            &script,
            "the generated helper attributes or bodies leaked into ACMD statements"
        );

        let reemitted = preview_game_fn(&parsed, "attackairlw");
        for helper in [
            "visionary_set_speed",
            "visionary_clr_speed",
            "visionary_set_air",
        ] {
            assert_eq!(
                reemitted.matches(&format!("unsafe fn {helper}(")).count(),
                1,
                "generated helper {helper} was duplicated"
            );
        }
    }

    #[test]
    fn generated_effect_helpers_match_emitted_form_with_or_without_attributes() {
        for helper in [
            emit_last_effect_set_work_int_helper("    "),
            emit_last_effect_set_scale_w_helper("    "),
        ] {
            let lines = helper.iter().map(String::as_str).collect::<Vec<_>>();
            assert_eq!(
                generated_effect_helper_end(&lines, 0),
                Some(lines.len() - 1)
            );
            let without_attributes = &lines[2..];
            assert_eq!(
                generated_effect_helper_end(without_attributes, 0),
                Some(without_attributes.len() - 1)
            );

            let mut changed_body = helper.clone();
            let body_line = changed_body
                .iter_mut()
                .find(|line| line.contains("agent.clear_lua_stack();"))
                .unwrap();
            *body_line = "    agent.clear_lua_stack(); // user change".into();
            let changed_body = changed_body.iter().map(String::as_str).collect::<Vec<_>>();
            assert_eq!(generated_effect_helper_end(&changed_body, 0), None);
        }
    }

    #[test]
    fn generated_effect_helper_with_extra_cfg_attribute_remains_intact() {
        let helper = emit_last_effect_set_scale_w_helper("    ");
        let prefixes = [
            vec!["    #[cfg(custom_effect_helper)]", ""],
            vec!["    // retain this helper's annotation", ""],
            vec!["    #[cfg(", "        custom_effect_helper", "    )"],
        ];
        for prefix in prefixes {
            let mut body = prefix
                .iter()
                .map(|line| (*line).to_string())
                .collect::<Vec<_>>();
            body.extend(helper.iter().cloned());
            body.push("    frame(agent.lua_state_agent, 2.0);".into());
            let source = format!(
                "unsafe extern \"C\" fn effect_custom(agent: &mut L2CAgentBase) {{\n{}\n}}\n",
                body.join("\n")
            );
            let parsed = parse_effect_script(&source);
            assert!(matches!(
                parsed.stmts.first(),
                Some(crate::data::EffectStmt::Raw(prefix))
                    if prefix.starts_with("#[cfg(") || prefix.starts_with("// retain")
            ));
            assert!(matches!(
                parsed.stmts.get(1),
                Some(crate::data::EffectStmt::Raw(attribute)) if attribute == "#[inline]"
            ));
            assert!(matches!(
                parsed.stmts.get(2),
                Some(crate::data::EffectStmt::Raw(attribute))
                    if attribute == "#[allow(dead_code)]"
            ));
            assert!(matches!(
                parsed.stmts.get(3),
                Some(crate::data::EffectStmt::Cond { header, .. })
                    if header.contains("unsafe fn visionary_last_effect_set_scale_w")
            ));
            assert!(matches!(
                parsed.stmts.get(4),
                Some(crate::data::EffectStmt::Frame(frame)) if *frame == 2.0
            ));
        }
    }

    #[test]
    fn consecutive_generated_effect_helpers_are_consumed_from_function_prolog() {
        let mut body = emit_last_effect_set_work_int_helper("    ");
        body.extend(emit_last_effect_set_scale_w_helper("    "));
        body.push("    frame(agent.lua_state_agent, 2.0);".into());
        let source = format!(
            "unsafe extern \"C\" fn effect_custom(agent: &mut L2CAgentBase) {{\n{}\n}}\n",
            body.join("\n")
        );
        let parsed = parse_effect_script(&source);
        assert!(matches!(
            parsed.stmts.as_slice(),
            [crate::data::EffectStmt::Frame(frame)] if *frame == 2.0
        ));
    }

    #[test]
    fn generated_effect_helpers_after_statements_or_inside_blocks_are_preserved() {
        let helper = emit_last_effect_set_scale_w_helper("    ").join("\n");
        let after_statement = format!(
            "unsafe extern \"C\" fn effect_after(agent: &mut L2CAgentBase) {{\n    frame(agent.lua_state_agent, 1.0);\n{helper}\n}}\n"
        );
        let parsed = parse_effect_script(&after_statement);
        assert!(matches!(
            parsed.stmts.first(),
            Some(crate::data::EffectStmt::Frame(frame)) if *frame == 1.0
        ));
        assert!(matches!(
            parsed.stmts.get(1),
            Some(crate::data::EffectStmt::Raw(line)) if line == "#[inline]"
        ));
        assert!(matches!(
            parsed.stmts.get(2),
            Some(crate::data::EffectStmt::Raw(line)) if line == "#[allow(dead_code)]"
        ));
        assert!(matches!(
            parsed.stmts.get(3),
            Some(crate::data::EffectStmt::Cond { header, .. })
                if header.contains("unsafe fn visionary_last_effect_set_scale_w")
        ));

        let nested = format!(
            "unsafe extern \"C\" fn effect_nested(agent: &mut L2CAgentBase) {{\n    if custom_effect_guard(agent) {{\n{helper}\n    }}\n}}\n"
        );
        let parsed = parse_effect_script(&nested);
        assert!(matches!(
            parsed.stmts.first(),
            Some(crate::data::EffectStmt::Cond { body, .. })
                if matches!(body.first(), Some(crate::data::EffectStmt::Raw(line)) if line == "#[inline]")
                    && matches!(body.get(2), Some(crate::data::EffectStmt::Cond { header, .. })
                        if header.contains("unsafe fn visionary_last_effect_set_scale_w"))
        ));
    }

    #[test]
    fn similarly_named_effect_helpers_and_unrelated_attributes_remain_raw() {
        let source = r#"unsafe extern "C" fn effect_custom(agent: &mut L2CAgentBase) {
    #[allow(unused_variables)]
    #[inline]
    #[allow(dead_code)]
    unsafe fn visionary_last_effect_set_scale_w(agent: &mut L2CAgentBase, values: &[f32]) {
        custom_effect_extension(agent, values);
    }
    frame(agent.lua_state_agent, 2.0);
}
"#;
        let parsed = parse_effect_script(source);
        assert!(matches!(
            parsed.stmts.first(),
            Some(crate::data::EffectStmt::Raw(attribute))
                if attribute == "#[allow(unused_variables)]"
        ));
        assert!(matches!(
            parsed.stmts.get(1),
            Some(crate::data::EffectStmt::Raw(attribute)) if attribute == "#[inline]"
        ));
        assert!(matches!(
            parsed.stmts.get(2),
            Some(crate::data::EffectStmt::Raw(attribute))
                if attribute == "#[allow(dead_code)]"
        ));
        assert!(matches!(
            parsed.stmts.get(3),
            Some(crate::data::EffectStmt::Cond { header, body })
                if header.contains("unsafe fn visionary_last_effect_set_scale_w")
                    && matches!(body.as_slice(), [crate::data::EffectStmt::Raw(line)]
                        if line == "custom_effect_extension(agent, values);")
        ));
        assert!(matches!(
            parsed.stmts.get(4),
            Some(crate::data::EffectStmt::Frame(frame)) if *frame == 2.0
        ));
    }

    #[test]
    fn unrelated_user_attributes_and_opaque_acmd_lines_are_retained() {
        let source = r#"unsafe extern "C" fn game_custom(agent: &mut L2CAgentBase) {
    #[allow(unused_variables)]
    custom_acmd_extension(agent);
    #[cfg(custom_acmd_feature)]
    another_opaque_extension(agent);
}
"#;
        let parsed = parse_acmd_script(source);
        assert_json_eq!(
            &parsed.stmts,
            vec![
                AcmdStmt::Raw("#[allow(unused_variables)]".into()),
                AcmdStmt::Raw("custom_acmd_extension(agent);".into()),
                AcmdStmt::Raw("#[cfg(custom_acmd_feature)]".into()),
                AcmdStmt::Raw("another_opaque_extension(agent);".into()),
            ]
        );
        let emitted = preview_game_fn(&parsed, "custom");
        assert!(emitted.contains("#[allow(unused_variables)]\n"));
        assert!(emitted.contains("custom_acmd_extension(agent);\n"));
        assert!(emitted.contains("#[cfg(custom_acmd_feature)]\n"));
        assert!(emitted.contains("another_opaque_extension(agent);\n"));
    }

    #[test]
    fn similarly_named_user_helpers_do_not_consume_attributes_or_opaque_body() {
        let source = r#"unsafe extern "C" fn game_custom(agent: &mut L2CAgentBase) {
    #[allow(dead_code)]
    unsafe fn visionary_set_air(agent: &mut L2CAgentBase) {
        custom_acmd_extension(agent);
    }
    frame(agent.lua_state_agent, 2.0);
}
"#;
        let parsed = parse_acmd_script(source);
        assert!(matches!(
            parsed.stmts.first(),
            Some(AcmdStmt::Raw(attribute)) if attribute == "#[allow(dead_code)]"
        ));
        assert!(
            matches!(parsed.stmts.get(1), Some(AcmdStmt::RawBlock { header, body })
            if header.contains("unsafe fn visionary_set_air")
                && matches!(body.as_slice(), [AcmdStmt::Raw(line)] if line == "custom_acmd_extension(agent);"))
        );
        assert!(matches!(parsed.stmts.get(2), Some(AcmdStmt::Frame(frame)) if *frame == 2.0));
    }

    #[test]
    fn malformed_native_calls_stay_raw_without_losing_valid_neighbors() {
        for (line, bare, expected) in [
            ("macros::SET_SPEED(agent, 0);", false, None),
            ("macros::SET_SPEED(agent, 0, 1, 2);", false, None),
            ("macros::FT_CATCH_STOP(agent, 6);", false, None),
            ("macros::FT_CATCH_STOP(agent, 6, 1, 2);", false, None),
            ("macros::FT_CATCH_STOP(other, 6, 1);", false, None),
            ("KineticModule::add_speed(agent.module_accessor, &Vector3f{x: 1.0, y: 2.0, z: 1.0});", false, None),
            ("KineticModule::add_speed(other, &Vector3f{x: 1.0, y: 2.0, z: 0.0});", false, None),
            ("KineticModule::add_speed(agent.module_accessor, &Vector3f{x: 1.0, y: 2.0});", false, None),
            ("WorkModule::on_flag(agent.module_accessor, *FLAG, 1);", false, None),
            ("WorkModule::off_flag(other, 2);", false, None),
            ("WorkModule::off_flag(agent.module_accessor, make_flag());", false, None),
            ("WorkModule::on_flag(agent.module_accessor, 3);", false, Some("WorkFlag")),
            ("WorkModule::enable_transition_term(agent.module_accessor, *TERM, 1);", false, None),
            ("WorkModule::unable_transition_term(other, 2);", false, None),
            ("WorkModule::unable_transition_term(agent.module_accessor, make_term());", false, None),
            ("WorkModule::enable_transition_term(agent.module_accessor, 3);", false, Some("WorkTransitionTerm")),
            ("WorkModule::enable_transition_term_group(boma, 4);", false, None),
            ("WorkModule::unable_transition_term_group_ex(agent.module_accessor, make_group());", false, None),
            ("WorkModule::unable_transition_term_group_ex(agent.module_accessor, 5);", false, Some("WorkTransitionTerm")),
            ("WorkModule::set_int(agent.module_accessor, 1);", false, None),
            ("WorkModule::set_int(agent.module_accessor, 1, 2, 3);", false, None),
            ("WorkModule::set_int(other.module_accessor, 1, 2);", false, None),
            ("WorkModule::set_int(agent.module_accessor, make_value(), 2);", false, None),
            ("WorkModule::set_float(agent.module_accessor, value, 2);", false, None),
            ("WorkModule::set_float(agent.module_accessor, NaN, 2);", false, None),
            ("WorkModule::set_float(agent.module_accessor, 1.0, make_slot());", false, None),
            ("WorkModule::set_int(agent.module_accessor, 1, slot);", false, None),
            ("WorkModule::set_int64(agent.module_accessor, hash40(\"fall_damage\"), 2);", false, None),
            ("WorkModule::set_int64(other.module_accessor, hash40(\"fall_damage\") as i64, 2);", false, None),
            ("WorkModule::set_int64(agent.module_accessor, make_value(), 2);", false, None),
            ("WorkModule::set_int64(agent.module_accessor, hash40(\"fall_damage\") as i64, make_slot());", false, None),
            ("WorkModule::inc_int(agent.module_accessor, 1, 2);", false, None),
            ("WorkModule::inc_int(other.module_accessor, 3);", false, None),
            ("WorkModule::inc_int(agent.module_accessor, make_slot());", false, None),
            ("WorkModule::inc_int(agent.module_accessor, SLOT_NAME);", false, None),
            ("WorkModule::inc_int(agent.module_accessor, *VALID_SLOT);", false, Some("WorkModuleIncInt")),
            ("KineticModule::suspend_energy(agent.module_accessor);", false, None),
            ("KineticModule::resume_energy(other, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);", false, None),
            ("KineticModule::resume_energy(agent.module_accessor, );", false, None),
            ("KineticModule::clear_speed_all(agent.module_accessor, 0);", false, None),
            ("KineticModule::clear_speed_all(other);", false, None),
            ("KineticModule::clear_speed_all(agent.module_accessor);", false, Some("KineticClearSpeedAll")),
            ("KineticModule::set_consider_ground_friction(agent.module_accessor, 1, 0);", false, None),
            ("KineticModule::set_consider_ground_friction(agent.module_accessor, true);", false, None),
            ("KineticModule::set_consider_ground_friction(other, false, 0);", false, None),
            ("KineticModule::set_consider_ground_friction(agent.module_accessor, false, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);", false, Some("KineticSetConsiderGroundFriction")),
            ("macros::CLR_SPEED(agent);", false, None),
            ("macros::CLR_SPEED(agent, 0, 1);", false, None),
            ("macros::SET_AIR(agent, 0);", false, None),
            ("macros::SET_AIR(other);", false, None),
            ("KineticModule::change_kinetic(agent.module_accessor);", false, None),
            ("KineticModule::change_kinetic(agent.module_accessor, 0, 1);", false, None),
            ("KineticModule::change_kinetic(other.module_accessor, 0);", false, None),
            ("macros::FT_START_ADJUST_MOTION_FRAME_arg1(agent);", true, None),
            ("macros::FT_START_ADJUST_MOTION_FRAME_arg1(agent, 1, 2);", true, None),
            ("FT_START_ADJUST_MOTION_FRAME_REVISED_arg1(1.0);", true, None),
            ("MotionModule::set_rate(agent.module_accessor);", false, None),
            ("MotionModule::set_rate(agent.module_accessor, 0.8, 1.0);", false, None),
            ("MotionModule::set_rate(other.module_accessor, 0.8);", false, None),
            ("MotionModule::set_rate(agent.module_accessor, -378992935);", false, None),
            ("MotionModule::set_helper_calculation(agent.module_accessor);", false, None),
            ("MotionModule::set_helper_calculation(agent.module_accessor, 1);", false, None),
            ("MotionModule::set_helper_calculation(other.module_accessor, true);", false, None),
            ("MotionModule::set_helper_calculation(agent.module_accessor, enabled);", false, None),
            ("MotionModule::set_helper_calculation(agent.module_accessor, false, true);", false, None),
            ("MotionModule::set_rate_partial(agent.module_accessor, 2);", false, None),
            ("MotionModule::set_rate_partial(agent.module_accessor, *FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY);", false, None),
            ("MotionModule::set_rate_partial(agent.module_accessor, *FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY, -1);", false, None),
            ("MotionModule::set_rate_partial(other.module_accessor, *FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY, 1);", false, None),
            ("MotionModule::set_rate_partial(agent.module_accessor, part_kind, 1);", false, None),
            ("MotionModule::set_rate_partial(agent.module_accessor, *FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY, rate);", false, None),
            ("MotionModule::set_frame_partial(agent.module_accessor, 2);", false, None),
            ("MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL);", false, None),
            ("MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, -1);", false, None),
            ("MotionModule::set_frame_partial(other.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 1);", false, None),
            ("MotionModule::set_frame_partial(agent.module_accessor, part_kind, 1);", false, None),
            ("MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, frame);", false, None),
            ("MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 1, sync);", false, None),
            ("macros::SET_SPEED_EX(agent, 0, 1);", false, None),
            ("macros::SET_SPEED_EX(agent, 0, 1, 0, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);", false, None),
            ("macros::ADD_SPEED_NO_LIMIT(agent, 0);", false, None),
            ("macros::ADD_SPEED_NO_LIMIT(agent, 0, 1, 2);", false, None),
            ("macros::CORRECT(agent);", false, None),
            ("macros::CORRECT(agent, 1, 2);", false, None),
        ] {
            let body = if bare { line.to_owned() } else { format!("if macros::is_excute(agent) {{\n{line}\n}}") };
            let source = format!("unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n{body}\n}}\n");
            let script = parse_acmd_script(&source);
            if let Some(expected) = expected {
                let [AcmdStmt::Excute(commands)] = script.stmts.as_slice() else { panic!("{line}") };
                let encoded = serde_json::to_value(&commands[0]).unwrap();
                let tag = encoded.as_str().or_else(|| encoded.as_object().and_then(|value|value.keys().next().map(String::as_str)));
                assert_eq!(tag, Some(expected), "{line}");
            } else {
                let raw = match script.stmts.as_slice() {
                    [AcmdStmt::Raw(raw)] => raw,
                    [AcmdStmt::Excute(commands)] => match commands.as_slice() {
                        [ExcuteStmt::Raw(raw)] => raw,
                        _ => panic!("malformed call became typed: {line}"),
                    },
                    _ => panic!("malformed call changed scope: {line}"),
                };
                assert_eq!(raw.trim(), line);
            }
            let emitted = preview_game_fn(&script, "x");
            assert_json_eq!(&script, parse_acmd_script(&emitted), "{line}");
        }
    }

    #[test]
    fn add_speed_no_limit_and_correct_parse_export_as_typed_points() {
        let source = r#"unsafe extern "C" fn game_attackairlw(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::ADD_SPEED_NO_LIMIT(agent, 0, -3.8);
        macros::CORRECT(agent, *GROUND_CORRECT_KIND_GROUND);
    }
}
"#;
        let script = parse_acmd_script(source);
        let speed = script.to_add_speed_no_limit_events();
        let correct = script.to_correct_events();
        assert_eq!(speed.len(), 1);
        assert_eq!(speed[0].frame, 4);
        assert_eq!(speed[0].call.speed_x, 0.0);
        assert_eq!(speed[0].call.speed_y, -3.8);
        assert_eq!(correct.len(), 1);
        assert_eq!(correct[0].frame, 4);
        assert_eq!(correct[0].call.kind, "*GROUND_CORRECT_KIND_GROUND");

        let emitted = preview_game_fn(&script, "attack_air_lw");
        assert!(emitted.contains("macros::ADD_SPEED_NO_LIMIT(agent, 0.0, -3.8);"));
        assert!(emitted.contains("macros::CORRECT(agent, *GROUND_CORRECT_KIND_GROUND);"));
        let round_trip = parse_acmd_script(&emitted);
        assert_eq!(round_trip.to_add_speed_no_limit_events(), speed);
        assert_eq!(round_trip.to_correct_events(), correct);
    }

    #[test]
    fn ft_catch_stop_parse_export_as_typed_point() {
        let source = r#"unsafe extern "C" fn game_throwf(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::FT_CATCH_STOP(agent, 6, 1);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_ft_catch_stop_events();
        assert_eq!(
            events,
            vec![crate::data::FtCatchStopEvent {
                frame: 6,
                call: crate::data::FtCatchStopCall {
                    arg1: 6.0,
                    arg2: 1.0,
                },
                site: 0,
            }]
        );
        let emitted = preview_game_fn(&script, "throw_f");
        assert!(emitted.contains("macros::FT_CATCH_STOP(agent, 6.0, 1.0);"));
        assert_eq!(
            parse_acmd_script(&emitted).to_ft_catch_stop_events(),
            events
        );
    }

    #[test]
    fn ft_start_adjust_motion_frame_parse_export_as_typed_point() {
        let source = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 17.0);
    macros::FT_START_ADJUST_MOTION_FRAME_arg1(agent, 0.85);
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_ft_start_adjust_motion_frame_events();
        assert_eq!(
            events,
            vec![crate::data::FtStartAdjustMotionFrameEvent {
                frame: 17,
                call: crate::data::FtStartAdjustMotionFrameCall { value: 0.85 },
                site: 0,
            }]
        );
        let emitted = preview_game_fn(&script, "attack_air_n");
        assert!(emitted.contains("macros::FT_START_ADJUST_MOTION_FRAME_arg1(agent, 0.85);"));
        assert_eq!(
            parse_acmd_script(&emitted).to_ft_start_adjust_motion_frame_events(),
            events
        );
    }

    #[test]
    fn clr_speed_and_set_air_parse_export_as_typed_points() {
        let source = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::CLR_SPEED(agent, *FIGHTER_KINETIC_ENERGY_ID_GRAVITY);
        macros::SET_AIR(agent);
    }
}
"#;
        let script = parse_acmd_script(source);
        let clr = script.to_clr_speed_events();
        let air = script.to_set_air_events();
        assert_eq!(clr.len(), 1);
        assert_eq!(clr[0].frame, 4);
        assert_eq!(
            clr[0].call.kinetic_kind,
            "*FIGHTER_KINETIC_ENERGY_ID_GRAVITY"
        );
        assert_eq!(air, vec![crate::data::SetAirEvent { frame: 4, site: 0 }]);

        let emitted = preview_game_fn(&script, "attack_air_n");
        assert!(emitted.contains("visionary_clr_speed(agent, *FIGHTER_KINETIC_ENERGY_ID_GRAVITY);"));
        assert!(emitted.contains("visionary_set_air(agent);"));
        assert!(emitted.contains("sv_kinetic_energy::clear_speed"));
        assert!(emitted.contains("sv_animcmd::SET_AIR"));
        assert_eq!(parse_acmd_script(&emitted).to_clr_speed_events(), clr);
        assert_eq!(parse_acmd_script(&emitted).to_set_air_events(), air);
        let reemitted = preview_game_fn(&parse_acmd_script(&emitted), "attack_air_n");
        assert_eq!(
            reemitted.matches("unsafe fn visionary_clr_speed(").count(),
            1
        );
        assert_eq!(reemitted.matches("unsafe fn visionary_set_air(").count(), 1);
    }

    #[test]
    fn kinetic_add_speed_parses_standard_and_hdr_vectors_and_round_trips() {
        let source = r#"unsafe extern "C" fn game_escapeair(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        KineticModule::add_speed(agent.module_accessor, &Vector3f{x: 0.72, y: -1.5, z: 0.0});
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_kinetic_add_speed_events();
        assert_eq!(
            events,
            vec![crate::data::KineticAddSpeedEvent {
                frame: 1,
                call: crate::data::KineticAddSpeedCall {
                    speed_x: 0.72,
                    speed_y: -1.5,
                },
                site: 0,
            }]
        );
        let emitted = preview_game_fn(&script, "escape_air");
        assert!(emitted.contains(
            "KineticModule::add_speed(agent.module_accessor, &Vector3f{x: 0.72, y: -1.5, z: 0.0});"
        ));
        assert_eq!(
            parse_acmd_script(&emitted).to_kinetic_add_speed_events(),
            events
        );

        let hdr = source.replace("agent.module_accessor", "boma");
        assert_eq!(
            parse_acmd_script(&hdr).to_kinetic_add_speed_events(),
            events
        );
    }

    #[test]
    fn work_module_flags_parse_standard_and_hdr_and_round_trip() {
        let source = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD);
        WorkModule::off_flag(agent.module_accessor, 17);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_work_flag_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame, 4);
        assert_eq!(events[0].site, 0);
        assert_eq!(events[0].call.action, crate::data::WorkFlagAction::On);
        assert_eq!(
            events[0].call.flag,
            "*FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD"
        );
        assert_eq!(events[1].call.action, crate::data::WorkFlagAction::Off);
        assert_eq!(events[1].call.flag, "17");

        let emitted = preview_game_fn(&script, "attack_air_n");
        assert!(emitted.contains(
            "WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD);"
        ));
        assert!(emitted.contains("WorkModule::off_flag(agent.module_accessor, 17);"));
        assert_eq!(parse_acmd_script(&emitted).to_work_flag_events(), events);

        let hdr = source.replace("agent.module_accessor", "boma");
        assert_eq!(parse_acmd_script(&hdr).to_work_flag_events(), events);
    }

    #[test]
    fn work_module_transition_terms_parse_standard_and_hdr_and_round_trip() {
        let source = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        WorkModule::enable_transition_term(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN);
        WorkModule::unable_transition_term(agent.module_accessor, 17);
        WorkModule::enable_transition_term_group(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_LANDING);
        WorkModule::unable_transition_term_group_ex(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_work_transition_term_events();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].frame, 4);
        assert_eq!(events[0].site, 0);
        assert_eq!(
            events[0].call.action,
            crate::data::WorkTransitionTermAction::Enable
        );
        assert_eq!(
            events[0].call.transition_term,
            "*FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN"
        );
        assert_eq!(
            events[1].call.action,
            crate::data::WorkTransitionTermAction::Unable
        );
        assert_eq!(events[1].call.transition_term, "17");
        assert_eq!(
            events[2].call.action,
            crate::data::WorkTransitionTermAction::EnableGroup
        );
        assert_eq!(
            events[2].call.transition_term,
            "*FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_LANDING"
        );
        assert_eq!(
            events[3].call.action,
            crate::data::WorkTransitionTermAction::UnableGroupEx
        );
        assert_eq!(
            events[3].call.transition_term,
            "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI"
        );

        let emitted = preview_game_fn(&script, "attack_air_n");
        assert!(emitted.contains(
            "WorkModule::enable_transition_term(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN);"
        ));
        assert!(emitted.contains("WorkModule::unable_transition_term(agent.module_accessor, 17);"));
        assert!(emitted.contains(
            "WorkModule::enable_transition_term_group(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_LANDING);"
        ));
        assert!(emitted.contains(
            "WorkModule::unable_transition_term_group_ex(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI);"
        ));
        assert_eq!(
            parse_acmd_script(&emitted).to_work_transition_term_events(),
            events
        );

        let hdr = source
            .replace(
                "WorkModule::enable_transition_term(agent.module_accessor",
                "WorkModule::enable_transition_term(boma",
            )
            .replace(
                "WorkModule::unable_transition_term(agent.module_accessor",
                "WorkModule::unable_transition_term(boma",
            );
        let hdr_script = parse_acmd_script(&hdr);
        let hdr_events = hdr_script.to_work_transition_term_events();
        assert_eq!(hdr_events.len(), 4);
        assert_eq!(
            hdr_events[0].call.receiver,
            crate::data::WorkTransitionTermReceiver::Boma
        );
        assert_eq!(
            hdr_events[1].call.receiver,
            crate::data::WorkTransitionTermReceiver::Boma
        );
        let hdr_preview = preview_game_fn(&hdr_script, "attack_air_n");
        assert!(hdr_preview.contains(
            "WorkModule::enable_transition_term(boma, *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN);"
        ));
        assert!(hdr_preview.contains("WorkModule::unable_transition_term(boma, 17);"));
        assert!(hdr_preview.contains(
            "WorkModule::enable_transition_term_group(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_LANDING);"
        ));
    }

    #[test]
    fn transition_term_parser_only_types_one_direct_standalone_statement() {
        let call = "WorkModule::enable_transition_term(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN);";
        assert!(matches!(
            parse_work_transition_term_call(call),
            Some(ExcuteStmt::WorkTransitionTerm(_))
        ));
        let boma = call.replace("agent.module_accessor", "boma");
        assert!(matches!(
            parse_work_transition_term_call(&boma),
            Some(ExcuteStmt::WorkTransitionTerm(
                crate::data::WorkTransitionTermCall {
                    receiver: crate::data::WorkTransitionTermReceiver::Boma,
                    ..
                }
            ))
        ));
        assert!(parse_work_transition_term_call(
            "WorkModule::enable_transition_term_group(boma, *TERM);"
        )
        .is_none());

        let rejected = [
            format!("prefix {call}"),
            format!("Other::WorkModule::{}", &call["WorkModule::".len()..]),
            format!("wrapped({})", call.trim_end_matches(';')),
            format!("{call} WorkModule::unable_transition_term(agent.module_accessor, 2);"),
            format!("{call} // trailing comment"),
            format!("/* leading comment */ {call}"),
            "WorkModule::enable_transition_term(agent.module_accessor, /* arg */ *TERM);".into(),
            "WorkModule::enable_transition_term(agent.module_accessor, *TERM) + 1;".into(),
            "WorkModule::enable_transition_term(agent.module_accessor, *TERM); /* tail */".into(),
            format!("\"{}\";", call.trim_end_matches(';')),
        ];
        for source_line in &rejected {
            assert!(
                parse_work_transition_term_call(source_line).is_none(),
                "unexpectedly typed non-standalone form: {source_line}"
            );
        }

        let source = format!(
            "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n{}\n    }}\n}}\n",
            rejected
                .iter()
                .map(|line| format!("        {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let script = parse_acmd_script(&source);
        assert!(script.to_work_transition_term_events().is_empty());
        let emitted = preview_game_fn(&script, "x");
        for source_line in rejected {
            assert!(
                emitted.contains(source_line.trim()),
                "opaque source form was not preserved: {source_line}"
            );
        }
    }

    #[test]
    fn work_module_value_setters_parse_export_and_round_trip_the_measured_shape() {
        let source = r#"unsafe extern "C" fn game_itemgrasspull(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        WorkModule::set_int(agent.module_accessor, *FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP, *FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_NEXT_STEP);
    }
    frame(agent.lua_state_agent, 47.0);
    if is_excute(agent) {
        WorkModule::set_float(agent.module_accessor, 5.0, *FIGHTER_INSTANCE_WORK_ID_FLOAT_FINISH_CAMERA_THROW_RAY_LENGTH);
    }
    frame(agent.lua_state_agent, 70.0);
    if is_excute(agent) {
        WorkModule::set_int64(boma, hash40("fall_damage") as i64, FIGHTER_STATUS_FINAL_WORK_INT_REQUEST_LOOP_DAMAGE_MOTION);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_work_module_set_events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].frame, 5);
        assert_eq!(events[0].call.kind, crate::data::WorkModuleSetKind::Int);
        assert_eq!(events[0].call.value, "*FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP");
        assert_eq!(
            events[0].call.slot,
            "*FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_NEXT_STEP"
        );
        assert_eq!(events[1].frame, 47);
        assert_eq!(events[1].call.kind, crate::data::WorkModuleSetKind::Float);
        assert_eq!(events[1].call.value, "5.0");
        assert_eq!(events[2].frame, 70);
        assert_eq!(events[2].call.kind, crate::data::WorkModuleSetKind::Int64);
        assert_eq!(events[2].call.receiver, "boma");
        assert_eq!(events[2].call.value, "hash40(\"fall_damage\") as i64");
        assert_eq!(
            events[2].call.slot,
            "FIGHTER_STATUS_FINAL_WORK_INT_REQUEST_LOOP_DAMAGE_MOTION"
        );

        let emitted = preview_game_fn(&script, "item_grass_pull");
        assert!(emitted.contains(
            "WorkModule::set_int(agent.module_accessor, *FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP, *FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_NEXT_STEP);"
        ));
        assert!(emitted.contains(
            "WorkModule::set_float(agent.module_accessor, 5.0, *FIGHTER_INSTANCE_WORK_ID_FLOAT_FINISH_CAMERA_THROW_RAY_LENGTH);"
        ));
        assert!(emitted.contains(
            "WorkModule::set_int64(boma, hash40(\"fall_damage\") as i64, FIGHTER_STATUS_FINAL_WORK_INT_REQUEST_LOOP_DAMAGE_MOTION);"
        ));
        assert_eq!(
            parse_acmd_script(&emitted).to_work_module_set_events(),
            events
        );
    }

    #[test]
    fn work_module_value_setter_parser_requires_a_standalone_call() {
        let lines = [
            "let saved = WorkModule::set_float(agent.module_accessor, 0.0, *FIGHTER_MIISWORDSMAN_STATUS_FINAL_WORK_ID_FLOAT_WAVE_ANGLE);",
            "wrap(WorkModule::set_float(agent.module_accessor, 0.0, *FIGHTER_MIISWORDSMAN_STATUS_FINAL_WORK_ID_FLOAT_WAVE_ANGLE));",
            "WorkModule::set_float(agent.module_accessor, 0.0, *FIGHTER_MIISWORDSMAN_STATUS_FINAL_WORK_ID_FLOAT_WAVE_ANGLE); custom_call();",
            "WorkModule::set_float(agent.module_accessor, 0.0, *FIGHTER_MIISWORDSMAN_STATUS_FINAL_WORK_ID_FLOAT_WAVE_ANGLE,);",
            "WorkModule::set_float(agent.module_accessor, 0.0, *FIGHTER_MIISWORDSMAN_STATUS_FINAL_WORK_ID_FLOAT_WAVE_ANGLE); // trailing source",
            "WorkModule::set_int64(agent.module_accessor, hash40(\"final_air_attack\") as i64 + injected(), *FIGHTER_ELIGHT_INSTANCE_WORK_ID_INT_ESWORD_INHERIT_OPEN_MOTION_KIND);",
            "shadow::WorkModule::set_float(agent.module_accessor, 0.0, *FIGHTER_MIISWORDSMAN_STATUS_FINAL_WORK_ID_FLOAT_WAVE_ANGLE);",
            "/* WorkModule::set_float(agent.module_accessor, 0.0, *FIGHTER_MIISWORDSMAN_STATUS_FINAL_WORK_ID_FLOAT_WAVE_ANGLE); */",
        ];
        for line in lines {
            let source = format!(
                "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}\n"
            );
            let script = parse_acmd_script(&source);
            assert!(
                script.to_work_module_set_events().is_empty(),
                "non-standalone setter was typed: {line}"
            );
            let emitted = preview_game_fn(&script, "x");
            assert!(
                emitted.contains(line),
                "unrecognized source changed: {line}\n{emitted}"
            );
        }

        let accepted = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        WorkModule::set_int64(agent.module_accessor, hash40("final_air_attack") as i64, *FIGHTER_ELIGHT_INSTANCE_WORK_ID_INT_ESWORD_INHERIT_OPEN_MOTION_KIND);
    }
}
"#;
        let script = parse_acmd_script(accepted);
        assert_eq!(script.to_work_module_set_events().len(), 1);
        assert!(preview_game_fn(&script, "x").contains(
            r#"WorkModule::set_int64(agent.module_accessor, hash40("final_air_attack") as i64, *FIGHTER_ELIGHT_INSTANCE_WORK_ID_INT_ESWORD_INHERIT_OPEN_MOTION_KIND);"#
        ));
    }

    #[test]
    fn slow_module_whole_parses_only_the_measured_native_call_shapes() {
        use crate::data::{ExcuteStmt, SlowWholeCall, SLOW_WHOLE_SET_PAIRS};

        for pair in SLOW_WHOLE_SET_PAIRS {
            let source = format!(
                "SlowModule::set_whole(agent.module_accessor, {}, {});",
                pair.multiplier, pair.frames
            );
            assert!(matches!(
                parse_slow_module_whole_call(&source),
                Some(ExcuteStmt::SlowWhole(SlowWholeCall::Set { multiplier, frames }))
                    if multiplier == pair.multiplier && frames == pair.frames
            ));
        }
        assert!(matches!(
            parse_slow_module_whole_call("SlowModule::clear_whole(agent.module_accessor);"),
            Some(ExcuteStmt::SlowWhole(SlowWholeCall::Clear))
        ));

        // A syntactically valid but unmeasured value is retained as a typed native call so the
        // timeline can preserve it without authorizing edits.
        assert!(matches!(
            parse_slow_module_whole_call("SlowModule::set_whole(agent.module_accessor, 7, 0);"),
            Some(ExcuteStmt::SlowWhole(SlowWholeCall::Set {
                multiplier: 7,
                frames: 0
            }))
        ));

        let rejected = [
            "SlowModule::set_whole(boma, 4, 0);",
            "SlowModule::set_whole(other.module_accessor, 4, 0);",
            "SlowModule::set_whole(agent.module_accessor, 4);",
            "SlowModule::set_whole(agent.module_accessor, 4, 0, 1);",
            "SlowModule::set_whole(agent.module_accessor, 4, 0,);",
            "SlowModule::set_whole(agent.module_accessor, 256, 0);",
            "SlowModule::set_whole(agent.module_accessor, 4, 2147483648);",
            "SlowModule::set_whole(agent.module_accessor, +4, 0);",
            "SlowModule::set_whole(agent.module_accessor, 04, 0);",
            "SlowModule::set_whole(agent.module_accessor, 4, -0);",
            "SlowModule::clear_whole(agent.module_accessor, 0);",
            "SlowModule::clear_whole(agent.module_accessor,);",
            "let x = SlowModule::set_whole(agent.module_accessor, 4, 0);",
            "wrap(SlowModule::set_whole(agent.module_accessor, 4, 0));",
            "SlowModule::set_whole(agent.module_accessor, 4, 0); other();",
            "SlowModule::set_whole(agent.module_accessor, 4, 0); // trailing",
            "/* comment */ SlowModule::clear_whole(agent.module_accessor);",
            "shadow::SlowModule::clear_whole(agent.module_accessor);",
            "SlowModule::clear_whole(agent.module_accessor) + 0;",
        ];
        for line in rejected {
            assert!(
                parse_slow_module_whole_call(line).is_none(),
                "non-standalone slow call was typed: {line}"
            );
        }

        let source = format!(
            "unsafe extern \"C\" fn game_slow_whole(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n{}\n    }}\n}}\n",
            rejected
                .iter()
                .map(|line| format!("        {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let parsed = parse_acmd_script(&source);
        let emitted = preview_game_fn(&parsed, "slow_whole");
        for line in rejected {
            assert!(
                emitted.contains(line),
                "opaque call was lost: {line}\n{emitted}"
            );
        }
        assert_json_eq!(&parsed, parse_acmd_script(&emitted));
    }

    #[test]
    fn slow_module_whole_emits_measured_set_clear_pairs_without_shape_drift() {
        use crate::data::{AcmdScript, AcmdStmt, ExcuteStmt, SlowWholeCall, SLOW_WHOLE_SET_PAIRS};

        let commands = SLOW_WHOLE_SET_PAIRS
            .iter()
            .map(|pair| {
                ExcuteStmt::SlowWhole(SlowWholeCall::Set {
                    multiplier: pair.multiplier,
                    frames: pair.frames,
                })
            })
            .chain(std::iter::once(ExcuteStmt::SlowWhole(SlowWholeCall::Clear)))
            .collect::<Vec<_>>();
        let source = AcmdScript {
            stmts: vec![AcmdStmt::Excute(commands)],
        };
        let emitted = preview_game_fn(&source, "slow_whole");
        assert!(emitted.contains("SlowModule::set_whole(agent.module_accessor, 4, 0);"));
        assert!(emitted.contains("SlowModule::set_whole(agent.module_accessor, 20, 30);"));
        assert!(emitted.contains("SlowModule::clear_whole(agent.module_accessor);"));
        assert_json_eq!(parse_acmd_script(&emitted), source);
    }

    #[test]
    fn slow_opponent_accepts_only_closed_finite_direct_game_execute_calls() {
        use crate::data::{ExcuteStmt, SlowOpponentCall};

        let observed_pairs = [
            ("2", "60"),
            ("2.5", "5"),
            ("3", "30"),
            ("3", "40"),
            ("4", "30"),
            ("4", "45"),
            ("4", "50"),
            ("4", "60"),
            ("4", "120"),
            ("5", "20"),
            ("5", "30"),
            ("5", "40"),
            ("5", "50"),
            ("5", "60"),
            ("5", "80"),
            ("5", "90"),
            ("6", "30"),
            ("6", "55"),
            ("6", "60"),
            ("6", "85"),
            ("6", "120"),
            ("7", "22"),
            ("7", "30"),
            ("7", "120"),
            ("8", "34"),
            ("8", "50"),
            ("8", "60"),
            ("8", "70"),
            ("9", "45"),
            ("9", "130"),
            ("10", "60"),
            ("10", "64"),
            ("10", "65"),
            ("10", "70"),
            ("10", "75"),
            ("10", "90"),
            ("12", "44"),
            ("13", "60"),
            ("14", "60"),
            ("15", "30"),
            ("15", "40"),
            ("15", "60"),
            ("15", "70"),
            ("20", "55"),
            ("20", "60"),
            ("20", "65"),
            ("20", "75"),
            ("20", "85"),
            ("30", "25"),
            ("30", "30"),
            ("30", "48"),
            ("30", "60"),
            ("30", "70"),
            ("30", "80"),
            ("30", "100"),
            ("40", "57"),
            ("80", "50"),
        ];
        let mut distinct = std::collections::BTreeSet::new();
        for (slow_mul, slow_time) in observed_pairs {
            let source = format!("macros::SLOW_OPPONENT(agent, {slow_mul}, {slow_time});");
            let Some(ExcuteStmt::SlowOpponent(call)) = parse_slow_opponent_call(&source) else {
                panic!("observed pair was not admitted: {source}");
            };
            distinct.insert((call.slow_mul.to_bits(), call.slow_time.to_bits()));
        }
        assert_eq!(
            distinct.len(),
            57,
            "the positive fixture covers the measured pair table"
        );

        let accepted = [
            ("macros::SLOW_OPPONENT(agent, 2, 5);", 2.0, 5.0),
            ("macros::SLOW_OPPONENT(agent, 13.5, 60.25);", 13.5, 60.25),
            ("macros::SLOW_OPPONENT(agent, 80, 130);", 80.0, 130.0),
        ];
        for (source, slow_mul, slow_time) in accepted {
            assert!(matches!(
                parse_slow_opponent_call(source),
                Some(ExcuteStmt::SlowOpponent(SlowOpponentCall { slow_mul: actual_mul, slow_time: actual_time }))
                    if actual_mul == slow_mul && actual_time == slow_time
            ));
        }

        let rejected = [
            "macros::SLOW_OPPONENT(boma, 13, 60);",
            "macros::SLOW_OPPONENT(agent, 1.99, 60);",
            "macros::SLOW_OPPONENT(agent, 80.01, 60);",
            "macros::SLOW_OPPONENT(agent, 13, 4.99);",
            "macros::SLOW_OPPONENT(agent, 13, 130.01);",
            "macros::SLOW_OPPONENT(agent, 13);",
            "macros::SLOW_OPPONENT(agent, 13, 60, 1);",
            "macros::SLOW_OPPONENT(agent, 13, 60,);",
            "macros::SLOW_OPPONENT(agent, f32::NAN, 60);",
            "macros::SLOW_OPPONENT(agent, 13 + 1, 60);",
            "macros::SLOW_OPPONENT(agent, -13, 60);",
            "macros::SLOW_OPPONENT(agent, 13, 60); other();",
            "wrap(macros::SLOW_OPPONENT(agent, 13, 60));",
            "shadow::macros::SLOW_OPPONENT(agent, 13, 60);",
        ];
        for source in rejected {
            assert!(
                parse_slow_opponent_call(source).is_none(),
                "typed: {source}"
            );
        }

        let exact = r#"unsafe extern "C" fn game_slow_opponent(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::SLOW_OPPONENT(agent, 13, 60);
    }
}
"#;
        let parsed = parse_acmd_script(exact);
        assert!(matches!(
            parsed.stmts.as_slice(),
            [crate::data::AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::SlowOpponent(_)])
        ));
        let emitted = preview_game_fn(&parsed, "slow_opponent");
        assert_json_eq!(parse_acmd_script(&emitted), &parsed);

        let wrong_track = crate::move_nodes::timeline::import_for_fighter(
            "kirby",
            crate::move_nodes::model::Track::Effect,
            &parsed,
        );
        assert!(matches!(
            wrong_track.nodes.as_slice(),
            [crate::move_nodes::model::MoveNode {
                kind: crate::move_nodes::model::NodeKind::Preserved { .. },
                ..
            }]
        ));

        fn typed_slow_calls(stmts: &[crate::data::AcmdStmt]) -> usize {
            stmts
                .iter()
                .map(|stmt| match stmt {
                    crate::data::AcmdStmt::Excute(commands) => commands
                        .iter()
                        .filter(|command| matches!(command, ExcuteStmt::SlowOpponent(_)))
                        .count(),
                    crate::data::AcmdStmt::RawBlock { body, .. }
                    | crate::data::AcmdStmt::Loop { body, .. } => typed_slow_calls(body),
                    _ => 0,
                })
                .sum()
        }

        // These are independently valid Rust bodies: an attributed call, a wrapped call, and a
        // nested call remain opaque while a separate direct sibling is still the sole typed call.
        let source = r#"unsafe extern "C" fn game_slow_opponent(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        #[allow(unused)]
        macros::SLOW_OPPONENT(agent, 13, 60);
        consume(macros::SLOW_OPPONENT(agent, 13, 60));
        if true { macros::SLOW_OPPONENT(agent, 13, 60); }
        macros::SLOW_OPPONENT(agent, 13, 60);
    }
}
"#;
        let parsed = parse_acmd_script(source);
        assert_eq!(
            typed_slow_calls(&parsed.stmts),
            1,
            "only the direct sibling is typed"
        );
        let emitted = preview_game_fn(&parsed, "slow_opponent");
        assert!(emitted.contains("consume(macros::SLOW_OPPONENT(agent, 13, 60));"));
        assert!(emitted.contains("if true { macros::SLOW_OPPONENT(agent, 13, 60); }"));
        assert_json_eq!(parse_acmd_script(&emitted), parsed);

        let wrapped_only = r#"unsafe extern "C" fn game_wrapped(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        consume(macros::SLOW_OPPONENT(agent, 13, 60));
    }
}
"#;
        let wrapped = parse_acmd_script(wrapped_only);
        assert_eq!(typed_slow_calls(&wrapped.stmts), 0);
        assert!(preview_game_fn(&wrapped, "wrapped")
            .contains("consume(macros::SLOW_OPPONENT(agent, 13, 60));"));

        let nested_only = r#"unsafe extern "C" fn game_nested(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        if true {
            macros::SLOW_OPPONENT(agent, 13, 60);
        }
    }
}
"#;
        let nested = parse_acmd_script(nested_only);
        assert_eq!(typed_slow_calls(&nested.stmts), 0);
        assert!(
            preview_game_fn(&nested, "nested").contains("macros::SLOW_OPPONENT(agent, 13, 60);")
        );

        let attributed_only = r#"unsafe extern "C" fn game_attributed(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        #[allow(unused)]
        macros::SLOW_OPPONENT(agent, 13, 60);
    }
}
"#;
        let attributed = parse_acmd_script(attributed_only);
        assert_eq!(typed_slow_calls(&attributed.stmts), 0);
        assert!(preview_game_fn(&attributed, "attributed")
            .contains("macros::SLOW_OPPONENT(agent, 13, 60);"));
    }

    #[test]
    fn work_module_inc_int_parses_standard_and_hdr_calls_and_round_trips() {
        let source = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        WorkModule::inc_int(agent.module_accessor, *FIGHTER_STATUS_WORK_INT_NEXT_STEP);
    }
    frame(agent.lua_state_agent, 47.0);
    if is_excute(agent) {
        WorkModule::inc_int(boma, 17);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_work_module_inc_int_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame, 5);
        assert_eq!(events[0].call.slot, "*FIGHTER_STATUS_WORK_INT_NEXT_STEP");
        assert_eq!(events[1].frame, 47);
        assert_eq!(events[1].call.slot, "17");

        let emitted = preview_game_fn(&script, "attack_air_n");
        assert!(emitted.contains(
            "WorkModule::inc_int(agent.module_accessor, *FIGHTER_STATUS_WORK_INT_NEXT_STEP);"
        ));
        assert!(emitted.contains("WorkModule::inc_int(agent.module_accessor, 17);"));
        assert_eq!(
            parse_acmd_script(&emitted).to_work_module_inc_int_events(),
            events
        );

        let hdr = source.replace("agent.module_accessor", "boma");
        assert_eq!(
            parse_acmd_script(&hdr).to_work_module_inc_int_events(),
            events
        );
    }

    #[test]
    fn kinetic_energy_parses_standard_and_hdr_suspend_resume_calls_and_round_trips() {
        let source = r#"unsafe extern "C" fn game_attackairlw(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        KineticModule::suspend_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);
        KineticModule::resume_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_kinetic_energy_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame, 1);
        assert_eq!(
            events[0].call.action,
            crate::data::KineticEnergyAction::Suspend
        );
        assert_eq!(
            events[0].call.kinetic_energy_id,
            "*FIGHTER_KINETIC_ENERGY_ID_CONTROL"
        );
        assert_eq!(
            events[1].call.action,
            crate::data::KineticEnergyAction::Resume
        );
        let emitted = preview_game_fn(&script, "attack_air_lw");
        assert!(emitted.contains(
            "KineticModule::suspend_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);"
        ));
        assert!(emitted.contains(
            "KineticModule::resume_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);"
        ));
        assert_eq!(
            parse_acmd_script(&emitted).to_kinetic_energy_events(),
            events
        );

        let hdr = source.replace("agent.module_accessor", "boma");
        assert_eq!(parse_acmd_script(&hdr).to_kinetic_energy_events(), events);
    }

    #[test]
    fn kinetic_clear_speed_all_parses_standard_and_hdr_and_round_trips() {
        let source = r#"unsafe extern "C" fn game_specialhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        KineticModule::clear_speed_all(agent.module_accessor);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_kinetic_clear_speed_all_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].frame, 6);
        assert_eq!(events[0].site, 0);
        let emitted = preview_game_fn(&script, "special_hi");
        assert!(emitted.contains("KineticModule::clear_speed_all(agent.module_accessor);"));
        assert_eq!(
            parse_acmd_script(&emitted).to_kinetic_clear_speed_all_events(),
            events
        );

        let hdr = source.replace("agent.module_accessor", "boma");
        assert_eq!(
            parse_acmd_script(&hdr).to_kinetic_clear_speed_all_events(),
            events
        );
    }

    #[test]
    fn kinetic_set_consider_ground_friction_parses_standard_and_hdr_and_round_trips() {
        let source = r#"unsafe extern "C" fn game_specialhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        KineticModule::set_consider_ground_friction(agent.module_accessor, true, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_kinetic_set_consider_ground_friction_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].frame, 6);
        assert!(events[0].call.consider_ground_friction);
        assert_eq!(
            events[0].call.kinetic_energy_attribute,
            "*KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN"
        );
        let emitted = preview_game_fn(&script, "special_hi");
        assert!(emitted.contains(
            "KineticModule::set_consider_ground_friction(agent.module_accessor, true, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);"
        ));
        assert_eq!(
            parse_acmd_script(&emitted).to_kinetic_set_consider_ground_friction_events(),
            events
        );

        let hdr = source.replace("agent.module_accessor", "boma");
        assert_eq!(
            parse_acmd_script(&hdr).to_kinetic_set_consider_ground_friction_events(),
            events
        );
    }

    #[test]
    fn kinetic_energy_enable_and_unable_round_trip_with_measured_ids() {
        let source = r#"unsafe extern "C" fn game_specialhi(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        KineticModule::enable_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_GRAVITY);
        KineticModule::unable_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_kinetic_energy_events();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].call.action,
            crate::data::KineticEnergyAction::Enable
        );
        assert_eq!(
            events[1].call.action,
            crate::data::KineticEnergyAction::Unable
        );
        let emitted = preview_game_fn(&script, "special_hi");
        assert!(emitted.contains(
            "KineticModule::enable_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_GRAVITY);"
        ));
        assert!(emitted.contains(
            "KineticModule::unable_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);"
        ));
        assert_eq!(
            parse_acmd_script(&emitted).to_kinetic_energy_events(),
            events
        );
    }

    #[test]
    fn direct_change_kinetic_parses_and_exports_the_measured_game_shape() {
        let source = r#"unsafe extern "C" fn game_escapeair(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 19.0);
    if macros::is_excute(agent) {
        KineticModule::change_kinetic(agent.module_accessor, *FIGHTER_KINETIC_TYPE_FALL);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_change_kinetic_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].frame, 19);
        assert_eq!(events[0].site, 0);
        assert_eq!(events[0].call.kinetic_type, "*FIGHTER_KINETIC_TYPE_FALL");

        let emitted = preview_game_fn(&script, "escape_air");
        assert!(emitted.contains(
            "KineticModule::change_kinetic(agent.module_accessor, *FIGHTER_KINETIC_TYPE_FALL);"
        ));
        assert_eq!(
            parse_acmd_script(&emitted).to_change_kinetic_events(),
            events
        );
    }

    #[test]
    fn direct_change_kinetic_parses_the_measured_boma_shape() {
        let source = r#"unsafe extern "C" fn game_passiveceil(agent: &mut L2CAgentBase) {
    let boma = agent.boma();
    frame(agent.lua_state_agent, 18.0);
    if is_excute(agent) {
        KineticModule::change_kinetic(boma, *FIGHTER_KINETIC_TYPE_PASSIVE_CEIL);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_change_kinetic_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].frame, 18);
        assert_eq!(
            events[0].call.kinetic_type,
            "*FIGHTER_KINETIC_TYPE_PASSIVE_CEIL"
        );

        let emitted = preview_game_fn(&script, "passive_ceil");
        assert!(emitted.contains(
            "KineticModule::change_kinetic(agent.module_accessor, *FIGHTER_KINETIC_TYPE_PASSIVE_CEIL);"
        ));
        assert_eq!(
            parse_acmd_script(&emitted).to_change_kinetic_events(),
            events
        );
    }

    #[test]
    fn motion_module_set_rate_parse_export_round_trips_standard_and_hdr_shapes() {
        let source = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        MotionModule::set_rate(agent.module_accessor, 0.8);
    }
    frame(agent.lua_state_agent, 12.0);
    if is_excute(agent) {
        MotionModule::set_rate(boma, 1);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_motion_module_set_rate_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame, 6);
        assert_eq!(events[0].call.rate, 0.8);
        assert_eq!(events[1].frame, 12);
        assert_eq!(events[1].call.rate, 1.0);

        let emitted = preview_game_fn(&script, "attack_air_n");
        assert!(emitted.contains("MotionModule::set_rate(agent.module_accessor, 0.8);"));
        assert!(emitted.contains("MotionModule::set_rate(agent.module_accessor, 1.0);"));
        assert_eq!(
            parse_acmd_script(&emitted).to_motion_module_set_rate_events(),
            events
        );
    }

    #[test]
    fn motion_module_set_helper_calculation_parse_export_round_trips_standard_and_hdr_shapes() {
        let source = r#"unsafe extern "C" fn game_specialairhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        MotionModule::set_helper_calculation(agent.module_accessor, false);
    }
    frame(agent.lua_state_agent, 6.0);
    if is_excute(agent) {
        MotionModule::set_helper_calculation(boma, true);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_motion_module_set_helper_calculation_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame, 3);
        assert!(!events[0].call.enabled);
        assert_eq!(events[1].frame, 6);
        assert!(events[1].call.enabled);

        let emitted = preview_game_fn(&script, "special_air_hi");
        assert!(
            emitted.contains("MotionModule::set_helper_calculation(agent.module_accessor, false);")
        );
        assert!(
            emitted.contains("MotionModule::set_helper_calculation(agent.module_accessor, true);")
        );
        assert_eq!(
            parse_acmd_script(&emitted).to_motion_module_set_helper_calculation_events(),
            events
        );
    }

    #[test]
    fn motion_module_set_rate_partial_parse_export_round_trips_standard_and_hdr_shapes() {
        let source = r#"unsafe extern "C" fn game_specialairhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        MotionModule::set_rate_partial(agent.module_accessor, *FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY, 1.25);
    }
    frame(agent.lua_state_agent, 6.0);
    if is_excute(agent) {
        MotionModule::set_rate_partial(boma, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 0);
    }
}
"#;
        let script = parse_acmd_script(source);
        let events = script.to_motion_module_set_rate_partial_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame, 3);
        assert_eq!(
            events[0].call.part_kind,
            "*FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY"
        );
        assert_eq!(events[0].call.rate, 1.25);
        assert_eq!(events[1].frame, 6);
        assert_eq!(
            events[1].call.part_kind,
            "*FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL"
        );
        assert_eq!(events[1].call.rate, 0.0);

        let emitted = preview_game_fn(&script, "special_air_hi");
        assert!(emitted.contains(
            "MotionModule::set_rate_partial(agent.module_accessor, *FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY, 1.25);"
        ));
        assert!(emitted.contains(
            "MotionModule::set_rate_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 0.0);"
        ));
        let reparsed = parse_acmd_script(&emitted).to_motion_module_set_rate_partial_events();
        assert_eq!(reparsed.len(), events.len());
        assert_eq!(reparsed[0].call, events[0].call);
        assert_eq!(reparsed[1].call.rate, events[1].call.rate);
        assert_eq!(
            reparsed[1].call.part_kind,
            "*FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL"
        );
    }

    #[test]
    fn motion_module_set_frame_partial_parse_export_round_trips_measured_and_native_shapes() {
        let source = r#"unsafe extern "C" fn expression_appeallwl(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 2);
    }
    frame(agent.lua_state_agent, 13.0);
    if is_excute(agent) {
        MotionModule::set_frame_partial(boma, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 3);
    }
    frame(agent.lua_state_agent, 18.0);
    if is_excute(agent) {
        MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 0, false);
    }
    frame(agent.lua_state_agent, 23.0);
    if is_excute(agent) {
        MotionModule::set_frame_partial_sync_anim_cmd(boma, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 4);
    }
}
"#;
        let script = parse_expression_script(source);
        let events = script.to_motion_module_set_frame_partial_events();

        // The corpus's three-argument form and the native four-argument form are one family. The
        // difference between them is whether the author wrote `sync`, which is why the omission
        // is `None` rather than the `true` the game substitutes for it.
        assert_eq!(
            events
                .iter()
                .map(|event| (event.frame, event.call.frame, event.call.sync, event.site))
                .collect::<Vec<_>>(),
            [
                (8, 2.0, None, 0),
                (13, 3.0, None, 1),
                (18, 0.0, Some(false), 2),
            ]
        );
        assert!(events
            .iter()
            .all(|event| event.call.part_kind == "*FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL"));

        // An omitted `sync` is `true` at runtime; an authored `false` is the only way to get the
        // other behaviour, so the two must not collapse into one another.
        assert!(events[0].call.effective_sync());
        assert!(!events[2].call.effective_sync());

        // The sync-anim-cmd wrapper is a different binding with no corpus calls, so it is still
        // carried verbatim rather than folded into this family.
        let raw = script.to_raw_partial_frame_events();
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].frame, 23);
        assert_eq!(raw[0].source, "MotionModule::set_frame_partial_sync_anim_cmd(boma, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 4);");

        let emitted = preview_expression_fn(&script, "appeal_lw_l");
        for line in [
            "MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 2.0);",
            "MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 3.0);",
            "MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 0.0, false);",
            "MotionModule::set_frame_partial_sync_anim_cmd(boma, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 4);",
        ] {
            assert!(emitted.contains(line), "partial-frame call was lost: {line}");
        }
        assert_eq!(
            parse_expression_script(&emitted).to_motion_module_set_frame_partial_events(),
            events
        );
    }

    #[test]
    fn ftilt_and_fsmash_motions_fall_back_to_their_script_spelling() {
        assert_eq!(
            move_script_aliases("attack_s3_s"),
            vec!["attack_s3_s".to_string(), "attack_s3".to_string()]
        );
        assert_eq!(
            move_script_aliases("attack_s4_s"),
            vec!["attack_s4_s".to_string(), "attack_s4".to_string()]
        );
        assert_eq!(
            move_script_aliases("attack_s3_s2"),
            vec!["attack_s3_s2".to_string(), "attack_s3_2".to_string()]
        );
        assert_eq!(
            move_script_aliases("attack_s3_s3"),
            vec!["attack_s3_s3".to_string(), "attack_s3_3".to_string()]
        );
        assert_eq!(
            move_script_aliases("attack_air_n"),
            vec!["attack_air_n".to_string()]
        );
        assert_eq!(
            acmd_script_candidates("game", "attack_s3_s"),
            vec!["game_attacks3s".to_string(), "game_attacks3".to_string()]
        );
        assert_eq!(
            acmd_script_candidates("game", "attack_s4_s"),
            vec!["game_attacks4s".to_string(), "game_attacks4".to_string()]
        );
        assert_eq!(move_name_to_pascal("attack_s3"), "AttackS3".to_string());
        assert_eq!(
            acmd_script_name("game", "attack_s3"),
            "game_attacks3".to_string()
        );
    }

    #[test]
    fn script_name_aliases_cover_both_spellings() {
        assert_eq!(
            script_name_aliases("game_attacks3s"),
            vec!["game_attacks3s".to_string(), "game_attacks3".to_string()]
        );
        assert_eq!(
            script_name_aliases("game_attacks3"),
            vec!["game_attacks3".to_string(), "game_attacks3s".to_string()]
        );
        assert_eq!(
            script_name_aliases("effect_attacks4s"),
            vec![
                "effect_attacks4s".to_string(),
                "effect_attacks4".to_string()
            ]
        );
        assert_eq!(
            script_name_aliases("game_attacks32"),
            vec!["game_attacks32".to_string(), "game_attacks3s2".to_string()]
        );
        assert_eq!(
            script_name_aliases("game_attackairn"),
            vec!["game_attackairn".to_string()]
        );
    }

    #[test]
    fn export_resolves_ftilt_to_the_script_the_game_calls() {
        // A fighter name that can never have a populated script cache, so the
        // static mapping decides without touching the network.
        let nowhere = "visionary_test_no_such_fighter_34";
        assert_eq!(
            resolve_script_move_name(nowhere, "attack_s3_s"),
            "attack_s3".to_string()
        );
        assert_eq!(
            resolve_script_move_name(nowhere, "attack_s4_s"),
            "attack_s4".to_string()
        );
        assert_eq!(
            resolve_script_move_name(nowhere, "attack_s3_s2"),
            "attack_s3_2".to_string()
        );
        assert_eq!(
            resolve_script_move_name("ryu", "attack_s3_s"),
            "attack_s3_s".to_string()
        );
        assert_eq!(
            resolve_script_move_name("ken", "attack_s3_s"),
            "attack_s3_s".to_string()
        );
        assert_eq!(
            resolve_script_move_name("mario", "attack_air_n"),
            "attack_air_n".to_string()
        );
        // The resolved spelling emits the function the game installs.
        let script = parse_acmd_script(
            "unsafe extern \"C\" fn game_attacks3(agent: &mut L2CAgentBase) {\n}\n",
        );
        let emitted = preview_game_fn(&script, &resolve_script_move_name(nowhere, "attack_s3_s"));
        assert!(
            emitted.contains("fn game_attacks3("),
            "expected game_attacks3, got: {emitted}"
        );
    }

    fn optional_feedback_calls(
        stmts: &[crate::data::AcmdStmt],
    ) -> Vec<crate::data::OptionalHitFeedbackCall> {
        use crate::data::{AcmdStmt, ExcuteStmt};
        let mut calls = Vec::new();
        for stmt in stmts {
            match stmt {
                AcmdStmt::Excute(commands) => calls.extend(commands.iter().filter_map(|command| {
                    if let ExcuteStmt::OptionalHitFeedback(call) = command {
                        Some(call.clone())
                    } else {
                        None
                    }
                })),
                AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. } => {
                    calls.extend(optional_feedback_calls(body));
                }
                _ => {}
            }
        }
        calls
    }

    #[test]
    fn optional_hit_feedback_parses_emits_and_serializes_closed_hash40_values() {
        use crate::data::{Hash40Literal, OptionalHitFeedbackKind};

        let source = r#"unsafe extern "C" fn game_feedback(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_optional_hit_effect(agent.module_accessor, 2, Hash40::new("sys_hit_fire"));
        AttackModule::set_optional_hit_sound(agent.module_accessor, 4, Hash40::new_raw(0x148397b09f));
        AttackModule::set_optional_hit_effect(agent.module_accessor, 1, Hash40::new("custom \"feedback\"\\variant"));
        AttackModule::set_optional_hit_sound(agent.module_accessor, 0, Hash40::new("line\nλ\t\u{1f}"));
        AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new_raw(0xffffffffffffffff));
    }
}
"#;
        let parsed = parse_acmd_script(source);
        let calls = optional_feedback_calls(&parsed.stmts);
        assert_eq!(calls.len(), 5, "all five direct calls must be typed");
        assert_eq!(calls[0].kind, OptionalHitFeedbackKind::Effect);
        assert_eq!(calls[0].arg2, 2);
        assert_eq!(calls[0].hash, Hash40Literal::Name("sys_hit_fire".into()));
        assert_eq!(calls[1].kind, OptionalHitFeedbackKind::Sound);
        assert_eq!(calls[1].hash, Hash40Literal::Raw(0x148397b09f));
        assert_eq!(
            calls[2].hash,
            Hash40Literal::Name("custom \"feedback\"\\variant".into())
        );
        assert_eq!(calls[3].hash, Hash40Literal::Name("line\nλ\t\u{1f}".into()));
        assert_eq!(calls[4].hash, Hash40Literal::Raw(u64::MAX));
        assert_eq!(
            serde_json::to_value(crate::data::ExcuteStmt::OptionalHitFeedback(
                calls[0].clone()
            ))
            .unwrap(),
            serde_json::json!({"optional_hit_feedback": {"kind": "effect", "arg2": 2, "hash": {"name": "sys_hit_fire"}}}),
            "native command and closed kind serde IDs remain stable"
        );

        let emitted = preview_game_fn(&parsed, "feedback");
        assert!(emitted.contains("AttackModule::set_optional_hit_effect(agent.module_accessor, 2, Hash40::new(\"sys_hit_fire\"));"));
        assert!(emitted.contains("AttackModule::set_optional_hit_sound(agent.module_accessor, 4, Hash40::new_raw(0x148397b09f));"));
        assert!(emitted.contains("AttackModule::set_optional_hit_effect(agent.module_accessor, 1, Hash40::new(\"custom \\\"feedback\\\"\\\\variant\"));"));
        assert!(
            syn::parse_file(&emitted).is_ok(),
            "Debug string escaping must emit valid Rust for control and Unicode values"
        );
        assert!(
            emitted.contains("Hash40::new_raw(0xffffffffffffffff)"),
            "raw Hash40 values retain full u64 width without masking"
        );
        assert_json_eq!(
            parse_acmd_script(&emitted),
            &parsed,
            "whole Execute tree must round-trip through source"
        );

        let name_json =
            serde_json::to_value(Hash40Literal::Name("modded_feedback".into())).unwrap();
        let raw_json = serde_json::to_value(Hash40Literal::Raw(0x1234)).unwrap();
        assert_eq!(name_json["name"], "modded_feedback");
        assert_eq!(raw_json["raw"], 0x1234);
        assert_eq!(
            serde_json::from_value::<Hash40Literal>(name_json).unwrap(),
            Hash40Literal::Name("modded_feedback".into())
        );
        assert_eq!(
            serde_json::from_value::<Hash40Literal>(raw_json).unwrap(),
            Hash40Literal::Raw(0x1234)
        );
        assert_eq!(
            serde_json::from_value::<Hash40Literal>(
                serde_json::to_value(Hash40Literal::Raw(u64::MAX)).unwrap(),
            )
            .unwrap(),
            Hash40Literal::Raw(u64::MAX),
            "raw values preserve all 64 ABI bits through serde"
        );
        assert!(!Hash40Literal::Name(String::new()).is_valid());
        assert!(!Hash40Literal::Name("x".repeat(256)).is_valid());
        assert!(!Hash40Literal::Name("x\0y".into()).is_valid());
        assert!(!crate::data::OptionalHitFeedbackCall {
            kind: OptionalHitFeedbackKind::Effect,
            arg2: -1,
            hash: Hash40Literal::Name("valid".into()),
        }
        .is_valid());
        assert!(!crate::data::OptionalHitFeedbackCall {
            kind: OptionalHitFeedbackKind::Effect,
            arg2: 5,
            hash: Hash40Literal::Name("valid".into()),
        }
        .is_valid());
    }

    #[test]
    fn optional_hit_feedback_rejects_unmeasured_shape_and_context_without_losing_source() {
        use crate::data::{AcmdStmt, ExcuteStmt};

        // Each source is independently valid Rust; rejection is due to the closed native
        // admission boundary, not a malformed sibling or an empty parse result.
        let cases = [
            ("wrong receiver", "AttackModule::set_optional_hit_effect(other.module_accessor, 0, Hash40::new(\"x\"));"),
            ("wrong method base", "OtherModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new(\"x\"));"),
            ("wrapper", "consume(AttackModule::set_optional_hit_sound(agent.module_accessor, 0, Hash40::new(\"x\")));"),
            ("attribute", "#[allow(unused)]\n        AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new(\"x\"));"),
            ("nested", "if true {\n            AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new(\"x\"));\n        }"),
            ("negative arg2", "AttackModule::set_optional_hit_effect(agent.module_accessor, -1, Hash40::new(\"x\"));"),
            ("arg2 above observed bound", "AttackModule::set_optional_hit_effect(agent.module_accessor, 5, Hash40::new(\"x\"));"),
            ("nonliteral hash", "AttackModule::set_optional_hit_effect(agent.module_accessor, 0, make_hash());"),
            ("name expression", "AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new(name));"),
            ("leading-colon constructor", "AttackModule::set_optional_hit_effect(agent.module_accessor, 0, ::Hash40::new(\"x\"));"),
            ("empty name", "AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new(\"\"));"),
            ("nul name", "AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new(\"\\0\"));"),
            ("raw decimal", "AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new_raw(1234));"),
            ("raw suffixed", "AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new_raw(0x12u64));"),
            ("extra arg", "AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new(\"x\"), 1);"),
        ];
        for (label, statement) in cases {
            let source = format!("unsafe extern \"C\" fn game_negative(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {statement}\n    }}\n}}\n");
            assert!(
                syn::parse_file(&source).is_ok(),
                "{label}: negative fixture must be valid Rust"
            );
            let parsed = parse_acmd_script(&source);
            assert!(
                optional_feedback_calls(&parsed.stmts).is_empty(),
                "{label}: unsupported source became typed"
            );
            let preserved = parsed.stmts.iter().any(|stmt| match stmt {
                AcmdStmt::Excute(commands) => commands.iter().any(|command| matches!(command, ExcuteStmt::Raw(raw) if raw.contains("set_optional_hit_"))),
                _ => false,
            });
            assert!(
                preserved,
                "{label}: original native source was not retained: {parsed:?}"
            );
            assert!(
                preview_game_fn(&parsed, "negative").contains("set_optional_hit_"),
                "{label}: emitted source lost the native call"
            );
        }

        // A malformed sibling invalidates the complete execute block admission set. This is
        // intentionally separate from the independent well-formed negative cases above.
        let malformed = r#"unsafe extern "C" fn game_malformed(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new("x"));
        let broken = ;
    }
}
"#;
        let parsed = parse_acmd_script(malformed);
        assert!(optional_feedback_calls(&parsed.stmts).is_empty());
        assert!(parsed.stmts.iter().any(|stmt| matches!(stmt, AcmdStmt::Excute(commands) if commands.iter().any(|command| matches!(command, ExcuteStmt::Raw(raw) if raw.contains("set_optional_hit_effect"))) && commands.iter().any(|command| matches!(command, ExcuteStmt::Raw(raw) if raw.contains("let broken = ;"))))), "malformed complete execute must retain both the call and malformed sibling as Raw: {parsed:?}");
        let emitted = preview_game_fn(&parsed, "malformed");
        assert!(emitted
            .contains("set_optional_hit_effect(agent.module_accessor, 0, Hash40::new(\"x\"));"));
        assert!(
            emitted.contains("let broken = ;"),
            "malformed sibling text must not be repaired"
        );
        assert!(
            syn::parse_file(&emitted).is_err(),
            "source-only compilation must refuse malformed original syntax"
        );
        let graph = crate::move_nodes::model::MoveGraph {
            tracks: vec![crate::move_nodes::timeline::import_for_fighter(
                "mario",
                crate::move_nodes::model::Track::Game,
                &parsed,
            )],
            ..Default::default()
        };
        let errors = crate::move_nodes::timeline::compile("mario", "malformed", &graph)
            .expect_err("malformed original source must fail closed before source export");
        assert!(errors.iter().any(|error| error
            .reason
            .starts_with("generated ACMD failed Rust parsing:")));
    }

    #[test]
    fn hash40_literal_parser_rejects_leading_colon_and_attribute_expressions() {
        let base = syn::parse_str::<syn::Expr>("Hash40::new(\"name\")")
            .expect("baseline exact Hash40 expression parses");
        assert!(parse_hash40_literal_expr(base.clone()).is_some());

        let mut call_attribute = base.clone();
        let syn::Expr::Call(call) = &mut call_attribute else {
            panic!("expected call expression")
        };
        call.attrs.push(syn::parse_quote!(#[allow(unused)]));
        assert!(
            parse_hash40_literal_expr(call_attribute).is_none(),
            "call attributes are not part of the closed grammar"
        );

        let mut path_attribute = base.clone();
        let syn::Expr::Call(call) = &mut path_attribute else {
            panic!("expected call expression")
        };
        let syn::Expr::Path(path) = call.func.as_mut() else {
            panic!("expected constructor path")
        };
        path.attrs.push(syn::parse_quote!(#[allow(unused)]));
        assert!(
            parse_hash40_literal_expr(path_attribute).is_none(),
            "path attributes are not part of the closed grammar"
        );

        let mut literal_attribute = base;
        let syn::Expr::Call(call) = &mut literal_attribute else {
            panic!("expected call expression")
        };
        let syn::Expr::Lit(literal) = call.args.first_mut().expect("one string argument") else {
            panic!("expected string literal")
        };
        literal.attrs.push(syn::parse_quote!(#[allow(unused)]));
        assert!(
            parse_hash40_literal_expr(literal_attribute).is_none(),
            "literal attributes are not part of the closed grammar"
        );

        assert!(parse_hash40_literal("::Hash40::new(\"name\")").is_none());
        let leading_colon = r#"unsafe extern "C" fn game_hash40_leading_colon(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_optional_hit_effect(agent.module_accessor, 0, ::Hash40::new("name"));
    }
}
"#;
        assert!(syn::parse_file(leading_colon).is_ok());
        let parsed = parse_acmd_script(leading_colon);
        assert!(optional_feedback_calls(&parsed.stmts).is_empty());
        let emitted = preview_game_fn(&parsed, "hash40_leading_colon");
        assert!(
            emitted.contains("::Hash40::new(\"name\")"),
            "rejected constructor source must not be normalized: {emitted}"
        );
    }

    #[test]
    fn optional_hit_feedback_is_game_execute_only_and_source_only_in_graph() {
        use crate::data::ExcuteStmt;
        use crate::move_nodes::model::{MoveGraph, NodeKind, Track};

        let source = r#"unsafe extern "C" fn game_context(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_optional_hit_effect(agent.module_accessor, 0, Hash40::new("direct"));
        if true {
            AttackModule::set_optional_hit_sound(agent.module_accessor, 0, Hash40::new("nested"));
        }
        consume(AttackModule::set_optional_hit_sound(agent.module_accessor, 0, Hash40::new("wrapped")));
    }
}
"#;
        let parsed = parse_acmd_script(source);
        assert_eq!(optional_feedback_calls(&parsed.stmts).len(), 1);
        let emitted = preview_game_fn(&parsed, "context");
        assert!(emitted.contains(
            "set_optional_hit_sound(agent.module_accessor, 0, Hash40::new(\"nested\"));"
        ));
        assert!(emitted.contains("consume(AttackModule::set_optional_hit_sound(agent.module_accessor, 0, Hash40::new(\"wrapped\")));"));

        let graph = MoveGraph {
            tracks: vec![crate::move_nodes::timeline::import_for_fighter(
                "mario",
                Track::Game,
                &parsed,
            )],
            ..Default::default()
        };
        let compiled = crate::move_nodes::timeline::compile("mario", "context", &graph).unwrap();
        assert!(crate::move_nodes::live_support::source_only_tracks(&graph)
            .any(|track| track == Track::Game));
        let reimported =
            crate::move_nodes::timeline::import_for_fighter("mario", Track::Game, &compiled[0].1);
        fn count_editable(nodes: &[crate::move_nodes::model::MoveNode]) -> usize {
            nodes
                .iter()
                .map(|node| match &node.kind {
                    NodeKind::Execute { commands } => commands
                        .iter()
                        .filter(|command| matches!(command, ExcuteStmt::OptionalHitFeedback(_)))
                        .count(),
                    NodeKind::MixedExecute { commands } => commands
                        .iter()
                        .filter(|command| {
                            matches!(
                                command,
                                crate::move_nodes::model::MoveCommand::Editable(
                                    ExcuteStmt::OptionalHitFeedback(_)
                                )
                            )
                        })
                        .count(),
                    NodeKind::Repeat { body, .. } => count_editable(body),
                    NodeKind::Branch {
                        then_nodes,
                        else_nodes,
                        ..
                    } => count_editable(then_nodes) + count_editable(else_nodes),
                    _ => 0,
                })
                .sum()
        }
        assert_eq!(
            count_editable(&reimported.nodes),
            1,
            "only direct source statement is editable"
        );
        let sound_script = r#"unsafe extern "C" fn game_sound_context(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_optional_hit_sound(agent.module_accessor, 0, Hash40::new("sound_track"));
    }
}
"#;
        let sound_parsed = parse_acmd_script(sound_script);
        assert_eq!(
            optional_feedback_calls(&sound_parsed.stmts).len(),
            1,
            "source parser recognizes the exact Game execute shape"
        );
        let sound_graph = MoveGraph {
            tracks: vec![crate::move_nodes::timeline::import_for_fighter(
                "mario",
                Track::Sound,
                &sound_parsed,
            )],
            ..Default::default()
        };
        assert_eq!(
            count_editable(&sound_graph.tracks[0].nodes),
            0,
            "wrong track does not authorize Game-only command"
        );
        let emitted_sound =
            crate::move_nodes::timeline::compile("mario", "sound_context", &sound_graph).unwrap();
        assert!(preview_game_fn(&emitted_sound[0].1, "sound_context")
            .contains("set_optional_hit_sound"));

        let unknown_owner =
            crate::move_nodes::timeline::import_for_fighter("not_a_fighter", Track::Game, &parsed);
        assert!(matches!(
            unknown_owner.nodes.as_slice(),
            [crate::move_nodes::model::MoveNode {
                kind: NodeKind::Preserved { .. },
                ..
            }]
        ));
    }
}
