// Interior-mutable cell types for device register state.
// Provides per-device locking granularity instead of
// wrapping entire device in Mutex.

use std::cell::UnsafeCell;

// ---- DeviceCell<T>: for Copy scalars ----

/// Interior-mutable cell for `Copy` types used in device
/// state accessed under single-writer guarantee (MMIO
/// dispatch serializes per-region).
pub struct DeviceCell<T: Copy> {
    value: UnsafeCell<T>,
}

// SAFETY: MMIO dispatch guarantees single-writer per
// device region.
unsafe impl<T: Copy + Send> Sync for DeviceCell<T> {}
unsafe impl<T: Copy + Send> Send for DeviceCell<T> {}

impl<T: Copy> DeviceCell<T> {
    pub const fn new(val: T) -> Self {
        Self {
            value: UnsafeCell::new(val),
        }
    }

    pub fn get(&self) -> T {
        unsafe { *self.value.get() }
    }

    pub fn set(&self, val: T) {
        unsafe {
            *self.value.get() = val;
        }
    }
}

// ---- DeviceRefCell<T>: for complex mutable state ----

/// Lightweight mutex wrapper for device register state.
/// Uses parking_lot::Mutex (no poisoning, 1 byte,
/// zero-cost uncontended).
pub struct DeviceRefCell<T> {
    inner: parking_lot::Mutex<T>,
}

unsafe impl<T: Send> Sync for DeviceRefCell<T> {}

impl<T> DeviceRefCell<T> {
    pub fn new(val: T) -> Self {
        Self {
            inner: parking_lot::Mutex::new(val),
        }
    }

    pub fn borrow(&self) -> parking_lot::MutexGuard<'_, T> {
        self.inner.lock()
    }

    pub fn try_borrow(&self) -> Option<parking_lot::MutexGuard<'_, T>> {
        self.inner.try_lock()
    }
}

impl<T: Default> Default for DeviceRefCell<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}
