// Game link: TCP client to the slight_replica plugin (127.0.0.1:7878 by default).
// Speaks the plugin's `<TCP_MESSAGE>{json}</TCP_MESSAGE>` framing (formerly RPM's role):
//  - inbound  `{"header":"Notify","body":"{\"Notify\":{id,name,value_in_json}}"}` = live
//    effect-kind tabs (id = hash40 of the effect name, value = RpmEffectData JSON)
//  - outbound `{"id":<hash>,"newValue":"<sparse JSON>"}` = an edit; only controls the
//    user actually changed are sent and pinned.
//
// Connection robustness notes (see `link_thread` / `serve_connection`):
//  - the outbound queue survives disconnects: edits made while offline are flushed on
//    reconnect, with full-replace families coalesced to their latest state;
//  - a periodic `ping` probes a silent connection so a half-open socket (emulator
//    stalled, NAT timeout, missed FIN) is detected even when the user is idle;
//  - the plugin answers `ping` with `Pong`; any inbound frame also proves liveness.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::gimmicks::registry::{self, IdentifierKind, LookupError};

/// The deliberately small live-operation set. Identifiers remain symbols on
/// the wire; `*_boid` fields select already-live game objects and are never
/// interpreted as engine identifiers.
#[derive(Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum GimmickOperationKind<'a> {
    GenerateArticle { symbol: &'a str },
    RemoveArticle { symbol: &'a str },
    RequestWeaponStatus { weapon_boid: u32, symbol: &'a str },
}

#[derive(Serialize)]
struct GimmickOperationRequest<'a> {
    version: u8,
    fighter: &'a str,
    fighter_boid: u32,
    #[serde(flatten)]
    operation: GimmickOperationKind<'a>,
}

/// Local validation failure for a constrained live gimmick operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GimmickOperationError {
    Identifier(LookupError),
    MissingWeaponKind { fighter: String, group: String },
    InvalidWeaponObjectId,
    QueueFull,
    QueueUnavailable,
}

impl From<LookupError> for GimmickOperationError {
    fn from(error: LookupError) -> Self {
        Self::Identifier(error)
    }
}

impl std::fmt::Display for GimmickOperationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identifier(error) => write!(formatter, "{error}"),
            Self::MissingWeaponKind { fighter, group } => write!(
                formatter,
                "weapon-status group {group:?} for fighter {fighter:?} has no safe concrete weapon-kind guard"
            ),
            Self::InvalidWeaponObjectId => {
                write!(formatter, "weapon_boid must be a nonzero live battle-object id")
            }
            Self::QueueFull => write!(
                formatter,
                "the live-operation queue is full; the action was not queued"
            ),
            Self::QueueUnavailable => write!(
                formatter,
                "the live-operation queue is unavailable; the action was not queued"
            ),
        }
    }
}

impl std::error::Error for GimmickOperationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Identifier(error) => Some(error),
            Self::MissingWeaponKind { .. }
            | Self::InvalidWeaponObjectId
            | Self::QueueFull
            | Self::QueueUnavailable => None,
        }
    }
}

pub const PLUGIN_ADDR: &str = "127.0.0.1:7878";
const DEFAULT_PLUGIN_PORT: u16 = 7878;
/// How long a `connect()` may take before the attempt is abandoned. The emulator's
/// virtual network can stall under load; 1s was tight enough to fail connects that
/// would have succeeded a moment later.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Pause between reconnect attempts. Fixed 1s keeps recovery snappy without hammering.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// A stalled emulator can stop reading while still holding the socket open. Without a
/// write timeout the link thread wedges inside `write_all` forever: status stays
/// "Connected" while nothing moves. 2s is generous for localhost yet bounds the stall.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// Idle probe period. Forces traffic on an otherwise silent connection so a dead peer
/// is noticed via a send/recv error rather than lingering as a phantom "Connected".
const PING_INTERVAL: Duration = Duration::from_secs(5);
/// No inbound frame (Notify / capture / CarrierStatus heartbeat / Pong) for this long
/// means the connection is half-open: drop it and reconnect rather than showing stale
/// "Connected". 20s comfortably covers menu idling (no spawns) while still recovering
/// within a reasonable wait — the 5s ping guarantees traffic in the meantime.
const STALE_TIMEOUT: Duration = Duration::from_secs(20);
/// Bound for queued outbound frames while offline. Full-replace families are coalesced
/// to latest, so this is only reached by pathological churn; dropping oldest keeps
/// memory bounded and preserves the newest (authoritative) state. Constrained gimmick
/// operations are non-idempotent, so they are never silently evicted: their caller gets
/// [`GimmickOperationError::QueueFull`] instead.
const OUTBOX_CAP: usize = 8192;

/// Address of the in-game plugin, honouring explicit overrides first.
///
/// 1. `VISIONARY_PLUGIN_ADDR` (full `ip:port`) when set and parseable.
/// 2. `VISIONARY_PLUGIN_PORT` (port only) when set.
/// 3. The port in `<emulator SD>/slight/user/gateway.txt`, which is the same file the
///    plugin reads its listen port from — a custom port there must be matched here or
///    the editor dials 7878 while the game listens elsewhere.
/// 4. [`PLUGIN_ADDR`] as the fallback.
pub fn plugin_addr() -> SocketAddr {
    if let Ok(addr) = std::env::var("VISIONARY_PLUGIN_ADDR") {
        let addr = addr.trim();
        if !addr.is_empty() {
            if let Ok(parsed) = addr.parse::<SocketAddr>() {
                return parsed;
            }
        }
    }
    let mut port = DEFAULT_PLUGIN_PORT;
    if let Ok(port_str) = std::env::var("VISIONARY_PLUGIN_PORT") {
        if let Ok(parsed) = port_str.trim().parse::<u16>() {
            if parsed != 0 {
                port = parsed;
            }
        }
    } else if let Some(gateway_port) = gateway_file_port() {
        port = gateway_port;
    }
    format!("127.0.0.1:{port}")
        .parse()
        .unwrap_or_else(|_| PLUGIN_ADDR.parse().unwrap())
}

/// Port from the plugin's `gateway.txt` on the emulator SD, if present and parseable.
///
/// Mirrors the plugin's own `rpm_listen_port` parsing (dotted-quad with optional
/// `:port`; port out of range falls back to 7878). Returns `None` when the SD root or
/// file cannot be read, so callers fall back to the default.
fn gateway_file_port() -> Option<u16> {
    let sd = crate::scratch_dirs::emulator_sd_root()?;
    let text = std::fs::read_to_string(sd.join("slight/user/gateway.txt")).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (addr, port_str) = match line.rsplit_once(':') {
            Some((a, p)) => (a, Some(p)),
            None => (line, None),
        };
        if !is_dotted_quad(addr) {
            continue;
        }
        if let Some(port_str) = port_str {
            match port_str.parse::<u32>() {
                Ok(port) if port > 0 && port <= 65535 => return Some(port as u16),
                _ => return Some(DEFAULT_PLUGIN_PORT),
            }
        }
        return Some(DEFAULT_PLUGIN_PORT);
    }
    None
}

fn is_dotted_quad(s: &str) -> bool {
    let mut parts = s.split('.');
    let valid = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(a), Some(b), Some(c), Some(d), None)
            if valid(a) && valid(b) && valid(c) && valid(d)
    )
}

// ── Wire structs (must match slight_replica effect_data.rs exactly) ──────────

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Point3D {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Color {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
}

impl Default for Color {
    fn default() -> Self {
        Self {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            alpha: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Rainbow {
    pub color: Color,
    pub movement_state: f32,
}

/// One live effect kind as the plugin reports it. `rainbow.color` and `speed` are runtime
/// MULTIPLIERS (the game has no getters for authored color); `pos`/`rot` are in ACMD script
/// offset space for script-spawned effects; `scale` is the spawn size argument.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RpmEffectData {
    pub index: u32,
    pub effect_name: String,
    pub bone_name: String,
    pub is_follow: bool,
    pub visible: bool,
    pub scale: f32,
    pub frame: f32,
    pub pos: Point3D,
    pub rot: Point3D,
    pub speed: f32,
    pub rainbow: Rainbow,
}

impl Default for RpmEffectData {
    fn default() -> Self {
        Self {
            index: 0,
            effect_name: "0x0".into(),
            bone_name: "0x0".into(),
            is_follow: false,
            visible: true,
            scale: 1.0,
            frame: 0.0,
            pos: Point3D::default(),
            rot: Point3D::default(),
            speed: 1.0,
            rainbow: Rainbow::default(),
        }
    }
}

/// Wire form of a plugin spawn rule (matches slight_replica spawn_rules::SpawnRule).
/// `motion` + frame window scope the rule to ONE spawn so editing one spawn of an effect
/// doesn't move every spawn; `pos`/`rot`/`scale` are the per-spawn transform override.
#[derive(Clone, Debug, Serialize)]
pub struct SpawnRuleWire {
    pub eff_hash: u64,
    pub suppress: bool,
    /// Optional lifetime command this rule suppresses. Ordinary `suppress` rules match spawn
    /// hooks; a stop rule is consumed only by the named termination hook. Keeping this optional
    /// makes the wire readable by older plugins and prevents a retimed end from accidentally
    /// suppressing a start call with the same effect hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_func: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion: Option<u64>,
    pub frame_start: Option<f32>,
    pub frame_end: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rot: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
    /// Per-spawn playback rate — the live counterpart of the spawn's `LAST_EFFECT_SET_RATE`.
    ///
    /// Sent only when the user changed it, so an untouched spawn keeps whatever its script
    /// asks for. Separate from `scale` because it is not part of the spawn's argument list at
    /// all: the plugin applies it to the handle after the spawn, and rewrites the script's own
    /// rate line if there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<f32>,
    /// Per-spawn camera-flat offset — the live counterpart of the spawn's
    /// `LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT` line. It is a separate modifier because it is
    /// not part of the spawn's three-dimensional position arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_offset: Option<f32>,
    /// Per-spawn tint and opacity — the live counterparts of `LAST_EFFECT_SET_COLOR` and
    /// `LAST_EFFECT_SET_ALPHA`, applied and rewritten exactly the way `rate` is.
    ///
    /// Not to be confused with `color` below, which is a whole-fighter `FLASH` / `BURN_COLOR`
    /// payload keyed on a command name rather than an effect kind. These two are scoped to one
    /// spawn of one effect, like everything else above them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<[f32; 3]>,
    /// Per-spawn particle tint — the live counterpart of `LAST_PARTICLE_SET_COLOR`. It is
    /// separate from `tint` because the game primitive targets the last particle, not the last
    /// effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub particle_tint: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f32>,
    /// Per-spawn values for the native dynamic-arity `LAST_EFFECT_SET_SCALE_W` modifier. The
    /// vector preserves the authored one-to-three-value Lua stack shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_w: Option<Vec<f32>>,
    /// Live values for a colour command. For such a rule `eff_hash` is hash40 of the
    /// lowercased command name — `burn_color`, not an effect kind — because these macros name
    /// no effect at all; see `SpawnRule::color` in the plugin for why that field is reused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 4]>,
    /// Frames a `_FRM` / `_FRAME` command interpolates over. Sent apart from `color` so
    /// retiming a ramp does not have to restate its colour, and vice versa.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition: Option<f32>,
    /// Live retime: re-fire a captured spawn at a new frame (paired with a suppress rule at
    /// the pristine frame). Omitted for plain transform/suppress rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inject: Option<SpawnInjectWire>,
    /// Live retime for a colour/control-style effect command. Kept separate from graphic
    /// injection so the plugin can dispatch the command family with its own argument contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_inject: Option<SpawnInjectWire>,
}

/// Wire form of plugin `spawn_rules::SpawnInject` — a captured EFFECT spawn to replay.
#[derive(Clone, Debug, Serialize)]
pub struct SpawnInjectWire {
    pub frame: f32,
    pub func: String,
    pub args: Vec<LuaArgWire>,
    /// Condition from an If block, checked in the match when this spawn is due.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard: Option<LiveGuard>,
}

/// One live effect point-control rule. Matching includes the captured argument vector so two
/// `ENABLE_AREA` calls on one frame cannot suppress each other accidentally.
#[derive(Clone, Debug, Serialize)]
pub struct EffectControlRuleWire {
    pub motion: Option<u64>,
    pub func: String,
    pub args: Vec<LuaArgWire>,
    pub suppress: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_start: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_end: Option<f32>,
    /// Replacement WorkModule slot for `EFFECT_DETACH_KIND_WORK` injection. The desktop sends this
    /// only for numeric values or symbolic IDs covered by its measured constant table; the
    /// primitive itself receives the resolved effect handle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_slot: Option<i32>,
    /// Re-fire the captured point at a new motion frame after suppressing its pristine call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inject: Option<EffectControlInjectWire>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EffectControlInjectWire {
    pub frame: f32,
    pub func: String,
    pub args: Vec<LuaArgWire>,
}

/// Wire form of plugin `spawn_rules::EffectAlias` — live transplant kind substitution:
/// a copy/replaced entry that doesn't exist in the running game spawns as its donor.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectAliasWire {
    /// Requested kind (hash40 of the copy / replaced entry name, lowercase).
    pub from: u64,
    /// Kind that exists in the loaded eff resources (the donor).
    pub to: u64,
    /// Costume slots (c00…) the alias applies to; empty = all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<u8>,
}

/// Wire form of the plugin's `effect_reload::DonorSpec` (field names must match).
#[derive(Clone, PartialEq, serde::Serialize)]
pub struct DonorEffWire {
    /// Target fighter's eff arc path (lowercase), e.g. "effect/fighter/kirby/ef_kirby.eff".
    pub target: String,
    /// Donor eff arc paths to co-load whenever the target's effects load.
    pub donors: Vec<String>,
}

/// A stripped donor eff (only the referenced effects + their resources), base64-encoded,
/// that the plugin injects as resident data for a live cross-character transplant.
#[derive(Clone, PartialEq, serde::Serialize)]
pub struct DonorBytesWire {
    /// Donor eff arc path (lowercase), e.g. "effect/assist/alucard/ef_alucard.eff".
    pub path: String,
    /// base64(stripped ef bytes). Empty when [`Self::file`] carries the payload instead.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub b64: String,
    /// sd:-relative file holding the payload, which the plugin reads directly.
    ///
    /// The emulator's sdmc is a directory on this machine, so multi-MB payloads go to disk
    /// rather than through base64 in a JSON frame over a socket read in 8 KB chunks. `b64`
    /// remains the fallback for when that directory cannot be found.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
}

/// One immutable SD-backed model, animation, or swing file for the preview carrier.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetBundleFileWire {
    /// Lowercase ARC path, e.g. `fighter/mario/model/body/c00/model.numdlb`.
    pub path: String,
    /// SD-relative payload path. The plugin requires this to mirror `path` under
    /// `effect_viewer/live_assets/files/<generation>/`.
    pub file: String,
    /// Exact payload length. A mismatch rejects the whole snapshot and preserves the previous one.
    pub size: u64,
}

/// Full replacement snapshot activated by retiring and recreating the asset carrier.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetBundleWire {
    /// Nonzero and monotonically increasing across sends. UNIX milliseconds are suitable.
    pub generation: u64,
    /// Lowercase internal fighter name. Every file path must belong to this fighter.
    pub target: String,
    pub files: Vec<AssetBundleFileWire>,
}

// ── Live ACMD capture + hitbox rules (wire forms match slight_replica hitbox_viewer) ──

/// One typed lua argument (plugin `LuaArg`): losslessly round-trips capture → edit → inject.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
pub enum LuaArgWire {
    #[serde(rename = "h")]
    Hash(u64),
    #[serde(rename = "n")]
    Num(f32),
    #[serde(rename = "i")]
    Int(i64),
    #[serde(rename = "b")]
    Bool(bool),
    #[serde(rename = "x")]
    Nil,
}

impl LuaArgWire {
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            LuaArgWire::Num(n) => Some(*n),
            LuaArgWire::Int(i) => Some(*i as f32),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            LuaArgWire::Int(i) => Some(*i),
            LuaArgWire::Num(n) => Some(*n as i64),
            LuaArgWire::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }
    /// This argument as a boolean, or `None` when it is not one.
    ///
    /// `Int` is accepted for `0`/`1` only. The wire genuinely delivers Lua values under a
    /// different tag than the source spelling suggests — hashes arrive tagged `Int` — so
    /// refusing `Int` outright would lose real captures. Accepting *any* integer would not:
    /// it would turn a misread slot into `true` and write that into an export. Values outside
    /// `0`/`1` are left for the caller to treat as "not captured".
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            LuaArgWire::Bool(b) => Some(*b),
            LuaArgWire::Int(0) => Some(false),
            LuaArgWire::Int(1) => Some(true),
            _ => None,
        }
    }
    pub fn as_hash(&self) -> Option<u64> {
        match self {
            LuaArgWire::Hash(h) => Some(*h),
            LuaArgWire::Int(i) => Some(*i as u64),
            _ => None,
        }
    }

    /// Render this argument as Rust source, for exports that replay a captured call.
    ///
    /// `None` for `Nil`: lua's nil has no single Rust spelling, and a spawn whose tail
    /// contains one has to fall back to a known-good macro rather than emit a guess.
    ///
    /// Floats go through the emitter's own renderer, not a fixed decimal count: a captured
    /// tail is replayed verbatim, so rounding one here changes the move with nothing left to
    /// compare against — the export verifier sees the already-rounded string on both sides.
    pub fn to_source_arg(&self) -> Option<String> {
        Some(match self {
            LuaArgWire::Hash(h) => format!("Hash40::new_raw({h:#x})"),
            LuaArgWire::Num(n) => crate::acmd::num(*n),
            LuaArgWire::Int(i) => i.to_string(),
            LuaArgWire::Bool(b) => b.to_string(),
            LuaArgWire::Nil => return None,
        })
    }
}

/// Stable rule key for one captured expression call. It includes the primitive name and every
/// pristine Lua argument, so two equal camera calls on the same frame do not share a live rule.
/// The plugin mirrors this small hash in its expression hooks.
pub fn expression_key(func: &str, args: &[LuaArgWire]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in func.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    for arg in args {
        let value = match arg {
            LuaArgWire::Hash(h) => 0x1000_0000_0000_0000 ^ h,
            LuaArgWire::Num(n) => 0x2000_0000_0000_0000 ^ n.to_bits() as u64,
            LuaArgWire::Int(i) => 0x3000_0000_0000_0000 ^ *i as u64,
            LuaArgWire::Bool(b) => 0x4000_0000_0000_0000 ^ *b as u64,
            LuaArgWire::Nil => 0x5000_0000_0000_0000,
        };
        for byte in value.to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    }
    hash
}

/// Stable key for a point call whose payload is a fixed list of numeric `ToF32` arguments.
///
/// The plugin derives the same key from the Lua stack before applying an override. Keeping the
/// function name in the hash prevents two numeric point families from sharing a rule by accident.
pub fn numeric_point_key(func: &str, args: &[f32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in func.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    for arg in args {
        for byte in arg.to_bits().to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    }
    hash
}

/// Stable key for a point call whose payload is a fixed list of exact integer arguments.
///
/// This is deliberately separate from [`numeric_point_key`]: a WorkModule `set_int64` value
/// commonly carries a hash40 and must not be narrowed through `f32` before the desktop and
/// plugin derive the same rule identity.
pub fn integer_point_key(func: &str, args: &[i64]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in func.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    for arg in args {
        for byte in arg.to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    }
    hash
}

/// One captured ACMD call, as streamed by the plugin (`AcmdCapture`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureLine {
    /// Fighter kind of the performing agent.
    pub kind: i32,
    /// hash40 of the motion name (e.g. "attack_air_n").
    pub motion: u64,
    /// Motion frame at call time.
    pub frame: f32,
    /// sv_animcmd function name ("ATTACK", "EFFECT_FOLLOW", …).
    pub func: String,
    /// The primitive hook's source ACMD category is not available here. In particular, direct
    /// MotionModule bindings can be called by either `game_` or `expression_` scripts.
    pub args: Vec<LuaArgWire>,
    /// Which single playback of the motion produced this line. Current plugins lock one run per
    /// fighter-kind + motion until captures are explicitly cleared. Defaults to 0 for older
    /// plugins that predate run ownership.
    #[serde(default)]
    pub run: u32,
    /// The call came from the game's COMMON animcmd agent — invincibility flashing, damage burn,
    /// and the rest of the status feedback that plays over the move without being part of it.
    ///
    /// Such a line shares the move's fighter, motion, and frame, so nothing downstream can tell
    /// it apart after the fact; the plugin is the only place that knows. It is captured (it is a
    /// true observation, and the plugin needs the same hook as an injection boundary) but never
    /// promoted into an editable row or offered as a donor. Defaults to false for older plugins,
    /// which is the pre-tag behaviour.
    #[serde(default)]
    pub common: bool,
}

/// Aggregate capture outcomes for one cache key. Counts are cumulative for the current plugin
/// capture session and replace the previous row whenever a newer snapshot arrives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CaptureDebugSnapshot {
    pub kind: i32,
    pub motion: u64,
    pub attempted: u64,
    pub succeeded: u64,
    pub contention: u64,
    pub claim_conflicts: u64,
    pub duplicates: u64,
    pub last_frame: f32,
}

impl CaptureDebugSnapshot {
    pub fn failed(self) -> u64 {
        self.contention + self.claim_conflicts
    }
}

/// Rule key for `COL_PRI`, which is per fighter rather than per target.
///
/// The hurtbox category matches a rule on motion + key + frame, and this call has no target to
/// build a key from. `u64::MAX` is safe as a stand-in because no bone hash and no group number
/// reaches it. **Must equal the plugin's value** — see `hitbox_viewer::HURT_KEY_COL_PRI`.
pub const HURT_KEY_COL_PRI: u64 = u64::MAX;

/// Rule key for `WHOLE_HIT`, the other targetless member of the hurtbox category.
///
/// Deliberately *not* [`HURT_KEY_COL_PRI`]. Sharing one sentinel across two targetless macros
/// would let a `COL_PRI` rule fire on a `WHOLE_HIT` in the same frame window and write a
/// priority into a status slot — the cross-family corruption this codebase keeps rediscovering,
/// arrived at from the other direction. **Must equal the plugin's value.**
pub const HURT_KEY_WHOLE: u64 = u64::MAX - 1;

/// Rule key for the fighter-wide `damage!(…, MA_MSC_DAMAGE_DAMAGE_NO_REACTION, …)` call.
///
/// It shares the hurtbox category but not a target namespace: the plugin uses this sentinel to
/// apply super armor/damage-based armor to the damage-reaction command without ever treating it
/// as a `WHOLE_HIT` status update. **Must equal the plugin's value.**
pub const HURT_KEY_DAMAGE_REACTION: u64 = u64::MAX - 3;

/// Wire category for `HIT_NODE` / `HIT_NO` / `WHOLE_HIT` status rules. Kept explicit rather than
/// using the display category of a collision, because the plugin reserves this family at 3.
pub const CAT_HURT: u8 = 3;

/// Rule key for the targetless `SET_AIR` kinetic point.
///
/// Kept separate from the hurtbox sentinels so a structural kinetic rule cannot match a
/// `WHOLE_HIT` or `COL_PRI` call in an older/current plugin. **Must equal the plugin's value.**
pub const KINETIC_KEY_SET_AIR: u64 = u64::MAX - 2;

/// Rule key for the targetless direct `KineticModule::clear_speed_all` kinetic point.
pub const KINETIC_KEY_CLEAR_SPEED_ALL: u64 = u64::MAX - 3;

/// Wire category for `ATTACK_ABS`. **Must equal the plugin's `CAT_ABS`.**
///
/// Deliberately *not* [`crate::data::CAT_ABS`], which is `3`. These are two different numbering
/// spaces that happen to agree for attack, grab and wind: the editor's numbers only cover things
/// that are [`crate::data::Hitbox`]es, while the wire's also carry hurtbox state, which took `3`
/// first. They were assumed identical, so every live `ATTACK_ABS` edit went out as category `3`
/// and reached the plugin's hurtbox hook instead — B1's live surface has never worked.
pub const CAT_ABS: u8 = 4;

/// Wire category for `SEARCH`. **Must equal the plugin's value.**
pub const CAT_SEARCH: u8 = 7;

/// Wire category for `ATTACK_FP`. **Must equal the plugin's `CAT_ATTACK_FP`.**
///
/// The value is kept outside the display-category range because the plugin's wire categories
/// already reserve 3 for hurtbox state and 4 for `ATTACK_ABS`.
pub const CAT_ATTACK_FP: u8 = 12;

/// Translate a [`crate::data::Hitbox`]'s display category into the one the plugin matches on.
///
/// Every place that puts a collision on the wire goes through this. Passing the display number
/// straight out is what aimed throw-damage rules at the hurtbox hook, and the two spaces diverge
/// further with each family added — so the conversion is explicit rather than an identity that
/// happens to be right for the first three.
pub fn wire_category(display: u8) -> u8 {
    match display {
        crate::data::CAT_ABS => CAT_ABS,
        crate::data::CAT_SEARCH => CAT_SEARCH,
        crate::data::CAT_ATTACK_FP => CAT_ATTACK_FP,
        // Attack, grab and wind agree in both spaces.
        other => other,
    }
}

/// Wire category for `ATK_POWER`. **Must equal the plugin's value.**
///
/// The two post-hoc modifiers get a category each rather than sharing one keyed by hitbox id.
/// They can legally name the same id in the same frame window — kirby/Attack100Sub tunes id 0
/// and a move may retune the same box two ways — so one shared category would let an
/// `ATK_POWER` rule fire on an `ATK_SET_SHIELD_SETOFF_MUL` call and write damage into a shield
/// multiplier. Separating them makes that unrepresentable instead of merely unlikely, which is
/// the lesson `HURT_KEY_WHOLE` records from the other direction.
pub const CAT_ATK_POWER: u8 = 5;

/// Wire category for `ATK_SET_SHIELD_SETOFF_MUL`. **Must equal the plugin's value.**
pub const CAT_ATK_SETOFF_MUL: u8 = 6;

/// Wire category for the whole `PLAY_SE` family. **Must equal the plugin's value.**
///
/// One category for all twelve members, which reads as inconsistent beside the two above and
/// is a different situation. There, slot 1 meant a different thing in each member, so a
/// misapplied rule wrote damage into a shield multiplier — a wrong value in a real field.
/// Here every member declares a `Hash40` in slot 0 and nothing else is ever written, so the
/// worst a misapplied rule can do is put a sound where a sound goes. The macro name travels on
/// [`HitboxRuleWire::func`] to close even that.
pub const CAT_SOUND: u8 = 8;

/// Wire category for `FT_MOTION_RATE`. **Must equal the plugin's value.**
///
/// One category covers all three rate macros because `smash-script` compiles
/// `FT_MOTION_RATE_RANGE` and `FT_DESIRED_RATE` down to the same `sv_animcmd::FT_MOTION_RATE`,
/// so the plugin cannot tell them apart and does not try. The editor only models the plain form;
/// a rule keys on the motion and frame alone, because a rate call has no id.
pub const CAT_MOTION_RATE: u8 = 9;

/// Wire category for the measured `expression_` camera/rumble primitives. The function name
/// travels on [`HitboxRuleWire::func`] because the three members have different argument shapes.
pub const CAT_EXPRESSION: u8 = 10;

/// Wire category for the argument-less `REVERSE_LR` facing-direction point.
pub const CAT_REVERSE_LR: u8 = 11;

/// Wire category for the verified three-argument `SET_SPEED_EX` velocity point.
pub const CAT_SPEED_EX: u8 = 13;

/// Wire category for the verified `(x, y)` `SET_SPEED` velocity point.
pub const CAT_SPEED: u8 = 16;

/// Wire category for the verified `ADD_SPEED_NO_LIMIT` x/y velocity point.
pub const CAT_ADD_SPEED_NO_LIMIT: u8 = 14;

/// Wire category for the verified `CORRECT` ground-correction point.
pub const CAT_CORRECT: u8 = 15;

/// Wire category for the measured two-argument `FT_CATCH_STOP` point.
pub const CAT_FT_CATCH_STOP: u8 = 17;

/// Wire category for the measured one-argument motion-frame adjustment point.
pub const CAT_FT_START_ADJUST_MOTION_FRAME: u8 = 18;

/// Wire category for the measured `CLR_SPEED` kinetic point.
pub const CAT_CLR_SPEED: u8 = 19;

/// Wire category for the measured argument-less `SET_AIR` kinetic point.
pub const CAT_SET_AIR: u8 = 20;

/// Wire category for the direct `KineticModule::change_kinetic` kinetic point.
pub const CAT_CHANGE_KINETIC: u8 = 21;

/// Wire category for the direct `KineticModule::add_speed` x/y vector point.
pub const CAT_KINETIC_ADD_SPEED: u8 = 22;

/// Wire category for direct `KineticModule::suspend_energy` points.
pub const CAT_KINETIC_SUSPEND_ENERGY: u8 = 23;

/// Wire category for direct `KineticModule::resume_energy` points.
pub const CAT_KINETIC_RESUME_ENERGY: u8 = 24;

/// Wire category for direct `KineticModule::enable_energy` points.
pub const CAT_KINETIC_ENABLE_ENERGY: u8 = 25;

/// Wire category for direct `KineticModule::unable_energy` points.
pub const CAT_KINETIC_UNABLE_ENERGY: u8 = 26;

/// Wire category for direct `KineticModule::clear_speed_all` points.
pub const CAT_KINETIC_CLEAR_SPEED_ALL: u8 = 27;

/// Wire category for direct `KineticModule::set_consider_ground_friction` points.
pub const CAT_KINETIC_SET_CONSIDER_GROUND_FRICTION: u8 = 28;

/// Wire category for direct `MotionModule::set_rate` point overrides.
pub const CAT_MOTION_MODULE_SET_RATE: u8 = 29;

/// Wire category for direct `MotionModule::set_helper_calculation` point overrides.
pub const CAT_MOTION_MODULE_SET_HELPER_CALCULATION: u8 = 30;

/// Wire category for direct `MotionModule::set_rate_partial` point overrides.
pub const CAT_MOTION_MODULE_SET_RATE_PARTIAL: u8 = 31;

/// Wire category for direct `WorkModule::on_flag` / `off_flag` point overrides.
pub const CAT_WORK_FLAG: u8 = 32;

/// Wire category for direct WorkModule transition-term and transition-term-group point overrides.
pub const CAT_WORK_TRANSITION_TERM: u8 = 33;

/// Wire category for direct `WorkModule::set_int` / `set_float` / `set_int64` point overrides.
pub const CAT_WORK_MODULE_SET: u8 = 34;

/// Wire category for direct `WorkModule::inc_int` point overrides.
pub const CAT_WORK_MODULE_INC_INT: u8 = 35;

/// Wire category for direct `MotionModule::set_frame_partial` point overrides.
pub const CAT_MOTION_MODULE_SET_FRAME_PARTIAL: u8 = 36;

/// The wire category a modifier's rules go out under.
pub fn attack_mod_category(kind: crate::data::AttackModKind) -> u8 {
    match kind {
        crate::data::AttackModKind::Power => CAT_ATK_POWER,
        crate::data::AttackModKind::ShieldSetoffMul => CAT_ATK_SETOFF_MUL,
    }
}

/// Sparse ATTACK-arg overrides (plugin `HbOverrides`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct HbOverridesWire {
    /// Complete AREA_WIND argument vector when a wind payload changes. Wind calls do not share
    /// ATTACK's slot layout, so the plugin swaps this exact typed vector into the original hook.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wind_args: Option<Vec<LuaArgWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bone: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damage: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub angle: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kbg: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fkb: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bkb: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub z: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x2: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y2: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub z2: Option<f32>,
    /// Explicit capsule state. `Some(false)` is distinct from an absent x2/y2/z2 override:
    /// it replaces the source call's numeric second endpoint with three Lua nils.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capsule: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hitlag: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdi: Option<f32>,
    // ── Attribute slots 17..35 ───────────────────────────────────────────────
    //
    // Sent as the NUMBERS the lua stack wants (the editor holds them as symbolic names);
    // `collision_attr` is a hash40. `None` means "leave the game's own value alone", so a
    // hitbox whose attributes were never resolved to a known constant is not clobbered.
    // Plugins predating these fields ignore them, so an old plugin build still applies the
    // geometry/damage overrides exactly as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setoff: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lr_check: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clang: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_attack: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hitbox_attr: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground_or_air: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtk: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shield_disable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reflectable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub absorbable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landing_attack: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub situation_mask: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_mask: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_mask: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_finish_camera: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collision_attr: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_level: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_attr: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attack_region: Option<i64>,
    // ── Hurtbox state (category 3 only) ──────────────────────────────────────
    //
    // Skipped when absent like everything above, which is what keeps a plugin build predating
    // this family working: it deserialises the rule, finds no field it knows, and applies
    // nothing rather than failing the whole message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_status: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_target: Option<LuaArgWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col_pri: Option<i64>,
    /// `DAMAGE_NO_REACTION` mode and value (category 3, damage-reaction sentinel only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hurt_condition_mode: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hurt_condition_value: Option<f32>,
    // ── Post-hoc hitbox tuning (category 5 only) ─────────────────────────────
    //
    // `ATK_POWER` and `ATK_SET_SHIELD_SETOFF_MUL` share one `(id, value)` layout, so one pair of
    // fields covers both; the rule's own category and key say which call it is aimed at. Skipped
    // when absent like everything above, so a plugin build predating this family ignores them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atk_mod_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atk_mod_value: Option<f32>,
    // ── Sound (category 8 only) ──────────────────────────────────────────────
    //
    // The hashes to play instead, positional into the call's leading `Hash40` slots. A list
    // because `PLAY_STEP_FLIPPABLE` and `PLAY_FLY_VOICE` each name a pair, which is why
    // `SoundCall::sounds` is a list too.
    //
    // The plugin bounds the write by the member's own declared hash-slot count rather than by
    // this vector's length, so a longer list cannot reach `SET_PLAY_INHIVIT`'s trailing
    // duration argument. Sending a shorter one leaves the remaining slots as the script wrote
    // them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_hashes: Option<Vec<u64>>,
    /// Replacement `FT_MOTION_RATE` argument.
    ///
    /// **Below 1.0 plays FASTER**: `game_frames = motion_frames * rate`. The plugin refuses a
    /// value that is not finite and positive rather than trusting this end, because a zero rate
    /// freezes the animation and nothing below the call would run again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_rate: Option<f32>,
    /// Replacement `SET_SPEED_EX` or `ADD_SPEED_NO_LIMIT` x/y velocity components.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_y: Option<f32>,
    /// Replacement numeric `CORRECT` kind. Named source constants remain source-owned when a
    /// live capture cannot prove their numeric value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correct_kind: Option<i64>,
    /// Replacement `FT_CATCH_STOP` numeric `ToF32` arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ft_catch_stop_arg1: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ft_catch_stop_arg2: Option<f32>,
    /// Replacement `FT_START_ADJUST_MOTION_FRAME_arg1` numeric value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ft_start_adjust_motion_frame_value: Option<f32>,
    /// Replacement direct `MotionModule::set_rate` value. This is separate from
    /// `motion_rate`, which belongs to the `FT_MOTION_RATE` ACMD primitive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_module_rate: Option<f32>,
    /// Replacement direct `MotionModule::set_helper_calculation` boolean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_module_helper_calculation: Option<bool>,
    /// Replacement direct `MotionModule::set_rate_partial` rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_module_rate_partial: Option<f32>,
    /// Replacement direct `MotionModule::set_frame_partial` seek frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_module_frame_partial: Option<f32>,
    /// Replacement direct `MotionModule::set_frame_partial` sync flag.
    ///
    /// The native binding always receives this boolean; source may omit it, in which case the
    /// game's own Lua reader passes `true`. The editor sends a value only when the user changed
    /// one that the capture also carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_module_frame_partial_sync: Option<bool>,
    /// Replacement numeric flag for direct `WorkModule::on_flag` / `off_flag` calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_flag: Option<i64>,
    /// Replacement numeric transition term for direct WorkModule transition-term calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_transition_term: Option<i64>,
    /// Replacement integer value for a direct `WorkModule::set_int` call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_module_set_int_value: Option<i64>,
    /// Replacement exact 64-bit value for a direct `WorkModule::set_int64` call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_module_set_int64_value: Option<i64>,
    /// Replacement WorkModule slot for a direct `WorkModule::inc_int` call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_module_inc_int_slot: Option<i64>,
    /// Replacement float value for a direct `WorkModule::set_float` call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_module_set_float_value: Option<f32>,
    /// Replacement WorkModule slot for either direct value setter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_module_set_slot: Option<i64>,
    /// Replacement numeric kinetic-energy kind for `CLR_SPEED`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clr_speed_kinetic_kind: Option<i64>,
    /// Replacement numeric kinetic type for `KineticModule::change_kinetic`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_kinetic_type: Option<i64>,
    /// Replacement numeric energy ID for direct suspend/resume kinetic calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinetic_energy_id: Option<i64>,
    /// Replacement bool for `KineticModule::set_consider_ground_friction`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinetic_ground_friction: Option<bool>,
    /// Replacement resolved reserve attribute for `set_consider_ground_friction`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinetic_ground_friction_energy: Option<i64>,
    /// Complete replacement argument vector for a measured expression primitive. The plugin
    /// preserves the captured Lua types while swapping these values into the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression_args: Option<Vec<LuaArgWire>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InjectRuleWire {
    pub frame: f32,
    pub args: Vec<LuaArgWire>,
    /// Exact AREA_WIND family function for wind injection. Attack/grab injections omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Condition from an If block, checked in the match when this call is due.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard: Option<LiveGuard>,
}

/// An If block's condition as the plugin evaluates it. **Must match the plugin's `LiveGuard`.**
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveGuard {
    Grounded,
    Airborne,
    ItemHeld,
    FacingLeft,
    AttackHit,
    AttackShield,
    Stick {
        vertical: bool,
        op: i8,
        value: f32,
        facing: bool,
    },
    Button(String),
    Flag {
        symbol: u64,
        expected: bool,
    },
    Int {
        symbol: u64,
        op: i8,
        value: i32,
    },
    Float {
        symbol: u64,
        op: i8,
        value: f32,
    },
    Status {
        symbol: u64,
    },
    Article {
        symbol: u64,
    },
    Item {
        symbol: u64,
    },
    AnimationChanging,
    Value {
        chart: String,
        var: String,
        op: i8,
        value: f32,
    },
    All(Vec<LiveGuard>),
    Any(Vec<LiveGuard>),
    Not(Box<LiveGuard>),
}

/// Wire category for a block the game's script never does. **Must equal the plugin's value.**
pub const CAT_NODE_STEP: u8 = 37;
/// Wire category for `ArticleModule::generate_article`. **Must equal the plugin's value.**
pub const CAT_ARTICLE_GENERATE: u8 = 38;

/// One live hitbox rule (plugin `HitboxRule`); the full list replaces on every send.
fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

/// `frame_start`/`frame_end` scope suppress/override to ONE hit so multi-hit moves (which
/// reuse the same id across frames) stay independent.
#[derive(Clone, Debug, Serialize)]
pub struct HitboxRuleWire {
    pub motion: u64,
    /// Rule family: 0 attack, 1 grab, 2 wind, and 11 `REVERSE_LR`. Omitted when 0 so old
    /// plugins default to the attack family.
    #[serde(skip_serializing_if = "is_zero_u8")]
    pub category: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hitbox_id: Option<u64>,
    pub suppress: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_start: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_end: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overrides: Option<HbOverridesWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inject: Option<InjectRuleWire>,
    /// The exact ACMD call this rule is for, when the category alone does not identify it.
    ///
    /// [`CAT_SOUND`] and [`CAT_EXPRESSION`] set it. Sound members share one category and every
    /// one of them carries a `Hash40` in slot 0, so a rule for `PLAY_SE` would otherwise apply
    /// cleanly and silently to a `PLAY_SE_REMAIN` on the same frame naming the same sound.
    /// Expression members have different argument shapes, so their macro name is equally part
    /// of the match. Twelve sound categories plus three expression categories to keep in step
    /// across the wire is a worse trade than one name for each family.
    ///
    /// A plugin predating this field ignores it and matches on the category alone — too broad
    /// rather than silently dead, which is the right way round for a preview.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub func: Option<String>,
}

// ── Shared live-override store ───────────────────────────────────────────────
//
// ONE per-kind runtime form that both the Effects panel and the Eff Editor game panel read
// and write, flushed to the plugin by a single debounced sender. (They used to keep separate
// forms with separate debounces and stomped each other's sends for the same kind id.)

const OVERRIDE_DEBOUNCE_MS: u128 = 200;

// This store carries ONLY user-set kind-level tweaks (color× / speed×, which apply to every
// spawn of the effect — that is what the user is asking for when they drag those fields).
// It used to carry a second class of entry, "modifiers derived from authored .eff edits",
// which also pushed `scale`. That derivation was removed: authored values are per emitter
// and this wire message is per kind, so it recoloured whole effects when one emitter was
// edited. Authored edits now go through the eff rebuild + hot-reload path (app::
// apply_authored_eff_live), which is per-emitter exact.

#[derive(Clone, Debug)]
pub struct LiveOverride {
    pub form: RpmEffectData,
    dirty_at: Option<Instant>,
    /// The USER set this entry's color×/speed — only these export as LAST_EFFECT_SET_*
    /// tweaks and persist in the project.
    user_tweaked: bool,
}

impl LiveOverride {
    fn new(form: RpmEffectData) -> Self {
        Self {
            form,
            dirty_at: None,
            user_tweaked: false,
        }
    }
}

/// Are these two capture lines the same script line, at the resolution the editor works in?
///
/// Only ever true within one run. This guards against the plugin re-sending its immutable log
/// on reconnect.
///
/// Frames are compared as the ROUNDED integer the editor displays rather than as raw floats.
/// The float is `MotionModule::frame` at call time and differs in the low bits between
/// performances; that only matters for a resend within a run, but comparing what is actually
/// displayed is the honest test either way.
fn same_capture(a: &CaptureLine, b: &CaptureLine) -> bool {
    a.run == b.run
        && a.kind == b.kind
        && a.motion == b.motion
        && a.func == b.func
        && a.frame.max(0.0).round() == b.frame.max(0.0).round()
        && a.args == b.args
}

#[derive(Default)]
pub struct LiveOverrides {
    entries: BTreeMap<u64, LiveOverride>,
}

/// Kind-level color×/speed× multipliers.
///
/// These predate the live carrier. They were once the only way to change how an effect looked
/// at runtime, and they are whole-effect by construction — one multiplier tints every emitter
/// of every spawn, and cannot express a per-key or color1 edit at all. Authored eff edits now
/// do that exactly, so the eff editor no longer offers a second, cruder way to recolour the
/// same effect.
///
/// The store stays because the multipliers still ARRIVE from two places the user did not type
/// them into: a project's saved `live_tweaks` (restored and re-sent on load) and pins already
/// set in the running game (importable, or clearable, from the pin-sync prompt). Both paths
/// go through `set_form`/`restore_tweak`, and `flush_due` sends them.
impl LiveOverrides {
    /// Adopt a form already reported by the game. Importing the game's current pins does not
    /// need to echo the complete form back; doing so could race a new spawn observation.
    pub fn set_form(&mut self, hash: u64, form: RpmEffectData) {
        let e = self
            .entries
            .entry(hash)
            .or_insert_with(|| LiveOverride::new(form.clone()));
        e.form = form;
        e.dirty_at = None;
    }

    /// Send every entry whose debounce has elapsed. Returns how many were sent.
    /// `include_scale` is false: size is a per-spawn value pushed through the spawn rules,
    /// never inferred from this kind-level form. Transform fields are omitted for the same
    /// reason — ACMD position/rotation are per spawn.
    pub fn flush_due(&mut self, link: &GameLink) -> usize {
        let mut sent = 0;
        for (hash, e) in self.entries.iter_mut() {
            if let Some(t) = e.dirty_at {
                if t.elapsed().as_millis() > OVERRIDE_DEBOUNCE_MS {
                    e.dirty_at = None;
                    link.send_modifier_edit(*hash, &e.form, false);
                    sent += 1;
                }
            }
        }
        sent
    }

    /// Send every restored/project entry immediately, bypassing the interactive debounce.
    /// Project loading is an explicit state restore rather than a live drag, so waiting for a
    /// timer would leave the game temporarily different from the loaded project.
    pub fn flush_all(&mut self, link: &GameLink) -> usize {
        let mut sent = 0;
        for (hash, entry) in self.entries.iter_mut() {
            if entry.dirty_at.take().is_some() {
                link.send_modifier_edit(*hash, &entry.form, false);
                sent += 1;
            }
        }
        sent
    }

    /// Re-send every user-owned multiplier after a socket reconnect. A completed TCP write is
    /// not an application acknowledgement: the plugin may restart before its game thread
    /// applies the edit, so each handshake replays the editor's authoritative state.
    pub fn resend_tweaks(&self, link: &GameLink) -> usize {
        let mut sent = 0;
        for (hash, entry) in &self.entries {
            if entry.user_tweaked {
                link.send_modifier_edit(*hash, &entry.form, false);
                sent += 1;
            }
        }
        sent
    }

    /// True while a debounced send is pending (keep repainting so it fires).
    pub fn any_dirty(&self) -> bool {
        self.entries.values().any(|e| e.dirty_at.is_some())
    }

    /// Flag the entry's color×/speed as USER-set (exports + persists as a live tweak).
    pub fn mark_tweak(&mut self, hash: u64) {
        if let Some(e) = self.entries.get_mut(&hash) {
            e.dirty_at = Some(Instant::now());
            e.user_tweaked = true;
        }
    }

    /// All user-set tweak entries: (hash, form) — export/persist as LiveTweaks.
    pub fn tweaked(&self) -> Vec<(u64, RpmEffectData)> {
        self.entries
            .iter()
            .filter(|(_, e)| e.user_tweaked)
            .map(|(h, e)| (*h, e.form.clone()))
            .collect()
    }

    /// Revert a user tweak: color/speed back to identity, unflag, and re-send.
    pub fn clear_tweak(&mut self, hash: u64) {
        if let Some(e) = self.entries.get_mut(&hash) {
            e.form.rainbow = Rainbow::default();
            e.form.speed = 1.0;
            e.user_tweaked = false;
            e.dirty_at = Some(Instant::now());
        }
    }

    /// Restore a tweak from a loaded project: sets color/speed, flags user_tweaked,
    /// and schedules a send.
    pub fn restore_tweak(&mut self, hash: u64, init: RpmEffectData) {
        let e = self
            .entries
            .entry(hash)
            .or_insert_with(|| LiveOverride::new(init.clone()));
        e.form.rainbow = init.rainbow;
        e.form.speed = init.speed;
        if e.form.effect_name == "0x0" {
            e.form.effect_name = init.effect_name;
        }
        e.user_tweaked = true;
        e.dirty_at = Some(Instant::now());
    }
}

// ── Link state ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkStatus {
    Disconnected,
    Connecting,
    Connected,
}

/// Plugin acknowledgement for the hidden asset-carrier lifecycle.
///
/// `served` counts current-generation files genuinely read through Arcropolis. `ready` is only
/// true after the game-thread carrier owner is live and every file in that generation has served;
/// a queued or retiring snapshot is never presented as active.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetBundleStatus {
    pub target: String,
    pub generation: u64,
    pub staged: usize,
    pub served: usize,
    pub serving_generation: u64,
    pub phase: String,
    pub ready: bool,
    pub reports: u64,
}

/// One live fighter or weapon identity reported by the plugin for constrained
/// gimmick-operation pickers. Engine writes never trust this desktop copy;
/// the plugin re-proves identity, ownership, kind, and symbol at execution.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct LiveGimmickAgent {
    pub boid: u32,
    pub category: i32,
    pub kind: i32,
    pub entry_id: i32,
    pub owner_boid: Option<u32>,
    pub fighter: Option<String>,
}

#[derive(Deserialize)]
struct GimmickAgentsWire {
    version: u8,
    seq: u64,
    /// Rows stay untyped until individually validated so one transient object
    /// cannot hide every otherwise valid live fighter from the selector.
    agents: Vec<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveGimmickMeterValue {
    Float(f32),
    Int(i32),
    Flag(bool),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LiveGimmickMeter {
    pub binding_id: String,
    pub fighter: String,
    pub entry_id: i32,
    pub boid: u32,
    pub symbol: String,
    pub kind: String,
    pub value: LiveGimmickMeterValue,
}

#[derive(Deserialize)]
struct GimmickMetersWire {
    version: u8,
    seq: u64,
    values: Vec<LiveGimmickMeter>,
}

pub type GimmickLiveAck = crate::gimmicks::live_protocol::Ack;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GimmickLiveSnapshot {
    pub version: u8,
    pub seq: u64,
    pub behavior_revision: u64,
    pub parameter_revision: u64,
    pub behavior: Vec<serde_json::Value>,
    pub parameters: Vec<serde_json::Value>,
    #[serde(default)]
    pub parameter_health: Option<crate::gimmicks::live_params::ParameterHealth>,
}

/// Separately negotiated bounds for the V5 multi-chart telemetry rows. Keeping
/// this out of `GimmickLiveSnapshot` lets older strict snapshot readers ignore
/// the optional feature frame without rejecting the established V1 snapshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GimmickControllerCapabilities {
    pub version: u8,
    pub seq: u64,
    pub request_version: u8,
    pub model_version: u8,
    pub max_charts: u8,
    pub max_actors: u16,
    pub max_rows: u16,
}

impl GimmickControllerCapabilities {
    fn is_supported_contract(&self) -> bool {
        self.version == 1
            && self.seq > 0
            && self.request_version == crate::gimmicks::controller_protocol::REQUEST_VERSION
            && self.model_version == crate::gimmicks::live_model::MODEL_VERSION
            && self.max_charts == crate::gimmicks::controller_runtime::MAX_CHARTS as u8
            && self.max_actors == 128
            && self.max_rows == 1024
    }
}

/// Optional capabilities negotiated independently from the strict V5 controller
/// contract. Older desktops ignore this unknown frame; older plugins omit it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct GimmickFeatureCapabilities {
    version: u8,
    seq: u64,
    features: Vec<String>,
}

impl GimmickFeatureCapabilities {
    fn is_well_formed(&self) -> bool {
        self.version == 1
            && self.seq > 0
            && self.features.len() <= 32
            && self.features.iter().all(|feature| {
                !feature.is_empty() && feature.len() <= 64 && !feature.contains('\0')
            })
            && self
                .features
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.features.len()
    }

    fn supports_feature(&self, feature: &str) -> bool {
        self.is_well_formed() && self.features.iter().any(|candidate| candidate == feature)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyGimmickBehaviorRow {
    fighter: String,
    boid: u32,
    entry_id: i32,
    revision: u64,
    state: String,
    vars: BTreeMap<String, crate::gimmicks::live_model::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControllerGimmickBehaviorRow {
    fighter: String,
    boid: u32,
    entry_id: i32,
    revision: u64,
    state: String,
    vars: BTreeMap<String, crate::gimmicks::live_model::Value>,
    chart_id: String,
    program_id: String,
    stock_generation: u32,
}

#[cfg(test)]
fn valid_gimmick_live_rows(snapshot: &GimmickLiveSnapshot) -> bool {
    valid_gimmick_live_rows_with_capabilities(snapshot, None)
}

fn has_controller_chart_rows(snapshot: &GimmickLiveSnapshot) -> bool {
    snapshot.behavior.iter().any(|value| {
        value.as_object().is_some_and(|row| {
            row.contains_key("chart_id")
                || row.contains_key("program_id")
                || row.contains_key("stock_generation")
        })
    })
}

fn valid_gimmick_behavior_common(
    fighter: &str,
    entry_id: i32,
    revision: u64,
    snapshot: &GimmickLiveSnapshot,
    state: &str,
    vars: &BTreeMap<String, crate::gimmicks::live_model::Value>,
) -> bool {
    registry::fighter_kind_symbol(fighter).is_some()
        && (0..8).contains(&entry_id)
        && revision == snapshot.behavior_revision
        && !state.is_empty()
        && state.len() <= 48
        && vars.len() <= crate::gimmicks::live_model::MAX_VARIABLES
        && vars.iter().all(|(name, value)| {
            !name.is_empty()
                && name.len() <= 48
                && !matches!(
                    value,
                    crate::gimmicks::live_model::Value::Float(number) if !number.is_finite()
                )
        })
}

fn valid_gimmick_live_rows_with_capabilities(
    snapshot: &GimmickLiveSnapshot,
    capabilities: Option<&GimmickControllerCapabilities>,
) -> bool {
    if snapshot
        .parameter_health
        .as_ref()
        .is_some_and(|health| health.revision != snapshot.parameter_revision)
    {
        return false;
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct ParameterRow {
        fighter: String,
        path: String,
        hits: u64,
        baseline: Option<crate::gimmicks::live_params::LiveParamValue>,
        last_observed: Option<crate::gimmicks::live_params::LiveParamValue>,
    }
    let controller_rows = has_controller_chart_rows(snapshot);
    if controller_rows {
        let Some(capabilities) = capabilities.filter(|caps| caps.is_supported_contract()) else {
            return false;
        };
        if snapshot.behavior.len() > capabilities.max_rows as usize {
            return false;
        }
        let mut actors = BTreeMap::<u32, (String, i32, u32)>::new();
        let mut actor_charts = BTreeMap::<u32, BTreeSet<String>>::new();
        let mut actor_variables = BTreeMap::<u32, usize>::new();
        let mut chart_programs = BTreeMap::<(String, String), String>::new();
        let mut actor_chart_rows = BTreeSet::<(u32, String)>::new();
        for value in &snapshot.behavior {
            let Ok(row) = serde_json::from_value::<ControllerGimmickBehaviorRow>(value.clone())
            else {
                return false;
            };
            if !valid_gimmick_behavior_common(
                &row.fighter,
                row.entry_id,
                row.revision,
                snapshot,
                &row.state,
                &row.vars,
            ) || row.chart_id.trim().is_empty()
                || row.chart_id.len() > crate::gimmicks::controller_protocol::MAX_CHART_ID_BYTES
                || row.chart_id.as_bytes().contains(&0)
                || row.program_id.trim().is_empty()
                || row.program_id.len() > crate::gimmicks::live_model::MAX_PROGRAM_ID_BYTES
                || row.program_id.as_bytes().contains(&0)
                || row.vars.len() > 64
                || !actor_chart_rows.insert((row.boid, row.chart_id.clone()))
            {
                return false;
            }
            let identity = (row.fighter.clone(), row.entry_id, row.stock_generation);
            if actors
                .get(&row.boid)
                .is_some_and(|known| known != &identity)
            {
                return false;
            }
            actors.entry(row.boid).or_insert(identity);
            let charts = actor_charts.entry(row.boid).or_default();
            charts.insert(row.chart_id.clone());
            if charts.len() > capabilities.max_charts as usize {
                return false;
            }
            let variables = actor_variables.entry(row.boid).or_default();
            *variables = variables.saturating_add(row.vars.len());
            if *variables > 128 {
                return false;
            }
            let key = (row.fighter.clone(), row.chart_id.clone());
            if chart_programs
                .get(&key)
                .is_some_and(|known| known != &row.program_id)
            {
                return false;
            }
            chart_programs.entry(key).or_insert(row.program_id);
        }
        if actors.len() > capabilities.max_actors as usize {
            return false;
        }
    } else {
        // The established rows retain their original 16-row ceiling and strict shape.
        if snapshot.behavior.len() > 16 {
            return false;
        }
        let mut objects = BTreeSet::new();
        for value in &snapshot.behavior {
            let Ok(row) = serde_json::from_value::<LegacyGimmickBehaviorRow>(value.clone()) else {
                return false;
            };
            if !valid_gimmick_behavior_common(
                &row.fighter,
                row.entry_id,
                row.revision,
                snapshot,
                &row.state,
                &row.vars,
            ) || !objects.insert(row.boid)
            {
                return false;
            }
        }
    }
    let mut paths = std::collections::BTreeSet::new();
    for value in &snapshot.parameters {
        let Ok(row) = serde_json::from_value::<ParameterRow>(value.clone()) else {
            return false;
        };
        if registry::fighter_kind_symbol(&row.fighter).is_none() || row.path.len() > 256
            || crate::gimmicks::live_params::classify_engine_path(&row.path).is_err()
            || !paths.insert((row.fighter, row.path))
            || (row.hits == 0 && row.last_observed.is_some())
            || [row.baseline, row.last_observed].into_iter().flatten().any(|value|
                matches!(value, crate::gimmicks::live_params::LiveParamValue::Float(number) if !number.is_finite())) {
            return false;
        }
    }
    true
}

/// Sparse pins-only form the plugin reports alongside merged values (newer plugins) —
/// mirrors slight_replica kinds::Pinned. `Some` fields are active user overrides in-game.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PinsWire {
    pub scale: Option<f32>,
    pub rate: Option<f32>,
    pub pos: Option<Point3D>,
    pub rot: Option<Point3D>,
    pub visible: Option<bool>,
    pub frame: Option<f32>,
    pub color: Option<Color>,
    pub movement_state: Option<f32>,
}

impl PinsWire {
    pub fn any(&self) -> bool {
        self.scale.is_some()
            || self.rate.is_some()
            || self.pos.is_some()
            || self.rot.is_some()
            || self.visible.is_some()
            || self.frame.is_some()
            || self.color.is_some()
            || self.movement_state.is_some()
    }
}

#[derive(Clone, Debug)]
pub struct LiveKind {
    pub name: String,
    /// Latest values from the plugin (merged observed + pins).
    pub data: RpmEffectData,
    /// First values seen for this kind this connection — the pristine spawn baseline
    /// edits are computed against so repeated edits don't compound.
    pub first: RpmEffectData,
    /// Active in-game pins (None on older plugins or when nothing is pinned).
    pub pins: Option<PinsWire>,
    pub updates: u64,
    pub last_update: Instant,
}

struct Shared {
    status: LinkStatus,
    client_id: Option<u64>,
    connection_epoch: u64,
    /// Complete identity-only object snapshot from the current connection.
    /// Replaced atomically and cleared at every socket/session boundary.
    gimmick_agents: BTreeMap<u32, LiveGimmickAgent>,
    gimmick_agents_seq: u64,
    gimmick_agents_wire_seq: Option<u64>,
    gimmick_agents_epoch: u64,
    gimmick_agents_received: Option<Instant>,
    /// Most recent current-session roster rejection, cleared by a valid
    /// replacement. Exposed as quiet hover detail instead of a red UI error.
    gimmick_agents_error: Option<String>,
    gimmick_meters: Vec<LiveGimmickMeter>,
    gimmick_live_expected: BTreeMap<String, u64>,
    gimmick_live_acks: BTreeMap<String, GimmickLiveAck>,
    gimmick_live_snapshot: Option<GimmickLiveSnapshot>,
    gimmick_controller_capabilities: Option<GimmickControllerCapabilities>,
    gimmick_controller_capabilities_seq: Option<u64>,
    gimmick_feature_capabilities: Option<GimmickFeatureCapabilities>,
    gimmick_feature_capabilities_seq: Option<u64>,
    gimmick_meters_wire_seq: Option<u64>,
    kinds: BTreeMap<u64, LiveKind>,
    /// Live ACMD capture log, keyed by motion hash (deduped; survives reconnects).
    captures: BTreeMap<u64, Vec<CaptureLine>>,
    /// First run observed for each fighter-kind + motion. Later runs are rejected even if an
    /// old or mismatched plugin tries to send them.
    capture_claimed_runs: BTreeMap<(i32, u64), u32>,
    /// Bumped on every new capture line — lets the app cheaply notice new data.
    captures_seq: u64,
    /// Compact outcome rows keyed exactly like the plugin capture cache.
    capture_debug: BTreeMap<(i32, u64), CaptureDebugSnapshot>,
    /// Live-carrier readiness reported by the plugin: 0 = none, 1 = staged/building, 2 = live.
    carrier_state: u8,
    /// The carrier battle object exists and is active — the real "can spawn now" signal.
    carrier_spawned: bool,
    /// Kinds the current carrier can serve — distinguishes "not up yet" from "up but does
    /// not know this kind".
    carrier_kinds: usize,
    /// Bumped on every CarrierStatus so callers can tell "no report yet" from "reported 0".
    carrier_seq: u64,
    /// Donor-bytes generation the LIVE carrier was built from. A send is only complete once
    /// this exceeds whatever was live when the send started — otherwise the previous carrier,
    /// still up and still reporting ready, reads as instant success.
    carrier_gen: u64,
    /// Why the plugin rejected the last carrier push, if it did. Taken by the app, which turns
    /// it into a status line and stops waiting — a rejected push never advances `carrier_gen`,
    /// so without this the editor waits for a carrier that is never coming.
    carrier_error: Option<String>,
    /// Asset-carrier progress. These files are never installed by resident-buffer mutation.
    asset_bundle_target: String,
    asset_bundle_generation: u64,
    asset_bundle_staged: usize,
    asset_bundle_served: usize,
    asset_bundle_serving_generation: u64,
    asset_bundle_phase: String,
    asset_bundle_ready: bool,
    asset_bundle_seq: u64,
    asset_bundle_error: Option<String>,
    /// (fighter kind, motion hash) → number of completed playbacks the plugin reported.
    /// A bump means "every line that motion produces has now been streamed".
    capture_ends: BTreeMap<(i32, u64), u64>,
    /// Exact completed runs. Unlike the compatibility counter above, this cannot be advanced by
    /// another instance or a later playback of the same motion.
    capture_completed_runs: BTreeSet<(i32, u64, u32)>,
    /// Fighter kinds forgotten by the desktop roster. Their capture stream is ignored until the
    /// user restores the fighter, so a later playback cannot silently repopulate its memory.
    ignored_capture_kinds: BTreeSet<i32>,
    outbox: Vec<String>,
    /// Gimmick operations temporarily removed from `outbox` for the current socket
    /// flush. Counting them prevents a concurrent UI action burst from overfilling the
    /// queue before an interrupted flush can put its unsent suffix back.
    gimmick_operations_in_flight: usize,
    last_error: Option<String>,
    frames_rx: u64,
    edits_tx: u64,
    /// When the last inbound frame arrived (any header, including `Pong`). Drives the
    /// half-open detector in `serve_connection`: a connection that stops delivering
    /// anything for `STALE_TIMEOUT` is dropped and redialled.
    last_rx: Option<Instant>,
}

/// Markers for every full-list-replace family on the wire. Each such message replaces
/// the plugin's entire list for its family, so only the newest per marker matters;
/// intermediate states replay stale edits and churn per-playback injection identity.
const FULL_REPLACE_MARKERS: &[&str] = &[
    "\"spawn_rules\":",
    "\"hitbox_rules\":",
    "\"effect_control_rules\":",
    "\"effect_aliases\":",
    "\"donor_effs\":",
    "\"donor_bytes\":",
    "\"asset_bundle\":",
    "\"effect_names\":",
    "\"gimmick_behavior\":",
    "\"gimmick_parameters\":",
];

const IDEMPOTENT_COMMAND_MARKERS: &[&str] = &[
    r#""command":"reset_pins""#,
    r#""command":"clear_acmd_captures""#,
    r#""command":"live_eff_reload""#,
    r#""command":"live_eff_probe""#,
];

const GIMMICK_OPERATION_MARKER: &str = r#""gimmick_operation":"#;
const GIMMICK_RESTART_MARKER: &str = r#""gimmick_restart":"#;

fn is_gimmick_operation(message: &str) -> bool {
    message.contains(GIMMICK_OPERATION_MARKER) || message.contains(GIMMICK_RESTART_MARKER)
}

fn discard_session_bound_messages(messages: &mut Vec<String>) {
    messages.retain(|message| {
        if message.contains(GIMMICK_RESTART_MARKER) {
            return false;
        }
        let Some(body) = message
            .strip_prefix("<TCP_MESSAGE>")
            .and_then(|message| message.strip_suffix("</TCP_MESSAGE>"))
        else {
            return true;
        };
        // The old full replacements are reconnect-safe. V5 requires a new
        // capability confirmation before being sent to a possibly older peer.
        let controller_bundle = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|root| root.get("gimmick_behavior")?.get("version")?.as_u64())
            == Some(u64::from(
                crate::gimmicks::controller_protocol::REQUEST_VERSION,
            ));
        !controller_bundle
    });
}

fn retain_latest_for_marker(messages: &mut Vec<String>, marker: &str) {
    let Some(latest) = messages.iter().rposition(|m| m.contains(marker)) else {
        return;
    };
    let mut index = 0;
    messages.retain(|m| {
        let keep = !m.contains(marker) || index == latest;
        index += 1;
        keep
    });
}

/// Keep only the newest message per full-replace family in a batch. Applied both when
/// queueing (so offline drags cannot grow the outbox without bound) and when flushing
/// (so a burst of queued states sends once).
fn retain_latest_full_replace(messages: &mut Vec<String>) {
    for marker in FULL_REPLACE_MARKERS {
        retain_latest_for_marker(messages, marker);
    }
}

fn retain_latest_idempotent_commands(messages: &mut Vec<String>) {
    for marker in IDEMPOTENT_COMMAND_MARKERS {
        retain_latest_for_marker(messages, marker);
    }
}

fn trim_outbox(messages: &mut Vec<String>) {
    while messages.len() > OUTBOX_CAP {
        // A symbolic operation represents one user action, not an authoritative
        // replacement snapshot. Prefer discarding ordinary edits, then idempotent
        // commands, but never silently remove an operation. The operation-specific
        // enqueue path refuses the only state in which every frame is protected.
        let removable = messages
            .iter()
            .position(|message| {
                !is_gimmick_operation(message)
                    && !IDEMPOTENT_COMMAND_MARKERS
                        .iter()
                        .any(|marker| message.contains(marker))
            })
            .or_else(|| {
                messages
                    .iter()
                    .position(|message| !is_gimmick_operation(message))
            });
        let Some(removable) = removable else {
            debug_assert!(
                false,
                "gimmick operation queue exceeded its reserved capacity"
            );
            break;
        };
        messages.remove(removable);
    }
}

/// Push a full-replace frame, coalescing superseded states for the same family.
fn queue_latest_full_replace(outbox: &mut Vec<String>, frame: String) {
    outbox.push(frame);
    retain_latest_full_replace(outbox);
    trim_outbox(outbox);
}

/// Push a command or sparse modifier edit while keeping the queue bounded. Repeated idempotent
/// commands coalesce, and overflow discards the oldest modifier before a control command.
fn push_outbox_capped(outbox: &mut Vec<String>, frame: String) {
    outbox.push(frame);
    retain_latest_idempotent_commands(outbox);
    trim_outbox(outbox);
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            status: LinkStatus::Disconnected,
            client_id: None,
            connection_epoch: 0,
            gimmick_agents: BTreeMap::new(),
            gimmick_agents_seq: 0,
            gimmick_agents_wire_seq: None,
            gimmick_agents_epoch: 0,
            gimmick_agents_received: None,
            gimmick_agents_error: None,
            gimmick_meters: Vec::new(),
            gimmick_live_expected: BTreeMap::new(),
            gimmick_live_acks: BTreeMap::new(),
            gimmick_live_snapshot: None,
            gimmick_controller_capabilities: None,
            gimmick_controller_capabilities_seq: None,
            gimmick_feature_capabilities: None,
            gimmick_feature_capabilities_seq: None,
            gimmick_meters_wire_seq: None,
            kinds: BTreeMap::new(),
            captures: BTreeMap::new(),
            capture_claimed_runs: BTreeMap::new(),
            captures_seq: 0,
            capture_debug: BTreeMap::new(),
            carrier_state: 0,
            carrier_spawned: false,
            carrier_kinds: 0,
            carrier_seq: 0,
            carrier_gen: 0,
            carrier_error: None,
            asset_bundle_target: String::new(),
            asset_bundle_generation: 0,
            asset_bundle_staged: 0,
            asset_bundle_served: 0,
            asset_bundle_serving_generation: 0,
            asset_bundle_phase: String::new(),
            asset_bundle_ready: false,
            asset_bundle_seq: 0,
            asset_bundle_error: None,
            capture_ends: BTreeMap::new(),
            capture_completed_runs: BTreeSet::new(),
            ignored_capture_kinds: BTreeSet::new(),
            outbox: Vec::new(),
            gimmick_operations_in_flight: 0,
            last_error: None,
            frames_rx: 0,
            edits_tx: 0,
            last_rx: None,
        }
    }
}

pub struct GameLink {
    shared: Arc<Mutex<Shared>>,
    started: AtomicBool,
    rule_send_suppressed: AtomicBool,
    project_send_suppressed: AtomicBool,
}

impl Default for GameLink {
    fn default() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared::default())),
            started: AtomicBool::new(false),
            rule_send_suppressed: AtomicBool::new(false),
            project_send_suppressed: AtomicBool::new(false),
        }
    }
}

impl GameLink {
    /// Send a complete, replaceable live gimmick snapshot. The game acknowledgement,
    /// not this queueing result, is the authority on acceptance.
    pub fn send_gimmick_live(
        &self,
        family: &str,
        request: serde_json::Value,
    ) -> Result<(), String> {
        let key = match family {
            "behavior" => "gimmick_behavior",
            "parameters" => "gimmick_parameters",
            _ => return Err("Unknown live gimmick family".into()),
        };
        let revision = request
            .get("revision")
            .and_then(serde_json::Value::as_u64)
            .ok_or("Live gimmick snapshot has no revision")?;
        let controller_bundle = family == "behavior" && request.get("charts").is_some();
        let mut required_features = std::collections::BTreeSet::new();
        if controller_bundle {
            let decoded = serde_json::from_value::<
                crate::gimmicks::controller_protocol::ControllerBundleRequest,
            >(request.clone())
            .map_err(|error| format!("Invalid controller bundle: {error}"))?;
            crate::gimmicks::controller_protocol::encode_request(&decoded)?;
            for feature in decoded
                .charts
                .iter()
                .flat_map(|chart| chart.program.required_native_features())
            {
                required_features.insert(feature.to_owned());
            }
        } else if family == "behavior" {
            let decoded = serde_json::from_value::<crate::gimmicks::live_protocol::LiveRequest>(
                request.clone(),
            )
            .map_err(|error| format!("Invalid live behavior snapshot: {error}"))?;
            crate::gimmicks::live_protocol::encode_request(&decoded)?;
            required_features.extend(
                decoded
                    .programs
                    .iter()
                    .flat_map(|program| program.required_native_features())
                    .map(str::to_owned),
            );
        }
        let payload = serde_json::to_string(&serde_json::json!({key: request}))
            .map_err(|error| error.to_string())?;
        let limit = if family == "behavior" {
            crate::gimmicks::live_protocol::MAX_REQUEST_BYTES
        } else {
            crate::gimmicks::live_params::MAX_REQUEST_BYTES
        };
        if payload.len() > limit {
            return Err(format!(
                "Live {family} snapshot exceeds {} KiB",
                limit / 1024
            ));
        }
        let mut shared = self.shared.lock().map_err(|_| "Live queue unavailable")?;
        if controller_bundle
            && (shared.status != LinkStatus::Connected
                || shared.client_id.is_none()
                || shared
                    .gimmick_controller_capabilities
                    .as_ref()
                    .is_none_or(|capabilities| !capabilities.is_supported_contract()))
        {
            return Err(
                "The current connection has not confirmed multi-chart controller support".into(),
            );
        }
        if !required_features.is_empty() {
            let capabilities = shared
                .gimmick_feature_capabilities
                .as_ref()
                .filter(|_| shared.client_id.is_some() && shared.status == LinkStatus::Connected)
                .ok_or("The current plugin does not advertise optional native features")?;
            if required_features
                .iter()
                .any(|feature| !capabilities.supports_feature(feature))
            {
                return Err(
                    "The current plugin does not advertise every required controller feature"
                        .into(),
                );
            }
        }
        shared
            .gimmick_live_expected
            .insert(family.to_owned(), revision);
        shared.gimmick_live_acks.remove(family);
        queue_latest_full_replace(
            &mut shared.outbox,
            format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>"),
        );
        shared.edits_tx += 1;
        Ok(())
    }

    pub fn gimmick_live_ack(&self, family: &str) -> Option<GimmickLiveAck> {
        let shared = self.shared.lock().ok()?;
        (shared.status == LinkStatus::Connected && shared.client_id.is_some())
            .then(|| shared.gimmick_live_acks.get(family).cloned())
            .flatten()
    }

    pub fn gimmick_live_snapshot(&self) -> Option<GimmickLiveSnapshot> {
        let shared = self.shared.lock().ok()?;
        if shared.status != LinkStatus::Connected || shared.client_id.is_none() {
            return None;
        }
        let mut snapshot = shared.gimmick_live_snapshot.clone()?;
        // Legacy callers see only the reserved main chart. Strip V5-only fields
        // so their existing strict row readers continue to accept the projection.
        snapshot.behavior.retain(|value| {
            value
                .get("chart_id")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|chart_id| chart_id == "main")
        });
        for value in &mut snapshot.behavior {
            if let Some(row) = value.as_object_mut() {
                row.remove("chart_id");
                row.remove("program_id");
                row.remove("stock_generation");
            }
        }
        Some(snapshot)
    }

    /// Validated V5 multi-chart capability from the current connection.
    #[cfg(test)]
    pub fn gimmick_controller_capabilities(&self) -> Option<GimmickControllerCapabilities> {
        let shared = self.shared.lock().ok()?;
        (shared.status == LinkStatus::Connected && shared.client_id.is_some())
            .then(|| shared.gimmick_controller_capabilities.clone())
            .flatten()
    }

    /// Full validated controller snapshot, including all chart rows and their
    /// actor/program identities. Legacy editor surfaces use the projected getter above.
    pub fn gimmick_controller_snapshot(&self) -> Option<GimmickLiveSnapshot> {
        let shared = self.shared.lock().ok()?;
        (shared.status == LinkStatus::Connected && shared.client_id.is_some())
            .then(|| shared.gimmick_live_snapshot.clone())
            .flatten()
    }

    /// One-shot restart, never replayed after a transport interruption.
    pub fn restart_gimmick_controller(
        &self,
        fighter: &str,
        boid: u32,
        revision: u64,
    ) -> Result<(), String> {
        self.queue_gimmick_restart(fighter, boid, None, revision)
    }

    /// One-shot restart of a specific chart after confirming that the current
    /// accepted behavior snapshot still reports that chart for this fighter.
    pub fn restart_gimmick_chart(
        &self,
        fighter: &str,
        boid: u32,
        chart_id: &str,
        revision: u64,
    ) -> Result<(), String> {
        if chart_id.len() > crate::gimmicks::controller_protocol::MAX_CHART_ID_BYTES
            || chart_id.trim().is_empty()
            || chart_id.as_bytes().contains(&0)
        {
            return Err("Chart ID must be nonblank, at most 48 bytes, and contain no NUL".into());
        }
        self.queue_gimmick_restart(fighter, boid, Some(chart_id), revision)
    }

    fn queue_gimmick_restart(
        &self,
        fighter: &str,
        boid: u32,
        chart_id: Option<&str>,
        revision: u64,
    ) -> Result<(), String> {
        let mut shared = self.shared.lock().map_err(|_| "Live queue unavailable")?;
        if shared.status != LinkStatus::Connected || shared.client_id.is_none() {
            return Err("Connect to the game before restarting a controller".into());
        }
        if shared.gimmick_live_expected.get("behavior") != Some(&revision)
            || !shared
                .gimmick_live_acks
                .get("behavior")
                .is_some_and(|ack| ack.accepted && ack.revision == revision)
        {
            return Err("Wait for the current behavior snapshot to be accepted".into());
        }
        if let Some(chart_id) = chart_id {
            let snapshot = shared
                .gimmick_live_snapshot
                .as_ref()
                .filter(|snapshot| snapshot.behavior_revision == revision)
                .ok_or("Wait for the accepted behavior snapshot to report this chart")?;
            if chart_id != "main"
                && shared
                    .gimmick_controller_capabilities
                    .as_ref()
                    .is_none_or(|capabilities| !capabilities.is_supported_contract())
            {
                return Err(
                    "The current connection has not confirmed multi-chart controller support"
                        .into(),
                );
            }
            let chart_exists = snapshot.behavior.iter().any(|row| {
                let Some(row) = row.as_object() else {
                    return false;
                };
                let row_chart_id = row
                    .get("chart_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("main");
                row.get("fighter").and_then(serde_json::Value::as_str) == Some(fighter)
                    && row.get("boid").and_then(serde_json::Value::as_u64) == Some(u64::from(boid))
                    && row_chart_id == chart_id
            });
            if !chart_exists {
                return Err(
                    "The accepted behavior snapshot does not contain this fighter chart".into(),
                );
            }
        }
        if shared
            .outbox
            .iter()
            .filter(|message| is_gimmick_operation(message))
            .count()
            + shared.gimmick_operations_in_flight
            >= OUTBOX_CAP
        {
            return Err("Live operation queue is full".into());
        }
        let mut request = serde_json::json!({
            "fighter": fighter,
            "boid": boid,
            "expected_revision": revision
        });
        if let (Some(chart_id), Some(request)) = (chart_id, request.as_object_mut()) {
            request.insert("chart_id".into(), chart_id.into());
        }
        let payload = serde_json::json!({"gimmick_restart": request});
        shared
            .outbox
            .push(format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>"));
        trim_outbox(&mut shared.outbox);
        shared.edits_tx += 1;
        Ok(())
    }
    /// Render authoring controls in tests without starting a game connection.
    #[cfg(test)]
    pub fn offline_for_test() -> Self {
        let link = Self::default();
        link.started.store(true, Ordering::SeqCst);
        link
    }

    /// Queue a constrained article generate request. The plugin repeats all
    /// identifier, fighter, and live-object checks on its game thread before
    /// it can call `ArticleModule`. `Ok(())` means only that this process
    /// accepted the action into its bounded outbound queue; inspect the plugin
    /// diagnostic log for game-thread acceptance or rejection.
    pub fn generate_gimmick_article(
        &self,
        fighter: &str,
        fighter_boid: u32,
        symbol: &str,
    ) -> Result<(), GimmickOperationError> {
        registry::resolve(fighter, symbol, IdentifierKind::Article)?;
        self.queue_gimmick_operation(GimmickOperationRequest {
            version: 1,
            fighter,
            fighter_boid,
            operation: GimmickOperationKind::GenerateArticle { symbol },
        })
    }

    /// Queue a constrained article removal request. Removing an absent
    /// article is guarded and diagnostic-only on the plugin side.
    pub fn remove_gimmick_article(
        &self,
        fighter: &str,
        fighter_boid: u32,
        symbol: &str,
    ) -> Result<(), GimmickOperationError> {
        registry::resolve(fighter, symbol, IdentifierKind::Article)?;
        self.queue_gimmick_operation(GimmickOperationRequest {
            version: 1,
            fighter,
            fighter_boid,
            operation: GimmickOperationKind::RemoveArticle { symbol },
        })
    }

    /// Queue a status request for one live weapon owned by the named fighter.
    /// The generated concrete weapon-kind table prevents a status intended for
    /// one article from being sent to another owned weapon.
    pub fn request_gimmick_weapon_status(
        &self,
        fighter: &str,
        fighter_boid: u32,
        weapon_boid: u32,
        symbol: &str,
    ) -> Result<(), GimmickOperationError> {
        if weapon_boid == 0 {
            return Err(GimmickOperationError::InvalidWeaponObjectId);
        }
        let status = registry::resolve(fighter, symbol, IdentifierKind::WeaponStatus)?;
        if registry::weapon_kind(fighter, status.group).is_none() {
            return Err(GimmickOperationError::MissingWeaponKind {
                fighter: fighter.to_owned(),
                group: status.group.to_owned(),
            });
        }
        self.queue_gimmick_operation(GimmickOperationRequest {
            version: 1,
            fighter,
            fighter_boid,
            operation: GimmickOperationKind::RequestWeaponStatus {
                weapon_boid,
                symbol,
            },
        })
    }

    fn queue_gimmick_operation(
        &self,
        request: GimmickOperationRequest<'_>,
    ) -> Result<(), GimmickOperationError> {
        let payload = serde_json::to_string(&serde_json::json!({
            "gimmick_operation": request
        }))
        .expect("gimmick operation request contains only serializable fields");
        let frame = format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>");
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| GimmickOperationError::QueueUnavailable)?;
        let queued_operations = shared
            .outbox
            .iter()
            .filter(|message| is_gimmick_operation(message))
            .count()
            .saturating_add(shared.gimmick_operations_in_flight);
        if queued_operations >= OUTBOX_CAP {
            return Err(GimmickOperationError::QueueFull);
        }
        shared.outbox.push(frame);
        trim_outbox(&mut shared.outbox);
        shared.edits_tx += 1;
        Ok(())
    }

    /// Temporarily hold live rule replacements while a project is materialized. The editor
    /// builds each move through the ordinary pushers, but the plugin must receive only the
    /// final flattened replacement for each rule family.
    pub fn begin_rule_batch(&self) {
        self.rule_send_suppressed.store(true, Ordering::SeqCst);
    }

    /// Resume live rule sends after a project batch has staged its complete stores.
    pub fn end_rule_batch(&self) {
        self.rule_send_suppressed.store(false, Ordering::SeqCst);
    }

    /// Hold every outbound live-edit message while a project is being replaced and rebuilt.
    /// Unlike [`Self::begin_rule_batch`], this also covers pins, aliases, donor payloads,
    /// effect reloads, and kind modifiers: clearing the old project must not briefly publish
    /// an empty or partially restored state before the new project's stores are complete.
    pub fn begin_project_batch(&self) {
        self.project_send_suppressed.store(true, Ordering::SeqCst);
    }

    /// Allow the caller to publish the final, fully materialized project state.
    pub fn end_project_batch(&self) {
        self.project_send_suppressed.store(false, Ordering::SeqCst);
    }

    fn sends_suppressed(&self) -> bool {
        self.project_send_suppressed.load(Ordering::SeqCst)
            || self.rule_send_suppressed.load(Ordering::SeqCst)
    }

    /// Spawn the connection thread (idempotent). Called lazily when the eff editor opens.
    pub fn ensure_started(&self) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = Arc::clone(&self.shared);
        std::thread::Builder::new()
            .name("game-link".into())
            .spawn(move || link_thread(shared))
            .expect("spawn game-link thread");
    }

    pub fn status(&self) -> LinkStatus {
        self.shared
            .lock()
            .map(|s| s.status)
            .unwrap_or(LinkStatus::Disconnected)
    }

    pub fn last_error(&self) -> Option<String> {
        self.shared.lock().ok().and_then(|s| s.last_error.clone())
    }

    pub fn stats(&self) -> (u64, u64) {
        self.shared
            .lock()
            .map(|s| (s.frames_rx, s.edits_tx))
            .unwrap_or((0, 0))
    }

    /// Current connection's complete live fighter/weapon snapshot.
    pub fn gimmick_agents(&self) -> Vec<LiveGimmickAgent> {
        self.shared
            .lock()
            .map(|shared| {
                if current_gimmick_agents_snapshot(&shared) {
                    shared.gimmick_agents.values().cloned().collect()
                } else {
                    Vec::new()
                }
            })
            .unwrap_or_default()
    }

    /// Whether this connection has received a complete live-object snapshot,
    /// including a valid empty snapshot while the game is outside a match.
    pub fn has_gimmick_agents_snapshot(&self) -> bool {
        self.shared
            .lock()
            .is_ok_and(|shared| current_gimmick_agents_snapshot(&shared))
    }

    /// Why the newest live-object roster was rejected, if it was malformed.
    pub fn gimmick_agents_error(&self) -> Option<String> {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.gimmick_agents_error.clone())
    }

    /// Current-session live values for one generated symbolic meter source.
    pub fn gimmick_meter_values(&self, fighter: &str, symbol: &str) -> Vec<LiveGimmickMeter> {
        self.shared
            .lock()
            .map(|shared| {
                shared
                    .gimmick_meters
                    .iter()
                    .filter(|meter| meter.fighter == fighter && meter.symbol == symbol)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Snapshot of all live kinds (id = hash40 of effect name).
    pub fn kinds(&self) -> Vec<(u64, LiveKind)> {
        self.shared
            .lock()
            .map(|s| s.kinds.iter().map(|(k, v)| (*k, v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn kind(&self, id: u64) -> Option<LiveKind> {
        self.shared
            .lock()
            .ok()
            .and_then(|s| s.kinds.get(&id).cloned())
    }

    pub fn is_live(&self, id: u64) -> bool {
        self.shared
            .lock()
            .map(|s| s.kinds.contains_key(&id))
            .unwrap_or(false)
    }

    /// Replace the plugin's live spawn-rule list (suppress/retime ACMD effect spawns).
    /// Send the FULL current rule set every time — an empty slice clears all rules.
    pub fn send_spawn_rules(&self, rules: &[SpawnRuleWire]) {
        if self.sends_suppressed() {
            return;
        }
        let Ok(payload) = serde_json::to_string(&serde_json::json!({ "spawn_rules": rules }))
        else {
            return;
        };
        let frame = format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>");
        if let Ok(mut s) = self.shared.lock() {
            queue_latest_full_replace(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }

    /// Replace the live effect point-control rules (detach and area toggles).
    pub fn send_effect_control_rules(&self, rules: &[EffectControlRuleWire]) {
        if self.sends_suppressed() {
            return;
        }
        let Ok(payload) = serde_json::to_string(&serde_json::json!({
            "effect_control_rules": rules
        })) else {
            return;
        };
        let frame = format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>");
        if let Ok(mut s) = self.shared.lock() {
            queue_latest_full_replace(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }

    /// Replace the plugin's live transplant alias list (copy/replaced kind → donor kind,
    /// optionally costume-gated). Full-list replace; empty clears all aliases.
    pub fn send_effect_aliases(&self, aliases: &[EffectAliasWire]) {
        if self.sends_suppressed() {
            return;
        }
        let Ok(payload) = serde_json::to_string(&serde_json::json!({ "effect_aliases": aliases }))
        else {
            return;
        };
        let frame = format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>");
        if let Ok(mut s) = self.shared.lock() {
            queue_latest_full_replace(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }

    /// Cross-fighter donor eff files the plugin co-loads with each target fighter's
    /// effects (smashline-transplant mechanism), so donor content is spawnable live.
    pub fn send_donor_effs(&self, specs: &[DonorEffWire]) {
        if self.sends_suppressed() {
            return;
        }
        let Ok(payload) = serde_json::to_string(&serde_json::json!({ "donor_effs": specs })) else {
            return;
        };
        let frame = format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>");
        if let Ok(mut s) = self.shared.lock() {
            queue_latest_full_replace(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }

    /// Stripped donor eff bytes for the plugin to inject as resident data (live
    /// cross-character transplant). Sent whenever the referenced donor set changes.
    pub fn send_donor_bytes(&self, donors: &[DonorBytesWire]) {
        if self.sends_suppressed() {
            return;
        }
        let Ok(payload) = serde_json::to_string(&serde_json::json!({ "donor_bytes": donors }))
        else {
            return;
        };
        let frame = format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>");
        if let Ok(mut s) = self.shared.lock() {
            queue_latest_full_replace(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }

    /// Queue a complete SD-backed asset snapshot for the plugin's carrier reload lifecycle.
    /// Activation is acknowledged separately after the owner is recreated and files are loaded.
    pub fn send_asset_bundle(&self, bundle: &AssetBundleWire) {
        if self.sends_suppressed() {
            return;
        }
        let mut asset_bundle = serde_json::json!(bundle);
        asset_bundle["fighter_binding_version"] = serde_json::json!(1);
        let Ok(payload) = serde_json::to_string(&serde_json::json!({
            "asset_bundle": asset_bundle
        })) else {
            return;
        };
        let frame = format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>");
        if let Ok(mut shared) = self.shared.lock() {
            queue_latest_full_replace(&mut shared.outbox, frame);
            shared.edits_tx += 1;
        }
    }

    /// Custom names (transplant copies) so the plugin resolves their hashes for display
    /// instead of falling back to hex.
    pub fn send_effect_names(&self, names: &[String]) {
        if names.is_empty() || self.sends_suppressed() {
            return;
        }
        let Ok(payload) = serde_json::to_string(&serde_json::json!({ "effect_names": names }))
        else {
            return;
        };
        let frame = format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>");
        if let Ok(mut s) = self.shared.lock() {
            queue_latest_full_replace(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }

    /// Desktop-local session identity. Unlike the plugin's client id, this stays unique when a
    /// restarted plugin resets its counter and assigns the same numeric id again.
    pub fn connection_epoch(&self) -> Option<u64> {
        self.shared
            .lock()
            .ok()
            .and_then(|s| s.client_id.is_some().then_some(s.connection_epoch))
    }

    /// Kinds the plugin reports active user pins for (fresh-session desync detection).
    pub fn pinned_kinds(&self) -> Vec<(u64, LiveKind)> {
        self.shared
            .lock()
            .map(|s| {
                s.kinds
                    .iter()
                    .filter(|(_, v)| v.pins.as_ref().map(|p| p.any()).unwrap_or(false))
                    .map(|(k, v)| (*k, v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Ask the plugin to clear ALL its pinned edits (incl. the SD save) and re-notify
    /// pristine values.
    pub fn send_reset_pins(&self) {
        if self.sends_suppressed() {
            return;
        }
        let frame = "<TCP_MESSAGE>{\"command\":\"reset_pins\"}</TCP_MESSAGE>".to_string();
        if let Ok(mut s) = self.shared.lock() {
            push_outbox_capped(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }

    /// Tell the plugin the live-eff manifest / merged files on the Eden SD changed —
    /// it refreshes its Arcropolis file-provider registrations.
    pub fn send_live_eff_reload(&self) {
        if self.sends_suppressed() {
            return;
        }
        let frame = "<TCP_MESSAGE>{\"command\":\"live_eff_reload\"}</TCP_MESSAGE>".to_string();
        if let Ok(mut s) = self.shared.lock() {
            push_outbox_capped(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }

    /// Ask the plugin to write `sd:/effect_viewer_probe.txt` (serving-chain diagnosis).
    pub fn send_live_eff_probe(&self) {
        if self.sends_suppressed() {
            return;
        }
        let frame = "<TCP_MESSAGE>{\"command\":\"live_eff_probe\"}</TCP_MESSAGE>".to_string();
        if let Ok(mut s) = self.shared.lock() {
            push_outbox_capped(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }

    /// Lines from the MOST RECENT playback of `motion` by `kind` — one performance, never a
    /// merge of several.
    ///
    /// The store keeps more than one run so that a performance still streaming in cannot
    /// replace the last complete one mid-arrival; this is what picks between them. Sorted into
    /// script time, which within a single run is also the order the game executed.
    pub fn latest_run_for(&self, motion: u64, kind: Option<i32>) -> Vec<CaptureLine> {
        let Ok(s) = self.shared.lock() else {
            return Vec::new();
        };
        let Some(lines) = s.captures.get(&motion) else {
            return Vec::new();
        };
        let matching = |l: &&CaptureLine| kind.map(|k| l.kind == k).unwrap_or(true);
        let Some(latest) = lines.iter().filter(matching).map(|l| l.run).max() else {
            return Vec::new();
        };
        let mut v: Vec<CaptureLine> = lines
            .iter()
            .filter(|l| matching(l) && l.run == latest)
            .cloned()
            .collect();
        v.sort_by(|a, b| {
            a.frame
                .partial_cmp(&b.frame)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    /// Forget every captured line locally and ask the plugin to release its per-move ownership
    /// claims. A game-thread acknowledgement clears any old line already in flight.
    pub fn clear_captures(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.captures.clear();
            s.capture_claimed_runs.clear();
            s.capture_debug.clear();
            s.capture_ends.clear();
            s.capture_completed_runs.clear();
            s.captures_seq += 1;
            push_outbox_capped(
                &mut s.outbox,
                "<TCP_MESSAGE>{\"command\":\"clear_acmd_captures\"}</TCP_MESSAGE>".into(),
            );
            s.edits_tx += 1;
        }
    }

    /// Forget local capture lines and completion state for one fighter kind without clearing
    /// another fighter's captures. The plugin's capture stream is global, so this intentionally
    /// does not send the global `clear_acmd_captures` command.
    pub fn forget_captures_for_kind(&self, kind: i32) {
        if let Ok(mut s) = self.shared.lock() {
            s.ignored_capture_kinds.insert(kind);
            for lines in s.captures.values_mut() {
                lines.retain(|line| line.kind != kind);
            }
            s.captures.retain(|_, lines| !lines.is_empty());
            s.capture_claimed_runs
                .retain(|(capture_kind, _), _| *capture_kind != kind);
            s.capture_debug
                .retain(|(capture_kind, _), _| *capture_kind != kind);
            s.capture_ends
                .retain(|(capture_kind, _), _| *capture_kind != kind);
            s.capture_completed_runs
                .retain(|(capture_kind, _, _)| *capture_kind != kind);
            s.captures_seq += 1;
        }
    }

    /// Allow capture collection for a fighter restored to the desktop roster.
    pub fn restore_captures_for_kind(&self, kind: i32) {
        if let Ok(mut s) = self.shared.lock() {
            s.ignored_capture_kinds.remove(&kind);
        }
    }

    /// Every captured line across ALL motions, tagged with its motion hash. Used to discover
    /// every place an effect is used (each move performed live contributes its motion's lines).
    pub fn all_captures(&self) -> Vec<(u64, CaptureLine)> {
        self.shared
            .lock()
            .ok()
            .map(|s| {
                s.captures
                    .iter()
                    .flat_map(|(m, lines)| lines.iter().map(move |l| (*m, l.clone())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Current per-character/per-motion capture outcome cache for the diagnostics window.
    pub fn capture_debug(&self) -> Vec<CaptureDebugSnapshot> {
        self.shared
            .lock()
            .map(|s| s.capture_debug.values().copied().collect())
            .unwrap_or_default()
    }

    /// Monotonic counter of received capture lines (cheap "anything new?" check).
    /// Live-carrier readiness: `(state, kinds, reports_seen)`.
    ///
    /// `state` is 0 none / 1 staged / 2 live. `reports_seen` is 0 with plugin builds that
    /// predate the `CarrierStatus` notify, so callers can degrade gracefully rather than
    /// waiting forever for a signal that will never arrive.
    pub fn carrier_status(&self) -> (u8, usize, u64, bool) {
        self.shared
            .lock()
            .map(|s| {
                (
                    s.carrier_state,
                    s.carrier_kinds,
                    s.carrier_seq,
                    s.carrier_spawned,
                )
            })
            .unwrap_or((0, 0, 0, false))
    }

    /// Donor-bytes generation of the carrier currently live in game.
    pub fn carrier_gen(&self) -> u64 {
        self.shared.lock().map(|s| s.carrier_gen).unwrap_or(0)
    }

    /// Take the reason the plugin rejected the last carrier push, clearing it.
    pub fn take_carrier_error(&self) -> Option<String> {
        self.shared
            .lock()
            .ok()
            .and_then(|mut s| s.carrier_error.take())
    }

    /// Latest hidden asset-carrier progress reported by the plugin.
    pub fn asset_bundle_status(&self) -> AssetBundleStatus {
        self.shared
            .lock()
            .map(|shared| AssetBundleStatus {
                target: shared.asset_bundle_target.clone(),
                generation: shared.asset_bundle_generation,
                staged: shared.asset_bundle_staged,
                served: shared.asset_bundle_served,
                serving_generation: shared.asset_bundle_serving_generation,
                phase: shared.asset_bundle_phase.clone(),
                ready: shared.asset_bundle_ready,
                reports: shared.asset_bundle_seq,
            })
            .unwrap_or_default()
    }

    /// Take the reason the plugin rejected the last asset snapshot, preserving prior staging.
    pub fn take_asset_bundle_error(&self) -> Option<String> {
        self.shared
            .lock()
            .ok()
            .and_then(|mut shared| shared.asset_bundle_error.take())
    }

    pub fn captures_seq(&self) -> u64 {
        self.shared.lock().map(|s| s.captures_seq).unwrap_or(0)
    }

    /// How many lines the CURRENT playback of one motion has produced so far. Cheap "did more
    /// arrive?" check that avoids cloning the bucket.
    ///
    /// Counts the claimed run only. The count rises while that one move plays and then remains
    /// frozen; unrelated or later runs are rejected before they reach this store.
    pub fn captures_count(&self, motion: u64, kind: Option<i32>) -> usize {
        self.shared
            .lock()
            .ok()
            .and_then(|s| {
                let lines = s.captures.get(&motion)?;
                let matching = |l: &&CaptureLine| kind.map(|k| l.kind == k).unwrap_or(true);
                let latest = lines.iter().filter(matching).map(|l| l.run).max()?;
                Some(
                    lines
                        .iter()
                        .filter(|l| matching(l) && l.run == latest)
                        .count(),
                )
            })
            .unwrap_or(0)
    }

    pub fn latest_run_id(&self, motion: u64, kind: Option<i32>) -> Option<u32> {
        self.shared.lock().ok().and_then(|s| {
            s.captures
                .get(&motion)?
                .iter()
                .filter(|line| kind.map(|value| line.kind == value).unwrap_or(true))
                .map(|line| line.run)
                .max()
        })
    }

    pub fn capture_run_complete(&self, kind: Option<i32>, motion: u64, run: u32) -> bool {
        self.shared.lock().is_ok_and(|s| match kind {
            Some(kind) => s.capture_completed_runs.contains(&(kind, motion, run)),
            None => s
                .capture_completed_runs
                .iter()
                .any(|(_, captured_motion, captured_run)| {
                    *captured_motion == motion && *captured_run == run
                }),
        })
    }

    /// How many times the plugin has reported this motion finishing (i.e. "its script has
    /// been streamed in full"). `kind = None` sums every fighter that played the motion.
    /// Stays 0 with plugin builds predating the `AcmdCaptureEnd` notify.
    #[cfg(test)]
    pub fn capture_end_count(&self, motion: u64, kind: Option<i32>) -> u64 {
        self.shared
            .lock()
            .map(|s| match kind {
                Some(k) => s.capture_ends.get(&(k, motion)).copied().unwrap_or(0),
                None => s
                    .capture_ends
                    .iter()
                    .filter(|((_, m), _)| *m == motion)
                    .map(|(_, n)| *n)
                    .sum(),
            })
            .unwrap_or(0)
    }

    /// Replace the plugin's live hitbox-rule list (modify/suppress/inject ATTACKs).
    /// Always the FULL set — an empty slice clears all rules.
    pub fn send_hitbox_rules(&self, rules: &[HitboxRuleWire]) {
        if self.sends_suppressed() {
            return;
        }
        let Ok(payload) = serde_json::to_string(&serde_json::json!({ "hitbox_rules": rules }))
        else {
            return;
        };
        let frame = format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>");
        if let Ok(mut s) = self.shared.lock() {
            queue_latest_full_replace(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }

    /// Queue a sparse kind-modifier edit. Color/speed controls must not upload the live
    /// form's position/rotation: those values are ACMD script offsets, while a carrier-owned
    /// transplant is physically world-space, so accidentally pinning them moves it to the
    /// stage origin. Authored EFF modifiers may additionally include kind-global scale.
    pub fn send_modifier_edit(&self, id: u64, data: &RpmEffectData, include_scale: bool) {
        if self.sends_suppressed() {
            return;
        }
        let mut value = serde_json::json!({
            "speed": data.speed,
            "rainbow": {
                "color": data.rainbow.color,
            },
        });
        if include_scale {
            value["scale"] = serde_json::json!(data.scale);
        }
        let payload = serde_json::json!({ "id": id, "newValue": value.to_string() });
        let frame = format!("<TCP_MESSAGE>{payload}</TCP_MESSAGE>");
        if let Ok(mut s) = self.shared.lock() {
            push_outbox_capped(&mut s.outbox, frame);
            s.edits_tx += 1;
        }
    }
}

// ── Connection thread ─────────────────────────────────────────────────────────

fn connect_hint(error: &std::io::Error) -> &'static str {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionRefused => {
            " — is the game running with the Visionary plugin? In Eden set \
             Configure → System → Network → Network Interface to your active card, \
             apply, then restart the game"
        }
        ErrorKind::TimedOut => {
            " — timed out; the emulator's virtual network may be stalled or the \
             Network Interface may be unset (Eden Configure → System → Network)"
        }
        _ => "",
    }
}

fn current_gimmick_agents_snapshot(shared: &Shared) -> bool {
    // The plugin replaces the complete list and the socket/session boundary
    // clears it. Do not expire a valid snapshot on wall-clock time: pausing the
    // game also pauses game-thread publication, and that used to turn working
    // selectors back into raw ID fields after three seconds. Every operation is
    // revalidated in-game, so a paused snapshot is safe selection metadata.
    shared.status == LinkStatus::Connected
        && shared.client_id.is_some()
        && shared.gimmick_agents_epoch == shared.connection_epoch
        && shared.gimmick_agents_received.is_some()
}

fn link_thread(shared: Arc<Mutex<Shared>>) {
    loop {
        {
            let mut s = shared.lock().unwrap();
            s.status = LinkStatus::Connecting;
            s.client_id = None;
            s.gimmick_agents.clear();
            s.gimmick_agents_wire_seq = None;
            s.gimmick_agents_received = None;
            s.gimmick_agents_error = None;
            s.gimmick_meters.clear();
            s.gimmick_meters_wire_seq = None;
            s.gimmick_live_acks.clear();
            s.gimmick_live_snapshot = None;
            s.gimmick_controller_capabilities = None;
            s.gimmick_controller_capabilities_seq = None;
            s.gimmick_feature_capabilities = None;
            s.gimmick_feature_capabilities_seq = None;
            discard_session_bound_messages(&mut s.outbox);
            s.last_rx = None;
            s.gimmick_operations_in_flight = 0;
        }
        let addr = plugin_addr();
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => {
                {
                    let mut s = shared.lock().unwrap();
                    s.status = LinkStatus::Connected;
                    s.last_error = None;
                    // Fresh socket, fresh liveness baseline: the stale timer must not
                    // inherit silence from the previous (dead) connection.
                    s.last_rx = Some(Instant::now());
                }
                let reason = serve_connection(&shared, stream);
                let mut s = shared.lock().unwrap();
                s.status = LinkStatus::Disconnected;
                s.last_error = reason;
                s.client_id = None;
                s.gimmick_agents.clear();
                s.gimmick_agents_wire_seq = None;
                s.gimmick_agents_received = None;
                s.gimmick_agents_error = None;
                s.gimmick_meters.clear();
                s.gimmick_meters_wire_seq = None;
                s.last_rx = None;
                s.gimmick_live_acks.clear();
                s.gimmick_live_snapshot = None;
                s.gimmick_controller_capabilities = None;
                s.gimmick_controller_capabilities_seq = None;
                s.gimmick_feature_capabilities = None;
                s.gimmick_feature_capabilities_seq = None;
                s.gimmick_operations_in_flight = 0;
            }
            Err(e) => {
                let hint = connect_hint(&e);
                let mut s = shared.lock().unwrap();
                s.status = LinkStatus::Disconnected;
                s.last_error = Some(format!("connect to {addr}: {e}{hint}"));
                s.client_id = None;
                s.gimmick_agents.clear();
                s.gimmick_agents_wire_seq = None;
                s.gimmick_agents_received = None;
                s.gimmick_agents_error = None;
                s.gimmick_meters.clear();
                s.gimmick_meters_wire_seq = None;
                s.last_rx = None;
                s.gimmick_live_acks.clear();
                s.gimmick_live_snapshot = None;
                s.gimmick_controller_capabilities = None;
                s.gimmick_controller_capabilities_seq = None;
                s.gimmick_feature_capabilities = None;
                s.gimmick_feature_capabilities_seq = None;
            }
        }
        std::thread::sleep(RECONNECT_DELAY);
    }
}

fn serve_connection(shared: &Arc<Mutex<Shared>>, mut stream: TcpStream) -> Option<String> {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let mut buf = String::new();
    let mut chunk = [0u8; 8192];
    let mut last_ping = Instant::now();
    let mut ping_seq: u64 = 0;

    // Requeue unsent frames at the FRONT, preserving order, so an interrupted flush
    // loses nothing: edits made while offline are delivered on the next connection.
    let requeue_front = |shared: &Arc<Mutex<Shared>>, unsent: Vec<String>| {
        if let Ok(mut s) = shared.lock() {
            // The current socket no longer owns any dequeued messages. Once its
            // unsent suffix is restored below, those operations are counted from
            // `outbox` again; if every write succeeded, there is nothing to restore.
            s.gimmick_operations_in_flight = 0;
            if unsent.is_empty() {
                return;
            }
            let mut combined = unsent;
            combined.extend(std::mem::take(&mut s.outbox));
            discard_session_bound_messages(&mut combined);
            retain_latest_full_replace(&mut combined);
            retain_latest_idempotent_commands(&mut combined);
            trim_outbox(&mut combined);
            s.outbox = combined;
        }
    };

    loop {
        // Outbound edits first — they're latency-sensitive.
        let pending: Vec<String> = {
            let mut s = shared.lock().unwrap();
            let pending = std::mem::take(&mut s.outbox);
            s.gimmick_operations_in_flight = pending
                .iter()
                .filter(|message| is_gimmick_operation(message))
                .count();
            pending
        };
        let mut pending = pending;
        retain_latest_full_replace(&mut pending);
        let mut failed_at: Option<(usize, String)> = None;
        for (index, msg) in pending.iter().enumerate() {
            if let Err(e) = stream.write_all(msg.as_bytes()) {
                failed_at = Some((index, format!("send: {e}")));
                break;
            }
        }
        if let Some((index, reason)) = failed_at {
            // `index` failed; it and everything after it never hit the wire.
            let unsent: Vec<String> = pending.into_iter().skip(index).collect();
            requeue_front(shared, unsent);
            return Some(reason);
        }

        // `write_all` accepted every pending frame. There is intentionally no
        // completion acknowledgement in this legacy TCP protocol, so operations
        // are now only observable through the plugin diagnostic log; they must
        // not keep consuming the local queue reservation after this point.
        if let Ok(mut s) = shared.lock() {
            s.gimmick_operations_in_flight = 0;
        }

        // Idle probe: force traffic so a half-open socket surfaces as an error even
        // when the user makes no edits. The plugin answers with `Pong` (older builds
        // log `unknown command: ping` and stay connected — harmless).
        if last_ping.elapsed() >= PING_INTERVAL {
            ping_seq = ping_seq.wrapping_add(1);
            let ping =
                format!("<TCP_MESSAGE>{{\"command\":\"ping\",\"echo\":{ping_seq}}}</TCP_MESSAGE>");
            if let Err(e) = stream.write_all(ping.as_bytes()) {
                return Some(format!("send ping: {e}"));
            }
            last_ping = Instant::now();
        }

        match stream.read(&mut chunk) {
            Ok(0) => return Some("closed by plugin".into()),
            Ok(n) => {
                buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                for payload in extract_frames(&mut buf) {
                    handle_frame(shared, &payload);
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Some(format!("recv: {e}")),
        }

        // Half-open detector: pings guarantee regular traffic, and the plugin emits a
        // CarrierStatus heartbeat (~2s) while in a match, so a long silence means the
        // peer is gone even though the socket never closed.
        let stale = shared
            .lock()
            .ok()
            .and_then(|s| s.last_rx)
            .is_some_and(|t| t.elapsed() > STALE_TIMEOUT);
        if stale {
            return Some(
                "stale: no frames from the game for 20s (emulator stalled or connection \
                 half-open; reconnecting)"
                    .into(),
            );
        }
    }
}

fn extract_frames(buf: &mut String) -> Vec<String> {
    const OPEN: &str = "<TCP_MESSAGE>";
    const CLOSE: &str = "</TCP_MESSAGE>";
    let mut out = Vec::new();
    while let (Some(s), Some(e)) = (buf.find(OPEN), buf.find(CLOSE)) {
        if e < s {
            // Torn close tag before an open — drop the garbage prefix.
            *buf = buf[e + CLOSE.len()..].to_string();
            continue;
        }
        let payload = buf[s + OPEN.len()..e].trim().to_string();
        *buf = buf[e + CLOSE.len()..].to_string();
        if !payload.is_empty() {
            out.push(payload);
        }
    }
    // Runaway guard that never drops an in-flight message. The old code cleared the
    // whole buffer past 1 MiB, which discarded a fragmented frame mid-arrival. Only
    // genuine garbage (no opening tag) is dropped. A prefix before a recent opening tag is
    // trimmed, while one unterminated frame beyond the hard cap is rejected.
    const GARBAGE_CAP: usize = 1 << 20;
    const FRAME_CAP: usize = 16 << 20;
    if buf.len() > GARBAGE_CAP && !buf.contains(OPEN) {
        buf.clear();
    } else if buf.len() > FRAME_CAP {
        if let Some(last) = buf.rfind(OPEN) {
            if last > 0 && buf.len() - last <= FRAME_CAP {
                *buf = buf[last..].to_string();
            } else {
                buf.clear();
            }
        } else {
            buf.clear();
        }
    }
    out
}

fn handle_frame(shared: &Arc<Mutex<Shared>>, payload: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) else {
        return;
    };
    let Some(header) = v.get("header").and_then(|h| h.as_str()) else {
        return;
    };
    // The plugin serializes `body` as a JSON *string*; tolerate an object too.
    let body: serde_json::Value = match v.get("body") {
        Some(serde_json::Value::String(s)) => serde_json::from_str(s).unwrap_or_default(),
        Some(other) => other.clone(),
        None => serde_json::Value::Null,
    };

    let mut s = shared.lock().unwrap();
    s.frames_rx += 1;
    s.last_rx = Some(Instant::now());
    match header {
        // Heartbeat answer from the plugin. Any frame proves liveness; `Pong` carries
        // no state and needs no further handling.
        "Pong" => {}
        "Notify" => {
            let Some(n) = body.get("Notify") else { return };
            let Some(id) = n.get("id").and_then(|i| i.as_u64()) else {
                return;
            };
            let name = n
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
                .to_string();
            let data: RpmEffectData = match n.get("value_in_json") {
                Some(serde_json::Value::String(raw)) => match serde_json::from_str(raw) {
                    Ok(d) => d,
                    Err(_) => return,
                },
                Some(obj) => match serde_json::from_value(obj.clone()) {
                    Ok(d) => d,
                    Err(_) => return,
                },
                None => return,
            };
            // Sparse pins-only form (newer plugins) — user overrides already active in-game.
            let pins: Option<PinsWire> = match n.get("pinned_in_json") {
                Some(serde_json::Value::String(raw)) => serde_json::from_str(raw).ok(),
                Some(serde_json::Value::Null) | None => None,
                Some(obj) => serde_json::from_value(obj.clone()).ok(),
            };
            match s.kinds.get_mut(&id) {
                Some(k) => {
                    k.name = name;
                    k.data = data;
                    k.pins = pins;
                    k.updates += 1;
                    k.last_update = Instant::now();
                }
                None => {
                    s.kinds.insert(
                        id,
                        LiveKind {
                            name,
                            first: data.clone(),
                            data,
                            pins,
                            updates: 1,
                            last_update: Instant::now(),
                        },
                    );
                }
            }
        }
        "AcmdCapture" => {
            let Some(c) = body.get("AcmdCapture") else {
                return;
            };
            let Ok(line) = serde_json::from_value::<CaptureLine>(c.clone()) else {
                return;
            };
            if s.ignored_capture_kinds.contains(&line.kind) {
                return;
            }
            let claim = s
                .capture_claimed_runs
                .entry((line.kind, line.motion))
                .or_insert(line.run);
            if *claim != line.run {
                return;
            }
            let bucket = s.captures.entry(line.motion).or_default();
            // The plugin dedupes per session and a reconnect re-sends the whole log, so exact
            // duplicates have to be dropped here. Exact equality is not enough on its own:
            // `frame` is the raw `MotionModule::frame` float, and two performances of the same
            // move rarely produce bit-identical floats, so the SAME spawn arrived twice at
            // 5.0 and 5.0000019 and the editor showed the effect twice. Dedupe on what the
            // editor can actually distinguish — the whole line at frame resolution.
            if !bucket.iter().any(|existing| same_capture(existing, &line)) {
                bucket.push(line);
                s.captures_seq += 1;
            }
        }
        "AcmdCaptureEnd" => {
            // "That motion just finished playing" — every line it produces has already been
            // delivered (the plugin holds these back until the line backlog drains).
            let Some(e) = body.get("AcmdCaptureEnd") else {
                return;
            };
            let (Some(kind), Some(motion)) = (
                e.get("kind").and_then(|k| k.as_i64()),
                e.get("motion").and_then(|m| m.as_u64()),
            ) else {
                return;
            };
            if s.ignored_capture_kinds.contains(&(kind as i32)) {
                return;
            }
            let run = e.get("run").and_then(|value| value.as_u64()).unwrap_or(0) as u32;
            if s.capture_claimed_runs.get(&(kind as i32, motion)).copied() != Some(run) {
                return;
            }
            if s.capture_completed_runs.insert((kind as i32, motion, run)) {
                *s.capture_ends.entry((kind as i32, motion)).or_insert(0) += 1;
            }
        }
        "AcmdCaptureDebug" => {
            let Some(row) = body.get("AcmdCaptureDebug") else {
                return;
            };
            let Ok(row) = serde_json::from_value::<CaptureDebugSnapshot>(row.clone()) else {
                return;
            };
            if s.ignored_capture_kinds.contains(&row.kind) {
                return;
            }
            s.capture_debug.insert((row.kind, row.motion), row);
        }
        "AcmdCaptureCleared" => {
            // The acknowledgement is emitted only after the game thread has discarded its
            // pending queue and ownership claims. Clear again here so no pre-command line that
            // was already in flight can repopulate the editor after the user requested reset.
            s.captures.clear();
            s.capture_claimed_runs.clear();
            s.capture_debug.clear();
            s.capture_ends.clear();
            s.capture_completed_runs.clear();
            s.captures_seq += 1;
        }
        "CarrierStatus" => {
            // How far along the game is in taking the carrier we pushed. 0 = none staged,
            // 1 = staged/building, 2 = live and serving. The editor uses this to keep its
            // "sending" state up until the game has ACTUALLY taken the edit, instead of
            // clearing the moment the bytes left the socket.
            let Some(c) = body.get("CarrierStatus") else {
                return;
            };
            s.carrier_state = c.get("state").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
            // The battle object actually existing is what `spawn_via_carrier` requires;
            // `state` alone can be 2 while nothing can spawn yet.
            s.carrier_spawned = c.get("spawned").and_then(|v| v.as_bool()).unwrap_or(false);
            s.carrier_kinds = c.get("kinds").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            s.carrier_gen = c.get("gen").and_then(|v| v.as_u64()).unwrap_or(0);
            s.carrier_seq += 1;
        }
        "CarrierError" => {
            s.carrier_error = body
                .get("CarrierError")
                .and_then(|c| c.get("reason"))
                .and_then(|r| r.as_str())
                .map(str::to_owned);
        }
        "AssetBundleStatus" => {
            let Some(status) = body.get("AssetBundleStatus") else {
                return;
            };
            // Only accept the carrier lifecycle produced by the current plugin. A stale plugin
            // that still reports the old next-owner-load contract must not turn file service into
            // a false hot-reload success.
            if status.get("activation").and_then(|value| value.as_str()) != Some("carrier_recreate")
            {
                return;
            }
            let Some(target) = status.get("target").and_then(|value| value.as_str()) else {
                return;
            };
            let Some(generation) = status.get("generation").and_then(|value| value.as_u64()) else {
                return;
            };
            let Some(staged_wire) = status.get("staged").and_then(|value| value.as_u64()) else {
                return;
            };
            let Some(served_wire) = status.get("served").and_then(|value| value.as_u64()) else {
                return;
            };
            let (Ok(staged), Ok(served)) =
                (usize::try_from(staged_wire), usize::try_from(served_wire))
            else {
                return;
            };
            let serving_generation = status
                .get("serving_generation")
                .and_then(|value| value.as_u64())
                .unwrap_or(generation);
            let phase = status
                .get("phase")
                .and_then(|value| value.as_str())
                .unwrap_or("staged");
            let ready = status
                .get("ready")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            // A callback can only serve an entry from the staged snapshot. Reject malformed or
            // stale acknowledgements so an old plugin/status packet cannot make the UI regress.
            if served > staged
                || generation < s.asset_bundle_generation
                || serving_generation > generation
                || !matches!(
                    phase,
                    "idle" | "staged" | "retiring" | "loading" | "ready" | "failed"
                )
                || ready
                    && (phase != "ready" || served != staged || serving_generation != generation)
            {
                return;
            }
            if target.len() > 64
                || !target
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
                || (generation == 0 && (!target.is_empty() || staged != 0 || served != 0))
                || (generation != 0 && target.is_empty() && (staged != 0 || served != 0))
            {
                return;
            }
            s.asset_bundle_target = target.to_owned();
            s.asset_bundle_generation = generation;
            s.asset_bundle_staged = staged;
            s.asset_bundle_served = served;
            s.asset_bundle_serving_generation = serving_generation;
            s.asset_bundle_phase = phase.to_owned();
            s.asset_bundle_ready = ready;
            s.asset_bundle_seq += 1;
        }
        "AssetBundleError" => {
            s.asset_bundle_error = body
                .get("AssetBundleError")
                .and_then(|value| value.get("reason"))
                .and_then(|value| value.as_str())
                .map(str::to_owned);
        }
        "GimmickAgents" => {
            let Some(snapshot) = body.get("GimmickAgents") else {
                s.gimmick_agents_error = Some("live roster payload is missing".to_string());
                return;
            };
            let snapshot = match serde_json::from_value::<GimmickAgentsWire>(snapshot.clone()) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    s.gimmick_agents_error =
                        Some(format!("live roster could not be decoded: {error}"));
                    return;
                }
            };
            if snapshot.version != 1 {
                s.gimmick_agents_error = Some(format!(
                    "unsupported live roster version {} (expected 1)",
                    snapshot.version
                ));
                return;
            }
            if s.gimmick_agents_epoch == s.connection_epoch
                && s.gimmick_agents_wire_seq
                    .is_some_and(|known| snapshot.seq <= known)
            {
                return;
            }
            let mut next = BTreeMap::new();
            let row_count = snapshot.agents.len();
            let mut row_warning = None;
            for (index, value) in snapshot.agents.into_iter().enumerate() {
                let agent = match serde_json::from_value::<LiveGimmickAgent>(value) {
                    Ok(agent) => agent,
                    Err(error) => {
                        row_warning.get_or_insert_with(|| {
                            format!(
                                "live roster row {} could not be decoded: {error}",
                                index + 1
                            )
                        });
                        continue;
                    }
                };
                let reason = if !matches!(agent.category, 0 | 1) {
                    Some("unknown object category")
                } else if agent.category == 0
                    && (agent.fighter.as_deref().is_none_or(str::is_empty)
                        || agent.owner_boid.is_some())
                {
                    Some("invalid fighter identity")
                } else if agent.category == 1
                    && (agent.fighter.is_some() || agent.owner_boid.is_none())
                {
                    Some("invalid weapon ownership")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    row_warning.get_or_insert_with(|| {
                        format!("live roster row {:#010x} has {reason}", agent.boid)
                    });
                    continue;
                }
                let boid = agent.boid;
                if next.insert(boid, agent).is_some() {
                    row_warning
                        .get_or_insert_with(|| format!("live roster repeats object {boid:#010x}"));
                }
            }
            // A nonempty publication with no usable rows is still a rejected
            // snapshot. Preserve the last good list. Partial publications are
            // safe selection metadata: execution re-proves object identity,
            // ownership, kind, and symbol on the game thread.
            if row_count != 0 && next.is_empty() && row_warning.is_some() {
                s.gimmick_agents_error = row_warning;
                return;
            }
            s.gimmick_agents = next;
            s.gimmick_agents_seq = s.gimmick_agents_seq.wrapping_add(1);
            s.gimmick_agents_wire_seq = Some(snapshot.seq);
            s.gimmick_agents_epoch = s.connection_epoch;
            s.gimmick_agents_received = Some(Instant::now());
            s.gimmick_agents_error = row_warning;
        }
        "GimmickMeters" => {
            let Some(snapshot) = body.get("GimmickMeters") else {
                return;
            };
            let Ok(snapshot) = serde_json::from_value::<GimmickMetersWire>(snapshot.clone()) else {
                return;
            };
            if snapshot.version != 1
                || s.gimmick_meters_wire_seq
                    .is_some_and(|known| snapshot.seq <= known)
            {
                return;
            }
            let mut keys = BTreeSet::new();
            if snapshot.values.iter().any(|meter| {
                let typed = match (meter.kind.as_str(), meter.value) {
                    ("work_float", LiveGimmickMeterValue::Float(value)) => value.is_finite(),
                    ("work_int", LiveGimmickMeterValue::Int(_))
                    | ("work_flag", LiveGimmickMeterValue::Flag(_)) => true,
                    _ => false,
                };
                meter.binding_id.is_empty()
                    || meter.fighter.is_empty()
                    || meter.symbol.is_empty()
                    || meter.entry_id < 0
                    || !typed
                    || !keys.insert((meter.entry_id, meter.binding_id.clone()))
            }) {
                return;
            }
            s.gimmick_meters = snapshot.values;
            s.gimmick_meters_wire_seq = Some(snapshot.seq);
        }
        "GimmickLiveAck" => {
            let Some(value) = body.get("GimmickLiveAck") else {
                return;
            };
            let Ok(ack) = serde_json::from_value::<GimmickLiveAck>(value.clone()) else {
                return;
            };
            if ack.version != 1
                || !matches!(ack.family.as_str(), "parameters" | "behavior")
                || ack.message.len() > 4096
                || s.gimmick_live_expected.get(&ack.family) != Some(&ack.revision)
                || s.client_id.is_none()
            {
                return;
            }
            s.gimmick_live_acks.insert(ack.family.clone(), ack);
        }
        "GimmickControllerCapabilities" => {
            let Some(value) = body.get("GimmickControllerCapabilities") else {
                s.gimmick_controller_capabilities = None;
                s.gimmick_live_snapshot = None;
                return;
            };
            let Some(frame_seq) = value.get("seq").and_then(serde_json::Value::as_u64) else {
                s.gimmick_controller_capabilities = None;
                s.gimmick_live_snapshot = None;
                return;
            };
            let newer = s
                .gimmick_controller_capabilities_seq
                .is_none_or(|known| frame_seq > known);
            if !newer {
                s.gimmick_controller_capabilities = None;
                s.gimmick_live_snapshot = None;
                return;
            }
            // Consume a parseable sequence even when another strict field fails,
            // so replaying that sequence with a repaired body cannot grant support.
            s.gimmick_controller_capabilities_seq = Some(frame_seq);
            let Ok(capabilities) =
                serde_json::from_value::<GimmickControllerCapabilities>(value.clone())
            else {
                s.gimmick_controller_capabilities = None;
                s.gimmick_live_snapshot = None;
                return;
            };
            if capabilities.is_supported_contract() && s.client_id.is_some() {
                s.gimmick_controller_capabilities = Some(capabilities);
            } else {
                s.gimmick_controller_capabilities = None;
                s.gimmick_live_snapshot = None;
            }
        }
        "GimmickFeatureCapabilities" => {
            let Some(value) = body.get("GimmickFeatureCapabilities") else {
                s.gimmick_feature_capabilities = None;
                return;
            };
            let Some(frame_seq) = value.get("seq").and_then(serde_json::Value::as_u64) else {
                s.gimmick_feature_capabilities = None;
                return;
            };
            let newer = s
                .gimmick_feature_capabilities_seq
                .is_none_or(|known| frame_seq > known);
            if !newer {
                s.gimmick_feature_capabilities = None;
                return;
            }
            // This sequence is independent of V5 controller telemetry. Consume
            // it before strict validation to prevent repaired same-seq replays.
            s.gimmick_feature_capabilities_seq = Some(frame_seq);
            let Ok(capabilities) =
                serde_json::from_value::<GimmickFeatureCapabilities>(value.clone())
            else {
                s.gimmick_feature_capabilities = None;
                return;
            };
            if capabilities.is_well_formed() && s.client_id.is_some() {
                s.gimmick_feature_capabilities = Some(capabilities);
            } else {
                s.gimmick_feature_capabilities = None;
            }
        }
        "GimmickLiveState" => {
            let Some(value) = body.get("GimmickLiveState") else {
                return;
            };
            let Ok(snapshot) = serde_json::from_value::<GimmickLiveSnapshot>(value.clone()) else {
                return;
            };
            if snapshot.version != 1
                || snapshot.parameters.len() > crate::gimmicks::live_params::MAX_EDITS
                || s.client_id.is_none()
                || value.to_string().len() > 512 * 1024
                || s.gimmick_live_snapshot
                    .as_ref()
                    .is_some_and(|old| old.seq >= snapshot.seq)
                || !valid_gimmick_live_rows_with_capabilities(
                    &snapshot,
                    s.gimmick_controller_capabilities.as_ref(),
                )
            {
                return;
            }
            s.gimmick_live_snapshot = Some(snapshot);
        }
        "Remove" => {
            if let Some(id) = body
                .get("Remove")
                .and_then(|r| r.get("id"))
                .and_then(|i| i.as_u64())
            {
                s.kinds.remove(&id);
            }
        }
        "RemoveAll" => s.kinds.clear(),
        "GiveClientId" => {
            let client_id = body
                .get("GiveClientId")
                .and_then(|g| g.get("client_id"))
                .and_then(|c| c.as_u64());
            if client_id.is_some() {
                s.connection_epoch = s.connection_epoch.wrapping_add(1).max(1);
                discard_session_bound_messages(&mut s.outbox);
                s.gimmick_agents.clear();
                s.gimmick_agents_wire_seq = None;
                s.gimmick_agents_received = None;
                s.gimmick_agents_error = None;
                s.gimmick_meters.clear();
                s.gimmick_meters_wire_seq = None;
                s.gimmick_live_acks.clear();
                s.gimmick_live_snapshot = None;
                s.gimmick_controller_capabilities = None;
                s.gimmick_controller_capabilities_seq = None;
                s.gimmick_feature_capabilities = None;
                s.gimmick_feature_capabilities_seq = None;
            }
            s.client_id = client_id;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_sync_preflight_failure_is_visible_and_recovers_when_saved_data_is_restored() {
        let link = GameLink::offline_for_test();
        {
            let mut shared = link.shared.lock().unwrap();
            shared.status = LinkStatus::Connected;
            shared.client_id = Some(1);
            shared.gimmick_live_snapshot = Some(GimmickLiveSnapshot {
                version: 1,
                seq: 1,
                behavior_revision: 0,
                parameter_revision: 0,
                behavior: vec![],
                parameters: vec![],
                parameter_health: None,
            });
        }
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let mut sync = crate::gimmicks::live_sync::LiveSync::default();
        let mut params = BTreeMap::new();
        let graphs = BTreeMap::new();
        sync.update(&params, &graphs, &link);
        let expected = link.shared.lock().unwrap().gimmick_live_expected.clone();
        params.insert(
            "cloud".into(),
            crate::mod_project::ParamMod {
                files: BTreeMap::from([(
                    "fighter/cloud/param/vl.prc".into(),
                    (0..=crate::gimmicks::live_params::MAX_EDITS)
                        .map(|index| {
                            (
                                format!("value_{index}"),
                                crate::mod_project::ParamValue::Float(1.0),
                            )
                        })
                        .collect(),
                )]),
            },
        );
        sync.update(&params, &graphs, &link);
        assert!(sync.error_for("parameters").is_some());
        assert_eq!(link.shared.lock().unwrap().gimmick_live_expected, expected);
        params.clear();
        sync.update(&params, &graphs, &link);
        assert!(sync.error_for("parameters").is_none());
        assert!(sync.error.is_empty());
    }

    #[test]
    fn live_gimmick_rows_require_exact_typed_bounded_snapshot_data() {
        let mut snapshot = GimmickLiveSnapshot {
            version: 1,
            seq: 1,
            behavior_revision: 4,
            parameter_revision: 5,
            behavior: vec![
                serde_json::json!({"fighter":"mario","boid":0,"entry_id":0,"revision":4,"state":"waiting",
                "vars":{"remaining":{"Int":60},"ready":{"Flag":false}}}),
            ],
            parameters: vec![
                serde_json::json!({"fighter":"cloud","path":"param_special_lw[0].limit_gauge_add","hits":1,
                "baseline":{"Float":0.667},"lastObserved":{"Float":2.0}}),
            ],
            parameter_health: None,
        };
        assert!(valid_gimmick_live_rows(&snapshot));
        snapshot.parameter_health = Some(crate::gimmicks::live_params::ParameterHealth {
            revision: 5,
            ..Default::default()
        });
        assert!(valid_gimmick_live_rows(&snapshot));
        snapshot.parameter_health.as_mut().unwrap().revision = 6;
        assert!(!valid_gimmick_live_rows(&snapshot));
        snapshot.parameter_health = None;
        snapshot.behavior[0]["revision"] = 3.into();
        assert!(!valid_gimmick_live_rows(&snapshot));
        snapshot.behavior[0]["revision"] = 4.into();
        snapshot.behavior.push(snapshot.behavior[0].clone());
        assert!(!valid_gimmick_live_rows(&snapshot));
        snapshot.behavior.pop();
        snapshot.parameters[0]["lastObserved"] = serde_json::json!({"Flag":true});
        assert!(!valid_gimmick_live_rows(&snapshot));
        snapshot.parameters[0]["lastObserved"] = serde_json::json!({"Float":2.0});
        snapshot.parameters[0]["fighter"] = "not_a_fighter".into();
        assert!(!valid_gimmick_live_rows(&snapshot));
    }

    #[test]
    fn live_gimmick_full_snapshots_coalesce_per_family_and_ack_only_current_revision() {
        let link = GameLink::offline_for_test();
        {
            let mut shared = link.shared.lock().unwrap();
            shared.status = LinkStatus::Connected;
            shared.client_id = Some(1);
        }
        for revision in 1..=3 {
            link.send_gimmick_live(
                "behavior",
                serde_json::json!({"version":4,"revision":revision,"programs":[]}),
            )
            .unwrap();
            link.send_gimmick_live("parameters", serde_json::json!({"revision":revision+10}))
                .unwrap();
        }
        assert_eq!(link.shared.lock().unwrap().outbox.len(), 2);
        let deliver = |revision, version| {
            let mut frame = plugin_frame(
                "GimmickLiveAck",
                &serde_json::json!({"GimmickLiveAck":{
                "version":version,"family":"behavior","revision":revision,"accepted":true,"message":"accepted"}}),
            );
            for payload in extract_frames(&mut frame) {
                handle_frame(&link.shared, &payload);
            }
        };
        deliver(2, 1);
        deliver(3, 2);
        assert!(link.gimmick_live_ack("behavior").is_none());
        deliver(3, 1);
        assert!(link.gimmick_live_ack("behavior").unwrap().accepted);
        assert!(link
            .send_gimmick_live("unknown", serde_json::json!({"revision":4}))
            .is_err());
        assert!(link
            .send_gimmick_live(
                "behavior",
                serde_json::json!({"revision":4,"large":"x".repeat(crate::gimmicks::live_protocol::MAX_REQUEST_BYTES)})
            )
            .is_err());
    }

    #[test]
    fn live_gimmick_telemetry_is_ordered_and_new_connection_clears_state_and_ack() {
        let link = GameLink::offline_for_test();
        {
            let mut shared = link.shared.lock().unwrap();
            shared.status = LinkStatus::Connected;
            shared.client_id = Some(1);
        }
        let deliver = |header: &str, body: serde_json::Value| {
            let mut frame = plugin_frame(header, &body);
            for payload in extract_frames(&mut frame) {
                handle_frame(&link.shared, &payload);
            }
        };
        let state = |seq, version| {
            serde_json::json!({"GimmickLiveState": {
            "version":version,"seq":seq,"behavior_revision":5,"parameter_revision":6,"behavior":[],"parameters":[]}})
        };
        deliver("GimmickLiveState", state(10, 1));
        deliver("GimmickLiveState", state(9, 1));
        deliver("GimmickLiveState", state(11, 2));
        assert_eq!(link.gimmick_live_snapshot().unwrap().seq, 10);
        assert!(link.restart_gimmick_controller("mario", 0, 5).is_err());
        link.send_gimmick_live(
            "behavior",
            serde_json::json!({"version":4,"revision":5,"programs":[]}),
        )
        .unwrap();
        deliver(
            "GimmickLiveAck",
            serde_json::json!({"GimmickLiveAck":{
            "version":1,"family":"behavior","revision":5,"accepted":true,"message":"accepted"}}),
        );
        link.restart_gimmick_controller("mario", 0, 5).unwrap();
        assert!(link
            .shared
            .lock()
            .unwrap()
            .outbox
            .iter()
            .any(|message| message.contains("gimmick_restart") && is_gimmick_operation(message)));
        deliver(
            "GiveClientId",
            serde_json::json!({"GiveClientId":{"client_id":2}}),
        );
        assert!(!link
            .shared
            .lock()
            .unwrap()
            .outbox
            .iter()
            .any(|message| message.contains("gimmick_restart")));
        assert!(link.gimmick_live_snapshot().is_none());
        assert!(link.gimmick_live_ack("behavior").is_none());
        assert!(link.restart_gimmick_controller("mario", 0, 5).is_err());
    }

    #[test]
    fn targeted_chart_restart_requires_current_capability_and_snapshot_revision() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickLiveState",
            serde_json::json!({"GimmickLiveState": {
                "version": 1, "seq": 1, "behavior_revision": 4,
                "parameter_revision": 5,
                "behavior": [{"fighter":"mario","boid":10,"entry_id":0,
                    "revision":4,"state":"waiting","vars":{}}],
                "parameters": []
            }}),
        );
        accept_behavior_revision(&link, 4);
        assert!(link
            .restart_gimmick_chart("mario", 10, "secondary", 4)
            .is_err());

        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let row = controller_row(
            10,
            "mario",
            0,
            "secondary",
            "chart:0:secondary",
            2,
            serde_json::json!({}),
        );
        deliver(&link, "GimmickLiveState", controller_state(1, vec![row]));
        accept_behavior_revision(&link, 5);
        assert!(link
            .restart_gimmick_chart("mario", 10, "secondary", 5)
            .is_err());
    }

    #[test]
    fn targeted_chart_restart_requires_a_known_chart_owned_by_the_requested_actor() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let row = controller_row(
            10,
            "mario",
            0,
            "main",
            "live:mario",
            2,
            serde_json::json!({}),
        );
        deliver(&link, "GimmickLiveState", controller_state(1, vec![row]));
        accept_behavior_revision(&link, 4);
        assert!(link
            .restart_gimmick_chart("mario", 10, "missing", 4)
            .is_err());
        assert!(link.restart_gimmick_chart("luigi", 10, "main", 4).is_err());
        assert!(link.restart_gimmick_chart("mario", 11, "main", 4).is_err());
    }

    #[test]
    fn targeted_chart_restart_queues_exact_payload_and_is_discarded_on_session_change() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let rows = vec![
            controller_row(
                10,
                "mario",
                0,
                "main",
                "live:mario",
                2,
                serde_json::json!({}),
            ),
            controller_row(
                10,
                "mario",
                0,
                "secondary",
                "chart:0:secondary",
                2,
                serde_json::json!({}),
            ),
        ];
        deliver(&link, "GimmickLiveState", controller_state(1, rows));
        accept_behavior_revision(&link, 4);
        link.restart_gimmick_chart("mario", 10, "secondary", 4)
            .unwrap();
        let expected = r#"<TCP_MESSAGE>{"gimmick_restart":{"boid":10,"chart_id":"secondary","expected_revision":4,"fighter":"mario"}}</TCP_MESSAGE>"#;
        assert_eq!(link.shared.lock().unwrap().outbox, [expected]);

        deliver(
            &link,
            "GiveClientId",
            serde_json::json!({"GiveClientId":{"client_id":2}}),
        );
        assert!(!link
            .shared
            .lock()
            .unwrap()
            .outbox
            .iter()
            .any(|message| message.contains("gimmick_restart")));
    }

    #[test]
    fn targeted_chart_restart_respects_operation_outbox_capacity_and_chart_id_bounds() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let row = controller_row(
            10,
            "mario",
            0,
            "secondary",
            "chart:0:secondary",
            2,
            serde_json::json!({}),
        );
        deliver(&link, "GimmickLiveState", controller_state(1, vec![row]));
        accept_behavior_revision(&link, 4);
        let too_long = "x".repeat(49);
        for invalid in ["", "  ", "bad\0id", too_long.as_str()] {
            assert!(link.restart_gimmick_chart("mario", 10, invalid, 4).is_err());
        }

        {
            let mut shared = link.shared.lock().unwrap();
            shared.outbox = (0..OUTBOX_CAP)
                .map(|index| {
                    format!(
                        "<TCP_MESSAGE>{{\"gimmick_operation\":{{\"version\":1,\"fighter\":\"mario\",\"fighter_boid\":{index},\"operation\":\"generate_article\",\"symbol\":\"FIGHTER_MARIO_GENERATE_ARTICLE_FIRE\"}}}}</TCP_MESSAGE>"
                    )
                })
                .collect();
        }
        let before = link.shared.lock().unwrap().outbox.clone();
        assert!(link
            .restart_gimmick_chart("mario", 10, "secondary", 4)
            .unwrap_err()
            .contains("queue is full"));
        assert_eq!(link.shared.lock().unwrap().outbox, before);
    }

    #[test]
    fn live_sync_adopts_plugin_revision_and_resends_authoritatively_after_changes() {
        let link = GameLink::offline_for_test();
        {
            let mut shared = link.shared.lock().unwrap();
            shared.status = LinkStatus::Connected;
            shared.client_id = Some(1);
            shared.gimmick_live_snapshot = Some(GimmickLiveSnapshot {
                version: 1,
                seq: 1,
                behavior_revision: 99,
                parameter_revision: 100,
                behavior: vec![],
                parameters: vec![],
                parameter_health: None,
            });
        }
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let mut sync = crate::gimmicks::live_sync::LiveSync::default();
        let params = BTreeMap::new();
        let mut graphs = BTreeMap::from([(
            "mario".into(),
            crate::gimmicks::templates::Preset::Timer.graph(),
        )]);
        sync.update(&params, &graphs, &link);
        let expected = link.shared.lock().unwrap().gimmick_live_expected.clone();
        assert!(expected.values().all(|revision| *revision > 100));
        sync.update(&params, &graphs, &link);
        assert_eq!(link.shared.lock().unwrap().gimmick_live_expected, expected);
        graphs.clear();
        sync.update(&params, &graphs, &link);
        {
            let shared = link.shared.lock().unwrap();
            let message = shared
                .outbox
                .iter()
                .find(|message| message.contains("gimmick_behavior"))
                .unwrap();
            assert!(message.contains("disabled_fighters\":[\"mario\"]"));
        }
        sync.enabled = false;
        sync.update(&params, &graphs, &link);
        let shared = link.shared.lock().unwrap();
        let message = shared
            .outbox
            .iter()
            .find(|message| message.contains("gimmick_behavior"))
            .unwrap();
        assert!(message.contains("disabled_fighters\":[]"));
    }

    /// Byte-exact replica of the plugin's `emit()`: body is a JSON-escaped *string*.
    fn plugin_frame(header: &str, body: &serde_json::Value) -> String {
        let body_str = serde_json::to_string(body).unwrap();
        let body_esc = serde_json::to_string(&body_str).unwrap();
        format!("<TCP_MESSAGE>{{\"header\":\"{header}\",\"body\":{body_esc}}}</TCP_MESSAGE>")
    }

    #[test]
    fn parses_notify_remove_and_torn_frames() {
        let shared = Arc::new(Mutex::new(Shared::default()));

        let data = RpmEffectData {
            effect_name: "sys_flyroll_smoke".into(),
            scale: 1.2,
            ..Default::default()
        };
        let value_in_json = serde_json::to_string(&data).unwrap();
        let notify = plugin_frame(
            "Notify",
            &serde_json::json!({
                "Notify": { "id": 0x1154cb72bfu64, "name": "sys_flyroll_smoke", "value_in_json": value_in_json }
            }),
        );
        let handshake = "<TCP_MESSAGE>{\"header\":\"RemoveAll\",\"body\":\"{}\"}</TCP_MESSAGE>";

        // Feed the stream in torn chunks like a real socket would.
        let stream = format!("{handshake}{notify}");
        let (a, b) = stream.split_at(stream.len() / 2);
        let mut buf = String::new();
        buf.push_str(a);
        for payload in extract_frames(&mut buf) {
            handle_frame(&shared, &payload);
        }
        buf.push_str(b);
        for payload in extract_frames(&mut buf) {
            handle_frame(&shared, &payload);
        }

        let s = shared.lock().unwrap();
        let kind = s.kinds.get(&0x1154cb72bf).expect("kind parsed");
        assert_eq!(kind.name, "sys_flyroll_smoke");
        assert!((kind.data.scale - 1.2).abs() < 1e-6);
        assert_eq!(kind.first.scale, kind.data.scale);
        drop(s);

        let remove = plugin_frame(
            "Remove",
            &serde_json::json!({ "Remove": { "id": 0x1154cb72bfu64 } }),
        );
        let mut buf = remove;
        for payload in extract_frames(&mut buf) {
            handle_frame(&shared, &payload);
        }
        assert!(shared.lock().unwrap().kinds.is_empty());
    }

    #[test]
    fn acmd_capture_parses_from_plugin_emit_form() {
        let shared: Arc<Mutex<Shared>> = Arc::default();
        // Exactly what the plugin serializes: CaptureLine with tagged LuaArgs.
        let capture = plugin_frame(
            "AcmdCapture",
            &serde_json::json!({
                "AcmdCapture": {
                    "kind": 0,
                    "motion": 0x1234u64,
                    "frame": 3.0,
                    "func": "ATTACK",
                    "args": [
                        {"t":"i","v":0}, {"t":"i","v":0}, {"t":"h","v":0x031ed91fcau64},
                        {"t":"n","v":8.0}, {"t":"i","v":361}, {"t":"i","v":100},
                        {"t":"i","v":0}, {"t":"i","v":40}, {"t":"n","v":4.0},
                        {"t":"n","v":0.0}, {"t":"n","v":8.0}, {"t":"n","v":6.0},
                        {"t":"x","v":null}
                    ]
                }
            }),
        );
        let mut buf = capture;
        for payload in extract_frames(&mut buf) {
            handle_frame(&shared, &payload);
        }
        let s = shared.lock().unwrap();
        let lines = s.captures.get(&0x1234).expect("capture stored");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].func, "ATTACK");
        assert_eq!(lines[0].args[2].as_hash(), Some(0x031ed91fca));
        assert_eq!(lines[0].args[3].as_f32(), Some(8.0));
        assert_eq!(lines[0].args[12], LuaArgWire::Nil);
        assert_eq!(s.captures_seq, 1);
        drop(s);

        // Re-delivery (reconnect resend) must not duplicate.
        let s2 = shared.clone();
        let mut buf2 = plugin_frame(
            "AcmdCapture",
            &serde_json::json!({
                "AcmdCapture": {
                    "kind": 0, "motion": 0x1234u64, "frame": 3.0, "func": "ATTACK",
                    "args": [
                        {"t":"i","v":0}, {"t":"i","v":0}, {"t":"h","v":0x031ed91fcau64},
                        {"t":"n","v":8.0}, {"t":"i","v":361}, {"t":"i","v":100},
                        {"t":"i","v":0}, {"t":"i","v":40}, {"t":"n","v":4.0},
                        {"t":"n","v":0.0}, {"t":"n","v":8.0}, {"t":"n","v":6.0},
                        {"t":"x","v":null}
                    ]
                }
            }),
        );
        for payload in extract_frames(&mut buf2) {
            handle_frame(&s2, &payload);
        }
        assert_eq!(s2.lock().unwrap().captures.get(&0x1234).unwrap().len(), 1);
    }

    /// The same spawn seen on a second performance arrives with a slightly different
    /// `MotionModule::frame` float. Exact equality treated those as two spawns, and the editor
    /// then showed the effect twice for a move performed twice.
    #[test]
    fn a_replayed_capture_line_is_not_a_second_spawn() {
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let line = |frame: f64| {
            plugin_frame(
                "AcmdCapture",
                &serde_json::json!({
                    "AcmdCapture": {
                        "kind": 8, "motion": 0x1234u64, "frame": frame,
                        "func": "EFFECT_FOLLOW",
                        "args": [{"t":"h","v":0x1u64}, {"t":"h","v":0x2u64}]
                    }
                }),
            )
        };
        for frame in [5.0, 5.000_002, 4.999_998] {
            let mut buf = line(frame);
            for payload in extract_frames(&mut buf) {
                handle_frame(&shared, &payload);
            }
        }
        assert_eq!(
            shared.lock().unwrap().captures.get(&0x1234).unwrap().len(),
            1,
            "one spawn observed three times is still one spawn"
        );

        // A genuinely different frame is still its own spawn — multi-hit moves depend on it.
        let mut buf = line(9.0);
        for payload in extract_frames(&mut buf) {
            handle_frame(&shared, &payload);
        }
        assert_eq!(
            shared.lock().unwrap().captures.get(&0x1234).unwrap().len(),
            2
        );
    }

    #[test]
    fn clearing_captures_empties_every_motion() {
        let link = GameLink::default();
        {
            let mut s = link.shared.lock().unwrap();
            s.captures.entry(0x1234).or_default().push(CaptureLine {
                kind: 8,
                motion: 0x1234,
                frame: 1.0,
                func: "ATTACK".into(),
                args: Vec::new(),
                run: 1,
                common: false,
            });
            s.capture_ends.insert((8, 0x1234), 1);
        }
        assert_eq!(link.latest_run_for(0x1234, None).len(), 1);
        link.clear_captures();
        assert!(link.latest_run_for(0x1234, None).is_empty());
        assert_eq!(link.capture_end_count(0x1234, Some(8)), 0);
        let s = link.shared.lock().unwrap();
        assert!(s
            .outbox
            .last()
            .is_some_and(|frame| frame.contains("\"command\":\"clear_acmd_captures\"")));
        drop(s);

        // A line already in flight may arrive after the local clear. The plugin's game-thread
        // acknowledgement is the ordering barrier that removes it again.
        feed(&link, &[capture_frame(8, 0x1234, 9.0)]);
        assert_eq!(link.latest_run_for(0x1234, Some(8)).len(), 1);
        feed(
            &link,
            &[plugin_frame(
                "AcmdCaptureCleared",
                &serde_json::json!({ "AcmdCaptureCleared": true }),
            )],
        );
        assert!(link.latest_run_for(0x1234, Some(8)).is_empty());
    }

    #[test]
    fn forgetting_one_capture_kind_does_not_clear_or_reaccept_other_kinds() {
        let link = GameLink::default();
        link.forget_captures_for_kind(8);
        feed(
            &link,
            &[capture_frame(8, 0x1234, 3.0), capture_frame(9, 0x1234, 3.0)],
        );
        assert!(link.latest_run_for(0x1234, Some(8)).is_empty());
        assert_eq!(link.latest_run_for(0x1234, Some(9)).len(), 1);
        assert!(
            link.shared.lock().unwrap().outbox.is_empty(),
            "fighter-scoped forgetting must not issue a global capture clear"
        );

        link.restore_captures_for_kind(8);
        feed(&link, &[capture_frame(8, 0x1234, 3.0)]);
        assert_eq!(link.latest_run_for(0x1234, Some(8)).len(), 1);
    }

    /// A capture line for a fighter kind, in the plugin's exact emit form, with no `run`
    /// field — the shape a plugin build predating run ids sends.
    fn capture_frame(kind: i32, motion: u64, frame: f32) -> String {
        plugin_frame(
            "AcmdCapture",
            &serde_json::json!({
                "AcmdCapture": {
                    "kind": kind, "motion": motion, "frame": frame, "func": "ATTACK",
                    "args": [
                        {"t":"i","v":0}, {"t":"i","v":0}, {"t":"h","v":0x031ed91fcau64},
                        {"t":"n","v":8.0}, {"t":"i","v":361}, {"t":"i","v":100},
                        {"t":"i","v":0}, {"t":"i","v":40}, {"t":"n","v":4.0},
                        {"t":"n","v":0.0}, {"t":"n","v":8.0}, {"t":"n","v":6.0},
                        {"t":"x","v":null}
                    ]
                }
            }),
        )
    }

    /// The same helper with an explicit run id — one performance of the move.
    fn capture_run(kind: i32, motion: u64, frame: f32, run: u32, effect: u64) -> String {
        plugin_frame(
            "AcmdCapture",
            &serde_json::json!({
                "AcmdCapture": {
                    "kind": kind, "motion": motion, "frame": frame,
                    "func": "EFFECT_FOLLOW", "run": run,
                    "args": [{"t":"h","v":effect}, {"t":"h","v":0x2u64}]
                }
            }),
        )
    }

    fn feed(link: &GameLink, payloads: &[String]) {
        for p in payloads {
            let mut buf = p.clone();
            for payload in extract_frames(&mut buf) {
                handle_frame(&link.shared, &payload);
            }
        }
    }

    /// The first playback owns the snapshot. Later runs cannot merge into or replace it, even
    /// when a mismatched/older plugin still sends them.
    #[test]
    fn only_the_first_performance_of_a_move_is_accepted() {
        let link = GameLink::default();
        // Run 7: two spawns. Run 9: a different second spawn (the other branch).
        feed(
            &link,
            &[
                capture_run(8, 0x1234, 3.0, 7, 0xAA),
                capture_run(8, 0x1234, 9.0, 7, 0xBB),
                capture_run(8, 0x1234, 3.0, 9, 0xAA),
                capture_run(8, 0x1234, 9.0, 9, 0xCC),
            ],
        );

        let loaded = link.latest_run_for(0x1234, Some(8));
        assert_eq!(loaded.len(), 2, "one performance, not the union of two");
        assert!(loaded.iter().all(|l| l.run == 7));
        let effects: Vec<u64> = loaded
            .iter()
            .filter_map(|l| l.args.first().and_then(|a| a.as_hash()))
            .collect();
        assert_eq!(
            effects,
            vec![0xAA, 0xBB],
            "later activity must not replace the locked branch"
        );

        // The settle window watches the claimed run only.
        assert_eq!(link.captures_count(0x1234, Some(8)), 2);
    }

    /// Another fighter playing the same motion allocates its own runs. Picking "the newest run
    /// overall" would then hand the editor someone else's script — so the kind filter has to
    /// come first.
    #[test]
    fn a_newer_run_from_another_fighter_does_not_win() {
        let link = GameLink::default();
        feed(
            &link,
            &[
                capture_run(8, 0x1234, 3.0, 4, 0xAA),
                // Higher run id, different fighter kind.
                capture_run(9, 0x1234, 3.0, 12, 0xFF),
            ],
        );
        let loaded = link.latest_run_for(0x1234, Some(8));
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].args[0].as_hash(), Some(0xAA));
    }

    /// Repeated later runs are rejected at ingestion rather than merely hidden at load time.
    #[test]
    fn later_runs_never_enter_the_capture_store() {
        let link = GameLink::default();
        for run in 1..=6u32 {
            feed(&link, &[capture_run(8, 0x1234, 3.0, run, 0xAA)]);
        }
        let s = link.shared.lock().unwrap();
        let runs: std::collections::BTreeSet<u32> = s
            .captures
            .get(&0x1234)
            .unwrap()
            .iter()
            .map(|l| l.run)
            .collect();
        assert_eq!(
            runs.into_iter().collect::<Vec<_>>(),
            vec![1],
            "only the first claimed run is retained"
        );
    }

    /// A plugin build without run ids sends no `run` field at all. Everything then shares run
    /// 0 and behaves exactly as it did before — one bucket, no crash, no empty load.
    #[test]
    fn captures_without_run_ids_still_load() {
        let link = GameLink::default();
        feed(
            &link,
            &[capture_frame(8, 0x1234, 3.0), capture_frame(8, 0x1234, 9.0)],
        );
        assert_eq!(link.latest_run_for(0x1234, Some(8)).len(), 2);
    }

    #[test]
    fn capture_debug_snapshots_replace_the_matching_character_motion_row() {
        let link = GameLink::default();
        let debug = |attempted: u64, succeeded: u64, contention: u64| {
            plugin_frame(
                "AcmdCaptureDebug",
                &serde_json::json!({
                    "AcmdCaptureDebug": {
                        "kind": 0x18,
                        "motion": 0x1234u64,
                        "attempted": attempted,
                        "succeeded": succeeded,
                        "contention": contention,
                        "claim_conflicts": 1,
                        "duplicates": 2,
                        "last_frame": 14.0
                    }
                }),
            )
        };

        feed(&link, &[debug(4, 1, 0), debug(9, 3, 3)]);
        assert_eq!(
            link.capture_debug(),
            vec![CaptureDebugSnapshot {
                kind: 0x18,
                motion: 0x1234,
                attempted: 9,
                succeeded: 3,
                contention: 3,
                claim_conflicts: 1,
                duplicates: 2,
                last_frame: 14.0,
            }]
        );
        assert_eq!(link.capture_debug()[0].failed(), 4);

        link.clear_captures();
        assert!(link.capture_debug().is_empty());
    }

    #[test]
    fn capture_debug_window_uses_only_character_kinds_present_in_the_cache() {
        let app = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app.rs"),
        )
        .expect("read the desktop app");
        let window = app
            .split_once("fn draw_capture_debug_window")
            .expect("capture debug window exists")
            .1
            .split_once("/// The ACMD Source window")
            .expect("capture debug window stays a bounded method")
            .0;
        assert!(
            window.contains("by_kind.entry(row.kind)")
                && window.contains("let kinds: Vec<i32> = by_kind.keys()")
                && window.contains("selectable_label")
                && window.contains("row.failed()")
                && window.contains("capture_motion_names"),
            "the compact window must tab cache rows by character and surface failed outcomes"
        );
        assert!(
            app.contains("((kind, entry.hash), entry.name.clone())"),
            "capture names must use authoritative motion-list hashes and survive character tabs"
        );
    }

    /// `AcmdCaptureEnd` is the "this move finished, its script is fully streamed" signal the
    /// deferred auto-fetch keys off. It must be counted per (fighter kind, motion) and must
    /// NOT land in the capture buckets.
    #[test]
    fn acmd_capture_end_counts_per_kind_and_motion() {
        let link = GameLink::default();
        let end = |kind: i32, motion: u64, run: u32| {
            plugin_frame(
                "AcmdCaptureEnd",
                &serde_json::json!({
                    "AcmdCaptureEnd": { "kind": kind, "motion": motion, "run": run }
                }),
            )
        };

        assert_eq!(link.capture_end_count(0x1234, Some(8)), 0);
        feed(&link, &[capture_frame(8, 0x1234, 3.0)]);
        // An end marker from another playback cannot complete the claimed run.
        feed(&link, &[end(8, 0x1234, 9)]);
        assert_eq!(link.capture_end_count(0x1234, Some(8)), 0);
        feed(&link, &[end(8, 0x1234, 0)]);
        assert_eq!(link.capture_end_count(0x1234, Some(8)), 1);
        assert!(link.capture_run_complete(Some(8), 0x1234, 0));
        // The marker is not a capture line.
        assert_eq!(link.latest_run_for(0x1234, None).len(), 1);

        // An end from a different fighter cannot complete this fighter's snapshot.
        feed(&link, &[end(9, 0x1234, 12)]);
        assert_eq!(link.capture_end_count(0x1234, Some(8)), 1);
        // No line claimed kind 9/run 12, so its unrelated end is ignored.
        assert_eq!(link.capture_end_count(0x1234, Some(9)), 0);
        assert_eq!(link.capture_end_count(0x1234, None), 1);

        // A duplicate end for the same run cannot complete the snapshot twice.
        feed(&link, &[end(8, 0x1234, 0)]);
        assert_eq!(link.capture_end_count(0x1234, Some(8)), 1);
        // Unrelated motions stay at zero.
        assert_eq!(link.capture_end_count(0x5678, Some(8)), 0);
    }

    /// The settle window uses `captures_count` to tell "more lines arrived" from "some other
    /// move produced a line", so it has to be kind-filtered and per-motion.
    #[test]
    fn captures_count_filters_by_kind_and_motion() {
        let link = GameLink::default();
        feed(
            &link,
            &[
                capture_frame(8, 0x1234, 3.0),
                capture_frame(8, 0x1234, 5.0),
                capture_frame(9, 0x1234, 3.0),
                capture_frame(8, 0x5678, 3.0),
            ],
        );
        assert_eq!(link.captures_count(0x1234, Some(8)), 2);
        assert_eq!(link.captures_count(0x1234, Some(9)), 1);
        assert_eq!(link.captures_count(0x1234, None), 3);
        assert_eq!(link.captures_count(0x5678, Some(8)), 1);
        assert_eq!(link.captures_count(0xdead, Some(8)), 0);
    }

    #[test]
    fn outbound_hitbox_rules_match_plugin_field_names() {
        let link = GameLink::default();
        link.send_hitbox_rules(&[
            HitboxRuleWire {
                motion: 0x99,
                category: 0,
                hitbox_id: Some(1),
                suppress: false,
                frame_start: Some(6.5),
                frame_end: Some(8.5),
                overrides: Some(HbOverridesWire {
                    damage: Some(12.0),
                    ..Default::default()
                }),
                inject: None,
                func: None,
            },
            HitboxRuleWire {
                motion: 0x99,
                category: 1,
                hitbox_id: None,
                suppress: false,
                frame_start: None,
                frame_end: None,
                overrides: None,
                inject: Some(InjectRuleWire {
                    frame: 5.0,
                    args: vec![LuaArgWire::Int(2), LuaArgWire::Hash(0xabc), LuaArgWire::Nil],
                    command: None,
                    guard: None,
                }),
                func: None,
            },
            HitboxRuleWire {
                motion: 0x99,
                category: 2,
                hitbox_id: Some(3),
                suppress: false,
                frame_start: Some(11.5),
                frame_end: Some(12.5),
                overrides: Some(HbOverridesWire {
                    wind_args: Some(vec![LuaArgWire::Num(3.0), LuaArgWire::Num(1.0)]),
                    ..Default::default()
                }),
                inject: Some(InjectRuleWire {
                    frame: 12.0,
                    args: vec![LuaArgWire::Num(3.0), LuaArgWire::Num(1.0)],
                    command: Some("AREA_WIND_2ND".into()),
                    guard: None,
                }),
                func: None,
            },
        ]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let v: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rules = v.get("hitbox_rules").and_then(|r| r.as_array()).unwrap();
        // Field names the plugin's serde(Deserialize) expects.
        assert_eq!(rules[0]["motion"].as_u64(), Some(0x99));
        assert_eq!(rules[0]["hitbox_id"].as_u64(), Some(1));
        assert_eq!(rules[0]["overrides"]["damage"].as_f64(), Some(12.0));
        // Frame window scopes the override to one hit (multi-hit independence).
        assert_eq!(rules[0]["frame_start"].as_f64(), Some(6.5));
        assert_eq!(rules[0]["frame_end"].as_f64(), Some(8.5));
        assert!(rules[0].get("inject").is_none());
        // category 0 (attack) is omitted so old plugins default it; grab (1) is sent.
        assert!(rules[0].get("category").is_none());
        assert_eq!(rules[1]["category"].as_u64(), Some(1));
        // Inject rule carries no frame window (it fires at its own frame).
        assert!(rules[1].get("frame_start").is_none());
        assert_eq!(rules[1]["inject"]["frame"].as_f64(), Some(5.0));
        assert_eq!(rules[1]["inject"]["args"][0]["t"].as_str(), Some("i"));
        assert_eq!(rules[1]["inject"]["args"][1]["t"].as_str(), Some("h"));
        assert_eq!(rules[1]["inject"]["args"][2]["t"].as_str(), Some("x"));
        assert_eq!(rules[2]["category"].as_u64(), Some(2));
        assert_eq!(rules[2]["overrides"]["wind_args"][0]["v"], 3.0);
        assert_eq!(
            rules[2]["inject"]["command"].as_str(),
            Some("AREA_WIND_2ND")
        );
    }

    /// Attribute edits (Hit Properties / Collision Masks / Effect-Sound) used to be dropped
    /// on the floor: the override payload had no slot for them, so picking a value in the UI
    /// changed nothing in game. Guard both that they are SENT and that the JSON keys are the
    /// ones the plugin's `HbOverrides` deserializes.
    #[test]
    fn outbound_attribute_overrides_match_plugin_field_names() {
        let link = GameLink::default();
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: 0,
            hitbox_id: Some(0),
            suppress: false,
            frame_start: None,
            frame_end: None,
            overrides: Some(HbOverridesWire {
                part: Some(2),
                bone: Some(0x112233),
                capsule: Some(false),
                setoff: Some(1),
                lr_check: Some(3),
                clang: Some(true),
                add_attack: Some(0),
                hitbox_attr: Some(0.0),
                ground_or_air: Some(2),
                mtk: Some(false),
                shield_disable: Some(true),
                reflectable: Some(false),
                absorbable: Some(true),
                landing_attack: Some(false),
                situation_mask: Some(3),
                category_mask: Some(0x3F),
                part_mask: Some(0x1F),
                no_finish_camera: Some(true),
                collision_attr: Some(0x15a2c502b3),
                sound_level: Some(1),
                sound_attr: Some(1),
                attack_region: Some(4),
                ..Default::default()
            }),
            inject: None,
            func: None,
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let v: serde_json::Value = serde_json::from_str(inner).unwrap();
        let ov = &v["hitbox_rules"][0]["overrides"];
        assert_eq!(ov["part"].as_i64(), Some(2));
        assert_eq!(ov["bone"].as_u64(), Some(0x112233));
        assert_eq!(ov["capsule"].as_bool(), Some(false));
        assert_eq!(ov["setoff"].as_i64(), Some(1));
        assert_eq!(ov["lr_check"].as_i64(), Some(3));
        assert_eq!(ov["clang"].as_bool(), Some(true));
        assert_eq!(ov["add_attack"].as_i64(), Some(0));
        assert_eq!(ov["hitbox_attr"].as_f64(), Some(0.0));
        assert_eq!(ov["ground_or_air"].as_i64(), Some(2));
        assert_eq!(ov["mtk"].as_bool(), Some(false));
        assert_eq!(ov["shield_disable"].as_bool(), Some(true));
        assert_eq!(ov["reflectable"].as_bool(), Some(false));
        assert_eq!(ov["absorbable"].as_bool(), Some(true));
        assert_eq!(ov["landing_attack"].as_bool(), Some(false));
        assert_eq!(ov["situation_mask"].as_i64(), Some(3));
        assert_eq!(ov["category_mask"].as_i64(), Some(0x3F));
        assert_eq!(ov["part_mask"].as_i64(), Some(0x1F));
        assert_eq!(ov["no_finish_camera"].as_bool(), Some(true));
        assert_eq!(ov["collision_attr"].as_u64(), Some(0x15a2c502b3));
        assert_eq!(ov["sound_level"].as_i64(), Some(1));
        assert_eq!(ov["sound_attr"].as_i64(), Some(1));
        assert_eq!(ov["attack_region"].as_i64(), Some(4));
        // Unresolved slots stay absent so the plugin keeps the script's own value.
        let bare = serde_json::to_value(HbOverridesWire::default()).unwrap();
        assert!(bare.as_object().unwrap().is_empty());
    }

    #[test]
    fn outbound_effect_aliases_match_plugin_field_names() {
        let link = GameLink::default();
        link.send_effect_aliases(&[
            EffectAliasWire {
                from: 0x111,
                to: 0x222,
                slots: vec![],
            },
            EffectAliasWire {
                from: 0x333,
                to: 0x444,
                slots: vec![1, 3],
            },
        ]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let v: serde_json::Value = serde_json::from_str(inner).unwrap();
        // Field names the plugin's spawn_rules::EffectAlias serde(Deserialize) expects.
        let aliases = v.get("effect_aliases").and_then(|a| a.as_array()).unwrap();
        assert_eq!(aliases[0]["from"].as_u64(), Some(0x111));
        assert_eq!(aliases[0]["to"].as_u64(), Some(0x222));
        // Empty slots (all costumes) omitted so the plugin's serde(default) fills it.
        assert!(aliases[0].get("slots").is_none());
        assert_eq!(aliases[1]["slots"][0].as_u64(), Some(1));
        assert_eq!(aliases[1]["slots"][1].as_u64(), Some(3));
    }

    #[test]
    fn outbound_live_tweak_omits_spawn_transform_fields() {
        let link = GameLink::default();
        let mut data = RpmEffectData {
            pos: Point3D {
                x: 6.0,
                y: -2.0,
                z: 1.5,
            },
            rot: Point3D {
                x: 0.0,
                y: 110.0,
                z: 0.0,
            },
            scale: 0.7,
            speed: 1.25,
            ..Default::default()
        };
        data.rainbow.color.red = 0.5;
        link.send_modifier_edit(0x1311e844a4, &data, false);

        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let outer: serde_json::Value = serde_json::from_str(inner).unwrap();
        let edit: serde_json::Value =
            serde_json::from_str(outer["newValue"].as_str().unwrap()).unwrap();

        assert_eq!(edit["speed"].as_f64(), Some(1.25));
        assert_eq!(edit["rainbow"]["color"]["red"].as_f64(), Some(0.5));
        assert!(edit.get("pos").is_none());
        assert!(edit.get("rot").is_none());
        assert!(edit.get("scale").is_none());
    }

    #[test]
    fn outbound_authored_modifiers_only_add_scale() {
        let link = GameLink::default();
        let mut data = RpmEffectData {
            scale: 1.75,
            ..Default::default()
        };
        data.pos.x = 99.0;
        link.send_modifier_edit(0x109297479a, &data, true);

        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let outer: serde_json::Value = serde_json::from_str(inner).unwrap();
        let edit: serde_json::Value =
            serde_json::from_str(outer["newValue"].as_str().unwrap()).unwrap();

        assert_eq!(edit["scale"].as_f64(), Some(1.75));
        assert!(edit.get("pos").is_none());
        assert!(edit.get("rot").is_none());
    }

    // ── The sound family's two-sided constants ───────────────────────────────
    //
    // The plugin crate is not a member of this workspace, so `cargo test` never builds it and a
    // `#[test]` written over there would be a comment that looks like a gate. These read the
    // plugin's source as text instead. That is weaker than compiling against it — it pins the
    // source, not the linked `.nro` — but it is the only mechanism that runs, and the failure it
    // guards against (the two tables drifting) is silent in every other check: a rule with the
    // wrong arity still serialises, still sends, and still deserialises.

    fn plugin_sound_hooks_source() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/hitbox_viewer/sound_hooks.rs");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// The plugin's copy of the sound arity table still says what the editor's says.
    ///
    /// Deliberately spelled the same way on both sides so this is a literal comparison. If they
    /// diverge, the plugin bounds an override by the wrong number of hash slots: too few and a
    /// rename silently only takes on the first sound of a pair, too many and it writes a hash40
    /// into `SET_PLAY_INHIVIT`'s duration argument.
    #[test]
    fn the_plugin_sound_table_still_matches_the_editors() {
        let source = plugin_sound_hooks_source();
        // Split on the `= &[`, not on the declaration: the first `]` after `const SOUND_FUNCS`
        // closes the *type* — `&[(&str, usize, bool)]` — and slicing there leaves an empty
        // table that every `contains` then fails against for the wrong reason.
        let table = source
            .split_once("const SOUND_FUNCS")
            .expect("plugin still declares SOUND_FUNCS")
            .1
            .split_once("= &[")
            .expect("the table is a slice literal")
            .1
            .split_once("];")
            .expect("the table is closed")
            .0;
        for (func, hashes, has_tail) in crate::acmd::SOUND_FUNCS {
            let row = format!("(\"{func}\", {hashes}, {has_tail})");
            assert!(
                table.contains(&row),
                "plugin's SOUND_FUNCS is missing or disagrees on {row}"
            );
        }
        // The other direction: a member the plugin hooks but the editor does not model would
        // capture into a stream nothing reads, and take rules the editor can never send.
        let plugin_rows = table.matches("(\"").count();
        assert_eq!(
            plugin_rows,
            crate::acmd::SOUND_FUNCS.len(),
            "the two tables have different lengths"
        );
    }

    /// Structural sound rules must dispatch every table member through its native animation
    /// command after the editor has pushed the typed arguments. A missing arm would leave the
    /// base call suppressed with no replacement, which is the exact failure this wire path fixes.
    #[test]
    fn the_plugin_dispatches_every_sound_injection_command() {
        let source = plugin_sound_hooks_source();
        let body = source
            .split_once("pub(super) unsafe fn inject(")
            .expect("plugin still exposes the sound injection dispatcher")
            .1;
        for (func, _, _) in crate::acmd::SOUND_FUNCS {
            let arm = format!("\"{func}\" => smash::app::sv_animcmd::{func}(lua_state),");
            assert!(
                body.contains(&arm),
                "sound injection has no dispatch arm for {func}"
            );
        }
    }

    /// The editor's semantic Hash40 wire values must become the integer-tagged slots that the
    /// native sound macros receive at runtime. Without this boundary conversion, a structural
    /// `PLAY_SE` addition can crash while the existing rename/suppress hook path still works.
    #[test]
    fn the_plugin_normalizes_sound_hashes_before_native_injection() {
        let source = plugin_sound_hooks_source();
        assert!(source.contains("pub(super) fn normalize_injection_args"));
        assert!(source.contains("*arg = LuaArg::Int(hash as i64)"));
        let inject_tick = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/hitbox_viewer/mod.rs"),
        )
        .expect("read plugin hitbox viewer source");
        assert!(inject_tick.contains("sound_hooks::normalize_injection_args(&mut args, command)"));
    }

    /// Capture attribution is the plugin's job, and the SPLIT is the whole point of it.
    ///
    /// The common animcmd agent's status feedback must be tagged, not dropped: the colour hook is
    /// also one of the boundaries a live injection fires at, so a hook that returns early for the
    /// common agent takes an added effect's chance to apply with it. Assert both halves — the
    /// record and the boundary run unconditionally, and only the live rule rewrite sits behind
    /// the common check.
    #[test]
    fn the_plugin_tags_common_agent_captures_instead_of_dropping_them() {
        let hooks = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/effect_viewer/acmd_hooks.rs"),
        )
        .expect("read plugin effect hooks source");
        let macro_body = hooks
            .split_once("macro_rules! color_hook {")
            .expect("the colour hook macro")
            .1;
        let guard = macro_body
            .find("if !is_common_effect_lua_state(lua_state) {")
            .expect("the colour rule guard");
        let record = macro_body
            .find("crate::slight::hitbox_viewer::record(lua_state, $command, &typed);")
            .expect("the colour capture record");
        let boundary = macro_body
            .find("inject_before_acmd_wait(lua_state, CoroutineBoundary::Effect);")
            .expect("the colour injection boundary");
        assert!(
            record < guard && boundary < guard,
            "record and the injection boundary must run before the common check, not inside it"
        );

        let capture = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/hitbox_viewer/mod.rs"),
        )
        .expect("read plugin hitbox viewer source");
        assert!(
            capture.contains(
                "crate::slight::effect_viewer::acmd_hooks::is_common_effect_lua_state(lua_state)"
            ),
            "every lua-state record must carry its agent attribution, not just the colour family"
        );
        assert!(capture.contains("pub common: bool"));
        assert!(
            capture.contains("key = fnv(key, common as u64);"),
            "a common line must not dedupe the move's own identical line away"
        );
    }

    /// Every hook reads exactly as many lua arguments as its macro declares.
    ///
    /// The arity is a literal in each `sound_hook!` invocation, separate from the table above,
    /// so the two can disagree. They must not: `read_args_exact` short by one hands slot 0 of
    /// the wrong value to the rule matcher, and long by one reads whatever the stack happens to
    /// hold past the call. Neither shows up as an error anywhere — the rule simply stops
    /// matching, or matches the wrong call.
    #[test]
    fn every_sound_hook_reads_its_macros_declared_arity() {
        let source = plugin_sound_hooks_source();
        let mut checked = 0;
        for (func, hashes, has_tail) in crate::acmd::SOUND_FUNCS {
            let argc = hashes + usize::from(*has_tail);
            let needle = format!("\"{func}\",\n    {argc}\n);");
            let one_line = format!("\"{func}\", {argc});");
            assert!(
                source.contains(&needle) || source.contains(&one_line),
                "no sound_hook! for {func} at arity {argc}"
            );
            checked += 1;
        }
        // Guard against the whole loop passing because the table went empty.
        assert_eq!(checked, 12, "the sound family is 12 members");
    }

    /// A charged smash attack must not lose everything after the charge.
    ///
    /// `mark_capture_motion` refuses to open a second run for a `(kind, motion)` that is already
    /// claimed, and a claim lives until the editor clears captures. A smash attack's hold reads
    /// as "the playback ended" — the motion frame rewinds, or the fighter parks in another
    /// motion — so every call after the charge landed on that refusal and was discarded:
    /// `attack_lw4` kept its two `FT_MOTION_RATE` calls from frames 0 and 4 and lost the
    /// `ATTACK` on 10, the `ATK_POWER` on 15 and its sounds. Tilts have no hold and were fine,
    /// which is what made it look like a per-family bug for three rounds.
    ///
    /// This pins the *source*, not the linked `.nro`, and it is a weak check for a strong
    /// property — but the property is otherwise invisible from this crate, and the failure it
    /// guards is silent by construction: a dropped capture line looks exactly like a call the
    /// game never made.
    #[test]
    fn the_plugin_resumes_a_capture_claim_held_by_the_same_object() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/hitbox_viewer/mod.rs"),
        )
        .expect("read the plugin's hitbox_viewer");
        assert!(
            source.contains("Some(h) if h.boid == boid && !h.ended => h.run,"),
            "the same-object resume arm is gone — a charged smash attack will lose every call \
             after its hold, silently"
        );
        // **`!h.ended` is the other half, and each half alone is a bug that was shipped.**
        // Resuming on `boid` alone piles every later performance into one snapshot: a charged
        // smash releases at a different motion frame each time, so the frame in the dedupe key
        // differs, nothing collapses, and the timeline fills with duplicate effects.
        assert!(
            source.contains("ended: bool") && source.contains("claim.ended = true"),
            "the claim no longer records whether a playback finished — captures will accumulate \
             across performances instead of the newest run winning"
        );
        // The third: a claim held by a *different* object is a real conflict and must still be
        // refused, or one fighter's capture absorbs another's.
        assert!(
            source.contains("claimed by another object"),
            "the cross-object refusal is gone — captures from two objects can now merge"
        );
    }

    /// The plugin must move complete capture history out of its heap when no editor is connected.
    /// The archive is durable and unbounded; only the live delivery queue remains in memory.
    #[test]
    fn the_plugin_archives_capture_history_and_skips_no_destination_serialization() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/hitbox_viewer/mod.rs"),
        )
        .expect("read the plugin's hitbox_viewer");
        assert!(
            source.contains("mod capture_archive;")
                && source.contains("capture_archive::append_line(line)")
                && source.contains("capture_archive::begin_replay()")
                && !source.contains("MAX_CAPTURE_LINES"),
            "capture history must be archived instead of dropping complete old runs"
        );

        let archive = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/hitbox_viewer/capture_archive.rs"),
        )
        .expect("read the plugin's capture archive");
        assert!(
            archive.contains("acmd_captures.jsonl")
                && archive.contains("thread::spawn")
                && archive.contains("take_replay(max"),
            "capture history must use a worker-backed disk archive and chunked replay"
        );

        let server =
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
                "plugins/slight_replica/src/rust_extender/debugging/debuggable_server/mod.rs",
            ))
            .expect("read the plugin's debuggable server");
        assert!(
            server.contains("pub fn notify_acmd_capture(line")
                && server.contains("pub fn notify_acmd_capture_archive_record(record: &str)")
                && server.contains(
                    "if !emit_wanted() {\n        return;\n    }\n    emit(\"AcmdCapture\""
                ),
            "no-editor capture notifications must not serialize JSON that has nowhere to go"
        );
    }

    /// Capture hooks run on game workers, where parking on shared state can freeze the match.
    /// Charged moves make this especially easy to hit because several scripts keep recording at
    /// one held motion frame while the per-agent completion callback touches the same state.
    #[test]
    fn the_plugin_capture_cache_never_parks_a_game_worker() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/hitbox_viewer/mod.rs"),
        )
        .expect("read the plugin's hitbox_viewer");

        assert!(
            source.contains("struct CaptureState")
                && source.contains("CAPTURE_STATE.try_lock()")
                && source.contains("CAPTURE_CONTENTION_DROPS"),
            "capture ownership and dedupe must use one non-blocking transaction state"
        );
        for removed_lock in [
            "CAPTURE_SEEN.lock()",
            "CAPTURE_PENDING.lock()",
            "END_PENDING.lock()",
            "CAPTURE_CLAIMS.lock()",
            "MOTION_WATCH.lock()",
            "CAPTURE_STATE.lock()",
        ] {
            assert!(
                !source.contains(removed_lock),
                "game-facing capture path can park at {removed_lock}"
            );
        }

        let tick = source
            .split_once("pub unsafe fn capture_tick")
            .expect("capture_tick still exists")
            .1
            .split_once("// ── Rules")
            .expect("capture_tick ends before the rules section")
            .0;
        let first_state = tick
            .find("try_capture_state()")
            .expect("capture_tick snapshots the state");
        let frame_query = tick
            .find("MotionModule::frame(boma)")
            .expect("capture_tick reads the native motion frame");
        let second_state = tick[first_state + 1..]
            .find("try_capture_state()")
            .map(|offset| offset + first_state + 1)
            .expect("capture_tick reacquires state after native queries");
        assert!(
            first_state < frame_query && frame_query < second_state,
            "native MotionModule queries must run outside the capture-state guard"
        );

        let debug_hook = source
            .split_once("fn capture_debug_attempt")
            .expect("capture outcome accounting still exists")
            .1
            .split_once("pub fn take_capture_debug")
            .expect("the hook-side accounting ends before facade snapshots")
            .0;
        assert!(
            source.contains("static CAPTURE_DEBUG: [CaptureDebugSlot;")
                && source.contains(".contention")
                && source.contains(".succeeded")
                && !debug_hook.contains("Mutex")
                && !debug_hook.contains(".lock()"),
            "reporting a capture failure must remain fixed-size, atomic, and non-blocking"
        );

        let facade = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/main_smash/facades/debuggable_server.rs"),
        )
        .expect("read the plugin's debuggable facade");
        assert!(
            facade.contains("take_capture_debug(MAX_NOTIFY_PER_TICK)")
                && facade.contains("notify_acmd_capture_debug(&snapshot)"),
            "capture outcomes must leave hook workers through the throttled facade"
        );

        let archive = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/hitbox_viewer/capture_archive.rs"),
        )
        .expect("read the capture archive worker");
        assert!(
            archive.contains("COMMANDS.try_lock()")
                && archive.contains("DROPPED.fetch_add")
                && !archive.contains("writer.send(")
                && !archive.contains("mpsc::channel"),
            "archive handoff from a capture hook must drop under contention instead of waiting"
        );

        let server = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/rust_extender/net/simple_server.rs"),
        )
        .expect("read the plugin server");
        assert!(
            server.contains("pub fn outbox_depth() -> usize")
                && server.contains("OUTBOX.try_lock().map"),
            "periodic game-thread diagnostics must not park on the sender outbox"
        );
    }

    /// Effect, sound, game, and expression ACMD coroutines can enter the live-retime scheduler
    /// from different game workers. Its startup markers and motion hints used to share three
    /// blocking `parking_lot` collections; a contended waiter can spin forever on Horizon even
    /// after the holder leaves. The scheduler must keep that cross-coroutine state lock-free and
    /// skip its native-query path entirely when no structural retime exists.
    #[test]
    fn the_plugin_acmd_scheduler_never_parks_without_live_retimes() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/effect_viewer/acmd_hooks.rs"),
        )
        .expect("read the plugin's ACMD effect hooks");

        let lifecycle = source
            .split_once("struct MotionHintSlot")
            .expect("lock-free motion-hint table still exists")
            .1
            .split_once("fn choose_motion_hint")
            .expect("startup lifecycle ends before motion selection")
            .0;
        assert!(
            lifecycle.contains("static MOTION_HINTS: [MotionHintSlot;")
                && lifecycle.contains("static ACMD_STARTUP_PENDING: [")
                && lifecycle.contains("static ACMD_PLAYBACK_STARTED: [")
                && lifecycle.contains("compare_exchange")
                && !lifecycle.contains("Mutex")
                && !lifecycle.contains(".lock()"),
            "cross-coroutine startup state must remain fixed-size and lock-free"
        );

        let boundary = source
            .split_once("unsafe fn inject_before_acmd_wait")
            .expect("direct ACMD boundary still exists")
            .1
            .split_once("unsafe fn inject_after_acmd_frame")
            .expect("direct boundary ends before the post-frame helper")
            .0;
        assert!(
            boundary.contains("spawn_rules::any_inject()")
                && boundary.contains("control_rules::any_inject()")
                && boundary.find("return;").expect("no-retime fast return")
                    < boundary
                        .find("battle_object_module_accessor(lua_state)")
                        .expect("native module lookup remains below the gate"),
            "ordinary capture must not enter the live-retime scheduler or native motion queries"
        );
    }

    /// The pinned coroutine entry points return before the authored body later reaches direct
    /// lua_bind calls. A stack guard around those entry points therefore rejects every real
    /// direct capture and rule, including `AttackModule::clear_all`.
    #[test]
    fn direct_acmd_calls_do_not_depend_on_an_expired_coroutine_guard() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/hitbox_viewer/mod.rs"),
        )
        .expect("read the plugin's hitbox hooks");

        let direct_record = source
            .split_once("pub unsafe fn record_for_boma")
            .expect("direct capture entry exists")
            .1
            .split_once("unsafe fn record_for_boma_inner")
            .expect("direct capture entry ends before implementation")
            .0;
        let direct_rules = source
            .split_once("pub(super) unsafe fn any_direct_rules")
            .expect("direct rule gate exists")
            .1
            .split_once("fn fnv")
            .expect("direct rule gate ends before capture hashing")
            .0;
        assert!(
            direct_record.contains("record_for_boma_inner(boma, func, args, false)")
                && !direct_record.contains("is_acmd_execution_for")
                && direct_rules.contains("!boma.is_null() && any_rules()")
                && !source.contains("AcmdExecutionGuard")
                && !source.contains("ACMD_EXECUTION_SLOTS"),
            "direct capture and rules must remain reachable after coroutine entry returns"
        );
        assert!(
            source.contains("static INJECTION_SLOTS:")
                && source.contains("current_thread_key(INJECTION_SCOPE_IDENTITY)"),
            "replacement suppression must not leak across native game workers"
        );
    }

    /// ACMD capture must not patch the game's post-hit collision dispatcher.
    ///
    /// Both the manual A64 trampoline and the generated binding detour have stopped Eden during
    /// ordinary attacks. Neither is needed to observe ATTACK commands, and the collision queues
    /// have no runtime consumer, so keep the facade inert.
    #[test]
    fn the_plugin_does_not_patch_collision_dispatch() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/systems/skyline_hook.rs");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for needle in [
            "COLLISION_HOOK mode=disabled reason=unused-and-unsafe",
            "collision detours disabled",
            "ACMD capture observes ATTACK commands directly",
        ] {
            assert!(
                source.contains(needle),
                "disabled collision-hook guard is missing {needle:?}"
            );
        }
        for forbidden in [
            "A64HookFunction",
            "#[skyline::hook(",
            "skyline::install_hook!",
            "COLLISION_PATTERN",
            "CollisionHitTrampoline",
        ] {
            assert!(
                !source.contains(forbidden),
                "collision dispatch must remain unpatched: found {forbidden:?}"
            );
        }

        let diag_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/diag.rs");
        let diag_source = std::fs::read_to_string(&diag_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", diag_path.display()));
        for needle in [
            "static COLLISION_HITS: AtomicU64",
            "pub fn note_collision()",
            "frame_ms={fmin:.2}/{favg:.2}/{fmax:.2} collisions={}",
        ] {
            assert!(
                diag_source.contains(needle),
                "collision diagnostic counter is missing {needle:?}"
            );
        }

        let clear_start = source
            .find("pub fn clear()")
            .expect("collision hook must expose its match-state clear");
        let clear_body = &source[clear_start..];
        assert!(
            clear_body.contains("HITS.lock().clear();"),
            "collision hook clear must discard per-match hit records"
        );
        for needle in [
            "TRAMPOLINE = None;",
            "CALLBACKS.lock().clear();",
            "*INSTALLED.lock() = false;",
        ] {
            assert!(
                !clear_body.contains(needle),
                "fight-start clear must not tear down the boot-lifetime hook state: {needle:?}"
            );
        }
    }

    #[test]
    fn the_plugin_effect_kill_hook_preserves_the_native_return() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/effect_viewer/mod.rs");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let start = source
            .find("fn hook_kill(")
            .expect("EffectModule::kill hook must exist");
        let end = source[start..]
            .find("\n}\n\n/// Bounded trace")
            .map(|offset| start + offset)
            .expect("locate EffectModule::kill hook body");
        let hook = &source[start..end];
        for needle in [
            ") -> u64 {",
            "return original!()(module_accessor, handle, a3, a4);",
            "let result = original!()(owner, handle, a3, a4);",
            "result",
        ] {
            assert!(
                hook.contains(needle),
                "EffectModule::kill ABI/passthrough guard is missing {needle:?}"
            );
        }
    }

    /// The rate category and override field the editor sends are the ones the plugin reads.
    ///
    /// Same mechanism and same weakness as the sound checks above: this pins the plugin's
    /// source, not the linked `.nro`. It is worth having because every other check is blind
    /// here — a rate rule under the wrong category still serialises, still sends, and still
    /// deserialises, and simply matches nothing. That is precisely the shape that cost D1g four
    /// game restarts.
    #[test]
    fn the_motion_rate_category_and_override_match_the_plugin() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/hitbox_viewer");
        let module = std::fs::read_to_string(root.join("mod.rs")).expect("read hitbox_viewer");
        assert!(
            module.contains(&format!(
                "pub const CAT_MOTION_RATE: u8 = {CAT_MOTION_RATE};"
            )),
            "plugin's CAT_MOTION_RATE disagrees with the editor's {CAT_MOTION_RATE}"
        );
        // One flat category space shared by every family: a collision routes rate rules into
        // another family's hook, where they would be applied to the wrong argument.
        for other in [
            CAT_ABS,
            CAT_ATK_POWER,
            CAT_ATK_SETOFF_MUL,
            CAT_SEARCH,
            CAT_SOUND,
        ] {
            assert_ne!(
                CAT_MOTION_RATE, other,
                "CAT_MOTION_RATE collides with another family"
            );
        }
        assert!(
            module.contains("pub motion_rate: Option<f32>,"),
            "the plugin's HbOverrides has no motion_rate field to deserialise into"
        );

        let hooks = std::fs::read_to_string(root.join("rate_hooks.rs")).expect("read rate_hooks");
        assert!(
            hooks.contains("replace = smash::app::sv_animcmd::FT_MOTION_RATE"),
            "the plugin no longer hooks sv_animcmd::FT_MOTION_RATE"
        );
        // The guard that stops a bad rule wedging the fighter: a zero or negative rate freezes
        // the animation and nothing below the call runs again.
        assert!(
            hooks.contains("rate > 0.0") && hooks.contains("is_finite"),
            "the plugin no longer refuses a non-positive rate"
        );
    }

    /// A rate rule serialises under the field names the plugin deserialises.
    #[test]
    fn outbound_motion_rate_rules_match_plugin_field_names() {
        let link = GameLink::default();
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_MOTION_RATE,
            hitbox_id: None,
            suppress: false,
            frame_start: Some(8.5),
            frame_end: Some(9.5),
            overrides: Some(HbOverridesWire {
                motion_rate: Some(0.6),
                ..Default::default()
            }),
            inject: None,
            func: None,
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let v: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &v["hitbox_rules"][0];
        assert_eq!(rule["category"].as_u64(), Some(CAT_MOTION_RATE as u64));
        assert_eq!(
            rule["overrides"]["motion_rate"].as_f64().map(|f| f as f32),
            Some(0.6),
            "the rate did not go out under the plugin's field name"
        );
        // A rate rule keys on motion and frame alone. The plugin passes 0 as the id, so a
        // serialised id would make the rule match nothing at all.
        assert!(
            rule["hitbox_id"].is_null(),
            "a rate rule must not carry an id: {rule}"
        );
    }

    #[test]
    fn outbound_camera_flat_offset_rules_match_plugin_field_names() {
        let link = GameLink::default();
        link.send_spawn_rules(&[SpawnRuleWire {
            eff_hash: 0x99,
            suppress: false,
            stop_func: None,
            motion: Some(0x1234),
            frame_start: Some(5.0),
            frame_end: Some(5.0),
            pos: None,
            rot: None,
            scale: None,
            rate: None,
            camera_offset: Some(0.4),
            tint: None,
            particle_tint: Some([0.1, 1.2, 0.3]),
            alpha: None,
            scale_w: Some(vec![0.75]),
            color: None,
            transition: None,
            inject: None,
            color_inject: None,
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let v: serde_json::Value = serde_json::from_str(inner).unwrap();
        let offset = v["spawn_rules"][0]["camera_offset"]
            .as_f64()
            .map(|value| value as f32);
        assert_eq!(
            offset.map(|value| (value - 0.4).abs() < f32::EPSILON),
            Some(true),
            "the camera-flat value must use the plugin's field name"
        );
        assert_eq!(
            v["spawn_rules"][0]["particle_tint"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_f64)
                        .map(|value| value as f32)
                        .collect::<Vec<_>>()
                }),
            Some(vec![0.1, 1.2, 0.3]),
            "the particle tint must use its own wire field"
        );
        assert_eq!(
            v["spawn_rules"][0]["scale_w"].as_array().map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_f64)
                    .map(|value| value as f32)
                    .collect::<Vec<_>>()
            }),
            Some(vec![0.75]),
            "the dynamic scale-W values must use their own wire field"
        );

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/effect_viewer");
        let rules = std::fs::read_to_string(root.join("spawn_rules.rs"))
            .expect("read the plugin's spawn rules");
        assert!(
            rules.contains("pub camera_offset: Option<f32>,")
                && rules.contains("pub fn camera_offset_for")
                && rules.contains("pub particle_tint: Option<[f32; 3]>")
                && rules.contains("pub fn particle_tint_for")
                && rules.contains("pub scale_w: Option<Vec<f32>>")
                && rules.contains("pub fn scale_w_for"),
            "the plugin no longer exposes the camera-flat rule field and lookup"
        );
        let hooks = std::fs::read_to_string(root.join("acmd_hooks.rs"))
            .expect("read the plugin's ACMD hooks");
        assert!(
            hooks.contains(
                "replace = smash::app::sv_animcmd::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT"
            ) && hooks.contains("hook_last_effect_set_offset_to_camera_flat")
                && hooks.contains("apply_pending_camera_offset")
                && hooks.contains("LAST_PARTICLE_SET_COLOR")
                && hooks.contains("apply_pending_particle_tint")
                && hooks.contains("LAST_EFFECT_SET_SCALE_W")
                && hooks.contains("apply_pending_scale_w"),
            "the plugin no longer rewrites the camera-flat modifier hook"
        );
    }

    #[test]
    fn outbound_colour_injection_is_typed_and_optional_for_legacy_rules() {
        let link = GameLink::default();
        link.send_spawn_rules(&[SpawnRuleWire {
            eff_hash: hash40::hash40("flash").0,
            suppress: true,
            stop_func: None,
            motion: Some(0x99),
            frame_start: Some(4.5),
            frame_end: Some(5.5),
            pos: None,
            rot: None,
            scale: None,
            rate: None,
            camera_offset: None,
            tint: None,
            particle_tint: None,
            alpha: None,
            scale_w: None,
            color: None,
            transition: None,
            inject: None,
            color_inject: Some(SpawnInjectWire {
                frame: 7.0,
                func: "FLASH".into(),
                args: vec![
                    LuaArgWire::Num(0.9),
                    LuaArgWire::Num(0.8),
                    LuaArgWire::Num(0.7),
                    LuaArgWire::Num(0.6),
                ],
                guard: None,
            }),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let inject = &value["spawn_rules"][0]["color_inject"];
        assert!(value["spawn_rules"][0].get("stop_func").is_none());
        assert_eq!(
            inject["frame"].as_f64().map(|value| value as f32),
            Some(7.0)
        );
        assert_eq!(inject["func"], "FLASH");
        assert_eq!(inject["args"][0]["t"], "n");
        assert_eq!(
            inject["args"][0]["v"].as_f64().map(|value| value as f32),
            Some(0.9)
        );

        // The plugin's default makes a rule written by an older desktop build readable when the
        // optional field is absent.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/effect_viewer/spawn_rules.rs");
        let plugin = std::fs::read_to_string(root).expect("read plugin spawn rules");
        assert!(plugin.contains("#[serde(default)]\n    pub color_inject: Option<SpawnInject>"));
        assert!(plugin.contains("#[serde(default)]\n    pub stop_func: Option<String>"));

        let hooks = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/effect_viewer/acmd_hooks.rs"),
        )
        .expect("read plugin effect hooks");
        for needle in [
            "hook_after_image4_on_arg29",
            "hook_after_image4_on_work_arg29",
            "hook_raw_effect",
            "read_args_exact(lua_state, 27)",
            "trail_suppressed",
            "lifetime_stop_suppressed",
            "stop_suppressed",
            "L2CAgentBase_start_coroutine",
            "hook_start_coroutine",
            "L2CAgentBase_resume_coroutine",
            "hook_resume_coroutine",
            "L2CAgentBase_call_coroutine",
            "hook_call_coroutine",
            "MotionModule::change_motion",
            "hook_motion_change",
            "MotionModule::change_motion_inherit_frame",
            "hook_motion_change_inherit_frame",
            "MotionModule::change_motion_inherit_frame_keep_rate",
            "hook_motion_change_inherit_frame_keep_rate",
            "MotionModule::change_motion_force_inherit_frame",
            "hook_motion_change_force_inherit_frame",
            "MotionModule::change_motion_kind",
            "hook_motion_change_kind",
            "sv_animcmd::is_excute",
            "hook_acmd_is_excute",
            "sv_animcmd::frame",
            "hook_acmd_frame",
            "inject_after_acmd_frame",
            "sv_animcmd::wait",
            "hook_acmd_wait",
            "inject_for_acmd",
            "CoroutineBoundary::Call",
            "CoroutineBoundary::Start",
            "CoroutineBoundary::Effect",
            "CoroutineBoundary::Control",
            "MotionHint",
            "remember_motion_hint",
            "reset_effect_injection_latches();",
            "reset_control_injection_latches();",
            "ACMD_PLAYBACK_STARTED",
            "begin_acmd_playback",
            "resolve_injection_motion",
            "injection_frame",
            "MAX_ATTEMPTS_PER_FRAME",
            "\"AFTER_IMAGE3_ON\" => smash::app::sv_module_access::effect",
            "\"AFTER_IMAGE4_ON_arg29\" => sv::AFTER_IMAGE4_ON_arg29",
            "\"AFTER_IMAGE_OFF\" => sv::AFTER_IMAGE_OFF",
            "static EFF_FIRED",
            "InjectGuard::new()",
            "preserve_authored_effect",
        ] {
            assert!(
                hooks.contains(needle),
                "plugin trail/colour runtime guard is missing {needle:?}"
            );
        }
        assert!(
            !hooks.contains("effect_acmd_state_matches"),
            "frame-zero dispatch must not wait for a previously observed effect coroutine"
        );
        let start_original = hooks
            .find("let result = original!()(agent, coroutine_index, name, state);")
            .expect("start coroutine must call the native implementation");
        let start_injection = hooks
            .find("inject_for_acmd_at_frame(&mut *agent, CoroutineBoundary::Start, 0.0)")
            .expect("start coroutine must cover frame zero before the native body");
        assert!(
            start_injection < start_original,
            "startup injection must run before the original coroutine enters its first slice"
        );
        let call_original = hooks
            .find("let result = original!()(agent, coroutine_index, name);")
            .expect("call coroutine must call the native implementation");
        let call_injection = hooks
            .find("inject_for_acmd(&mut *agent, CoroutineBoundary::Call)")
            .expect("call coroutine must inject after the native implementation");
        assert!(
            call_original < call_injection,
            "call-boundary injection must run after the original coroutine call"
        );
        assert!(
            hooks.contains("inject_after_acmd_frame(lua_state, target)"),
            "absolute frame waits must inject at the native frame boundary"
        );
        assert!(
            hooks.contains("inject_before_acmd_wait(lua_state, CoroutineBoundary::Wait)"),
            "relative waits must inject at the native wait boundary"
        );
        let frame_injection = hooks
            .rfind("inject_after_acmd_frame(lua_state, target)")
            .expect("frame hook must inject after its native call");
        let frame_original = hooks
            .find("original!()(lua_state, target);")
            .expect("frame hook must retain the native call");
        assert!(
            frame_original < frame_injection,
            "frame-boundary injection must run after the script reaches its target"
        );
        let wait_injection = hooks
            .rfind("inject_before_acmd_wait(lua_state, CoroutineBoundary::Wait)")
            .expect("wait hook must inject at its returned target");
        let wait_original = hooks
            .rfind("original!()(lua_state, target);")
            .expect("wait hook must retain the native call");
        assert!(
            wait_original < wait_injection,
            "wait-boundary injection must run after the script reaches its target"
        );

        let spawn_rules = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/effect_viewer/spawn_rules.rs"),
        )
        .expect("read plugin spawn rules");
        assert!(spawn_rules.contains("replacement_injections_for_suppression"));
        assert!(spawn_rules.contains("PENDING_RULES"));
        assert!(spawn_rules.contains("pub fn service_pending"));

        let control_rules = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/effect_viewer/control_rules.rs"),
        )
        .expect("read plugin control rules");
        assert!(control_rules.contains("PENDING_RULES"));
        assert!(control_rules.contains("pub fn service_pending"));

        let server = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/rust_extender/net/simple_server.rs"),
        )
        .expect("read plugin TCP server");
        assert!(server.contains("apply_timing_rules_from_network"));

        let debugger =
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
                "plugins/slight_replica/src/rust_extender/debugging/debuggable_server/mod.rs",
            ))
            .expect("read plugin debug server");
        assert!(debugger.contains("pub fn apply_timing_rules_from_network"));

        let agent_extender = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins/slight_replica/src/slight/agent_extender/mod.rs"),
        )
        .expect("read plugin agent extender");
        assert!(agent_extender.contains("StatusLine::Main"));
        assert!(agent_extender.contains("hitbox_viewer::inject_tick"));
        assert!(!agent_extender.contains("StatusLine::Post"));
        assert!(!agent_extender.contains("acmd_hooks::inject_tick"));
    }

    #[test]
    fn plugin_uses_one_stable_fighter_to_drive_global_frame_work() {
        // The plugin is built for Skyline rather than this host target. Keep a source contract
        // here for the CPU-match regression: every agent receives a Main callback, but only one
        // stable fighter is allowed to run the complete global fighter/weapon dispatch.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let extender = std::fs::read_to_string(
            root.join("plugins/slight_replica/src/slight/agent_extender/mod.rs"),
        )
        .expect("read plugin agent extender");
        let agents =
            std::fs::read_to_string(root.join("plugins/slight_replica/src/slight/agents.rs"))
                .expect("read plugin agent registry");

        assert!(agents.contains("pub fn is_frame_driver(boid: u32) -> bool"));
        assert!(agents.contains("min_by_key(|rec| (rec.entry_id, rec.boid))"));
        assert!(extender.contains("if agents::is_frame_driver(boid)"));
        assert!(!extender.contains("FRAME_SEEN"));

        let global = extender
            .split_once("fn run_one_frame()")
            .expect("global frame dispatch")
            .1;
        for pump in [
            "pump_donor_queue",
            "pump_coload_tick",
            "pump_carrier_status",
        ] {
            assert!(
                global.contains(pump),
                "global frame dispatch must own {pump}"
            );
        }
    }

    #[test]
    fn outbound_stop_suppression_is_typed_and_serialized_only_when_present() {
        let link = GameLink::default();
        link.send_spawn_rules(&[SpawnRuleWire {
            eff_hash: hash40::hash40("moon_explosion").0,
            suppress: true,
            stop_func: Some("EFFECT_OFF_KIND".into()),
            motion: Some(hash40::hash40("attack_air_n").0),
            frame_start: Some(19.5),
            frame_end: Some(20.5),
            pos: None,
            rot: None,
            scale: None,
            rate: None,
            camera_offset: None,
            tint: None,
            particle_tint: None,
            alpha: None,
            scale_w: None,
            color: None,
            transition: None,
            inject: None,
            color_inject: None,
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        assert_eq!(
            value["spawn_rules"][0]["stop_func"],
            serde_json::Value::String("EFFECT_OFF_KIND".into())
        );
    }

    #[test]
    fn outbound_effect_control_rules_keep_exact_args_and_injection() {
        let link = GameLink::default();
        link.send_effect_control_rules(&[
            EffectControlRuleWire {
                motion: Some(0x99),
                func: "ENABLE_AREA".into(),
                args: vec![LuaArgWire::Int(2)],
                suppress: true,
                frame_start: Some(7.5),
                frame_end: Some(8.5),
                work_slot: None,
                inject: Some(EffectControlInjectWire {
                    frame: 12.0,
                    func: "ENABLE_AREA".into(),
                    args: vec![LuaArgWire::Int(3)],
                }),
            },
            EffectControlRuleWire {
                motion: Some(0x99),
                func: "EFFECT_DETACH_KIND_WORK".into(),
                args: vec![LuaArgWire::Int(0x1234), LuaArgWire::Int(2)],
                suppress: true,
                frame_start: Some(7.5),
                frame_end: Some(8.5),
                work_slot: Some(17),
                inject: Some(EffectControlInjectWire {
                    frame: 12.0,
                    func: "EFFECT_DETACH_KIND_WORK".into(),
                    args: vec![LuaArgWire::Int(0x5678), LuaArgWire::Int(3)],
                }),
            },
        ]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let area = &value["effect_control_rules"][0];
        assert_eq!(area["func"].as_str(), Some("ENABLE_AREA"));
        assert_eq!(area["args"][0]["t"].as_str(), Some("i"));
        assert_eq!(area["args"][0]["v"].as_i64(), Some(2));
        assert_eq!(area["inject"]["frame"].as_f64(), Some(12.0));
        assert_eq!(area["inject"]["args"][0]["v"].as_i64(), Some(3));

        let work = &value["effect_control_rules"][1];
        assert_eq!(work["func"].as_str(), Some("EFFECT_DETACH_KIND_WORK"));
        assert_eq!(work["args"][0]["t"].as_str(), Some("i"));
        assert_eq!(work["args"][0]["v"].as_i64(), Some(0x1234));
        assert_eq!(work["work_slot"].as_i64(), Some(17));
        assert_eq!(work["inject"]["frame"].as_f64(), Some(12.0));
        assert_eq!(work["inject"]["args"][0]["v"].as_i64(), Some(0x5678));

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/effect_viewer");
        let rules = std::fs::read_to_string(root.join("control_rules.rs"))
            .expect("read the plugin's effect control rules");
        let hooks = std::fs::read_to_string(root.join("acmd_hooks.rs"))
            .expect("read the plugin's ACMD hooks");
        assert!(
            rules.contains("pub struct ControlRule")
                && rules.contains("pub struct ControlInject")
                && rules.contains("pub work_slot: Option<i32>")
                && rules.contains("self.args == args")
                && hooks.contains("WorkModule::get_int64(boma, slot)")
                && hooks.contains("args[0] = crate::slight::hitbox_viewer::LuaArg::Int")
                && hooks.contains("effect control work-slot injection rejected"),
            "the plugin must match controls by their exact captured argument vector"
        );
    }

    /// The wire category the editor sends is the one the plugin matches on.
    #[test]
    fn the_sound_wire_category_matches_the_plugin() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/hitbox_viewer/mod.rs");
        let source = std::fs::read_to_string(path).expect("read the plugin's hitbox_viewer");
        assert!(
            source.contains(&format!("pub const CAT_SOUND: u8 = {CAT_SOUND};")),
            "plugin's CAT_SOUND disagrees with the editor's {CAT_SOUND}"
        );
        // The categories are one flat space shared by every family, so a new one colliding with
        // an existing one would route sound rules into another family's hook.
        for other in [CAT_ATK_POWER, CAT_ATK_SETOFF_MUL, CAT_ABS, CAT_SEARCH] {
            assert_ne!(CAT_SOUND, other, "CAT_SOUND collides with another family");
        }
    }

    /// A sound rule serialises under the field names the plugin deserialises.
    #[test]
    fn outbound_sound_rules_match_plugin_field_names() {
        let link = GameLink::default();
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_SOUND,
            hitbox_id: Some(0xabc),
            suppress: false,
            frame_start: Some(5.5),
            frame_end: Some(6.5),
            overrides: Some(HbOverridesWire {
                sound_hashes: Some(vec![0xdef, 0x123]),
                ..Default::default()
            }),
            inject: None,
            func: Some("PLAY_STEP_FLIPPABLE".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let v: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &v["hitbox_rules"][0];
        assert_eq!(rule["category"].as_u64(), Some(CAT_SOUND as u64));
        assert_eq!(rule["func"].as_str(), Some("PLAY_STEP_FLIPPABLE"));
        let hashes = rule["overrides"]["sound_hashes"].as_array().unwrap();
        assert_eq!(hashes[0].as_u64(), Some(0xdef));
        assert_eq!(hashes[1].as_u64(), Some(0x123));
    }

    /// Expression rules use the plugin's distinct category, macro discriminator, and complete
    /// typed argument vector. The three pieces have to travel together: the category selects the
    /// hook family, `func` selects its exact primitive, and the vector is what gets pushed back
    /// onto the Lua stack.
    #[test]
    fn outbound_expression_rules_match_plugin_field_names() {
        let link = GameLink::default();
        let pristine = vec![LuaArgWire::Int(5)];
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_EXPRESSION,
            hitbox_id: Some(expression_key("QUAKE", &pristine)),
            suppress: false,
            frame_start: Some(8.5),
            frame_end: Some(8.5),
            overrides: Some(HbOverridesWire {
                expression_args: Some(vec![LuaArgWire::Int(7)]),
                ..Default::default()
            }),
            inject: None,
            func: Some("QUAKE".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let v: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &v["hitbox_rules"][0];
        assert_eq!(rule["category"].as_u64(), Some(CAT_EXPRESSION as u64));
        assert_eq!(rule["func"].as_str(), Some("QUAKE"));
        assert_eq!(
            rule["hitbox_id"].as_u64(),
            Some(expression_key("QUAKE", &pristine))
        );
        assert_eq!(
            rule["overrides"]["expression_args"][0]["t"].as_str(),
            Some("i")
        );
        assert_eq!(
            rule["overrides"]["expression_args"][0]["v"].as_i64(),
            Some(7)
        );
    }

    /// The editor and plugin agree on the expression category, field, and every measured hook
    /// name, including the direct native rumble binding.
    #[test]
    fn the_expression_category_and_hooks_match_the_plugin() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/hitbox_viewer");
        let module = std::fs::read_to_string(root.join("mod.rs")).expect("read hitbox_viewer");
        assert!(module.contains(&format!("pub const CAT_EXPRESSION: u8 = {CAT_EXPRESSION};")));
        assert!(module.contains("pub expression_args: Option<Vec<LuaArg>>,"));
        assert!(module.contains("CAT_EXPRESSION => match inj.command.as_deref()"));
        assert!(module.contains("control_set_rumble_native_args(&args)"));
        assert!(module.contains("any_rules() && !is_injecting()"));
        assert!(module.contains("original!()(boma, kind, duration, looped, target)"));
        assert!(module.contains("category == CAT_SOUND || category == CAT_EXPRESSION"));
        for func in [
            "RUMBLE_HIT",
            "QUAKE",
            "FT_ATTACK_ABS_CAMERA_QUAKE",
            "ControlModule::set_rumble",
        ] {
            assert!(
                module.contains(&format!("\"{func}\"")),
                "missing {func} hook"
            );
        }
        for other in [0, CAT_SOUND, CAT_MOTION_RATE, CAT_SPEED_EX, CAT_SPEED] {
            assert_ne!(
                CAT_EXPRESSION, other,
                "CAT_EXPRESSION collides with another family"
            );
        }
    }

    /// The editor and plugin agree on the separate point-event category and its hook. Keeping it
    /// out of the collision categories matters: a facing rule has no hitbox id or payload to
    /// apply, and routing it through ATTACK would turn an orientation edit into a collision edit.
    #[test]
    fn the_reverse_lr_category_and_hook_match_the_plugin() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/hitbox_viewer");
        let module = std::fs::read_to_string(root.join("mod.rs")).expect("read hitbox_viewer");
        assert!(module.contains(&format!("pub const CAT_REVERSE_LR: u8 = {CAT_REVERSE_LR};")));
        assert!(module.contains("replace = smash::app::sv_animcmd::REVERSE_LR"));
        assert!(module.contains("hook_reverse_lr"));
        assert!(module.contains("CAT_REVERSE_LR =>"));
        for other in [
            0,
            CAT_ABS,
            CAT_SEARCH,
            CAT_ATK_POWER,
            CAT_ATK_SETOFF_MUL,
            CAT_SOUND,
            CAT_MOTION_RATE,
            CAT_EXPRESSION,
            CAT_SPEED_EX,
            CAT_SPEED,
        ] {
            assert_ne!(
                CAT_REVERSE_LR, other,
                "CAT_REVERSE_LR collides with {other}"
            );
        }
    }

    #[test]
    fn the_set_speed_ex_category_and_hook_match_the_plugin() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/hitbox_viewer");
        let module = std::fs::read_to_string(root.join("mod.rs")).expect("read hitbox_viewer");
        assert!(module.contains(&format!("pub const CAT_SPEED_EX: u8 = {CAT_SPEED_EX};")));
        assert!(module.contains("SET_SPEED_EX"));
        assert!(module.contains("hook_set_speed_ex"));
        assert!(module.contains("pub speed_x: Option<f32>"));
        assert!(module.contains("pub speed_y: Option<f32>"));
        for other in [
            0,
            CAT_SEARCH,
            CAT_SOUND,
            CAT_MOTION_RATE,
            CAT_EXPRESSION,
            CAT_REVERSE_LR,
        ] {
            assert_ne!(CAT_SPEED_EX, other, "CAT_SPEED_EX collides with {other}");
        }
    }

    #[test]
    fn the_speed_addition_and_correction_categories_match_the_plugin() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/hitbox_viewer");
        let module = std::fs::read_to_string(root.join("mod.rs")).expect("read hitbox_viewer");
        assert!(module.contains(&format!(
            "pub const CAT_ADD_SPEED_NO_LIMIT: u8 = {CAT_ADD_SPEED_NO_LIMIT};"
        )));
        assert!(module.contains(&format!("pub const CAT_CORRECT: u8 = {CAT_CORRECT};")));
        assert!(module.contains("replace = smash::app::sv_animcmd::ADD_SPEED_NO_LIMIT"));
        assert!(module.contains("replace = smash::app::sv_animcmd::CORRECT"));
        assert!(module.contains("hook_add_speed_no_limit"));
        assert!(module.contains("hook_correct"));
        assert!(module.contains("pub correct_kind: Option<i64>"));
        assert!(module.contains(&format!("pub const CAT_SPEED: u8 = {CAT_SPEED};")));
        assert!(module.contains("replace = smash::app::sv_animcmd::SET_SPEED"));
        assert!(module.contains("hook_set_speed"));
        assert!(module.contains(&format!(
            "pub const CAT_FT_CATCH_STOP: u8 = {CAT_FT_CATCH_STOP};"
        )));
        assert!(module.contains("replace = smash::app::sv_animcmd::FT_CATCH_STOP"));
        assert!(module.contains("hook_ft_catch_stop"));
        assert!(module.contains("pub ft_catch_stop_arg1: Option<f32>"));
        assert!(module.contains("pub ft_catch_stop_arg2: Option<f32>"));
        assert!(module.contains(&format!(
            "pub const CAT_FT_START_ADJUST_MOTION_FRAME: u8 = {CAT_FT_START_ADJUST_MOTION_FRAME};"
        )));
        assert!(
            module.contains("replace = smash::app::sv_animcmd::FT_START_ADJUST_MOTION_FRAME_arg1")
        );
        assert!(module.contains("hook_ft_start_adjust_motion_frame"));
        assert!(module.contains("pub ft_start_adjust_motion_frame_value: Option<f32>"));
        for (category, other) in [
            (CAT_ADD_SPEED_NO_LIMIT, CAT_SPEED_EX),
            (CAT_CORRECT, CAT_SPEED_EX),
            (CAT_SPEED, CAT_SPEED_EX),
            (CAT_ADD_SPEED_NO_LIMIT, CAT_CORRECT),
            (CAT_SPEED, CAT_ADD_SPEED_NO_LIMIT),
            (CAT_SPEED, CAT_CORRECT),
            (CAT_FT_CATCH_STOP, CAT_SPEED_EX),
            (CAT_FT_CATCH_STOP, CAT_SPEED),
            (CAT_FT_CATCH_STOP, CAT_ADD_SPEED_NO_LIMIT),
            (CAT_FT_CATCH_STOP, CAT_CORRECT),
            (CAT_FT_START_ADJUST_MOTION_FRAME, CAT_FT_CATCH_STOP),
            (CAT_FT_START_ADJUST_MOTION_FRAME, CAT_SPEED_EX),
            (CAT_FT_START_ADJUST_MOTION_FRAME, CAT_SPEED),
            (CAT_FT_START_ADJUST_MOTION_FRAME, CAT_ADD_SPEED_NO_LIMIT),
            (CAT_FT_START_ADJUST_MOTION_FRAME, CAT_CORRECT),
        ] {
            assert_ne!(category, other, "new point categories must not collide");
        }
    }

    #[test]
    fn outbound_ft_start_adjust_motion_frame_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        let key = numeric_point_key("FT_START_ADJUST_MOTION_FRAME_arg1", &[0.85]);
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_FT_START_ADJUST_MOTION_FRAME,
            hitbox_id: Some(key),
            suppress: false,
            frame_start: Some(17.0),
            frame_end: Some(17.0),
            overrides: Some(HbOverridesWire {
                ft_start_adjust_motion_frame_value: Some(1.25),
                ..Default::default()
            }),
            inject: None,
            func: Some("FT_START_ADJUST_MOTION_FRAME_arg1".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &value["hitbox_rules"][0];
        assert_eq!(
            rule["category"].as_u64(),
            Some(CAT_FT_START_ADJUST_MOTION_FRAME as u64)
        );
        assert_eq!(rule["hitbox_id"].as_u64(), Some(key));
        assert_eq!(
            rule["func"].as_str(),
            Some("FT_START_ADJUST_MOTION_FRAME_arg1")
        );
        assert_eq!(
            rule["overrides"]["ft_start_adjust_motion_frame_value"].as_f64(),
            Some(1.25)
        );
    }

    #[test]
    fn kinetic_point_categories_and_overrides_match_plugin_wire_fields() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/slight_replica/src/slight/hitbox_viewer");
        let module = std::fs::read_to_string(root.join("mod.rs")).expect("read hitbox_viewer");
        let rate_hooks =
            std::fs::read_to_string(root.join("rate_hooks.rs")).expect("read rate hooks");
        assert!(module.contains(&format!("pub const CAT_CLR_SPEED: u8 = {CAT_CLR_SPEED};")));
        assert!(module.contains(&format!("pub const CAT_SET_AIR: u8 = {CAT_SET_AIR};")));
        assert!(module.contains(&format!(
            "pub const CAT_CHANGE_KINETIC: u8 = {CAT_CHANGE_KINETIC};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_KINETIC_ADD_SPEED: u8 = {CAT_KINETIC_ADD_SPEED};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_KINETIC_SUSPEND_ENERGY: u8 = {CAT_KINETIC_SUSPEND_ENERGY};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_KINETIC_RESUME_ENERGY: u8 = {CAT_KINETIC_RESUME_ENERGY};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_KINETIC_ENABLE_ENERGY: u8 = {CAT_KINETIC_ENABLE_ENERGY};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_KINETIC_UNABLE_ENERGY: u8 = {CAT_KINETIC_UNABLE_ENERGY};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_KINETIC_CLEAR_SPEED_ALL: u8 = {CAT_KINETIC_CLEAR_SPEED_ALL};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_KINETIC_SET_CONSIDER_GROUND_FRICTION: u8 = {CAT_KINETIC_SET_CONSIDER_GROUND_FRICTION};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_MOTION_MODULE_SET_RATE: u8 = {CAT_MOTION_MODULE_SET_RATE};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_MOTION_MODULE_SET_HELPER_CALCULATION: u8 = {CAT_MOTION_MODULE_SET_HELPER_CALCULATION};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_MOTION_MODULE_SET_RATE_PARTIAL: u8 = {CAT_MOTION_MODULE_SET_RATE_PARTIAL};"
        )));
        assert!(module.contains(&format!("pub const CAT_WORK_FLAG: u8 = {CAT_WORK_FLAG};")));
        assert!(module.contains(&format!(
            "pub const CAT_WORK_TRANSITION_TERM: u8 = {CAT_WORK_TRANSITION_TERM};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_WORK_MODULE_SET: u8 = {CAT_WORK_MODULE_SET};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_WORK_MODULE_INC_INT: u8 = {CAT_WORK_MODULE_INC_INT};"
        )));
        assert!(module.contains(&format!(
            "pub const CAT_MOTION_MODULE_SET_FRAME_PARTIAL: u8 = {CAT_MOTION_MODULE_SET_FRAME_PARTIAL};"
        )));
        assert!(module.contains("pub clr_speed_kinetic_kind: Option<i64>"));
        assert!(module.contains("pub change_kinetic_type: Option<i64>"));
        assert!(module.contains("pub kinetic_energy_id: Option<i64>"));
        assert!(module.contains("pub kinetic_ground_friction: Option<bool>"));
        assert!(module.contains("pub kinetic_ground_friction_energy: Option<i64>"));
        assert!(module.contains("pub work_flag: Option<i64>"));
        assert!(module.contains("pub work_transition_term: Option<i64>"));
        assert!(module.contains("pub work_module_inc_int_slot: Option<i64>"));
        assert!(module.contains("pub work_module_set_int_value: Option<i64>"));
        assert!(module.contains("pub work_module_set_int64_value: Option<i64>"));
        assert!(module.contains("pub work_module_set_float_value: Option<f32>"));
        assert!(module.contains("pub work_module_set_slot: Option<i64>"));
        assert!(module.contains("pub motion_module_rate: Option<f32>"));
        assert!(module.contains("pub motion_module_helper_calculation: Option<bool>"));
        assert!(module.contains("pub motion_module_rate_partial: Option<f32>"));
        assert!(module.contains("pub motion_module_frame_partial: Option<f32>"));
        assert!(module.contains("pub motion_module_frame_partial_sync: Option<bool>"));
        assert!(module.contains("replace = smash::app::sv_kinetic_energy::clear_speed"));
        assert!(module.contains("replace = smash::app::sv_animcmd::SET_AIR"));
        assert!(module.contains("replace = smash::app::lua_bind::KineticModule::change_kinetic"));
        assert!(module.contains("replace = smash::app::lua_bind::KineticModule::add_speed"));
        assert!(module.contains("hook_kinetic_add_speed"));
        assert!(module.contains("hook_kinetic_suspend_energy"));
        assert!(module.contains("KineticModule::suspend_energy,"));
        assert!(module.contains("hook_kinetic_resume_energy"));
        assert!(module.contains("KineticModule::resume_energy,"));
        assert!(module.contains("hook_kinetic_enable_energy"));
        assert!(module.contains("KineticModule::enable_energy,"));
        assert!(module.contains("hook_kinetic_unable_energy"));
        assert!(module.contains("KineticModule::unable_energy,"));
        assert!(module.contains("replace = smash::app::lua_bind::KineticModule::clear_speed_all"));
        assert!(module.contains("hook_kinetic_clear_speed_all"));
        assert!(module.contains("smash::app::lua_bind::WorkModule::on_flag,"));
        assert!(module.contains("smash::app::lua_bind::WorkModule::off_flag,"));
        assert!(module.contains("hook_work_module_on_flag"));
        assert!(module.contains("hook_work_module_off_flag"));
        assert!(module.contains("smash::app::lua_bind::WorkModule::enable_transition_term,"));
        assert!(module.contains("smash::app::lua_bind::WorkModule::unable_transition_term,"));
        assert!(module.contains("hook_work_module_enable_transition_term"));
        assert!(module.contains("hook_work_module_unable_transition_term"));
        assert!(module.contains("smash::app::lua_bind::WorkModule::enable_transition_term_group,"));
        assert!(
            module.contains("smash::app::lua_bind::WorkModule::unable_transition_term_group_ex,")
        );
        assert!(module.contains("hook_work_module_enable_transition_term_group"));
        assert!(module.contains("hook_work_module_unable_transition_term_group_ex"));
        assert!(module.contains("replace = smash::app::lua_bind::WorkModule::inc_int"));
        assert!(module.contains("hook_work_module_inc_int"));
        assert!(module.contains("replace = smash::app::lua_bind::WorkModule::set_int)"));
        assert!(module.contains("replace = smash::app::lua_bind::WorkModule::set_float)"));
        assert!(module.contains("replace = smash::app::lua_bind::WorkModule::set_int64)"));
        assert!(module.contains("hook_work_module_set_int"));
        assert!(module.contains("hook_work_module_set_float"));
        assert!(module.contains("hook_work_module_set_int64"));
        assert!(rate_hooks.contains("replace = smash::app::lua_bind::MotionModule::set_rate"));
        assert!(rate_hooks.contains("hook_motion_module_set_rate"));
        assert!(rate_hooks
            .contains("replace = smash::app::lua_bind::MotionModule::set_helper_calculation"));
        assert!(rate_hooks.contains("hook_motion_module_set_helper_calculation"));
        assert!(
            rate_hooks.contains("replace = smash::app::lua_bind::MotionModule::set_rate_partial")
        );
        assert!(rate_hooks.contains("hook_motion_module_set_rate_partial"));
        assert!(
            rate_hooks.contains("replace = smash::app::lua_bind::MotionModule::set_frame_partial")
        );
        assert!(rate_hooks.contains("hook_motion_module_set_frame_partial"));
        // The hook takes the native four-argument form. The three-argument source shape is a Lua
        // convenience the game fills in before this binding is reached, so a hook written against
        // the source arity would not compile — and would be modelling the wrong contract.
        assert!(rate_hooks.contains("    sync: bool,\n"));
        assert!(rate_hooks.contains(".filter(|value| value.is_finite() && *value >= 0.0)"));
        for (category, other) in [
            (CAT_CLR_SPEED, CAT_FT_START_ADJUST_MOTION_FRAME),
            (CAT_SET_AIR, CAT_CLR_SPEED),
            (CAT_SET_AIR, CAT_SPEED),
            (CAT_SET_AIR, CAT_REVERSE_LR),
            (CAT_CHANGE_KINETIC, CAT_SET_AIR),
            (CAT_CHANGE_KINETIC, CAT_CLR_SPEED),
            (CAT_KINETIC_ADD_SPEED, CAT_CHANGE_KINETIC),
            (CAT_KINETIC_ADD_SPEED, CAT_SET_AIR),
            (CAT_KINETIC_ADD_SPEED, CAT_SPEED),
            (CAT_KINETIC_SUSPEND_ENERGY, CAT_KINETIC_ADD_SPEED),
            (CAT_KINETIC_RESUME_ENERGY, CAT_KINETIC_SUSPEND_ENERGY),
            (CAT_KINETIC_RESUME_ENERGY, CAT_CHANGE_KINETIC),
            (CAT_KINETIC_ENABLE_ENERGY, CAT_KINETIC_RESUME_ENERGY),
            (CAT_KINETIC_UNABLE_ENERGY, CAT_KINETIC_ENABLE_ENERGY),
            (CAT_KINETIC_CLEAR_SPEED_ALL, CAT_KINETIC_UNABLE_ENERGY),
            (
                CAT_KINETIC_SET_CONSIDER_GROUND_FRICTION,
                CAT_KINETIC_CLEAR_SPEED_ALL,
            ),
            (
                CAT_MOTION_MODULE_SET_RATE,
                CAT_KINETIC_SET_CONSIDER_GROUND_FRICTION,
            ),
            (
                CAT_MOTION_MODULE_SET_HELPER_CALCULATION,
                CAT_MOTION_MODULE_SET_RATE,
            ),
            (
                CAT_MOTION_MODULE_SET_RATE_PARTIAL,
                CAT_MOTION_MODULE_SET_HELPER_CALCULATION,
            ),
            (CAT_WORK_FLAG, CAT_MOTION_MODULE_SET_RATE_PARTIAL),
            (CAT_WORK_TRANSITION_TERM, CAT_WORK_FLAG),
            (CAT_WORK_MODULE_INC_INT, CAT_WORK_TRANSITION_TERM),
            (CAT_WORK_MODULE_SET, CAT_WORK_MODULE_INC_INT),
            (CAT_MOTION_MODULE_SET_FRAME_PARTIAL, CAT_WORK_MODULE_SET),
            (
                CAT_MOTION_MODULE_SET_FRAME_PARTIAL,
                CAT_MOTION_MODULE_SET_RATE_PARTIAL,
            ),
        ] {
            assert_ne!(category, other, "kinetic category collision");
        }
        assert!(module.contains("const KINETIC_KEY_SET_AIR: u64 = u64::MAX - 2;"));
        assert!(module.contains("const KINETIC_KEY_CLEAR_SPEED_ALL: u64 = u64::MAX - 3;"));

        let link = GameLink::default();
        let clr_key = numeric_point_key("CLR_SPEED", &[7.0]);
        link.send_hitbox_rules(&[
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_CLR_SPEED,
                hitbox_id: Some(clr_key),
                suppress: false,
                frame_start: Some(3.5),
                frame_end: Some(4.5),
                overrides: Some(HbOverridesWire {
                    clr_speed_kinetic_kind: Some(9),
                    ..Default::default()
                }),
                inject: None,
                func: Some("CLR_SPEED".into()),
            },
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_SET_AIR,
                hitbox_id: Some(KINETIC_KEY_SET_AIR),
                suppress: true,
                frame_start: Some(7.5),
                frame_end: Some(8.5),
                overrides: None,
                inject: None,
                func: Some("SET_AIR".into()),
            },
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_CHANGE_KINETIC,
                hitbox_id: Some(numeric_point_key("KineticModule::change_kinetic", &[4.0])),
                suppress: false,
                frame_start: Some(9.5),
                frame_end: Some(10.5),
                overrides: Some(HbOverridesWire {
                    change_kinetic_type: Some(5),
                    ..Default::default()
                }),
                inject: None,
                func: Some("KineticModule::change_kinetic".into()),
            },
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_KINETIC_ADD_SPEED,
                hitbox_id: Some(numeric_point_key(
                    "KineticModule::add_speed",
                    &[0.5, -1.25, 0.0],
                )),
                suppress: false,
                frame_start: Some(11.5),
                frame_end: Some(11.5),
                overrides: Some(HbOverridesWire {
                    speed_x: Some(1.5),
                    speed_y: Some(2.25),
                    ..Default::default()
                }),
                inject: None,
                func: Some("KineticModule::add_speed".into()),
            },
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_WORK_FLAG,
                hitbox_id: Some(numeric_point_key("WorkModule::on_flag", &[7.0])),
                suppress: false,
                frame_start: Some(13.5),
                frame_end: Some(13.5),
                overrides: Some(HbOverridesWire {
                    work_flag: Some(9),
                    ..Default::default()
                }),
                inject: None,
                func: Some("WorkModule::on_flag".into()),
            },
        ]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rules = value["hitbox_rules"].as_array().unwrap();
        assert_eq!(rules[0]["category"].as_u64(), Some(CAT_CLR_SPEED as u64));
        assert_eq!(rules[0]["hitbox_id"].as_u64(), Some(clr_key));
        assert_eq!(rules[0]["func"].as_str(), Some("CLR_SPEED"));
        assert_eq!(
            rules[0]["overrides"]["clr_speed_kinetic_kind"].as_i64(),
            Some(9)
        );
        assert_eq!(rules[1]["category"].as_u64(), Some(CAT_SET_AIR as u64));
        assert_eq!(rules[1]["hitbox_id"].as_u64(), Some(KINETIC_KEY_SET_AIR));
        assert_eq!(rules[1]["func"].as_str(), Some("SET_AIR"));
        assert_eq!(
            rules[2]["category"].as_u64(),
            Some(CAT_CHANGE_KINETIC as u64)
        );
        assert_eq!(
            rules[2]["overrides"]["change_kinetic_type"].as_i64(),
            Some(5)
        );
        assert_eq!(
            rules[2]["func"].as_str(),
            Some("KineticModule::change_kinetic")
        );
        assert_eq!(
            rules[3]["category"].as_u64(),
            Some(CAT_KINETIC_ADD_SPEED as u64)
        );
        assert_eq!(rules[3]["func"].as_str(), Some("KineticModule::add_speed"));
        assert_eq!(rules[3]["overrides"]["speed_x"].as_f64(), Some(1.5));
        assert_eq!(rules[3]["overrides"]["speed_y"].as_f64(), Some(2.25));
        assert_eq!(rules[4]["category"].as_u64(), Some(CAT_WORK_FLAG as u64));
        assert_eq!(
            rules[4]["hitbox_id"].as_u64(),
            Some(numeric_point_key("WorkModule::on_flag", &[7.0]))
        );
        assert_eq!(rules[4]["func"].as_str(), Some("WorkModule::on_flag"));
        assert_eq!(rules[4]["overrides"]["work_flag"].as_i64(), Some(9));
    }

    #[test]
    fn outbound_work_transition_term_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        let key = numeric_point_key("WorkModule::enable_transition_term", &[7.0]);
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_WORK_TRANSITION_TERM,
            hitbox_id: Some(key),
            suppress: false,
            frame_start: Some(13.5),
            frame_end: Some(13.5),
            overrides: Some(HbOverridesWire {
                work_transition_term: Some(9),
                ..Default::default()
            }),
            inject: None,
            func: Some("WorkModule::enable_transition_term".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &value["hitbox_rules"][0];
        assert_eq!(
            rule["category"].as_u64(),
            Some(CAT_WORK_TRANSITION_TERM as u64)
        );
        assert_eq!(rule["hitbox_id"].as_u64(), Some(key));
        assert_eq!(
            rule["func"].as_str(),
            Some("WorkModule::enable_transition_term")
        );
        assert_eq!(rule["overrides"]["work_transition_term"].as_i64(), Some(9));
    }

    #[test]
    fn outbound_work_module_set_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        let key = numeric_point_key("WorkModule::set_int", &[1.0, 7.0]);
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_WORK_MODULE_SET,
            hitbox_id: Some(key),
            suppress: false,
            frame_start: Some(13.5),
            frame_end: Some(13.5),
            overrides: Some(HbOverridesWire {
                work_module_set_int_value: Some(3),
                work_module_set_slot: Some(9),
                ..Default::default()
            }),
            inject: None,
            func: Some("WorkModule::set_int".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &value["hitbox_rules"][0];
        assert_eq!(rule["category"].as_u64(), Some(CAT_WORK_MODULE_SET as u64));
        assert_eq!(rule["hitbox_id"].as_u64(), Some(key));
        assert_eq!(rule["func"].as_str(), Some("WorkModule::set_int"));
        assert_eq!(
            rule["overrides"]["work_module_set_int_value"].as_i64(),
            Some(3)
        );
        assert_eq!(rule["overrides"]["work_module_set_slot"].as_i64(), Some(9));
    }

    #[test]
    fn outbound_work_module_set_int64_rules_preserve_exact_integer_wire_fields() {
        let link = GameLink::default();
        let value = 0x1000_0000_0001;
        let slot = 7;
        let key = integer_point_key("WorkModule::set_int64", &[value, slot]);
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_WORK_MODULE_SET,
            hitbox_id: Some(key),
            suppress: false,
            frame_start: Some(13.5),
            frame_end: Some(13.5),
            overrides: Some(HbOverridesWire {
                work_module_set_int64_value: Some(0x1000_0000_0003),
                work_module_set_slot: Some(9),
                ..Default::default()
            }),
            inject: None,
            func: Some("WorkModule::set_int64".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value_json: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &value_json["hitbox_rules"][0];
        assert_eq!(rule["category"].as_u64(), Some(CAT_WORK_MODULE_SET as u64));
        assert_eq!(rule["hitbox_id"].as_u64(), Some(key));
        assert_eq!(rule["func"].as_str(), Some("WorkModule::set_int64"));
        assert_eq!(
            rule["overrides"]["work_module_set_int64_value"].as_i64(),
            Some(0x1000_0000_0003)
        );
        assert_eq!(rule["overrides"]["work_module_set_slot"].as_i64(), Some(9));
    }

    #[test]
    fn outbound_work_module_inc_int_rules_preserve_exact_integer_wire_fields() {
        let link = GameLink::default();
        let slot = 7;
        let key = integer_point_key("WorkModule::inc_int", &[slot]);
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_WORK_MODULE_INC_INT,
            hitbox_id: Some(key),
            suppress: false,
            frame_start: Some(13.5),
            frame_end: Some(13.5),
            overrides: Some(HbOverridesWire {
                work_module_inc_int_slot: Some(9),
                ..Default::default()
            }),
            inject: None,
            func: Some("WorkModule::inc_int".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &value["hitbox_rules"][0];
        assert_eq!(
            rule["category"].as_u64(),
            Some(CAT_WORK_MODULE_INC_INT as u64)
        );
        assert_eq!(rule["hitbox_id"].as_u64(), Some(key));
        assert_eq!(rule["func"].as_str(), Some("WorkModule::inc_int"));
        assert_eq!(
            rule["overrides"]["work_module_inc_int_slot"].as_i64(),
            Some(9)
        );
    }

    #[test]
    fn outbound_motion_module_set_rate_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        let pristine_rate = 0.8;
        let key = numeric_point_key("MotionModule::set_rate", &[pristine_rate]);
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_MOTION_MODULE_SET_RATE,
            hitbox_id: Some(key),
            suppress: false,
            frame_start: Some(12.5),
            frame_end: Some(12.5),
            overrides: Some(HbOverridesWire {
                motion_module_rate: Some(1.25),
                ..Default::default()
            }),
            inject: None,
            func: Some("MotionModule::set_rate".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &value["hitbox_rules"][0];
        assert_eq!(
            rule["category"].as_u64(),
            Some(CAT_MOTION_MODULE_SET_RATE as u64)
        );
        assert_eq!(rule["hitbox_id"].as_u64(), Some(key));
        assert_eq!(rule["func"].as_str(), Some("MotionModule::set_rate"));
        assert_eq!(rule["overrides"]["motion_module_rate"].as_f64(), Some(1.25));
    }

    #[test]
    fn outbound_motion_module_set_helper_calculation_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        let pristine_enabled = false;
        let key = numeric_point_key(
            "MotionModule::set_helper_calculation",
            &[if pristine_enabled { 1.0 } else { 0.0 }],
        );
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_MOTION_MODULE_SET_HELPER_CALCULATION,
            hitbox_id: Some(key),
            suppress: false,
            frame_start: Some(12.5),
            frame_end: Some(12.5),
            overrides: Some(HbOverridesWire {
                motion_module_helper_calculation: Some(true),
                ..Default::default()
            }),
            inject: None,
            func: Some("MotionModule::set_helper_calculation".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &value["hitbox_rules"][0];
        assert_eq!(
            rule["category"].as_u64(),
            Some(CAT_MOTION_MODULE_SET_HELPER_CALCULATION as u64)
        );
        assert_eq!(rule["hitbox_id"].as_u64(), Some(key));
        assert_eq!(
            rule["func"].as_str(),
            Some("MotionModule::set_helper_calculation")
        );
        assert_eq!(
            rule["overrides"]["motion_module_helper_calculation"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn outbound_motion_module_set_rate_partial_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        let pristine_part = 2.0;
        let pristine_rate = 0.8;
        let key = numeric_point_key(
            "MotionModule::set_rate_partial",
            &[pristine_part, pristine_rate],
        );
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_MOTION_MODULE_SET_RATE_PARTIAL,
            hitbox_id: Some(key),
            suppress: false,
            frame_start: Some(12.5),
            frame_end: Some(12.5),
            overrides: Some(HbOverridesWire {
                motion_module_rate_partial: Some(1.25),
                ..Default::default()
            }),
            inject: None,
            func: Some("MotionModule::set_rate_partial".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &value["hitbox_rules"][0];
        assert_eq!(
            rule["category"].as_u64(),
            Some(CAT_MOTION_MODULE_SET_RATE_PARTIAL as u64)
        );
        assert_eq!(rule["hitbox_id"].as_u64(), Some(key));
        assert_eq!(
            rule["func"].as_str(),
            Some("MotionModule::set_rate_partial")
        );
        assert_eq!(
            rule["overrides"]["motion_module_rate_partial"].as_f64(),
            Some(1.25)
        );
    }

    #[test]
    fn outbound_motion_module_set_frame_partial_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        let pristine_part = 2.0;
        let pristine_frame = 4.0;
        let key = numeric_point_key(
            "MotionModule::set_frame_partial",
            &[pristine_part, pristine_frame],
        );
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_MOTION_MODULE_SET_FRAME_PARTIAL,
            hitbox_id: Some(key),
            suppress: false,
            frame_start: Some(12.5),
            frame_end: Some(12.5),
            overrides: Some(HbOverridesWire {
                motion_module_frame_partial: Some(6.5),
                motion_module_frame_partial_sync: Some(false),
                ..Default::default()
            }),
            inject: None,
            func: Some("MotionModule::set_frame_partial".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &value["hitbox_rules"][0];
        assert_eq!(
            rule["category"].as_u64(),
            Some(CAT_MOTION_MODULE_SET_FRAME_PARTIAL as u64)
        );
        assert_eq!(rule["hitbox_id"].as_u64(), Some(key));
        assert_eq!(
            rule["func"].as_str(),
            Some("MotionModule::set_frame_partial")
        );
        assert_eq!(
            rule["overrides"]["motion_module_frame_partial"].as_f64(),
            Some(6.5)
        );
        assert_eq!(
            rule["overrides"]["motion_module_frame_partial_sync"].as_bool(),
            Some(false)
        );
    }

    #[test]
    fn outbound_kinetic_energy_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        link.send_hitbox_rules(&[
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_KINETIC_SUSPEND_ENERGY,
                hitbox_id: Some(numeric_point_key("KineticModule::suspend_energy", &[2.0])),
                suppress: false,
                frame_start: Some(4.5),
                frame_end: Some(4.5),
                overrides: Some(HbOverridesWire {
                    kinetic_energy_id: Some(3),
                    ..Default::default()
                }),
                inject: None,
                func: Some("KineticModule::suspend_energy".into()),
            },
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_KINETIC_RESUME_ENERGY,
                hitbox_id: Some(numeric_point_key("KineticModule::resume_energy", &[2.0])),
                suppress: true,
                frame_start: Some(9.5),
                frame_end: Some(9.5),
                overrides: None,
                inject: None,
                func: Some("KineticModule::resume_energy".into()),
            },
        ]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rules = value["hitbox_rules"].as_array().unwrap();
        assert_eq!(
            rules[0]["category"].as_u64(),
            Some(CAT_KINETIC_SUSPEND_ENERGY as u64)
        );
        assert_eq!(
            rules[0]["func"].as_str(),
            Some("KineticModule::suspend_energy")
        );
        assert_eq!(rules[0]["overrides"]["kinetic_energy_id"].as_i64(), Some(3));
        assert_eq!(
            rules[1]["category"].as_u64(),
            Some(CAT_KINETIC_RESUME_ENERGY as u64)
        );
        assert_eq!(
            rules[1]["func"].as_str(),
            Some("KineticModule::resume_energy")
        );
        assert_eq!(rules[1]["suppress"].as_bool(), Some(true));
    }

    #[test]
    fn outbound_enable_unable_energy_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        link.send_hitbox_rules(&[
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_KINETIC_ENABLE_ENERGY,
                hitbox_id: Some(numeric_point_key("KineticModule::enable_energy", &[3.0])),
                suppress: false,
                frame_start: Some(4.5),
                frame_end: Some(4.5),
                overrides: Some(HbOverridesWire {
                    kinetic_energy_id: Some(4),
                    ..Default::default()
                }),
                inject: None,
                func: Some("KineticModule::enable_energy".into()),
            },
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_KINETIC_UNABLE_ENERGY,
                hitbox_id: Some(numeric_point_key("KineticModule::unable_energy", &[5.0])),
                suppress: true,
                frame_start: Some(9.5),
                frame_end: Some(9.5),
                overrides: None,
                inject: None,
                func: Some("KineticModule::unable_energy".into()),
            },
        ]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rules = value["hitbox_rules"].as_array().unwrap();
        assert_eq!(
            rules[0]["category"].as_u64(),
            Some(CAT_KINETIC_ENABLE_ENERGY as u64)
        );
        assert_eq!(
            rules[0]["func"].as_str(),
            Some("KineticModule::enable_energy")
        );
        assert_eq!(rules[0]["overrides"]["kinetic_energy_id"].as_i64(), Some(4));
        assert_eq!(
            rules[1]["category"].as_u64(),
            Some(CAT_KINETIC_UNABLE_ENERGY as u64)
        );
        assert_eq!(
            rules[1]["func"].as_str(),
            Some("KineticModule::unable_energy")
        );
        assert_eq!(rules[1]["suppress"].as_bool(), Some(true));
    }

    #[test]
    fn outbound_kinetic_clear_speed_all_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        link.send_hitbox_rules(&[
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_KINETIC_CLEAR_SPEED_ALL,
                hitbox_id: Some(KINETIC_KEY_CLEAR_SPEED_ALL),
                suppress: true,
                frame_start: Some(4.0),
                frame_end: Some(4.0),
                overrides: None,
                inject: None,
                func: Some("KineticModule::clear_speed_all".into()),
            },
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_KINETIC_CLEAR_SPEED_ALL,
                hitbox_id: None,
                suppress: false,
                frame_start: None,
                frame_end: None,
                overrides: None,
                inject: Some(InjectRuleWire {
                    frame: 8.0,
                    args: Vec::new(),
                    command: Some("KineticModule::clear_speed_all".into()),
                    guard: None,
                }),
                func: Some("KineticModule::clear_speed_all".into()),
            },
        ]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rules = value["hitbox_rules"].as_array().unwrap();
        assert_eq!(
            rules[0]["category"].as_u64(),
            Some(CAT_KINETIC_CLEAR_SPEED_ALL as u64)
        );
        assert_eq!(
            rules[0]["hitbox_id"].as_u64(),
            Some(KINETIC_KEY_CLEAR_SPEED_ALL)
        );
        assert_eq!(
            rules[0]["func"].as_str(),
            Some("KineticModule::clear_speed_all")
        );
        assert_eq!(rules[0]["suppress"].as_bool(), Some(true));
        assert_eq!(
            rules[1]["inject"]["command"].as_str(),
            Some("KineticModule::clear_speed_all")
        );
        assert!(rules[1]["inject"]["args"].as_array().unwrap().is_empty());
    }

    #[test]
    fn outbound_ground_friction_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        let key = numeric_point_key("KineticModule::set_consider_ground_friction", &[0.0, 7.0]);
        link.send_hitbox_rules(&[
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_KINETIC_SET_CONSIDER_GROUND_FRICTION,
                hitbox_id: Some(key),
                suppress: false,
                frame_start: Some(4.0),
                frame_end: Some(4.0),
                overrides: Some(HbOverridesWire {
                    kinetic_ground_friction: Some(true),
                    kinetic_ground_friction_energy: Some(9),
                    ..Default::default()
                }),
                inject: None,
                func: Some("KineticModule::set_consider_ground_friction".into()),
            },
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_KINETIC_SET_CONSIDER_GROUND_FRICTION,
                hitbox_id: None,
                suppress: false,
                frame_start: None,
                frame_end: None,
                overrides: None,
                inject: Some(InjectRuleWire {
                    frame: 6.0,
                    args: vec![LuaArgWire::Bool(false), LuaArgWire::Int(5)],
                    command: Some("KineticModule::set_consider_ground_friction".into()),
                    guard: None,
                }),
                func: Some("KineticModule::set_consider_ground_friction".into()),
            },
        ]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rules = value["hitbox_rules"].as_array().unwrap();
        assert_eq!(
            rules[0]["category"].as_u64(),
            Some(CAT_KINETIC_SET_CONSIDER_GROUND_FRICTION as u64)
        );
        assert_eq!(rules[0]["hitbox_id"].as_u64(), Some(key));
        assert_eq!(
            rules[0]["func"].as_str(),
            Some("KineticModule::set_consider_ground_friction")
        );
        assert_eq!(
            rules[0]["overrides"]["kinetic_ground_friction"].as_bool(),
            Some(true)
        );
        assert_eq!(
            rules[0]["overrides"]["kinetic_ground_friction_energy"].as_i64(),
            Some(9)
        );
        assert_eq!(rules[1]["inject"]["frame"].as_f64(), Some(6.0));
        assert_eq!(
            rules[1]["inject"]["command"].as_str(),
            Some("KineticModule::set_consider_ground_friction")
        );
        assert_eq!(rules[1]["inject"]["args"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn outbound_ft_catch_stop_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        let key = numeric_point_key("FT_CATCH_STOP", &[6.0, 1.0]);
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_FT_CATCH_STOP,
            hitbox_id: Some(key),
            suppress: false,
            frame_start: Some(6.0),
            frame_end: Some(6.0),
            overrides: Some(HbOverridesWire {
                ft_catch_stop_arg1: Some(7.5),
                ft_catch_stop_arg2: Some(0.25),
                ..Default::default()
            }),
            inject: None,
            func: Some("FT_CATCH_STOP".into()),
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rule = &value["hitbox_rules"][0];
        assert_eq!(rule["category"].as_u64(), Some(CAT_FT_CATCH_STOP as u64));
        assert_eq!(rule["hitbox_id"].as_u64(), Some(key));
        assert_eq!(rule["func"].as_str(), Some("FT_CATCH_STOP"));
        assert_eq!(rule["overrides"]["ft_catch_stop_arg1"].as_f64(), Some(7.5));
        assert_eq!(rule["overrides"]["ft_catch_stop_arg2"].as_f64(), Some(0.25));
    }

    #[test]
    fn outbound_set_speed_rules_match_plugin_wire_fields() {
        for (category, hitbox_id, frame_end, func) in [
            (CAT_SPEED_EX, Some(7), 5.5, "SET_SPEED_EX"),
            (CAT_SPEED, None, 4.5, "SET_SPEED"),
        ] {
            let link = GameLink::default();
            link.send_hitbox_rules(&[HitboxRuleWire {
                motion: 0x99,
                category,
                hitbox_id,
                suppress: false,
                frame_start: Some(4.5),
                frame_end: Some(frame_end),
                overrides: Some(HbOverridesWire {
                    speed_x: Some(1.5),
                    speed_y: Some(-3.8),
                    ..Default::default()
                }),
                inject: None,
                func: Some(func.into()),
            }]);
            let frame = link.shared.lock().unwrap().outbox[0].clone();
            let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
            let value: serde_json::Value = serde_json::from_str(inner).unwrap();
            let rule = &value["hitbox_rules"][0];
            assert_eq!(rule["category"].as_u64(), Some(category as u64));
            assert_eq!(rule["hitbox_id"], serde_json::json!(hitbox_id));
            assert_eq!(rule["func"].as_str(), Some(func));
            assert_eq!(rule["overrides"]["speed_x"].as_f64(), Some(1.5));
            assert!((rule["overrides"]["speed_y"].as_f64().unwrap() + 3.8).abs() < 1e-6);
        }
    }

    #[test]
    fn outbound_speed_addition_and_correction_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        link.send_hitbox_rules(&[
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_ADD_SPEED_NO_LIMIT,
                hitbox_id: None,
                suppress: false,
                frame_start: Some(4.0),
                frame_end: Some(4.0),
                overrides: Some(HbOverridesWire {
                    speed_x: Some(1.5),
                    speed_y: Some(-3.8),
                    ..Default::default()
                }),
                inject: None,
                func: Some("ADD_SPEED_NO_LIMIT".into()),
            },
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_CORRECT,
                hitbox_id: Some(1),
                suppress: false,
                frame_start: Some(4.0),
                frame_end: Some(4.0),
                overrides: Some(HbOverridesWire {
                    correct_kind: Some(2),
                    ..Default::default()
                }),
                inject: None,
                func: Some("CORRECT".into()),
            },
        ]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rules = value["hitbox_rules"].as_array().unwrap();
        assert_eq!(
            rules[0]["category"].as_u64(),
            Some(CAT_ADD_SPEED_NO_LIMIT as u64)
        );
        assert_eq!(rules[0]["overrides"]["speed_x"].as_f64(), Some(1.5));
        assert_eq!(rules[0]["func"].as_str(), Some("ADD_SPEED_NO_LIMIT"));
        assert_eq!(rules[1]["category"].as_u64(), Some(CAT_CORRECT as u64));
        assert_eq!(rules[1]["hitbox_id"].as_u64(), Some(1));
        assert_eq!(rules[1]["overrides"]["correct_kind"].as_i64(), Some(2));
        assert_eq!(rules[1]["func"].as_str(), Some("CORRECT"));
    }

    /// Suppression and zero-argument injection use the same wire lane. The command discriminator
    /// is explicit on injection so a future point-event category cannot accidentally fire the
    /// wrong no-argument animcmd primitive.
    #[test]
    fn outbound_reverse_lr_rules_match_plugin_wire_fields() {
        let link = GameLink::default();
        link.send_hitbox_rules(&[
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_REVERSE_LR,
                hitbox_id: Some(0),
                suppress: true,
                frame_start: Some(1.5),
                frame_end: Some(2.5),
                overrides: None,
                inject: None,
                func: None,
            },
            HitboxRuleWire {
                motion: 0x99,
                category: CAT_REVERSE_LR,
                hitbox_id: None,
                suppress: false,
                frame_start: None,
                frame_end: None,
                overrides: None,
                inject: Some(InjectRuleWire {
                    frame: 3.0,
                    args: Vec::new(),
                    command: Some("REVERSE_LR".into()),
                    guard: None,
                }),
                func: None,
            },
        ]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        let rules = value["hitbox_rules"].as_array().unwrap();
        assert_eq!(rules[0]["category"].as_u64(), Some(CAT_REVERSE_LR as u64));
        assert_eq!(rules[0]["hitbox_id"].as_u64(), Some(0));
        assert_eq!(rules[0]["suppress"].as_bool(), Some(true));
        assert_eq!(rules[1]["category"].as_u64(), Some(CAT_REVERSE_LR as u64));
        assert_eq!(rules[1]["inject"]["frame"].as_f64(), Some(3.0));
        assert_eq!(rules[1]["inject"]["args"].as_array().unwrap().len(), 0);
        assert_eq!(rules[1]["inject"]["command"].as_str(), Some("REVERSE_LR"));
    }

    /// A rule for any other family carries no `func`, and the field is omitted entirely.
    ///
    /// The plugin reads an absent `func` as "match any member of this category", which is only
    /// safe because no other family needs the discrimination. If some other rule started
    /// emitting `func: null` or an empty string, an old plugin build would still ignore it but a
    /// current one would compare against a name no macro has and match nothing — a live edit
    /// that silently does nothing, which is the failure B5b cost two tasks to find.
    #[test]
    fn non_sound_and_non_expression_rules_omit_a_macro_name() {
        let link = GameLink::default();
        link.send_hitbox_rules(&[HitboxRuleWire {
            motion: 0x99,
            category: CAT_ATK_POWER,
            hitbox_id: Some(0),
            suppress: false,
            frame_start: None,
            frame_end: None,
            overrides: Some(HbOverridesWire {
                atk_mod_value: Some(12.0),
                ..Default::default()
            }),
            inject: None,
            func: None,
        }]);
        let frame = link.shared.lock().unwrap().outbox[0].clone();
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let v: serde_json::Value = serde_json::from_str(inner).unwrap();
        assert!(v["hitbox_rules"][0].get("func").is_none());
        assert!(v["hitbox_rules"][0]["overrides"]
            .get("sound_hashes")
            .is_none());
    }

    /// A sound hash arrives from the game typed as `Int`, and every reader has to accept it.
    ///
    /// **Measured live 2026-08-06.** Kirby's up tilt passes `se_kirby_swing_l` as
    /// `L2CValueType::Int` holding `0x10556b83cc`, which is exactly `hash40("se_kirby_swing_l")`
    /// — the right value under the wrong type tag, because on the lua stack a hash and an int
    /// are both just numbers.
    ///
    /// This asymmetry is what made the bug so hard to see: `as_hash` here already accepted
    /// `Int`, so **live capture of sounds worked**, while the plugin's rule matcher required
    /// `LuaArg::Hash` and rejected every sound in the game. Two readers of one stream with
    /// different tolerance — the feature half-worked, which reads as "nearly right" rather than
    /// "wrong in one place".
    #[test]
    fn a_sound_hash_is_read_whether_it_arrives_as_a_hash_or_an_int() {
        let value = 0x10_556b_83ccu64;
        assert_eq!(LuaArgWire::Hash(value).as_hash(), Some(value));
        assert_eq!(LuaArgWire::Int(value as i64).as_hash(), Some(value));
        // And the plugin's side of the same stream agrees. Source text, because that crate is
        // not in this workspace — see `the_plugin_sound_table_still_matches_the_editors`.
        let source = plugin_sound_hooks_source();
        let body = source
            .split_once("fn hash_arg(")
            .expect("plugin still has hash_arg")
            .1;
        let body = &body[..body.find("\n}").unwrap_or(body.len())];
        assert!(
            body.contains("LuaArg::Int"),
            "the plugin's key reader must accept an Int-tagged hash, or live sound edits go \
             silently dead while live sound capture keeps working: {body}"
        );
    }

    #[test]
    fn repeated_spawn_rule_pushes_keep_only_the_latest_full_rule_list() {
        fn rule_at(frame: f32) -> SpawnRuleWire {
            SpawnRuleWire {
                eff_hash: 0x99,
                suppress: false,
                stop_func: None,
                motion: Some(0x1234),
                frame_start: Some(frame),
                frame_end: Some(frame),
                pos: None,
                rot: None,
                scale: None,
                rate: None,
                camera_offset: None,
                tint: None,
                particle_tint: None,
                alpha: None,
                scale_w: None,
                color: None,
                transition: None,
                inject: None,
                color_inject: None,
            }
        }

        let link = GameLink::default();
        link.send_spawn_rules(&[rule_at(3.0)]);
        link.send_spawn_rules(&[rule_at(2.0)]);
        link.send_spawn_rules(&[rule_at(1.0)]);

        let shared = link.shared.lock().unwrap();
        let pending: Vec<&String> = shared
            .outbox
            .iter()
            .filter(|message| message.contains("\"spawn_rules\":"))
            .collect();
        assert_eq!(pending.len(), 1, "superseded drag states must not be sent");
        let frame = pending[0];
        let inner = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(inner).unwrap();
        assert_eq!(value["spawn_rules"][0]["frame_start"].as_f64(), Some(1.0));
        assert_eq!(value["spawn_rules"][0]["frame_end"].as_f64(), Some(1.0));
    }

    #[test]
    fn project_rule_batches_publish_only_after_the_flattened_union_is_ready() {
        let link = GameLink::default();
        link.begin_rule_batch();
        link.send_spawn_rules(&[]);
        link.send_effect_control_rules(&[]);
        link.send_hitbox_rules(&[]);
        assert!(
            link.shared.lock().unwrap().outbox.is_empty(),
            "intermediate materialization sends must stay local"
        );
        link.end_rule_batch();
        link.send_spawn_rules(&[]);
        link.send_effect_control_rules(&[]);
        link.send_hitbox_rules(&[]);
        let shared = link.shared.lock().unwrap();
        assert_eq!(
            shared.outbox.len(),
            3,
            "the completed project publishes one replacement per live rule family"
        );
    }

    #[test]
    fn project_batches_hold_every_live_family_until_the_replacement_is_ready() {
        let link = GameLink::default();
        let asset_bundle = AssetBundleWire {
            generation: 1,
            target: "mario".into(),
            files: vec![AssetBundleFileWire {
                path: "fighter/mario/motion/body/c00/wait1.nuanmb".into(),
                file: "effect_viewer/live_assets/files/fighter/mario/motion/body/c00/wait1.nuanmb"
                    .into(),
                size: 123,
            }],
        };
        link.begin_project_batch();
        link.send_spawn_rules(&[]);
        link.send_effect_control_rules(&[]);
        link.send_hitbox_rules(&[]);
        link.send_effect_aliases(&[]);
        link.send_donor_effs(&[]);
        link.send_donor_bytes(&[]);
        link.send_asset_bundle(&asset_bundle);
        link.send_effect_names(&["example".into()]);
        link.send_reset_pins();
        link.send_live_eff_reload();
        link.send_live_eff_probe();
        link.send_modifier_edit(0x1234, &RpmEffectData::default(), false);
        assert!(
            link.shared.lock().unwrap().outbox.is_empty(),
            "project replacement must not expose clears or partial restores"
        );

        link.end_project_batch();
        link.send_spawn_rules(&[]);
        link.send_effect_control_rules(&[]);
        link.send_hitbox_rules(&[]);
        link.send_effect_aliases(&[]);
        link.send_donor_effs(&[]);
        link.send_donor_bytes(&[]);
        link.send_asset_bundle(&asset_bundle);
        link.send_effect_names(&["example".into()]);
        link.send_reset_pins();
        link.send_live_eff_reload();
        link.send_live_eff_probe();
        link.send_modifier_edit(0x1234, &RpmEffectData::default(), false);
        assert_eq!(
            link.shared.lock().unwrap().outbox.len(),
            12,
            "the finished import can publish one message per protocol family"
        );
    }

    #[test]
    fn asset_bundle_wire_and_carrier_status_round_trip() {
        let link = GameLink::default();
        let bundle = AssetBundleWire {
            generation: 1_725_000_000_123,
            target: "mario".into(),
            files: vec![AssetBundleFileWire {
                path: "fighter/mario/motion/body/c00/swing.prc".into(),
                file: "effect_viewer/live_assets/files/1725000000123/fighter/mario/motion/body/c00/swing.prc"
                    .into(),
                size: 4096,
            }],
        };
        link.send_asset_bundle(&bundle);
        let shared = link.shared.lock().unwrap();
        assert_eq!(shared.outbox.len(), 1);
        let frame = &shared.outbox[0];
        let payload = &frame["<TCP_MESSAGE>".len()..frame.len() - "</TCP_MESSAGE>".len()];
        let value: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(value["asset_bundle"]["generation"], bundle.generation);
        assert_eq!(value["asset_bundle"]["target"], "mario");
        assert_eq!(value["asset_bundle"]["fighter_binding_version"], 1);
        assert_eq!(value["asset_bundle"]["files"][0]["size"], 4096);
        drop(shared);

        let status_frame = plugin_frame(
            "AssetBundleStatus",
            &serde_json::json!({
                "AssetBundleStatus": {
                    "target": "mario",
                    "generation": bundle.generation,
                    "staged": 3,
                    "served": 2,
                    "activation": "carrier_recreate",
                    "serving_generation": bundle.generation,
                    "phase": "loading",
                    "ready": false
                }
            }),
        );
        let mut receive = status_frame;
        for payload in extract_frames(&mut receive) {
            handle_frame(&link.shared, &payload);
        }
        assert_eq!(
            link.asset_bundle_status(),
            AssetBundleStatus {
                target: "mario".into(),
                generation: bundle.generation,
                staged: 3,
                served: 2,
                serving_generation: bundle.generation,
                phase: "loading".into(),
                ready: false,
                reports: 1,
            }
        );

        let error_frame = plugin_frame(
            "AssetBundleError",
            &serde_json::json!({
                "AssetBundleError": { "reason": "payload missing" }
            }),
        );
        let mut receive = error_frame;
        for payload in extract_frames(&mut receive) {
            handle_frame(&link.shared, &payload);
        }
        assert_eq!(
            link.take_asset_bundle_error().as_deref(),
            Some("payload missing")
        );
        assert!(link.take_asset_bundle_error().is_none());
    }

    #[test]
    fn asset_bundle_status_ignores_stale_or_impossible_acknowledgements() {
        let link = GameLink::default();
        let accepted = |generation, target, staged, served| {
            let mut receive = plugin_frame(
                "AssetBundleStatus",
                &serde_json::json!({
                    "AssetBundleStatus": {
                        "target": target,
                        "generation": generation,
                        "staged": staged,
                        "served": served,
                        "activation": "carrier_recreate"
                    }
                }),
            );
            for payload in extract_frames(&mut receive) {
                handle_frame(&link.shared, &payload);
            }
        };
        accepted(7, "mario", 2, 1);
        assert_eq!(link.asset_bundle_status().reports, 1);
        for (generation, target, staged, served, activation) in [
            (6, "mario", 2, 2, "carrier_recreate"),
            (7, "mario", 1, 2, "carrier_recreate"),
            (8, "Mario", 2, 2, "carrier_recreate"),
            (8, "mario", 2, 2, "next_owner_load"),
            (8, "mario", 2, 2, "resident_buffer"),
        ] {
            let mut receive = plugin_frame(
                "AssetBundleStatus",
                &serde_json::json!({
                    "AssetBundleStatus": {
                        "target": target,
                        "generation": generation,
                        "staged": staged,
                        "served": served,
                        "activation": activation
                    }
                }),
            );
            for payload in extract_frames(&mut receive) {
                handle_frame(&link.shared, &payload);
            }
        }
        assert_eq!(
            link.asset_bundle_status(),
            AssetBundleStatus {
                target: "mario".into(),
                generation: 7,
                staged: 2,
                served: 1,
                serving_generation: 7,
                phase: "staged".into(),
                ready: false,
                reports: 1,
            }
        );
    }

    #[test]
    fn asset_carrier_ready_requires_current_generation_and_clear_is_acknowledged() {
        let link = GameLink::default();
        let report = |generation, serving_generation, target, staged, phase, ready| {
            let mut receive = plugin_frame(
                "AssetBundleStatus",
                &serde_json::json!({
                    "AssetBundleStatus": {
                        "target": target, "generation": generation,
                        "serving_generation": serving_generation,
                        "staged": staged, "served": staged,
                        "activation": "carrier_recreate", "phase": phase, "ready": ready
                    }
                }),
            );
            for payload in extract_frames(&mut receive) {
                handle_frame(&link.shared, &payload);
            }
        };
        report(9, 8, "mario", 2, "ready", true);
        report(9, 9, "mario", 2, "unknown", false);
        assert_eq!(link.asset_bundle_status().reports, 0);
        report(9, 9, "mario", 2, "ready", true);
        assert!(link.asset_bundle_status().ready);
        report(10, 10, "", 0, "idle", false);
        let cleared = link.asset_bundle_status();
        assert_eq!(cleared.generation, 10);
        assert!(cleared.target.is_empty());
        assert!(!cleared.ready);
        assert_eq!(cleared.reports, 2);
    }

    #[test]
    fn frame_extraction_handles_fragmented_adjacent_frames_and_leaves_trailing_bytes() {
        const OPEN: &str = "<TCP_MESSAGE>";
        const CLOSE: &str = "</TCP_MESSAGE>";
        let stream = format!("garbage{OPEN}{{\"id\":1}}{CLOSE}{OPEN}{{\"id\":2}}{CLOSE}trailing");
        let mut buf = String::new();
        let mut payloads = Vec::new();

        // Three-byte reads split opening and closing tags as well as JSON payloads, like a
        // socket read that happens to end in the middle of any protocol token.
        for chunk in stream.as_bytes().chunks(3) {
            buf.push_str(std::str::from_utf8(chunk).unwrap());
            payloads.extend(extract_frames(&mut buf));
        }

        assert_eq!(
            payloads,
            vec![r#"{"id":1}"#.to_string(), r#"{"id":2}"#.to_string()]
        );
        assert_eq!(buf, "trailing");
    }

    #[test]
    fn frame_extraction_discards_oversized_garbage_but_preserves_latest_partial_frame() {
        const OPEN: &str = "<TCP_MESSAGE>";
        const CLOSE: &str = "</TCP_MESSAGE>";

        let mut garbage = "g".repeat((1 << 20) + 1);
        assert!(extract_frames(&mut garbage).is_empty());
        assert!(garbage.is_empty(), "unframed garbage must not grow forever");

        // Put a valid opening tag after more than FRAME_CAP bytes of junk. The guard should trim
        // only the junk and retain the in-flight frame so a later socket read can finish it.
        let mut partial = "g".repeat((16 << 20) + 1);
        partial.push_str(OPEN);
        partial.push_str("partial");
        assert!(extract_frames(&mut partial).is_empty());
        assert_eq!(partial, format!("{OPEN}partial"));

        partial.push_str(CLOSE);
        assert_eq!(extract_frames(&mut partial), vec!["partial".to_string()]);
        assert!(partial.is_empty());

        let mut runaway_frame = format!("{OPEN}{}", "x".repeat(16 << 20));
        assert!(extract_frames(&mut runaway_frame).is_empty());
        assert!(
            runaway_frame.is_empty(),
            "one unterminated frame must be bounded"
        );
    }

    #[test]
    fn full_replace_outbox_coalesces_each_family_and_preserves_other_order() {
        fn family(name: &str, value: u8) -> String {
            format!("<TCP_MESSAGE>{{\"{name}\":{value}}}</TCP_MESSAGE>")
        }
        let command = |name: &str| format!("<TCP_MESSAGE>{{\"command\":\"{name}\"}}</TCP_MESSAGE>");
        let mut outbox = Vec::new();
        let spawn_old = family("spawn_rules", 1);
        let spawn_new = family("spawn_rules", 2);
        let hitbox_old = family("hitbox_rules", 3);
        let hitbox_new = family("hitbox_rules", 4);
        let alias_old = family("effect_aliases", 5);
        let alias_new = family("effect_aliases", 6);
        let before = command("before");
        let between = command("between");
        let after = command("after");

        for frame in [
            before.clone(),
            spawn_old,
            hitbox_old,
            between.clone(),
            spawn_new.clone(),
            alias_old,
            hitbox_new.clone(),
            alias_new.clone(),
            after.clone(),
        ] {
            queue_latest_full_replace(&mut outbox, frame);
        }

        assert_eq!(
            outbox,
            vec![before, between, spawn_new, hitbox_new, alias_new, after],
            "only superseded full replacements should disappear; command order must stay stable"
        );
    }

    #[test]
    fn outbox_overflow_preserves_latest_critical_commands() {
        let mut outbox = vec![
            "<TCP_MESSAGE>{\"command\":\"reset_pins\"}</TCP_MESSAGE>".to_string(),
            "<TCP_MESSAGE>{\"command\":\"clear_acmd_captures\"}</TCP_MESSAGE>".to_string(),
        ];
        for id in 0..=OUTBOX_CAP {
            push_outbox_capped(
                &mut outbox,
                format!("<TCP_MESSAGE>{{\"id\":{id},\"newValue\":\"{{}}\"}}</TCP_MESSAGE>"),
            );
        }
        assert_eq!(outbox.len(), OUTBOX_CAP);
        assert!(outbox.iter().any(|message| message.contains("reset_pins")));
        assert!(outbox
            .iter()
            .any(|message| message.contains("clear_acmd_captures")));

        push_outbox_capped(
            &mut outbox,
            "<TCP_MESSAGE>{\"command\":\"reset_pins\"}</TCP_MESSAGE>".to_string(),
        );
        assert_eq!(
            outbox
                .iter()
                .filter(|message| message.contains("reset_pins"))
                .count(),
            1,
            "idempotent commands should coalesce instead of consuming the queue"
        );
    }

    #[test]
    fn reconnect_replays_authoritative_user_tweaks() {
        let link = GameLink::default();
        let mut overrides = LiveOverrides::default();
        overrides.restore_tweak(
            0x1234,
            RpmEffectData {
                effect_name: "sys_test".to_string(),
                speed: 1.5,
                ..Default::default()
            },
        );
        assert_eq!(overrides.flush_all(&link), 1);
        link.shared.lock().unwrap().outbox.clear();

        assert_eq!(overrides.resend_tweaks(&link), 1);
        let outbox = &link.shared.lock().unwrap().outbox;
        assert_eq!(outbox.len(), 1);
        assert!(outbox[0].contains("\\\"speed\\\":1.5"));
    }

    #[test]
    fn failed_flush_requeues_unsent_frames_in_original_order() {
        use std::net::{Shutdown, TcpListener, TcpStream};

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        server.shutdown(Shutdown::Write).unwrap();

        let shared = Arc::new(Mutex::new(Shared::default()));
        let pending = vec![
            "<TCP_MESSAGE>{\"command\":\"first\"}</TCP_MESSAGE>".to_string(),
            "<TCP_MESSAGE>{\"command\":\"second\"}</TCP_MESSAGE>".to_string(),
        ];
        shared.lock().unwrap().outbox = pending.clone();

        let reason = serve_connection(&shared, server);
        assert!(
            reason
                .as_deref()
                .is_some_and(|message| message.starts_with("send:")),
            "a closed write half must report a send failure: {reason:?}"
        );
        assert_eq!(
            shared.lock().unwrap().outbox,
            pending,
            "the failed frame and every later frame must be available for reconnect"
        );
        drop(client);
    }

    #[test]
    fn successful_flush_writes_pending_frames_in_order() {
        use std::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let expected = [
            "<TCP_MESSAGE>{\"command\":\"first\"}</TCP_MESSAGE>",
            "<TCP_MESSAGE>{\"command\":\"second\"}</TCP_MESSAGE>",
        ];
        let wire = expected.concat();
        shared.lock().unwrap().outbox = expected.iter().map(|s| (*s).to_string()).collect();

        let thread_shared = Arc::clone(&shared);
        let worker = std::thread::spawn(move || serve_connection(&thread_shared, server));
        let mut received = vec![0u8; wire.len()];
        client.read_exact(&mut received).unwrap();
        assert_eq!(received, wire.as_bytes());
        assert!(shared.lock().unwrap().outbox.is_empty());

        drop(client);
        assert_eq!(worker.join().unwrap(), Some("closed by plugin".into()));
    }

    #[test]
    fn plugin_address_prefers_full_override_then_port_then_gateway_file() {
        use std::ffi::OsString;
        use std::sync::OnceLock;

        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

        struct EnvGuard {
            values: Vec<(&'static str, Option<OsString>)>,
        }

        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for (name, value) in self.values.drain(..) {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }

        let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _env = EnvGuard {
            values: [
                "VISIONARY_PLUGIN_ADDR",
                "VISIONARY_PLUGIN_PORT",
                "VISIONARY_SD_DIR",
            ]
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect(),
        };
        let sd = tempfile::tempdir().unwrap();
        let gateway = sd.path().join("slight/user/gateway.txt");
        std::fs::create_dir_all(gateway.parent().unwrap()).unwrap();
        std::env::set_var("VISIONARY_SD_DIR", sd.path());

        std::env::set_var("VISIONARY_PLUGIN_ADDR", "[::1]:3210");
        std::env::set_var("VISIONARY_PLUGIN_PORT", "4321");
        assert_eq!(plugin_addr(), "[::1]:3210".parse().unwrap());

        std::env::set_var("VISIONARY_PLUGIN_ADDR", "not-an-address");
        assert_eq!(plugin_addr(), "127.0.0.1:4321".parse().unwrap());

        std::env::remove_var("VISIONARY_PLUGIN_PORT");
        std::fs::write(&gateway, "127.0.0.1:5432\n").unwrap();
        assert_eq!(plugin_addr(), "127.0.0.1:5432".parse().unwrap());

        std::fs::write(&gateway, "127.0.0.1:65536\n").unwrap();
        assert_eq!(plugin_addr(), "127.0.0.1:7878".parse().unwrap());

        std::fs::write(&gateway, "not-a-dotted-quad:5432\n").unwrap();
        assert_eq!(plugin_addr(), "127.0.0.1:7878".parse().unwrap());
    }

    #[test]
    fn pong_updates_liveness_and_silent_stale_connection_requests_reconnect() {
        use std::net::{TcpListener, TcpStream};

        let shared = Arc::new(Mutex::new(Shared::default()));
        let stale = Instant::now() - STALE_TIMEOUT - Duration::from_secs(1);
        shared.lock().unwrap().last_rx = Some(stale);
        let mut pong = plugin_frame("Pong", &serde_json::json!({}));
        for payload in extract_frames(&mut pong) {
            handle_frame(&shared, &payload);
        }
        let refreshed = shared.lock().unwrap().last_rx.unwrap();
        assert!(refreshed > stale);
        assert_eq!(shared.lock().unwrap().frames_rx, 1);

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        shared.lock().unwrap().last_rx = Some(stale);

        let reason = serve_connection(&shared, server);
        assert_eq!(
            reason.as_deref(),
            Some(
                "stale: no frames from the game for 20s (emulator stalled or connection half-open; reconnecting)"
            )
        );
        drop(client);
    }

    #[test]
    fn repeated_plugin_ids_still_create_distinct_connection_epochs() {
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let handshake = || {
            plugin_frame(
                "GiveClientId",
                &serde_json::json!({ "GiveClientId": { "client_id": 1 } }),
            )
        };

        for expected_epoch in [1, 2] {
            let mut frame = handshake();
            for payload in extract_frames(&mut frame) {
                handle_frame(&shared, &payload);
            }
            let state = shared.lock().unwrap();
            assert_eq!(state.client_id, Some(1));
            assert_eq!(state.connection_epoch, expected_epoch);
        }
    }

    #[test]
    fn gimmick_agent_snapshots_are_versioned_ordered_and_connection_scoped() {
        let link = GameLink::default();
        link.shared.lock().unwrap().status = LinkStatus::Connected;
        let deliver = |header: &str, body: serde_json::Value| {
            let mut frame = plugin_frame(header, &body);
            for payload in extract_frames(&mut frame) {
                handle_frame(&link.shared, &payload);
            }
        };
        deliver(
            "GiveClientId",
            serde_json::json!({ "GiveClientId": { "client_id": 7 } }),
        );
        deliver(
            "GimmickAgents",
            serde_json::json!({
                "GimmickAgents": {
                    "version": 1,
                    "seq": 2,
                    "agents": [
                        {"boid": 0, "category": 0, "kind": 0x3E, "entry_id": 0, "owner_boid": null, "fighter": "cloud"},
                        {"boid": 0x10000001u32, "category": 1, "kind": 364, "entry_id": -1, "owner_boid": 0, "fighter": null}
                    ]
                }
            }),
        );
        assert_eq!(link.gimmick_agents().len(), 2);
        assert!(link.has_gimmick_agents_snapshot());
        link.shared.lock().unwrap().gimmick_agents_received =
            Some(Instant::now() - Duration::from_secs(4));
        assert_eq!(link.gimmick_agents().len(), 2);
        link.shared.lock().unwrap().gimmick_agents_received = Some(Instant::now());

        // TCP is ordered, but a sequence guard also refuses replayed snapshots.
        deliver(
            "GimmickAgents",
            serde_json::json!({
                "GimmickAgents": { "version": 1, "seq": 1, "agents": [] }
            }),
        );
        assert_eq!(link.gimmick_agents().len(), 2);
        // Future and malformed snapshots preserve the last complete good list.
        deliver(
            "GimmickAgents",
            serde_json::json!({
                "GimmickAgents": { "version": 2, "seq": 3, "agents": [] }
            }),
        );
        deliver(
            "GimmickAgents",
            serde_json::json!({
                "GimmickAgents": {
                    "version": 1,
                    "seq": 3,
                    "agents": [{"boid": 7, "category": 9, "kind": 29, "entry_id": 0, "owner_boid": null, "fighter": "cloud"}]
                }
            }),
        );
        assert_eq!(link.gimmick_agents().len(), 2);
        assert!(link.gimmick_agents_error().is_some());

        // A bad row cannot strand valid fighter choices. The warning remains
        // available while the usable subset replaces the previous snapshot.
        deliver(
            "GimmickAgents",
            serde_json::json!({
                "GimmickAgents": {
                    "version": 1,
                    "seq": 4,
                    "agents": [
                        {"boid": 0, "category": 0, "kind": 0x3E, "entry_id": 0, "owner_boid": null, "fighter": "cloud"},
                        {"boid": 7, "category": 9, "kind": 0x3E, "entry_id": 1, "owner_boid": null, "fighter": "cloud"}
                    ]
                }
            }),
        );
        assert_eq!(link.gimmick_agents().len(), 1);
        assert_eq!(link.gimmick_agents()[0].boid, 0);
        assert!(link.has_gimmick_agents_snapshot());
        assert!(link.gimmick_agents_error().is_some());

        // A valid empty full replacement removes despawned objects.
        deliver(
            "GimmickAgents",
            serde_json::json!({
                "GimmickAgents": { "version": 1, "seq": 5, "agents": [] }
            }),
        );
        assert!(link.gimmick_agents().is_empty());
        assert!(link.has_gimmick_agents_snapshot());
        assert_eq!(link.gimmick_agents_error(), None);

        // A new plugin session clears even when it reuses its numeric client id.
        deliver(
            "GiveClientId",
            serde_json::json!({ "GiveClientId": { "client_id": 7 } }),
        );
        let state = link.shared.lock().unwrap();
        assert!(state.gimmick_agents.is_empty());
        assert!(state.gimmick_agents_wire_seq.is_none());
        assert_eq!(state.connection_epoch, 2);
        drop(state);
        assert!(!link.has_gimmick_agents_snapshot());
    }

    #[test]
    fn gimmick_meter_snapshots_are_typed_ordered_and_connection_scoped() {
        let link = GameLink::default();
        let deliver = |header: &str, body: serde_json::Value| {
            let mut frame = plugin_frame(header, &body);
            for payload in extract_frames(&mut frame) {
                handle_frame(&link.shared, &payload);
            }
        };
        deliver(
            "GiveClientId",
            serde_json::json!({ "GiveClientId": { "client_id": 7 } }),
        );
        deliver(
            "GimmickMeters",
            serde_json::json!({
                "GimmickMeters": {
                    "version": 1,
                    "seq": 2,
                    "values": [{
                        "binding_id": "fixture.cloud.limit_work",
                        "fighter": "cloud",
                        "entry_id": 0,
                        "boid": 0,
                        "symbol": "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE",
                        "kind": "work_float",
                        "value": { "float": 47.5 }
                    }]
                }
            }),
        );
        let values =
            link.gimmick_meter_values("cloud", "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].boid, 0);
        assert_eq!(values[0].value, LiveGimmickMeterValue::Float(47.5));

        // Replayed and type-mismatched frames preserve the last good value.
        deliver(
            "GimmickMeters",
            serde_json::json!({
                "GimmickMeters": { "version": 1, "seq": 1, "values": [] }
            }),
        );
        deliver(
            "GimmickMeters",
            serde_json::json!({
                "GimmickMeters": {
                    "version": 1,
                    "seq": 3,
                    "values": [{
                        "binding_id": "bad",
                        "fighter": "cloud",
                        "entry_id": 0,
                        "boid": 0,
                        "symbol": "BAD",
                        "kind": "work_float",
                        "value": { "int": 1 }
                    }]
                }
            }),
        );
        assert_eq!(
            link.gimmick_meter_values("cloud", "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE")
                .len(),
            1
        );

        deliver(
            "GiveClientId",
            serde_json::json!({ "GiveClientId": { "client_id": 7 } }),
        );
        assert!(link
            .gimmick_meter_values("cloud", "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE")
            .is_empty());
    }

    #[test]
    fn gimmick_operations_keep_identifiers_symbolic_and_are_locally_typed() {
        let link = GameLink::default();
        let article = "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE";
        let status = "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR";
        link.generate_gimmick_article("cloud", 0x2000_0001, article)
            .unwrap();
        link.remove_gimmick_article("cloud", 0x2000_0001, article)
            .unwrap();
        link.request_gimmick_weapon_status("cloud", 0x2000_0001, 0x1000_0001, status)
            .unwrap();

        let shared = link.shared.lock().unwrap();
        assert_eq!(shared.outbox.len(), 3);
        let values: Vec<serde_json::Value> = shared
            .outbox
            .iter()
            .map(|frame| {
                serde_json::from_str(
                    frame
                        .strip_prefix("<TCP_MESSAGE>")
                        .unwrap()
                        .strip_suffix("</TCP_MESSAGE>")
                        .unwrap(),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(
            values[0]["gimmick_operation"]["operation"],
            "generate_article"
        );
        assert_eq!(
            values[1]["gimmick_operation"]["operation"],
            "remove_article"
        );
        assert_eq!(
            values[2]["gimmick_operation"]["operation"],
            "request_weapon_status"
        );
        assert_eq!(values[2]["gimmick_operation"]["symbol"], status);
        assert!(values[2]["gimmick_operation"].get("value").is_none());
    }

    #[test]
    fn desktop_refuses_numeric_wrong_kind_and_wrong_fighter_operations() {
        let link = GameLink::default();
        assert!(link.generate_gimmick_article("cloud", 1, "0").is_err());
        assert!(link
            .generate_gimmick_article("mario", 1, "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE")
            .is_err());
        assert!(link
            .generate_gimmick_article("cloud", 1, "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR")
            .is_err());
        assert!(link.shared.lock().unwrap().outbox.is_empty());
    }

    #[test]
    fn desktop_accepts_zero_fighter_id_but_refuses_zero_weapon_and_wrong_status_names() {
        let link = GameLink::default();
        let article = "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE";
        let status = "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR";

        link.generate_gimmick_article("cloud", 0, article).unwrap();
        assert_eq!(
            link.request_gimmick_weapon_status("cloud", 1, 0, status)
                .unwrap_err(),
            GimmickOperationError::InvalidWeaponObjectId
        );
        assert!(link
            .request_gimmick_weapon_status("cloud", 1, 2, article)
            .is_err());
        assert!(link
            .request_gimmick_weapon_status("mario", 1, 2, status)
            .is_err());
        assert_eq!(link.shared.lock().unwrap().outbox.len(), 1);
    }

    #[test]
    fn non_idempotent_gimmick_operations_are_never_evicted_by_outbox_pressure() {
        let operation = |id| {
            format!(
                "<TCP_MESSAGE>{{\"gimmick_operation\":{{\"version\":1,\"fighter\":\"cloud\",\"fighter_boid\":{id},\"operation\":\"generate_article\",\"symbol\":\"FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE\"}}}}</TCP_MESSAGE>"
            )
        };
        let link = GameLink::default();
        {
            let mut shared = link.shared.lock().unwrap();
            shared.outbox = (1..=OUTBOX_CAP as u32).map(operation).collect();
        }

        // Ordinary modifier traffic must give way to an already queued action.
        link.send_modifier_edit(7, &RpmEffectData::default(), false);
        {
            let shared = link.shared.lock().unwrap();
            assert_eq!(shared.outbox.len(), OUTBOX_CAP);
            assert!(shared
                .outbox
                .iter()
                .all(|frame| is_gimmick_operation(frame)));
        }

        // An additional action reports a local failure instead of silently
        // displacing the first action in the queue.
        assert_eq!(
            link.generate_gimmick_article(
                "cloud",
                OUTBOX_CAP as u32 + 1,
                "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE"
            )
            .unwrap_err(),
            GimmickOperationError::QueueFull
        );
    }

    #[test]
    fn operation_capacity_counts_the_socket_flush_reservation() {
        let link = GameLink::default();
        let operation = |id| {
            format!(
                "<TCP_MESSAGE>{{\"gimmick_operation\":{{\"version\":1,\"fighter\":\"cloud\",\"fighter_boid\":{id},\"operation\":\"generate_article\",\"symbol\":\"FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE\"}}}}</TCP_MESSAGE>"
            )
        };
        {
            let mut shared = link.shared.lock().unwrap();
            shared.outbox = (1..OUTBOX_CAP as u32).map(operation).collect();
            shared.gimmick_operations_in_flight = 1;
        }
        assert_eq!(
            link.generate_gimmick_article(
                "cloud",
                OUTBOX_CAP as u32,
                "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE"
            )
            .unwrap_err(),
            GimmickOperationError::QueueFull
        );
    }
    fn deliver(link: &GameLink, header: &str, body: serde_json::Value) {
        let mut frame = plugin_frame(header, &body);
        for payload in extract_frames(&mut frame) {
            handle_frame(&link.shared, &payload);
        }
    }

    fn supported_capabilities(seq: u64) -> serde_json::Value {
        serde_json::json!({
            "GimmickControllerCapabilities": {
                "version": 1,
                "seq": seq,
                "request_version": crate::gimmicks::controller_protocol::REQUEST_VERSION,
                "model_version": crate::gimmicks::live_model::MODEL_VERSION,
                "max_charts": 8,
                "max_actors": 128,
                "max_rows": 1024
            }
        })
    }

    fn feature_capabilities(seq: u64, features: Vec<&str>) -> serde_json::Value {
        serde_json::json!({
            "GimmickFeatureCapabilities": {
                "version": 1,
                "seq": seq,
                "features": features,
            }
        })
    }

    fn controller_row(
        boid: u32,
        fighter: &str,
        entry_id: i32,
        chart_id: &str,
        program_id: &str,
        stock_generation: u32,
        vars: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "fighter": fighter,
            "boid": boid,
            "entry_id": entry_id,
            "revision": 4,
            "state": "waiting",
            "vars": vars,
            "chart_id": chart_id,
            "program_id": program_id,
            "stock_generation": stock_generation
        })
    }

    fn controller_state(seq: u64, behavior: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({
            "GimmickLiveState": {
                "version": 1,
                "seq": seq,
                "behavior_revision": 4,
                "parameter_revision": 5,
                "behavior": behavior,
                "parameters": []
            }
        })
    }

    fn connected_test_link() -> GameLink {
        let link = GameLink::offline_for_test();
        {
            let mut shared = link.shared.lock().unwrap();
            shared.status = LinkStatus::Connected;
            shared.client_id = Some(1);
        }
        link
    }

    fn accept_behavior_revision(link: &GameLink, revision: u64) {
        let mut shared = link.shared.lock().unwrap();
        shared
            .gimmick_live_expected
            .insert("behavior".into(), revision);
        shared.gimmick_live_acks.insert(
            "behavior".into(),
            GimmickLiveAck {
                version: 1,
                family: "behavior".into(),
                revision,
                accepted: true,
                message: "accepted".into(),
            },
        );
    }

    fn valid_controller_request(revision: u64) -> serde_json::Value {
        let program = crate::gimmicks::live_protocol::program_from_graph(
            "mario",
            &crate::gimmicks::templates::Preset::Timer.graph(),
        )
        .unwrap();
        let request = crate::gimmicks::controller_protocol::ControllerBundleRequest {
            version: crate::gimmicks::controller_protocol::REQUEST_VERSION,
            revision,
            disabled_fighters: Vec::new(),
            charts: vec![crate::gimmicks::controller_protocol::ControllerProgram {
                chart_id: "main".into(),
                program,
            }],
        };
        crate::gimmicks::controller_protocol::encode_request(&request).unwrap();
        serde_json::to_value(request).unwrap()
    }

    fn valid_attack_result_controller_request(revision: u64) -> serde_json::Value {
        use crate::gimmicks::behavior::Predicate;
        use crate::gimmicks::native_inputs::{AttackResultMask, AttackResultQuery, NativeInput};
        let input = NativeInput::AttackResultFlag {
            fighter: "mario".into(),
            query: AttackResultQuery::DuringHitlag,
            mask: AttackResultMask::Hit,
        };
        let mut graph = crate::gimmicks::templates::Preset::Timer.graph();
        graph.transitions[0].guard = Predicate::NativeFlag(input.clone());
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let request = crate::gimmicks::controller_protocol::ControllerBundleRequest {
            version: crate::gimmicks::controller_protocol::REQUEST_VERSION,
            revision,
            disabled_fighters: Vec::new(),
            charts: vec![crate::gimmicks::controller_protocol::ControllerProgram {
                chart_id: "main".into(),
                program,
            }],
        };
        serde_json::to_value(request).unwrap()
    }

    fn valid_mixed_native_controller_request(revision: u64) -> serde_json::Value {
        use crate::gimmicks::behavior::Predicate;
        use crate::gimmicks::native_inputs::{AttackResultMask, AttackResultQuery, NativeInput};
        let attack = NativeInput::AttackResultFlag {
            fighter: "mario".into(),
            query: AttackResultQuery::DuringHitlag,
            mask: AttackResultMask::Hit,
        };
        let article = NativeInput::ArticleExists {
            fighter: "mario".into(),
            symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
        };
        let mut graph = crate::gimmicks::templates::Preset::Timer.graph();
        graph.transitions[0].guard = Predicate::And(
            Box::new(Predicate::NativeFlag(attack)),
            Box::new(Predicate::NativeFlag(article)),
        );
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let request = crate::gimmicks::controller_protocol::ControllerBundleRequest {
            version: crate::gimmicks::controller_protocol::REQUEST_VERSION,
            revision,
            disabled_fighters: Vec::new(),
            charts: vec![crate::gimmicks::controller_protocol::ControllerProgram {
                chart_id: "main".into(),
                program,
            }],
        };
        crate::gimmicks::controller_protocol::encode_request(&request).unwrap();
        serde_json::to_value(request).unwrap()
    }

    #[test]
    fn fighter_damage_feature_gates_main_and_controller_requests_before_queueing() {
        use crate::gimmicks::{
            behavior::{CmpOp, ConstValue, Expr, Predicate},
            native_inputs::{FighterValue, NativeInput, FIGHTER_DAMAGE_FEATURE},
        };
        let mut graph = crate::gimmicks::templates::Preset::Timer.graph();
        graph.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Gte,
            left: Expr::NativeFloat(NativeInput::Fighter {
                value: FighterValue::Damage,
                fighter: "mario".into(),
            }),
            right: Expr::Const(ConstValue::Float(0.0)),
        };
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", &graph).unwrap();
        let legacy = serde_json::to_value(crate::gimmicks::live_protocol::LiveRequest {
            version: crate::gimmicks::live_model::MODEL_VERSION,
            revision: 30,
            disabled_fighters: Vec::new(),
            programs: vec![program.clone()],
        })
        .unwrap();
        let bundle = serde_json::to_value(
            crate::gimmicks::controller_protocol::ControllerBundleRequest {
                version: crate::gimmicks::controller_protocol::REQUEST_VERSION,
                revision: 30,
                disabled_fighters: Vec::new(),
                charts: vec![crate::gimmicks::controller_protocol::ControllerProgram {
                    chart_id: "main".into(),
                    program,
                }],
            },
        )
        .unwrap();
        for request in [legacy, bundle] {
            let link = connected_test_link();
            deliver(
                &link,
                "GimmickControllerCapabilities",
                supported_capabilities(1),
            );
            let before = {
                let shared = link.shared.lock().unwrap();
                (
                    shared.outbox.clone(),
                    shared.gimmick_live_expected.clone(),
                    shared.edits_tx,
                )
            };
            for (seq, features) in [
                (1, vec![]),
                (
                    2,
                    vec![crate::gimmicks::native_inputs::ARTICLE_PRESENCE_FEATURE],
                ),
            ] {
                deliver(
                    &link,
                    "GimmickFeatureCapabilities",
                    feature_capabilities(seq, features),
                );
                assert!(link
                    .send_gimmick_live("behavior", request.clone())
                    .unwrap_err()
                    .contains("does not advertise"));
                let shared = link.shared.lock().unwrap();
                assert_eq!(
                    (
                        shared.outbox.clone(),
                        shared.gimmick_live_expected.clone(),
                        shared.edits_tx
                    ),
                    before
                );
            }
            deliver(
                &link,
                "GimmickFeatureCapabilities",
                feature_capabilities(3, vec![FIGHTER_DAMAGE_FEATURE]),
            );
            assert!(link.send_gimmick_live("behavior", request.clone()).is_ok());
            deliver(
                &link,
                "GiveClientId",
                serde_json::json!({"GiveClientId":{"client_id":2}}),
            );
            assert!(link.send_gimmick_live("behavior", request).is_err());
        }
    }

    #[test]
    fn attack_result_bundle_requires_advertised_feature_before_queueing() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let before = {
            let shared = link.shared.lock().unwrap();
            (
                shared.outbox.clone(),
                shared.gimmick_live_expected.clone(),
                shared.edits_tx,
            )
        };
        assert!(link
            .send_gimmick_live("behavior", valid_attack_result_controller_request(8))
            .unwrap_err()
            .contains("does not advertise"));
        {
            let shared = link.shared.lock().unwrap();
            assert_eq!(
                (
                    shared.outbox.clone(),
                    shared.gimmick_live_expected.clone(),
                    shared.edits_tx
                ),
                before
            );
        }
        deliver(
            &link,
            "GimmickFeatureCapabilities",
            feature_capabilities(
                1,
                vec![crate::gimmicks::native_inputs::ATTACK_RESULT_FLAGS_FEATURE],
            ),
        );
        assert!(link
            .send_gimmick_live("behavior", valid_attack_result_controller_request(9))
            .is_ok());
    }

    #[test]
    fn optional_feature_frame_is_independent_of_strict_controller_telemetry() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        assert!(link.gimmick_controller_capabilities().is_some());
        deliver(
            &link,
            "GimmickFeatureCapabilities",
            feature_capabilities(1, vec!["unknown.future.feature"]),
        );
        assert!(link.gimmick_controller_capabilities().is_some());
        assert!(link
            .send_gimmick_live("behavior", valid_controller_request(8))
            .is_ok());
        assert!(link
            .send_gimmick_live("behavior", valid_attack_result_controller_request(9))
            .unwrap_err()
            .contains("does not advertise"));
    }

    #[test]
    fn attack_feature_gate_also_covers_legacy_main_and_clears_on_bad_frames_and_new_client() {
        let link = connected_test_link();
        let bundle = valid_attack_result_controller_request(20);
        let legacy = serde_json::json!({"version": 4, "revision": 20, "programs": [bundle["charts"][0]["program"].clone()]});
        assert!(link.send_gimmick_live("behavior", legacy.clone()).is_err());
        assert!(link.shared.lock().unwrap().outbox.is_empty());
        let feature = crate::gimmicks::native_inputs::ATTACK_RESULT_FLAGS_FEATURE;
        deliver(
            &link,
            "GimmickFeatureCapabilities",
            feature_capabilities(1, vec![feature]),
        );
        assert!(link.send_gimmick_live("behavior", legacy.clone()).is_ok());
        for seq in [1, 2] {
            let mut invalid = feature_capabilities(seq, vec![feature]);
            invalid["GimmickFeatureCapabilities"]["unexpected"] = true.into();
            deliver(&link, "GimmickFeatureCapabilities", invalid);
            assert!(link.send_gimmick_live("behavior", legacy.clone()).is_err());
            deliver(
                &link,
                "GimmickFeatureCapabilities",
                feature_capabilities(seq, vec![feature]),
            );
            assert!(
                link.send_gimmick_live("behavior", legacy.clone()).is_err(),
                "same-sequence repairs cannot restore authority"
            );
        }
        deliver(
            &link,
            "GimmickFeatureCapabilities",
            feature_capabilities(3, vec![feature]),
        );
        assert!(link.send_gimmick_live("behavior", legacy.clone()).is_ok());
        deliver(
            &link,
            "GiveClientId",
            serde_json::json!({"GiveClientId":{"client_id":2}}),
        );
        assert!(link.send_gimmick_live("behavior", legacy).is_err());
        assert!(link
            .shared
            .lock()
            .unwrap()
            .gimmick_feature_capabilities
            .is_none());
    }

    #[test]
    fn contact_status_feature_is_required_for_v4_and_v5_even_without_sampled_native_inputs() {
        use crate::gimmicks::native_inputs::{
            DAMAGE_CAPTURE_STATUS_FEATURE, FIGHTER_STATUS_IS_FEATURE,
        };
        let program = serde_json::json!({
            "version":4,"id":"contact_status","fighter":"cloud","initial":"idle","states":["idle"],
            "vars":[{"name":"charging","kind":"flag","initial":{"Flag":false}}],
            "native_inputs":[],
            "transitions":[{"from":"idle","to":"idle","on_frame":true,"guard":{"Compare":{"op":"eq","left":{"Const":{"Int":0}},"right":{"Const":{"Int":0}}}},"actions":[{"SetFlag":{"var":"charging","value":{"DamageStatusIs":"status_is:cloud:FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE"}}}]}],
            "bindings":[{"hook":"onTakeDamage","transitions":[0]}],"event_bindings":[],"status_triggers":[],"resource_triggers":[]
        });
        let legacy = serde_json::json!({"version":4,"revision":71,"programs":[program.clone()]});
        let bundle = serde_json::json!({"version":5,"revision":71,"charts":[{"chart_id":"main","program":program}]});
        for request in [legacy, bundle] {
            let link = connected_test_link();
            deliver(
                &link,
                "GimmickControllerCapabilities",
                supported_capabilities(1),
            );
            let before = {
                let shared = link.shared.lock().unwrap();
                (
                    shared.outbox.clone(),
                    shared.gimmick_live_expected.clone(),
                    shared.edits_tx,
                )
            };
            assert!(link.send_gimmick_live("behavior", request.clone()).is_err());
            deliver(
                &link,
                "GimmickFeatureCapabilities",
                feature_capabilities(1, vec![FIGHTER_STATUS_IS_FEATURE]),
            );
            assert!(link
                .send_gimmick_live("behavior", request.clone())
                .unwrap_err()
                .contains("every required"));
            {
                let shared = link.shared.lock().unwrap();
                assert_eq!(
                    (
                        shared.outbox.clone(),
                        shared.gimmick_live_expected.clone(),
                        shared.edits_tx
                    ),
                    before
                );
            }
            deliver(
                &link,
                "GimmickFeatureCapabilities",
                feature_capabilities(2, vec![DAMAGE_CAPTURE_STATUS_FEATURE]),
            );
            assert!(link.send_gimmick_live("behavior", request).is_ok());
        }
    }

    #[test]
    fn mixed_native_controller_requires_both_advertised_features() {
        use crate::gimmicks::native_inputs::{
            ARTICLE_PRESENCE_FEATURE, ATTACK_RESULT_FLAGS_FEATURE,
        };
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let mixed = valid_mixed_native_controller_request(40);
        let before = {
            let shared = link.shared.lock().unwrap();
            (
                shared.outbox.clone(),
                shared.gimmick_live_expected.clone(),
                shared.gimmick_live_acks.clone(),
                shared.edits_tx,
            )
        };
        assert!(link.send_gimmick_live("behavior", mixed.clone()).is_err());
        deliver(
            &link,
            "GimmickFeatureCapabilities",
            feature_capabilities(1, vec![ARTICLE_PRESENCE_FEATURE]),
        );
        assert!(link
            .send_gimmick_live("behavior", mixed.clone())
            .unwrap_err()
            .contains("every required"));
        {
            let shared = link.shared.lock().unwrap();
            assert_eq!(
                (
                    shared.outbox.clone(),
                    shared.gimmick_live_expected.clone(),
                    shared.gimmick_live_acks.clone(),
                    shared.edits_tx,
                ),
                before
            );
        }
        deliver(
            &link,
            "GimmickFeatureCapabilities",
            feature_capabilities(
                2,
                vec![ARTICLE_PRESENCE_FEATURE, ATTACK_RESULT_FLAGS_FEATURE],
            ),
        );
        assert!(link.send_gimmick_live("behavior", mixed).is_ok());
        let legacy_main = serde_json::json!({
            "version": 4,
            "revision": 41,
            "programs": [valid_mixed_native_controller_request(41)["charts"][0]["program"].clone()]
        });
        deliver(
            &link,
            "GimmickFeatureCapabilities",
            feature_capabilities(3, vec![ARTICLE_PRESENCE_FEATURE]),
        );
        assert!(link
            .send_gimmick_live("behavior", legacy_main.clone())
            .is_err());
        deliver(
            &link,
            "GimmickFeatureCapabilities",
            feature_capabilities(
                4,
                vec![ARTICLE_PRESENCE_FEATURE, ATTACK_RESULT_FLAGS_FEATURE],
            ),
        );
        assert!(link.send_gimmick_live("behavior", legacy_main).is_ok());
    }

    #[test]
    fn controller_sends_require_current_capability_before_queue_or_ack_mutation() {
        for link in [GameLink::offline_for_test(), connected_test_link()] {
            let before = {
                let shared = link.shared.lock().unwrap();
                (
                    shared.outbox.clone(),
                    shared.gimmick_live_expected.clone(),
                    shared.edits_tx,
                )
            };
            assert!(link
                .send_gimmick_live("behavior", valid_controller_request(7))
                .is_err());
            let shared = link.shared.lock().unwrap();
            assert_eq!(
                (
                    shared.outbox.clone(),
                    shared.gimmick_live_expected.clone(),
                    shared.edits_tx
                ),
                before
            );
        }
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        link.send_gimmick_live("behavior", valid_controller_request(7))
            .unwrap();
        assert_eq!(
            link.shared.lock().unwrap().gimmick_live_expected["behavior"],
            7
        );
        assert_eq!(link.shared.lock().unwrap().outbox.len(), 1);
        let before = link.shared.lock().unwrap().outbox.clone();
        let mut invalid = valid_controller_request(8);
        invalid["charts"][0]["chart_id"] = " ".into();
        assert!(link.send_gimmick_live("behavior", invalid).is_err());
        assert_eq!(link.shared.lock().unwrap().outbox, before);
        assert_eq!(
            link.shared.lock().unwrap().gimmick_live_expected["behavior"],
            7
        );
    }

    #[test]
    fn controller_bundles_do_not_replay_on_a_new_client_epoch() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        link.send_gimmick_live("behavior", valid_controller_request(7))
            .unwrap();
        deliver(
            &link,
            "GiveClientId",
            serde_json::json!({"GiveClientId":{"client_id":2}}),
        );
        assert!(link.shared.lock().unwrap().outbox.is_empty());
        assert!(link.gimmick_controller_capabilities().is_none());
        assert!(link
            .send_gimmick_live("behavior", valid_controller_request(8))
            .is_err());
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        link.send_gimmick_live("behavior", valid_controller_request(9))
            .unwrap();
        assert_eq!(link.shared.lock().unwrap().outbox.len(), 1);
    }

    #[test]
    fn reconnect_filter_keeps_legacy_snapshots_and_parameters_not_chart_bundles_or_restarts() {
        let legacy = r#"<TCP_MESSAGE>{"gimmick_behavior":{"version":4,"revision":1,"programs":[]}}</TCP_MESSAGE>"#.to_owned();
        let params = r#"<TCP_MESSAGE>{"gimmick_parameters":{"version":1,"revision":2,"edits":[]}}</TCP_MESSAGE>"#.to_owned();
        let chart = format!(
            "<TCP_MESSAGE>{{\"gimmick_behavior\":{}}}</TCP_MESSAGE>",
            valid_controller_request(3)
        );
        let restart = r#"<TCP_MESSAGE>{"gimmick_restart":{"fighter":"mario","boid":1,"expected_revision":3}}</TCP_MESSAGE>"#.to_owned();
        let mut messages = vec![legacy.clone(), params.clone(), chart, restart];
        discard_session_bound_messages(&mut messages);
        assert_eq!(messages, [legacy, params]);
    }

    #[test]
    fn legacy_rows_remain_valid_without_optional_controller_capabilities() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickLiveState",
            serde_json::json!({"GimmickLiveState": {
                "version": 1, "seq": 1, "behavior_revision": 4,
                "parameter_revision": 5,
                "behavior": [{"fighter":"mario","boid":10,"entry_id":0,
                    "revision":4,"state":"waiting","vars":{}}],
                "parameters": []
            }}),
        );
        assert_eq!(link.gimmick_live_snapshot().unwrap().behavior.len(), 1);
        assert!(link.gimmick_controller_capabilities().is_none());
    }

    #[test]
    fn extended_rows_require_a_valid_capability_frame_first() {
        let link = connected_test_link();
        let row = controller_row(
            10,
            "mario",
            0,
            "main",
            "live:mario",
            2,
            serde_json::json!({}),
        );
        deliver(&link, "GimmickLiveState", controller_state(1, vec![row]));
        assert!(link.gimmick_controller_snapshot().is_none());

        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let row = controller_row(
            10,
            "mario",
            0,
            "main",
            "live:mario",
            2,
            serde_json::json!({}),
        );
        deliver(&link, "GimmickLiveState", controller_state(2, vec![row]));
        assert_eq!(
            link.gimmick_controller_snapshot().unwrap().behavior.len(),
            1
        );
    }

    #[test]
    fn malformed_capabilities_recover_only_with_a_valid_newer_sequence() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            serde_json::json!({}),
        );
        assert!(link.gimmick_controller_capabilities().is_none());
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        assert_eq!(link.gimmick_controller_capabilities().unwrap().seq, 1);

        let mut missing_sequence = supported_capabilities(2);
        missing_sequence["GimmickControllerCapabilities"]
            .as_object_mut()
            .unwrap()
            .remove("seq");
        deliver(&link, "GimmickControllerCapabilities", missing_sequence);
        assert!(link.gimmick_controller_capabilities().is_none());
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        assert!(link.gimmick_controller_capabilities().is_none());
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(2),
        );
        assert_eq!(link.gimmick_controller_capabilities().unwrap().seq, 2);

        let mut invalid = supported_capabilities(3);
        invalid["GimmickControllerCapabilities"]["unexpected"] = true.into();
        deliver(&link, "GimmickControllerCapabilities", invalid);
        assert!(link.gimmick_controller_capabilities().is_none());
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(3),
        );
        assert!(link.gimmick_controller_capabilities().is_none());
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(4),
        );
        assert_eq!(link.gimmick_controller_capabilities().unwrap().seq, 4);
    }

    #[test]
    fn invalid_and_out_of_order_capability_frames_never_grant_chart_rows() {
        let link = connected_test_link();
        let mut invalid = supported_capabilities(1);
        invalid["GimmickControllerCapabilities"]["request_version"] = 4.into();
        deliver(&link, "GimmickControllerCapabilities", invalid);
        assert!(link.gimmick_controller_capabilities().is_none());

        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(2),
        );
        assert_eq!(link.gimmick_controller_capabilities().unwrap().seq, 2);
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        assert!(link.gimmick_controller_capabilities().is_none());
        let row = controller_row(
            10,
            "mario",
            0,
            "secondary",
            "chart:0:secondary",
            2,
            serde_json::json!({}),
        );
        deliver(&link, "GimmickLiveState", controller_state(1, vec![row]));
        assert!(link.gimmick_controller_snapshot().is_none());
    }

    #[test]
    fn v5_snapshot_accepts_two_charts_for_one_boid_and_legacy_getter_projects_main_only() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let rows = vec![
            controller_row(
                10,
                "mario",
                0,
                "main",
                "live:mario",
                2,
                serde_json::json!({}),
            ),
            controller_row(
                10,
                "mario",
                0,
                "secondary",
                "chart:0:secondary",
                2,
                serde_json::json!({}),
            ),
        ];
        deliver(&link, "GimmickLiveState", controller_state(1, rows));

        assert_eq!(
            link.gimmick_controller_snapshot().unwrap().behavior.len(),
            2
        );
        let legacy = link.gimmick_live_snapshot().unwrap();
        assert_eq!(legacy.behavior.len(), 1);
        assert!(legacy.behavior[0].get("chart_id").is_none());
        assert!(legacy.behavior[0].get("program_id").is_none());
        assert!(legacy.behavior[0].get("stock_generation").is_none());
    }

    #[test]
    fn v5_snapshot_rejects_duplicate_actor_chart_rows() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let row = controller_row(
            10,
            "mario",
            0,
            "main",
            "live:mario",
            2,
            serde_json::json!({}),
        );
        deliver(
            &link,
            "GimmickLiveState",
            controller_state(1, vec![row.clone(), row]),
        );
        assert!(link.gimmick_controller_snapshot().is_none());
    }

    #[test]
    fn v5_snapshot_rejects_inconsistent_actor_fighter_entry_or_generation() {
        for second in [
            controller_row(
                10,
                "cloud",
                0,
                "secondary",
                "chart:0:secondary",
                2,
                serde_json::json!({}),
            ),
            controller_row(
                10,
                "mario",
                1,
                "secondary",
                "chart:0:secondary",
                2,
                serde_json::json!({}),
            ),
            controller_row(
                10,
                "mario",
                0,
                "secondary",
                "chart:0:secondary",
                3,
                serde_json::json!({}),
            ),
        ] {
            let link = connected_test_link();
            deliver(
                &link,
                "GimmickControllerCapabilities",
                supported_capabilities(1),
            );
            let first = controller_row(
                10,
                "mario",
                0,
                "main",
                "live:mario",
                2,
                serde_json::json!({}),
            );
            deliver(
                &link,
                "GimmickLiveState",
                controller_state(1, vec![first, second]),
            );
            assert!(link.gimmick_controller_snapshot().is_none());
        }
    }

    #[test]
    fn v5_snapshot_rejects_program_id_changes_for_one_fighter_chart() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let first = controller_row(
            10,
            "mario",
            0,
            "secondary",
            "chart:0:secondary",
            2,
            serde_json::json!({}),
        );
        let second = controller_row(
            11,
            "mario",
            1,
            "secondary",
            "different-program",
            7,
            serde_json::json!({}),
        );
        deliver(
            &link,
            "GimmickLiveState",
            controller_state(1, vec![first, second]),
        );
        assert!(link.gimmick_controller_snapshot().is_none());
    }

    #[test]
    fn v5_snapshot_checks_chart_program_and_stock_row_identity_fields() {
        let invalid_rows = [
            controller_row(10, "mario", 0, " ", "live:mario", 2, serde_json::json!({})),
            controller_row(
                10,
                "mario",
                0,
                &"x".repeat(49),
                "live:mario",
                2,
                serde_json::json!({}),
            ),
            controller_row(10, "mario", 0, "main", " ", 2, serde_json::json!({})),
            controller_row(
                10,
                "mario",
                0,
                "main",
                &"x".repeat(65),
                2,
                serde_json::json!({}),
            ),
        ];
        for row in invalid_rows {
            let link = connected_test_link();
            deliver(
                &link,
                "GimmickControllerCapabilities",
                supported_capabilities(1),
            );
            deliver(&link, "GimmickLiveState", controller_state(1, vec![row]));
            assert!(link.gimmick_controller_snapshot().is_none());
        }
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let mut missing_generation = controller_row(
            10,
            "mario",
            0,
            "main",
            "live:mario",
            2,
            serde_json::json!({}),
        );
        missing_generation
            .as_object_mut()
            .unwrap()
            .remove("stock_generation");
        deliver(
            &link,
            "GimmickLiveState",
            controller_state(1, vec![missing_generation]),
        );
        assert!(link.gimmick_controller_snapshot().is_none());
    }

    #[test]
    fn v5_snapshot_enforces_actor_chart_row_and_variable_bounds() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let eight = (0..8)
            .map(|index| {
                controller_row(
                    10,
                    "mario",
                    0,
                    &format!("c{index}"),
                    &format!("p{index}"),
                    2,
                    serde_json::json!({}),
                )
            })
            .collect::<Vec<_>>();
        deliver(&link, "GimmickLiveState", controller_state(1, eight));
        assert_eq!(
            link.gimmick_controller_snapshot().unwrap().behavior.len(),
            8
        );

        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let nine = (0..9)
            .map(|index| {
                controller_row(
                    10,
                    "mario",
                    0,
                    &format!("c{index}"),
                    &format!("p{index}"),
                    2,
                    serde_json::json!({}),
                )
            })
            .collect::<Vec<_>>();
        deliver(&link, "GimmickLiveState", controller_state(1, nine));
        assert!(link.gimmick_controller_snapshot().is_none());

        let too_many_vars = serde_json::Value::Object(
            (0..65)
                .map(|index| (format!("v{index}"), serde_json::json!({"Int": index})))
                .collect(),
        );
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let row = controller_row(10, "mario", 0, "main", "live:mario", 2, too_many_vars);
        deliver(&link, "GimmickLiveState", controller_state(1, vec![row]));
        assert!(link.gimmick_controller_snapshot().is_none());

        let vars = serde_json::Value::Object(
            (0..64)
                .map(|index| (format!("v{index}"), serde_json::json!({"Int": index})))
                .collect(),
        );
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let rows = (0..3)
            .map(|index| {
                controller_row(
                    10,
                    "mario",
                    0,
                    &format!("c{index}"),
                    &format!("p{index}"),
                    2,
                    vars.clone(),
                )
            })
            .collect::<Vec<_>>();
        deliver(&link, "GimmickLiveState", controller_state(1, rows));
        assert!(link.gimmick_controller_snapshot().is_none());

        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let actors = (0..129)
            .map(|boid| {
                controller_row(
                    boid,
                    "mario",
                    0,
                    "main",
                    "live:mario",
                    2,
                    serde_json::json!({}),
                )
            })
            .collect::<Vec<_>>();
        deliver(&link, "GimmickLiveState", controller_state(1, actors));
        assert!(link.gimmick_controller_snapshot().is_none());
    }

    #[test]
    fn controller_capability_and_snapshot_clear_on_connection_epoch_change() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let row = controller_row(
            10,
            "mario",
            0,
            "main",
            "live:mario",
            2,
            serde_json::json!({}),
        );
        deliver(&link, "GimmickLiveState", controller_state(1, vec![row]));
        assert!(link.gimmick_controller_capabilities().is_some());
        assert!(link.gimmick_controller_snapshot().is_some());

        deliver(
            &link,
            "GiveClientId",
            serde_json::json!({"GiveClientId":{"client_id":2}}),
        );
        assert!(link.gimmick_controller_capabilities().is_none());
        assert!(link.gimmick_controller_snapshot().is_none());
        let row = controller_row(
            10,
            "mario",
            0,
            "secondary",
            "chart:0:secondary",
            2,
            serde_json::json!({}),
        );
        deliver(&link, "GimmickLiveState", controller_state(2, vec![row]));
        assert!(link.gimmick_controller_snapshot().is_none());
    }

    #[test]
    fn v5_row_limit_accepts_exact_aggregate_bound_and_rejects_more() {
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        let rows = (0..128)
            .flat_map(|actor| {
                (0..8).map(move |chart| {
                    controller_row(
                        actor + 1,
                        "mario",
                        0,
                        &format!("c{chart}"),
                        &format!("p{chart}"),
                        2,
                        serde_json::json!({}),
                    )
                })
            })
            .collect::<Vec<_>>();
        deliver(&link, "GimmickLiveState", controller_state(1, rows.clone()));
        assert_eq!(
            link.gimmick_controller_snapshot().unwrap().behavior.len(),
            1024
        );

        // The contract product (128 actors × 8 charts) already equals 1024,
        // so a 1025-row fixture necessarily also crosses an actor/chart bound.
        // The packet-level row ceiling is checked before per-row constraints.
        let mut too_many = rows;
        too_many.push(controller_row(
            129,
            "mario",
            0,
            "main",
            "live:mario",
            2,
            serde_json::json!({}),
        ));
        let link = connected_test_link();
        deliver(
            &link,
            "GimmickControllerCapabilities",
            supported_capabilities(1),
        );
        deliver(&link, "GimmickLiveState", controller_state(1, too_many));
        assert!(link.gimmick_controller_snapshot().is_none());
    }
}
