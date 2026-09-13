/// Unset host identity must be outside the native u32 battle-object ID domain.
/// Fighter zero is a real owner, including the first fighter in a match.
pub const NO_HOST: u64 = u64::MAX;

pub fn host_id(stored: u64) -> Option<u32> {
    u32::try_from(stored).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fighter_remains_an_owner_until_explicitly_cleared() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let owner = AtomicU64::new(NO_HOST);
        assert_eq!(host_id(owner.load(Ordering::Acquire)), None);
        owner.store(0, Ordering::Release);
        assert_eq!(host_id(owner.load(Ordering::Acquire)), Some(0));
        owner.store(NO_HOST, Ordering::Release);
        assert_eq!(host_id(owner.load(Ordering::Acquire)), None);
    }

    #[test]
    fn host_identity_never_truncates_to_a_different_fighter() {
        assert_eq!(host_id(1), Some(1));
        assert_eq!(host_id(u32::MAX as u64), Some(u32::MAX));
        assert_eq!(host_id(u32::MAX as u64 + 1), None);
        assert_eq!(host_id(NO_HOST), None);
    }
}
