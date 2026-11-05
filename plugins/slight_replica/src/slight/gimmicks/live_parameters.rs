//! Bounded full-replacement live personal-parameter overlays.
//!
//! The getter hooks always call the game's original first. A matching override
//! is returned only for an active fighter whose accessor, category, and kind
//! still match the once-per-frame identity snapshot. Hook-side locks are
//! try-lock only: contention simply returns the native value.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::LazyLock;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use super::native_inputs::MovementStat;
use crate::slight::agents::AgentRecord;
use crate::slight::gimmicks::live_params::{
    classify_engine_path, EngineParamPath, LiveParamValue, ParameterGetterBackend, Request,
    UniqueAddressMap, MAX_REQUEST_BYTES,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    gimmick_parameters: Request,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    pub version: u32,
    pub revision: u64,
    pub accepted: bool,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireRow {
    pub fighter: String,
    pub path: String,
    pub hits: u64,
    pub baseline: Option<LiveParamValue>,
    pub last_observed: Option<LiveParamValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireStatus {
    pub version: u32,
    pub revision: u64,
    pub enabled: bool,
    pub rows: Vec<WireRow>,
}

pub type WireHealth = crate::slight::gimmicks::live_params::ParameterHealth;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ParamKey {
    fighter_kind: i32,
    parent: u64,
    child: u64,
}

#[derive(Clone, Copy, Debug)]
struct Identity {
    boid: u32,
    accessor: usize,
    kind: i32,
}

#[derive(Default)]
struct IdentitySnapshot {
    identities: HashMap<usize, Identity>,
    param_data_owners: UniqueAddressMap,
    work_modules: UniqueAddressMap,
}

impl IdentitySnapshot {
    fn clear(&mut self) {
        self.identities.clear();
        self.param_data_owners.clear();
        self.work_modules.clear();
    }
}

#[derive(Clone, Debug)]
struct Entry {
    fighter: String,
    path: String,
    value: LiveParamValue,
    hits: u64,
    baseline: Option<LiveParamValue>,
    last_observed: Option<LiveParamValue>,
}

#[derive(Default)]
struct Overlay {
    revision: u64,
    enabled: bool,
    entries: HashMap<ParamKey, Entry>,
}

#[derive(Clone, Copy)]
struct StatScale {
    boid: u32,
    stock_generation: u32,
    multipliers: [f32; MovementStat::ALL.len()],
}

static STATS_ACTIVE: AtomicBool = AtomicBool::new(false);
static STAT_SCALES: RwLock<Vec<StatScale>> = RwLock::new(Vec::new());
static STAT_READS: [AtomicU64; MovementStat::ALL.len()] =
    [const { AtomicU64::new(0) }; MovementStat::ALL.len()];

struct NativeTargetRetry {
    attempts: u8,
    next_tick: u64,
}

struct NativeTargetFailure {
    stage: &'static str,
    detail: String,
}

struct GetterHealth {
    float_wrapper_calls: AtomicU64,
    int_wrapper_calls: AtomicU64,
    identity_matches: AtomicU64,
    identity_misses: AtomicU64,
    identity_lock_busy: AtomicU64,
    overlay_lock_busy: AtomicU64,
    overlay_disabled: AtomicU64,
    parameter_key_matches: AtomicU64,
    parameter_key_misses: AtomicU64,
    type_mismatches: AtomicU64,
    overrides_returned: AtomicU64,
}

impl GetterHealth {
    const fn new() -> Self {
        Self {
            float_wrapper_calls: AtomicU64::new(0),
            int_wrapper_calls: AtomicU64::new(0),
            identity_matches: AtomicU64::new(0),
            identity_misses: AtomicU64::new(0),
            identity_lock_busy: AtomicU64::new(0),
            overlay_lock_busy: AtomicU64::new(0),
            overlay_disabled: AtomicU64::new(0),
            parameter_key_matches: AtomicU64::new(0),
            parameter_key_misses: AtomicU64::new(0),
            type_mismatches: AtomicU64::new(0),
            overrides_returned: AtomicU64::new(0),
        }
    }

    fn reset(&self) {
        for counter in [
            &self.float_wrapper_calls,
            &self.int_wrapper_calls,
            &self.identity_matches,
            &self.identity_misses,
            &self.identity_lock_busy,
            &self.overlay_lock_busy,
            &self.overlay_disabled,
            &self.parameter_key_matches,
            &self.parameter_key_misses,
            &self.type_mismatches,
            &self.overrides_returned,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
    }

    fn snapshot(&self, revision: u64) -> WireHealth {
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        WireHealth {
            revision,
            getter_backend: current_getter_backend(),
            float_wrapper_calls: read(&self.float_wrapper_calls),
            int_wrapper_calls: read(&self.int_wrapper_calls),
            identity_matches: read(&self.identity_matches),
            identity_misses: read(&self.identity_misses),
            identity_lock_busy: read(&self.identity_lock_busy),
            overlay_lock_busy: read(&self.overlay_lock_busy),
            overlay_disabled: read(&self.overlay_disabled),
            parameter_key_matches: read(&self.parameter_key_matches),
            parameter_key_misses: read(&self.parameter_key_misses),
            type_mismatches: read(&self.type_mismatches),
            overrides_returned: read(&self.overrides_returned),
        }
    }
}

fn current_getter_backend() -> Option<ParameterGetterBackend> {
    match GETTER_BACKEND.load(Ordering::Acquire) {
        1 => Some(ParameterGetterBackend::NativeParamData),
        2 => Some(ParameterGetterBackend::WorkModuleExport),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum IdentityLookupError {
    Missing,
    LockBusy,
}

static INSTALLED: AtomicBool = AtomicBool::new(false);
static CURRENT_REVISION: AtomicU64 = AtomicU64::new(0);
static IDENTITY_REFRESH_TICK: AtomicU64 = AtomicU64::new(0);
static GETTER_BACKEND: AtomicU8 = AtomicU8::new(0);
static GETTER_HEALTH: GetterHealth = GetterHealth::new();
static NATIVE_LAYOUT_CHECKED: AtomicBool = AtomicBool::new(false);
static NATIVE_LAYOUT_SUPPORTED: AtomicBool = AtomicBool::new(false);
static IDENTITIES: LazyLock<RwLock<IdentitySnapshot>> =
    LazyLock::new(|| RwLock::new(IdentitySnapshot::default()));
static NATIVE_TARGET_RETRIES: LazyLock<Mutex<HashMap<i32, NativeTargetRetry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NATIVE_TARGET_SAMPLES: LazyLock<Mutex<HashSet<(i32, usize, usize)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static OVERLAY: LazyLock<Mutex<Overlay>> = LazyLock::new(|| {
    Mutex::new(Overlay {
        revision: 0,
        enabled: false,
        entries: HashMap::new(),
    })
});

/// Install both typed getters once. Returns true only for the call that
/// registers the hooks.
pub fn install() -> bool {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return false;
    }
    // Capture the pristine main-image exports before Skyline replaces them.
    // The installed param-config plugin detours the separate shared dispatch
    // bodies, so those bytes are intentionally not part of this guard.
    let native_supported = cache_native_layout_guard();
    if native_supported {
        skyline::install_hooks!(hook_native_param_float, hook_native_param_int);
        GETTER_BACKEND.store(1, Ordering::Release);
        crate::slight::diag::note(
            "live personal-parameter native ParamData getters installed for verified 13.0.4; hooks are original-first and only overlay uniquely mapped active fighter ParamData objects",
        );
    } else {
        skyline::install_hooks!(hook_get_param_float, hook_get_param_int);
        GETTER_BACKEND.store(2, Ordering::Release);
        crate::slight::diag::note(
            "live personal-parameter WorkModule export fallback installed; native ParamData signatures/layout were not verified, hooks are original-first",
        );
    }
    true
}

pub fn is_installed() -> bool {
    INSTALLED.load(Ordering::Acquire)
}

/// Refresh the identity snapshot from the agent registry. Call from the
/// existing once-per-frame game-thread pump, never from a getter hook.
pub fn refresh_identities() {
    retire_stale_stats();
    let refresh_tick = IDENTITY_REFRESH_TICK.fetch_add(1, Ordering::Relaxed) + 1;
    let native_layout_supported = NATIVE_LAYOUT_CHECKED.load(Ordering::Acquire)
        && NATIVE_LAYOUT_SUPPORTED.load(Ordering::Relaxed);
    let records = crate::slight::agents::all_records();
    let mut next = HashMap::with_capacity(records.len());
    let mut next_param_data = UniqueAddressMap::default();
    let mut next_work_modules = UniqueAddressMap::default();
    let mut memory = NativeReadCache::default();
    for record in records {
        let Some(identity) = identity_from_record(&record) else {
            continue;
        };
        if native_layout_supported {
            sample_native_getter_targets(
                &record,
                refresh_tick,
                &mut memory,
                &mut next_param_data,
                &mut next_work_modules,
            );
        }
        next.insert(identity.accessor, identity);
    }
    *IDENTITIES.write() = IdentitySnapshot {
        identities: next,
        param_data_owners: next_param_data,
        work_modules: next_work_modules,
    };
}

/// Guard native-layout reads against a different main executable. This runs
/// once before our export hooks are installed, and checks only pristine C
/// exports; shared dispatch bodies may already be detoured by param-config.
fn cache_native_layout_guard() -> bool {
    let (text, rodata) = unsafe {
        (
            skyline::hooks::getRegionAddress(skyline::hooks::Region::Text) as usize,
            skyline::hooks::getRegionAddress(skyline::hooks::Region::Rodata) as usize,
        )
    };
    let supported = text != 0
        && rodata > text.saturating_add(0x0208de60)
        && readable_span(text + 0x0208de30, 16)
        && readable_span(text + 0x0208de50, 16)
        && instruction_signature_matches(
            text + 0x0208de30,
            [
                0x00, 0x28, 0x40, 0xf9, 0x08, 0x00, 0x40, 0xf9, 0x03, 0x25, 0x41, 0xf9, 0x60, 0x00,
                0x1f, 0xd6,
            ],
        )
        && instruction_signature_matches(
            text + 0x0208de50,
            [
                0x00, 0x28, 0x40, 0xf9, 0x08, 0x00, 0x40, 0xf9, 0x03, 0x35, 0x41, 0xf9, 0x60, 0x00,
                0x1f, 0xd6,
            ],
        )
        && rodata > text.saturating_add(0x77dbd0)
        && readable_span(text + 0x77d5b0, 16)
        && readable_span(text + 0x77dbc0, 16)
        && instruction_signature_matches(
            text + 0x77d5b0,
            [
                0xff, 0x43, 0x01, 0xd1, 0xf4, 0x4f, 0x03, 0xa9, 0xfd, 0x7b, 0x04, 0xa9, 0xfd, 0x03,
                0x01, 0x91,
            ],
        )
        && instruction_signature_matches(
            text + 0x77dbc0,
            [
                0xff, 0x83, 0x01, 0xd1, 0xf5, 0x1b, 0x00, 0xf9, 0xf4, 0x4f, 0x04, 0xa9, 0xfd, 0x7b,
                0x05, 0xa9,
            ],
        );
    NATIVE_LAYOUT_SUPPORTED.store(supported, Ordering::Relaxed);
    NATIVE_LAYOUT_CHECKED.store(true, Ordering::Release);
    if !supported {
        crate::slight::diag::note(
            "native personal-parameter hooks disabled: main executable C exports or ParamData getter prologues do not match verified 13.0.4 signatures",
        );
    }
    supported
}

fn instruction_signature_matches(address: usize, expected: [u8; 16]) -> bool {
    if address == 0 {
        return false;
    }
    unsafe { std::ptr::read_unaligned(address as *const [u8; 16]) == expected }
}

/// Read only engine-owned active fighter accessors, then follow the
/// disassembled BOMA+0x50 -> WorkModule+0x38 -> ParamData vtable chain. This
/// runs in the once-per-frame identity refresh, never in a getter hook.
fn sample_native_getter_targets(
    record: &AgentRecord,
    refresh_tick: u64,
    memory: &mut NativeReadCache,
    param_owners: &mut UniqueAddressMap,
    work_modules: &mut UniqueAddressMap,
) {
    if record.category != 0 || record.module_accessor_addr == 0 || record.kind < 0 {
        return;
    }
    let Some(identity) = identity_from_record(record) else {
        return;
    };
    let accessor = record.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor;
    let engine_owned = unsafe {
        smash::app::sv_battle_object::is_active(record.boid)
            && smash::app::sv_battle_object::category(record.boid) == 0
            && smash::app::sv_battle_object::kind(record.boid) == record.kind
            && smash::app::sv_battle_object::module_accessor(record.boid) == accessor
    };
    if !engine_owned {
        return;
    }
    let (text, rodata) = unsafe {
        (
            skyline::hooks::getRegionAddress(skyline::hooks::Region::Text) as usize,
            skyline::hooks::getRegionAddress(skyline::hooks::Region::Rodata) as usize,
        )
    };
    let (work_module, param_data, int_relative, float_relative) = match sample_native_targets(
        accessor as usize,
        text,
        rodata,
        memory,
    ) {
        Ok(sample) => sample,
        Err(failure) => {
            if native_target_attempt_due(record.kind, refresh_tick)
                && native_target_attempts(record.kind) >= 4
            {
                crate::slight::diag::note(format!(
                    "native personal-parameter target sampling failed after 4 attempts: fighter_kind={} stage={} detail={}",
                    record.kind, failure.stage, failure.detail,
                ));
            }
            return;
        }
    };
    if let Some(retry) = NATIVE_TARGET_RETRIES.lock().get_mut(&record.kind) {
        retry.attempts = 4;
    }
    param_owners.insert(param_data, identity.accessor);
    work_modules.insert(identity.accessor, work_module);
    let sample = (record.kind, int_relative, float_relative);
    let mut seen = NATIVE_TARGET_SAMPLES.lock();
    if seen.len() < 64 && seen.insert(sample) {
        drop(seen);
        crate::slight::diag::note(format!(
            "native personal-parameter target sample: fighter_kind={} boid={} work_module={:p} param_data={:p} int=main+{:#x} float=main+{:#x}",
            record.kind,
            record.boid,
            work_module as *const u8,
            param_data as *const u8,
            int_relative,
            float_relative,
        ));
    }
}

fn native_target_attempt_due(kind: i32, tick: u64) -> bool {
    let mut retries = NATIVE_TARGET_RETRIES.lock();
    if !retries.contains_key(&kind) && retries.len() >= 64 {
        return false;
    }
    let retry = retries.entry(kind).or_insert(NativeTargetRetry {
        attempts: 0,
        next_tick: tick.saturating_add(30),
    });
    if retry.attempts >= 4 || tick < retry.next_tick {
        return false;
    }
    retry.attempts += 1;
    retry.next_tick = tick.saturating_add(30);
    true
}

fn native_target_attempts(kind: i32) -> u8 {
    NATIVE_TARGET_RETRIES
        .lock()
        .get(&kind)
        .map_or(0, |retry| retry.attempts)
}

fn sample_native_targets(
    accessor: usize,
    text: usize,
    rodata: usize,
    memory: &mut NativeReadCache,
) -> Result<(usize, usize, usize, usize), NativeTargetFailure> {
    if text == 0 || rodata <= text {
        return Err(native_target_failure(
            "main image",
            "invalid text/rodata bounds",
        ));
    }
    let work_module = memory
        .read::<usize>(accessor.saturating_add(0x50))
        .ok_or_else(|| {
            native_target_failure("BOMA+0x50", "WorkModule pointer field is unreadable")
        })?;
    if work_module == 0 || work_module % std::mem::align_of::<usize>() != 0 {
        return Err(native_target_failure(
            "BOMA+0x50",
            format!("WorkModule pointer is null or unaligned: {work_module:#x}"),
        ));
    }
    let work_vtable = memory.read::<usize>(work_module).ok_or_else(|| {
        native_target_failure("WorkModule vtable", "vtable pointer field is unreadable")
    })?;
    if work_vtable == 0 || work_vtable % std::mem::align_of::<usize>() != 0 {
        return Err(native_target_failure(
            "WorkModule vtable",
            format!("vtable pointer is null or unaligned: {work_vtable:#x}"),
        ));
    }
    let work_int_dispatch = memory
        .read::<usize>(work_vtable.saturating_add(0x248))
        .ok_or_else(|| {
            native_target_failure("WorkModule dispatch", "int slot +0x248 unreadable")
        })?;
    let work_float_dispatch = memory
        .read::<usize>(work_vtable.saturating_add(0x268))
        .ok_or_else(|| {
            native_target_failure("WorkModule dispatch", "float slot +0x268 unreadable")
        })?;
    // The live WorkModule entries point at the aligned forwarding entries at
    // +0x4e5390/+0x4e53d0. Each forwards to the next WorkModule virtual slot
    // (+0x250/+0x270), whose implementation begins at +0x4e53a0/+0x4e53e0
    // and dispatches through ParamData's vtable. The latter are distinct
    // function bodies, not the entry addresses installed in slots +0x248/268.
    let expected_int = text.saturating_add(0x4e5390);
    let expected_float = text.saturating_add(0x4e53d0);
    let work_int_param_dispatch = memory
        .read::<usize>(work_vtable.saturating_add(0x250))
        .ok_or_else(|| {
            native_target_failure("WorkModule dispatch", "int slot +0x250 unreadable")
        })?;
    let work_float_param_dispatch = memory
        .read::<usize>(work_vtable.saturating_add(0x270))
        .ok_or_else(|| {
            native_target_failure("WorkModule dispatch", "float slot +0x270 unreadable")
        })?;
    let expected_int_param = text.saturating_add(0x4e53a0);
    let expected_float_param = text.saturating_add(0x4e53e0);
    if work_int_dispatch != expected_int
        || work_float_dispatch != expected_float
        || work_int_param_dispatch != expected_int_param
        || work_float_param_dispatch != expected_float_param
    {
        return Err(native_target_failure(
            "WorkModule dispatch",
            format!(
                "actual slot+0x248={} slot+0x250={} slot+0x268={} slot+0x270={}; expected main+0x4e5390/main+0x4e53a0/main+0x4e53d0/main+0x4e53e0",
                relative_or_address(work_int_dispatch, text, rodata),
                relative_or_address(work_int_param_dispatch, text, rodata),
                relative_or_address(work_float_dispatch, text, rodata),
                relative_or_address(work_float_param_dispatch, text, rodata),
            ),
        ));
    }
    // lib_param_config's primary hook source also uses WorkModule+8 as the
    // owning BOMA pointer; reject any object that fails that check.
    let owner = memory
        .read::<usize>(work_module.saturating_add(8))
        .ok_or_else(|| native_target_failure("WorkModule owner", "+0x08 owner field unreadable"))?;
    if owner != accessor {
        return Err(native_target_failure(
            "WorkModule owner",
            format!("owner={owner:#x} BOMA={accessor:#x}"),
        ));
    }
    let param_data = memory
        .read::<usize>(work_module.saturating_add(0x38))
        .ok_or_else(|| {
            native_target_failure("WorkModule+0x38", "ParamData pointer field unreadable")
        })?;
    if param_data == 0 {
        return Err(native_target_failure(
            "ParamData",
            "WorkModule+0x38 is null",
        ));
    }
    if param_data % std::mem::align_of::<usize>() != 0 {
        return Err(native_target_failure(
            "ParamData",
            format!("object pointer is unaligned: {param_data:#x}"),
        ));
    }
    // A non-null field is not evidence that ParamData is initialized. Require
    // its object and vtable slots to be mapped/readable; do not assume RELRO.
    let vtable = memory.read::<usize>(param_data).ok_or_else(|| {
        native_target_failure("ParamData mapped", "object/vtable word unreadable")
    })?;
    if vtable == 0 || vtable % std::mem::align_of::<usize>() != 0 {
        return Err(native_target_failure(
            "ParamData vtable",
            format!("vtable pointer is null or unaligned: {vtable:#x}"),
        ));
    }
    let int_target = memory
        .read::<usize>(vtable.saturating_add(0x20))
        .ok_or_else(|| native_target_failure("ParamData vtable", "+0x20 int method unreadable"))?;
    let float_target = memory
        .read::<usize>(vtable.saturating_add(0x30))
        .ok_or_else(|| {
            native_target_failure("ParamData vtable", "+0x30 float method unreadable")
        })?;
    if int_target < text
        || int_target >= rodata
        || float_target < text
        || float_target >= rodata
        || int_target % 4 != 0
        || float_target % 4 != 0
    {
        return Err(native_target_failure(
            "ParamData method bounds",
            format!(
                "int={} float={} main text range=[{text:#x},{rodata:#x})",
                relative_or_address(int_target, text, rodata),
                relative_or_address(float_target, text, rodata),
            ),
        ));
    }
    if int_target != text.saturating_add(0x77d5b0) || float_target != text.saturating_add(0x77dbc0)
    {
        return Err(native_target_failure(
            "ParamData method identity",
            format!(
                "vtable methods int={} float={} expected main+0x77d5b0/main+0x77dbc0",
                relative_or_address(int_target, text, rodata),
                relative_or_address(float_target, text, rodata),
            ),
        ));
    }
    Ok((
        work_module,
        param_data,
        int_target - text,
        float_target - text,
    ))
}

fn native_target_failure(stage: &'static str, detail: impl Into<String>) -> NativeTargetFailure {
    NativeTargetFailure {
        stage,
        detail: detail.into(),
    }
}

fn relative_or_address(address: usize, text: usize, rodata: usize) -> String {
    if (text..rodata).contains(&address) {
        format!("main+{:#x}", address - text)
    } else {
        format!("{address:#x}")
    }
}

#[repr(C)]
struct NativeMemoryInfo {
    addr: u64,
    size: u64,
    ty: u32,
    attr: u32,
    perm: u32,
    device_refcount: u32,
    ipc_refcount: u32,
    padding: u32,
}

#[derive(Clone, Copy)]
struct NativeMemoryRegion {
    start: usize,
    end: usize,
}

/// Refresh-local bounded cache: typical BOMA/WorkModule/ParamData addresses
/// share one heap mapping, while their vtables share one main-image mapping.
/// This keeps chain validation to a few QueryMemory calls per refresh, never
/// one SVC per field or per getter invocation.
#[derive(Default)]
pub(super) struct NativeReadCache {
    regions: [Option<NativeMemoryRegion>; 8],
    next: usize,
}

impl NativeReadCache {
    fn readable_span(&mut self, address: usize, len: usize) -> bool {
        let Some(end) = address.checked_add(len) else {
            return false;
        };
        if self
            .regions
            .iter()
            .flatten()
            .any(|region| region.start <= address && end <= region.end)
        {
            return true;
        }
        let Some(info) = query_memory(address) else {
            return false;
        };
        let start = info.addr as usize;
        let Some(region_end) = start.checked_add(info.size as usize) else {
            return false;
        };
        if info.perm & 0x1 == 0 || start > address || end > region_end {
            return false;
        }
        self.regions[self.next] = Some(NativeMemoryRegion {
            start,
            end: region_end,
        });
        self.next = (self.next + 1) % self.regions.len();
        true
    }

    pub(super) fn read<T: Copy>(&mut self, address: usize) -> Option<T> {
        if address % std::mem::align_of::<T>() != 0
            || !self.readable_span(address, std::mem::size_of::<T>())
        {
            return None;
        }
        Some(unsafe { std::ptr::read(address as *const T) })
    }
}

/// QueryMemory's ABI and SVC shim match the pinned local Smashline `rtld`
/// implementation (`svc 0x6`); keep this private rather than relying on its
/// transitive crate dependency.
fn query_memory(address: usize) -> Option<NativeMemoryInfo> {
    let mut info = std::mem::MaybeUninit::<NativeMemoryInfo>::uninit();
    let mut page_info = 0u32;
    let mut result: u64;
    unsafe {
        core::arch::asm!(
            "str x1, [sp, #-16]!",
            "svc #0x6",
            "ldr x2, [sp], #16",
            "str w1, [x2]",
            inlateout("x0") info.as_mut_ptr() as u64 => result,
            inlateout("x1") (&mut page_info as *mut u32) as u64 => _,
            inlateout("x2") address as u64 => _,
        );
    }
    if result != 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    let _ = page_info;
    Some(info)
}

fn readable_span(address: usize, len: usize) -> bool {
    let Some(end) = address.checked_add(len) else {
        return false;
    };
    let Some(info) = query_memory(address) else {
        return false;
    };
    let region_end = (info.addr as usize).checked_add(info.size as usize);
    (info.perm & 0x1) != 0
        && (info.addr as usize) <= address
        && region_end.is_some_and(|region_end| end <= region_end)
}

fn identity_from_record(record: &AgentRecord) -> Option<Identity> {
    if record.category != 0 || record.module_accessor_addr == 0 || record.kind < 0 {
        return None;
    }
    crate::slight::slight_consts::fighters::game_kind_name(record.kind)?;
    Some(Identity {
        boid: record.boid,
        accessor: record.module_accessor_addr as usize,
        kind: record.kind,
    })
}

/// Parse, validate, and atomically replace the current request. No partial
/// update is published. The request is capped before JSON parsing so hostile
/// input cannot trigger an unbounded allocation.
pub fn handle(raw: &str) -> Ack {
    if raw.len() > MAX_REQUEST_BYTES {
        return nack(0, format!("request exceeds {MAX_REQUEST_BYTES} bytes"));
    }
    let request: Request = match serde_json::from_str::<Envelope>(raw) {
        Ok(envelope) => envelope.gimmick_parameters,
        Err(error) => {
            let revision = nested_revision(raw).unwrap_or(0);
            return nack(revision, format!("invalid request: {error}"));
        }
    };
    if let Err(error) = request.validate() {
        return nack(request.revision, error);
    }

    let mut entries = HashMap::with_capacity(request.edits.len());
    let mut resolved = HashSet::with_capacity(request.edits.len());
    for edit in &request.edits {
        let Some(fighter_kind) =
            crate::slight::gimmicks::registry::fighter_kind_value(&edit.fighter)
        else {
            return nack(
                request.revision,
                format!(
                    "fighter {:?} has no exact registered fighter kind",
                    edit.fighter
                ),
            );
        };
        let path = match classify_engine_path(&edit.path) {
            Ok(path) => path,
            Err(error) => return nack(request.revision, format!("{}: {error}", edit.path)),
        };
        let (parent, child) = engine_hashes(&path);
        let key = ParamKey {
            fighter_kind,
            parent,
            child,
        };
        if !resolved.insert(key.clone()) {
            return nack(
                request.revision,
                format!(
                    "duplicate engine parameter lookup for {}:{} ({:#x}, {:#x})",
                    edit.fighter, edit.path, parent, child
                ),
            );
        }
        entries.insert(
            key,
            Entry {
                fighter: edit.fighter.clone(),
                path: edit.path.clone(),
                value: edit.value,
                hits: 0,
                baseline: None,
                last_observed: None,
            },
        );
    }

    let mut overlay = OVERLAY.lock();
    if request.revision < overlay.revision {
        return nack(request.revision, "stale revision".into());
    }
    if request.revision == overlay.revision {
        let same = overlay.enabled == request.enabled
            && overlay.entries.len() == entries.len()
            && overlay.entries.iter().all(|(key, old)| {
                entries
                    .get(key)
                    .is_some_and(|new| old.path == new.path && old.value == new.value)
            });
        return if same {
            ack(request.revision, "already applied".into())
        } else {
            nack(
                request.revision,
                "revision was already used for a different request".into(),
            )
        };
    }

    overlay.revision = request.revision;
    overlay.enabled = request.enabled;
    overlay.entries = entries;
    GETTER_HEALTH.reset();
    CURRENT_REVISION.store(request.revision, Ordering::Release);
    ack(request.revision, "accepted".into())
}

fn ack(revision: u64, message: String) -> Ack {
    Ack {
        version: 1,
        revision,
        accepted: true,
        message,
    }
}

fn nack(revision: u64, message: String) -> Ack {
    Ack {
        version: 1,
        revision,
        accepted: false,
        message,
    }
}

fn nested_revision(raw: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()?
        .get("gimmick_parameters")?
        .get("revision")?
        .as_u64()
}

fn engine_hashes(path: &EngineParamPath) -> (u64, u64) {
    let parent = smash::hash40(&path.parent);
    // Root scalar parameter lookups use an exact zero child key. The empty
    // string's hash must not be left to an implementation detail.
    let child = if path.child.is_empty() {
        0
    } else {
        smash::hash40(&path.child)
    };
    (parent, child)
}

/// Current replacement revision for periodic state packets.
pub fn revision() -> u64 {
    CURRENT_REVISION.load(Ordering::Acquire)
}

/// Snapshot the per-path proof that an override has been read by the engine.
/// A path with zero hits was accepted but no matching getter has observed it.
pub fn wire_status() -> WireStatus {
    let Some(overlay) = OVERLAY.try_lock() else {
        return WireStatus {
            version: 1,
            revision: 0,
            enabled: false,
            rows: Vec::new(),
        };
    };
    let mut rows: Vec<_> = overlay
        .entries
        .values()
        .map(|entry| WireRow {
            fighter: entry.fighter.clone(),
            path: entry.path.clone(),
            hits: entry.hits,
            baseline: entry.baseline,
            last_observed: entry.last_observed,
        })
        .collect();
    rows.sort_by(|left, right| {
        left.fighter
            .cmp(&right.fighter)
            .then_with(|| left.path.cmp(&right.path))
    });
    WireStatus {
        version: 1,
        revision: overlay.revision,
        enabled: overlay.enabled,
        rows,
    }
}

/// Compact value consumed by the existing `GimmickLiveState` packet builder.
pub fn wire_rows() -> Vec<WireRow> {
    wire_status().rows
}

/// Snapshot bounded path counters for the existing live-state transport.
pub fn wire_health() -> WireHealth {
    GETTER_HEALTH.snapshot(CURRENT_REVISION.load(Ordering::Acquire))
}

/// Clear overrides and identity snapshots. Installed game hooks remain
/// registered and continue to return native values until another request is
/// accepted.
pub fn clear() {
    let mut overlay = OVERLAY.lock();
    *overlay = Overlay::default();
    CURRENT_REVISION.store(0, Ordering::Release);
    GETTER_HEALTH.reset();
    drop(overlay);
    IDENTITIES.write().clear();
    STAT_SCALES.write().clear();
    STATS_ACTIVE.store(false, Ordering::Release);
}

fn identity_for_accessor(
    module_accessor: *mut smash::app::BattleObjectModuleAccessor,
) -> Result<Identity, IdentityLookupError> {
    if module_accessor.is_null() {
        return Err(IdentityLookupError::Missing);
    }
    let identity = {
        let snapshot = IDENTITIES.try_read().ok_or(IdentityLookupError::LockBusy)?;
        snapshot
            .identities
            .get(&(module_accessor as usize))
            .copied()
            .ok_or(IdentityLookupError::Missing)?
    };
    validate_live_identity(identity, module_accessor)?;
    Ok(identity)
}

fn identity_for_param_data(param_data: usize) -> Result<Identity, IdentityLookupError> {
    let (identity, work_module) = {
        let snapshot = IDENTITIES.try_read().ok_or(IdentityLookupError::LockBusy)?;
        let accessor = snapshot
            .param_data_owners
            .get(param_data)
            .ok_or(IdentityLookupError::Missing)?;
        let identity = snapshot
            .identities
            .get(&accessor)
            .copied()
            .ok_or(IdentityLookupError::Missing)?;
        let work_module = snapshot
            .work_modules
            .get(accessor)
            .ok_or(IdentityLookupError::Missing)?;
        (identity, work_module)
    };
    let accessor = identity.accessor as *mut smash::app::BattleObjectModuleAccessor;
    validate_live_identity(identity, accessor)?;

    // The just-refreshed snapshot already validated these mappings with
    // QueryMemory. Recheck the live owner chain without issuing an SVC from
    // the getter path; first prove BOMA still points at this exact WorkModule,
    // then inspect only fields on that currently-owned object.
    let (current_work_module, owner, current_param_data) = unsafe {
        let current_work_module =
            std::ptr::read_unaligned((accessor as *const u8).add(0x50) as *const usize);
        if current_work_module != work_module {
            return Err(IdentityLookupError::Missing);
        }
        let owner = std::ptr::read_unaligned((work_module as *const u8).add(8) as *const usize);
        let current_param_data =
            std::ptr::read_unaligned((work_module as *const u8).add(0x38) as *const usize);
        (current_work_module, owner, current_param_data)
    };
    if current_work_module != work_module
        || owner != identity.accessor
        || current_param_data != param_data
    {
        return Err(IdentityLookupError::Missing);
    }
    Ok(identity)
}

fn validate_live_identity(
    identity: Identity,
    module_accessor: *mut smash::app::BattleObjectModuleAccessor,
) -> Result<(), IdentityLookupError> {
    if module_accessor.is_null() || identity.accessor != module_accessor as usize {
        return Err(IdentityLookupError::Missing);
    }
    // Agent records are only the snapshot. These engine-side checks stop a
    // recycled BOID or replaced accessor from borrowing another fighter's
    // override during the frame before the next snapshot refresh.
    unsafe {
        let boid = identity.boid;
        if !smash::app::sv_battle_object::is_active(boid)
            || smash::app::sv_battle_object::category(boid) != 0
            || smash::app::sv_battle_object::kind(boid) != identity.kind
            || smash::app::sv_battle_object::module_accessor(boid) != module_accessor
        {
            return Err(IdentityLookupError::Missing);
        }
    }
    Ok(())
}

fn lookup_override(
    module_accessor: *mut smash::app::BattleObjectModuleAccessor,
    parent: u64,
    child: u64,
    requested: LiveParamValue,
    baseline: LiveParamValue,
) -> Option<LiveParamValue> {
    let identity = match identity_for_accessor(module_accessor) {
        Ok(identity) => {
            GETTER_HEALTH
                .identity_matches
                .fetch_add(1, Ordering::Relaxed);
            identity
        }
        Err(IdentityLookupError::Missing) => {
            GETTER_HEALTH
                .identity_misses
                .fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Err(IdentityLookupError::LockBusy) => {
            GETTER_HEALTH
                .identity_lock_busy
                .fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    lookup_override_for_identity(identity, parent, child, requested, baseline)
}

fn lookup_native_override(
    param_data: usize,
    parent: u64,
    child: u64,
    requested: LiveParamValue,
    baseline: LiveParamValue,
) -> Option<LiveParamValue> {
    let identity = match identity_for_param_data(param_data) {
        Ok(identity) => {
            GETTER_HEALTH
                .identity_matches
                .fetch_add(1, Ordering::Relaxed);
            identity
        }
        Err(IdentityLookupError::Missing) => {
            GETTER_HEALTH
                .identity_misses
                .fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Err(IdentityLookupError::LockBusy) => {
            GETTER_HEALTH
                .identity_lock_busy
                .fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    lookup_override_for_identity(identity, parent, child, requested, baseline)
}

fn lookup_override_for_identity(
    identity: Identity,
    parent: u64,
    child: u64,
    requested: LiveParamValue,
    baseline: LiveParamValue,
) -> Option<LiveParamValue> {
    let Some(mut overlay) = OVERLAY.try_lock() else {
        GETTER_HEALTH
            .overlay_lock_busy
            .fetch_add(1, Ordering::Relaxed);
        return None;
    };
    if !overlay.enabled {
        GETTER_HEALTH
            .overlay_disabled
            .fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let key = ParamKey {
        fighter_kind: identity.kind,
        parent,
        child,
    };
    let Some(entry) = overlay.entries.get_mut(&key) else {
        GETTER_HEALTH
            .parameter_key_misses
            .fetch_add(1, Ordering::Relaxed);
        return None;
    };
    GETTER_HEALTH
        .parameter_key_matches
        .fetch_add(1, Ordering::Relaxed);
    let result = observe_entry(entry, requested, baseline);
    if result.is_some() {
        GETTER_HEALTH
            .overrides_returned
            .fetch_add(1, Ordering::Relaxed);
    } else {
        GETTER_HEALTH
            .type_mismatches
            .fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn observe_entry(
    entry: &mut Entry,
    requested: LiveParamValue,
    baseline: LiveParamValue,
) -> Option<LiveParamValue> {
    let same_type = matches!(
        (requested, entry.value),
        (LiveParamValue::Float(_), LiveParamValue::Float(_))
            | (LiveParamValue::Int(_), LiveParamValue::Int(_))
    );
    if !same_type {
        return None;
    }
    entry.hits = entry.hits.saturating_add(1);
    entry.baseline.get_or_insert(baseline);
    entry.last_observed = Some(entry.value);
    Some(entry.value)
}

/// Set one fighter's stat multiplier; 1.0 removes it. Each stock is tagged so a
/// lost stock drops its multipliers even when no owning Main ever runs again.
pub(crate) fn set_stat(boid: u32, stock_generation: u32, stat: MovementStat, multiplier: f32) {
    let mut scales = STAT_SCALES.write();
    let index = match scales
        .iter()
        .position(|s| s.boid == boid && s.stock_generation == stock_generation)
    {
        Some(index) => index,
        None => {
            scales.retain(|s| s.boid != boid);
            scales.push(StatScale {
                boid,
                stock_generation,
                multipliers: [1.0; MovementStat::ALL.len()],
            });
            scales.len() - 1
        }
    };
    scales[index].multipliers[stat as usize] = multiplier;
    if scales[index].multipliers == [1.0; MovementStat::ALL.len()] {
        scales.swap_remove(index);
    }
    STATS_ACTIVE.store(!scales.is_empty(), Ordering::Release);
    drop(scales);
    if multiplier == 1.0 {
        let reads = STAT_READS[stat as usize].swap(0, Ordering::Relaxed);
        crate::slight::diag::note(format!(
            "stat object={boid} {} normal after {reads} scaled reads",
            stat.label()
        ));
    }
}

/// Once per frame: drop multipliers of a stock or fighter that is gone.
fn retire_stale_stats() {
    if !STATS_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let mut scales = STAT_SCALES.write();
    scales.retain(|scale| {
        let live = crate::slight::agents::lookup(scale.boid)
            .and_then(|record| crate::slight::agents::stock_generation(&record));
        live == Some(scale.stock_generation)
    });
    STATS_ACTIVE.store(!scales.is_empty(), Ordering::Release);
}

/// Hook-side: never blocks; on contention the read stays native for that one call.
fn scale_stat(boid: impl FnOnce() -> Option<u32>, parent: u64, child: u64, value: f32) -> f32 {
    if !STATS_ACTIVE.load(Ordering::Acquire) {
        return value;
    }
    let Some(stat) = MovementStat::for_param(parent, child) else {
        return value;
    };
    let Some(boid) = boid() else { return value };
    let Some(scales) = STAT_SCALES.try_read() else {
        return value;
    };
    let Some(scale) = scales.iter().find(|s| s.boid == boid) else {
        return value;
    };
    STAT_READS[stat as usize].fetch_add(1, Ordering::Relaxed);
    value * stat.read_factor(parent, &scale.multipliers)
}

#[skyline::hook(replace = smash::app::lua_bind::WorkModule::get_param_float)]
unsafe fn hook_get_param_float(
    module_accessor: *mut smash::app::BattleObjectModuleAccessor,
    parent_param: u64,
    child_param: u64,
) -> f32 {
    GETTER_HEALTH
        .float_wrapper_calls
        .fetch_add(1, Ordering::Relaxed);
    let baseline = original!()(module_accessor, parent_param, child_param);
    let value = lookup_override(
        module_accessor,
        parent_param,
        child_param,
        LiveParamValue::Float(baseline),
        LiveParamValue::Float(baseline),
    )
    .and_then(|value| match value {
        LiveParamValue::Float(value) => Some(value),
        LiveParamValue::Int(_) => None,
    })
    .unwrap_or(baseline);
    scale_stat(
        || identity_for_accessor(module_accessor).ok().map(|i| i.boid),
        parent_param,
        child_param,
        value,
    )
}

#[skyline::hook(offset = 0x77d5b0)]
unsafe fn hook_native_param_int(param_data: *mut u8, parent_param: u64, child_param: u64) -> i32 {
    GETTER_HEALTH
        .int_wrapper_calls
        .fetch_add(1, Ordering::Relaxed);
    // The target is the verified ParamData vtable +0x20 method. Preserve its
    // full native behavior first, then overlay only a unique live owner.
    let baseline = original!()(param_data, parent_param, child_param);
    lookup_native_override(
        param_data as usize,
        parent_param,
        child_param,
        LiveParamValue::Int(baseline),
        LiveParamValue::Int(baseline),
    )
    .and_then(|value| match value {
        LiveParamValue::Int(value) => Some(value),
        LiveParamValue::Float(_) => None,
    })
    .unwrap_or(baseline)
}

#[skyline::hook(offset = 0x77dbc0)]
unsafe fn hook_native_param_float(param_data: *mut u8, parent_param: u64, child_param: u64) -> f32 {
    GETTER_HEALTH
        .float_wrapper_calls
        .fetch_add(1, Ordering::Relaxed);
    // The target is the verified ParamData vtable +0x30 method. Keep original
    // first so all native lookup and failure behavior remains intact.
    let baseline = original!()(param_data, parent_param, child_param);
    let value = lookup_native_override(
        param_data as usize,
        parent_param,
        child_param,
        LiveParamValue::Float(baseline),
        LiveParamValue::Float(baseline),
    )
    .and_then(|value| match value {
        LiveParamValue::Float(value) => Some(value),
        LiveParamValue::Int(_) => None,
    })
    .unwrap_or(baseline);
    scale_stat(
        || {
            identity_for_param_data(param_data as usize)
                .ok()
                .map(|i| i.boid)
        },
        parent_param,
        child_param,
        value,
    )
}

#[skyline::hook(replace = smash::app::lua_bind::WorkModule::get_param_int)]
unsafe fn hook_get_param_int(
    module_accessor: *mut smash::app::BattleObjectModuleAccessor,
    parent_param: u64,
    child_param: u64,
) -> i32 {
    GETTER_HEALTH
        .int_wrapper_calls
        .fetch_add(1, Ordering::Relaxed);
    let baseline = original!()(module_accessor, parent_param, child_param);
    lookup_override(
        module_accessor,
        parent_param,
        child_param,
        LiveParamValue::Int(baseline),
        LiveParamValue::Int(baseline),
    )
    .and_then(|value| match value {
        LiveParamValue::Int(value) => Some(value),
        LiveParamValue::Float(_) => None,
    })
    .unwrap_or(baseline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_accepts_matching_revision_and_rejects_stale_or_reused_revision() {
        clear();
        let raw = r#"{"gimmick_parameters":{"version":1,"revision":10,"enabled":true,"edits":[{"fighter":"cloud","path":"param_special_lw[0].limit_gauge_add","value":{"Float":2.0}}]}}"#;
        assert!(handle(raw).accepted);
        assert!(handle(raw).accepted);
        let reused = raw.replace("2.0", "3.0");
        assert!(!handle(&reused).accepted);
        let stale = raw.replace("10", "9");
        assert!(!handle(&stale).accepted);
        clear();
    }

    #[test]
    fn rejects_alias_duplicates_that_resolve_to_the_same_getter_key() {
        clear();
        let raw = r#"{"gimmick_parameters":{"version":1,"revision":1,"enabled":true,"edits":[{"fighter":"cloud","path":"param_special_lw[0].limit_gauge_add","value":{"Float":2.0}},{"fighter":"cloud","path":"param_special_lw.limit_gauge_add","value":{"Float":3.0}}]}}"#;
        let ack = handle(raw);
        assert!(!ack.accepted);
        assert!(ack.message.contains("duplicate engine parameter lookup"));
        clear();
    }

    #[test]
    fn rejected_requests_do_not_partially_replace_state() {
        clear();
        let good = r#"{"gimmick_parameters":{"version":1,"revision":1,"enabled":true,"edits":[{"fighter":"cloud","path":"param_special_lw[0].limit_gauge_add","value":{"Float":2.0}}]}}"#;
        assert!(handle(good).accepted);
        let bad = r#"{"gimmick_parameters":{"version":1,"revision":2,"enabled":true,"edits":[{"fighter":"cloud","path":"param_special_lw[1].limit_gauge_add","value":{"Float":2.0}}]}}"#;
        assert!(!handle(bad).accepted);
        assert_eq!(revision(), 1);
        assert_eq!(wire_rows().len(), 1);
        clear();
    }

    #[test]
    fn malformed_and_oversized_payloads_are_rejected() {
        clear();
        assert!(!handle("not json").accepted);
        assert!(!handle(&" ".repeat(MAX_REQUEST_BYTES + 1)).accepted);
        let unsupported = r#"{"gimmick_parameters":{"version":1,"revision":1,"enabled":true,"edits":[{"fighter":"cloud","path":"param_special_lw[0].limit_gauge_add","value":{"Bool":true}}]}}"#;
        assert!(!handle(unsupported).accepted);
        clear();
    }

    #[test]
    fn wrapped_payload_rejection_preserves_nested_revision() {
        clear();
        let malformed =
            r#"{"gimmick_parameters":{"version":1,"revision":42,"enabled":true,"edits":{}}}"#;
        let ack = handle(malformed);
        assert!(!ack.accepted);
        assert_eq!(ack.revision, 42);
        clear();
    }

    #[test]
    fn matching_float_and_int_getters_record_baseline_and_observation() {
        let mut float = Entry {
            fighter: "cloud".into(),
            path: "param_special_lw[0].limit_gauge_add".into(),
            value: LiveParamValue::Float(2.5),
            hits: 0,
            baseline: None,
            last_observed: None,
        };
        assert_eq!(
            observe_entry(
                &mut float,
                LiveParamValue::Float(1.0),
                LiveParamValue::Float(1.0)
            ),
            Some(LiveParamValue::Float(2.5))
        );
        assert_eq!(float.hits, 1);
        assert_eq!(float.baseline, Some(LiveParamValue::Float(1.0)));
        assert_eq!(float.last_observed, Some(LiveParamValue::Float(2.5)));

        let mut int = Entry {
            fighter: "cloud".into(),
            path: "param_special_lw[0].clear_frame".into(),
            value: LiveParamValue::Int(45),
            hits: 0,
            baseline: None,
            last_observed: None,
        };
        assert_eq!(
            observe_entry(&mut int, LiveParamValue::Int(30), LiveParamValue::Int(30)),
            Some(LiveParamValue::Int(45))
        );
        assert_eq!(int.hits, 1);
        assert_eq!(int.baseline, Some(LiveParamValue::Int(30)));
        assert_eq!(int.last_observed, Some(LiveParamValue::Int(45)));
    }

    #[test]
    fn getter_type_mismatch_fails_open_without_recording_a_match() {
        let mut entry = Entry {
            fighter: "cloud".into(),
            path: "param_special_lw[0].limit_gauge_add".into(),
            value: LiveParamValue::Float(2.5),
            hits: 0,
            baseline: None,
            last_observed: None,
        };
        assert_eq!(
            observe_entry(&mut entry, LiveParamValue::Int(7), LiveParamValue::Int(7)),
            None
        );
        assert_eq!(entry.hits, 0);
        assert_eq!(entry.baseline, None);
        assert_eq!(entry.last_observed, None);
    }

    #[test]
    fn root_scalar_uses_hash_of_leaf_and_exact_zero_child() {
        let path = classify_engine_path("limit_max").unwrap();
        assert_eq!(engine_hashes(&path), (smash::hash40("limit_max"), 0));
    }
}
