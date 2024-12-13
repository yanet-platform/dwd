use crate::ffi;

/// Returns the number of execution units (lcores) on the system.
#[inline]
pub fn lcore_count() -> u32 {
    unsafe { ffi::rte_lcore_count() }
}
