//! Live character select order: writes the parsed `ui_chara_db` the select screen draws from.
//!
//! Only a measured layout is ever used, read from `sd:/visionary_heap_config.json`:
//! `{"array_ptr": "0x..", "stride": n, "id_offset": n, "disp_offset": n, "can_select_offset": n,
//! "rebuild": "0x.."}` (the last two are optional). An entry is written only where its own
//! `ui_chara_id` is found at `id_offset`; without the file, or where an id is not found, nothing
//! is written and the order shows on the next menu load.

use std::collections::{BTreeMap, BTreeSet};

use super::gimmicks::live_parameters::NativeReadCache;

const CONFIG: &str = "sd:/visionary_heap_config.json";
/// More entries than any select screen has; the array's own length is not measured.
const MAX_ENTRIES: usize = 512;

struct Layout {
    /// Where the pointer to the first entry is.
    array_ptr: usize,
    stride: usize,
    id_offset: usize,
    disp_offset: usize,
    can_select_offset: Option<usize>,
    rebuild: Option<usize>,
}

fn layout() -> Option<Layout> {
    let data = std::fs::read_to_string(CONFIG).ok()?;
    let config: serde_json::Value = serde_json::from_str(&data).ok()?;
    let number = |name: &str| {
        let value = config.get(name)?;
        let hex = |text: &str| {
            usize::from_str_radix(
                text.trim().to_ascii_lowercase().trim_start_matches("0x"),
                16,
            )
            .ok()
        };
        value
            .as_u64()
            .map(|value| value as usize)
            .or_else(|| hex(value.as_str()?))
    };
    let layout = Layout {
        array_ptr: number("array_ptr")?,
        stride: number("stride")?,
        id_offset: number("id_offset")?,
        disp_offset: number("disp_offset")?,
        can_select_offset: number("can_select_offset"),
        rebuild: number("rebuild"),
    };
    let inside = |offset: usize| offset < layout.stride;
    (inside(layout.id_offset)
        && inside(layout.disp_offset)
        && layout.can_select_offset.is_none_or(inside))
    .then_some(layout)
}

/// The address of the entry that holds `ui_chara_id`, among the measured array's entries.
fn entry(memory: &mut NativeReadCache, layout: &Layout, ui_chara_id: u64) -> Option<usize> {
    let array = memory.read::<usize>(layout.array_ptr)?;
    for index in 0..MAX_ENTRIES {
        let entry = array.checked_add(index * layout.stride)?;
        if memory.read::<u64>(entry + layout.id_offset)? == ui_chara_id {
            return Some(entry);
        }
    }
    None
}

/// Write the order into the select screen's own list. True when it was also told to redraw.
pub fn patch_instant(order: &BTreeMap<String, i8>, hidden: &BTreeSet<String>) -> bool {
    let Some(layout) = layout() else {
        crate::slight::diag::note(
            "heap_patch: no measured layout; the order shows on the next menu load",
        );
        return false;
    };
    let mut memory = NativeReadCache::default();
    let wanted = order
        .iter()
        .map(|(key, disp)| (key, *disp))
        .chain(hidden.iter().map(|key| (key, -1)))
        .filter(|(key, _)| !key.contains('#') && !key.starts_with("ui:"));
    let mut patched = 0;
    for (key, disp) in wanted {
        let id = smash::hash40(&format!("ui_chara_{key}"));
        let Some(entry) = entry(&mut memory, &layout, id) else {
            crate::slight::diag::note(format!(
                "heap_patch: {key} not found at the measured layout"
            ));
            continue;
        };
        unsafe {
            *((entry + layout.disp_offset) as *mut i8) = disp;
            if let Some(offset) = layout.can_select_offset {
                *((entry + offset) as *mut u8) = u8::from(disp != -1);
            }
        }
        patched += 1;
    }
    let rebuilt = patched > 0 && layout.rebuild.is_some();
    if let Some(rebuild) = layout.rebuild.filter(|_| patched > 0) {
        unsafe {
            let rebuild: extern "C" fn() = std::mem::transmute(rebuild as *const ());
            rebuild();
        }
    }
    crate::slight::diag::note(format!(
        "heap_patch: {patched} entries written, redraw={rebuilt}"
    ));
    rebuilt
}
