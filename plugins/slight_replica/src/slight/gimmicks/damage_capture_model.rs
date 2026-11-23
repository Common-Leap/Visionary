pub const MAX_MAIN_TEXT_BYTES: u64 = 128 * 1024 * 1024;

/// Normalize a saved native LR without retaining an ASLR-dependent address.
/// A return site identifies a caller, not the semantics of its arguments.
pub fn caller_return_offset(address: u64, text_start: u64, text_end: u64) -> Option<u64> {
    let text_size = text_end.checked_sub(text_start)?;
    if text_start == 0 || text_size == 0 || text_size > MAX_MAIN_TEXT_BYTES {
        return None;
    }
    let offset = address.checked_sub(text_start)?;
    (offset >= 4 && offset < text_size && offset % 4 == 0).then_some(offset)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawSample {
    pub epoch: u64,
    pub frame: u64,
    pub sample_index: u32,
    pub object_id_1: u32,
    pub object_id_2: u32,
    pub amount_bits: u32,
    pub arg3_bits: u32,
    pub arg4_bits: u32,
    pub caller_return_offset: Option<u64>,
}

pub const fn initial_control() -> u64 {
    0
}
pub fn is_armed(control: u64) -> bool {
    control & 1 != 0
}
pub fn epoch(control: u64) -> u64 {
    control >> 1
}
pub fn arm(control: u64) -> u64 {
    control | 1
}
pub fn reset_match(control: u64) -> u64 {
    epoch(control).wrapping_add(1) << 1
}
