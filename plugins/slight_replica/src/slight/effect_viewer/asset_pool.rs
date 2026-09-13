//! Shared names for the boot-installed carrier resource graph.
pub const TEXTURE_CAPACITY: usize = 512;
pub const MOTION_CAPACITY: usize = 2048;
pub const MODEL_ROOT: &str = "assist/alucard/model/body/c00";
pub const MOTION_ROOT: &str = "assist/alucard/motion/body/c00";
pub const VERSION: &str = "2";

pub fn texture_name(index: usize) -> String {
    format!("visionary_tex_{index:04}")
}

pub fn motion_name(index: usize) -> String {
    format!("visionary_motion_{index:04}.nuanmb")
}

pub fn is_pool_file(file: &str, motion: bool) -> bool {
    let (prefix, suffix, capacity) = if motion {
        ("visionary_motion_", ".nuanmb", MOTION_CAPACITY)
    } else {
        ("visionary_tex_", ".nutexb", TEXTURE_CAPACITY)
    };
    file.strip_prefix(prefix)
        .and_then(|s| s.strip_suffix(suffix))
        .filter(|s| s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<usize>().ok())
        .is_some_and(|index| index < capacity)
}
