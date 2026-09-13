//! libc++ shared-handle ownership used by the native model and motion resource binders.
use std::sync::atomic::{AtomicIsize, Ordering};

#[repr(C)]
#[derive(Default)]
pub struct SharedHandle {
    pub object: usize,
    pub control: usize,
}
impl SharedHandle {
    pub fn valid(&self) -> bool {
        self.object != 0 && self.control != 0
    }

    /// The caller must supply a live native shared handle with a libc++ control block.
    pub unsafe fn retain(&self) -> Self {
        if self.control != 0 {
            (*(self.control.wrapping_add(8) as *const AtomicIsize)).fetch_add(1, Ordering::AcqRel);
        }
        Self {
            object: self.object,
            control: self.control,
        }
    }

    /// Consume one strong reference using the native destructor and weak-count release.
    pub unsafe fn release(self, release_weak: unsafe extern "C" fn(usize)) {
        if self.control == 0 {
            return;
        }
        if (*(self.control.wrapping_add(8) as *const AtomicIsize)).fetch_sub(1, Ordering::AcqRel)
            == 0
        {
            let destroy: unsafe extern "C" fn(usize) =
                std::mem::transmute(*((*(self.control as *const usize) + 0x10) as *const usize));
            destroy(self.control);
            release_weak(self.control);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[repr(C)]
    struct Control {
        table: usize,
        owners: AtomicIsize,
        destroyed: usize,
        weak_released: usize,
    }
    unsafe extern "C" fn destroy(ptr: usize) {
        (*(ptr as *mut Control)).destroyed += 1;
    }
    unsafe extern "C" fn release_weak(ptr: usize) {
        (*(ptr as *mut Control)).weak_released += 1;
    }

    #[test]
    fn retained_resources_survive_carrier_destruction_until_fighter_releases_them() {
        let table = [0, 0, destroy as *const () as usize];
        let mut control = Control {
            table: table.as_ptr() as usize,
            owners: AtomicIsize::new(0),
            destroyed: 0,
            weak_released: 0,
        };
        let carrier = SharedHandle {
            object: 1,
            control: &mut control as *mut Control as usize,
        };
        unsafe {
            let pending = carrier.retain();
            carrier.release(release_weak);
            assert_eq!(control.destroyed, 0);
            assert_eq!(control.owners.load(Ordering::Acquire), 0);
            let fighter = pending.retain();
            pending.release(release_weak);
            assert_eq!(control.destroyed, 0);
            fighter.release(release_weak);
        }
        assert_eq!(control.destroyed, 1);
        assert_eq!(control.weak_released, 1);
        assert_eq!(control.owners.load(Ordering::Acquire), -1);
    }

    #[test]
    fn cancelling_a_pending_handoff_preserves_the_carrier_reference() {
        let table = [0, 0, destroy as *const () as usize];
        let mut control = Control {
            table: table.as_ptr() as usize,
            owners: AtomicIsize::new(0),
            destroyed: 0,
            weak_released: 0,
        };
        let carrier = SharedHandle {
            object: 1,
            control: &mut control as *mut Control as usize,
        };
        unsafe {
            carrier.retain().release(release_weak);
            assert_eq!(control.destroyed, 0);
            carrier.release(release_weak);
        }
        assert_eq!(control.destroyed, 1);
    }

    #[test]
    fn empty_handle_has_no_native_release() {
        let empty = SharedHandle::default();
        assert!(!empty.valid());
        unsafe {
            empty.retain().release(release_weak);
            empty.release(release_weak);
        }
    }
}
