use core::fmt::{self, Display, Formatter};
use std::ffi::CStr;

use crate::{ffi, rte_errno};

/// Tiny wrapper around DPDK errors.
#[derive(Debug)]
pub struct Error {
    errno: i32,
}

impl Error {
    #[inline]
    pub const fn new(errno: i32) -> Self {
        Self { errno }
    }

    /// Constructs a new `Error` using thread-local errno.
    pub fn from_errno() -> Self {
        let errno = unsafe { rte_errno() };
        Self::new(errno)
    }
}

impl From<i32> for Error {
    fn from(v: i32) -> Self {
        Self::new(v)
    }
}

impl Display for Error {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), fmt::Error> {
        let ptr = unsafe { ffi::rte_strerror(self.errno) };
        let desc = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();

        fmt.write_fmt(format_args!("[{}]: {}", self.errno, desc))
    }
}

impl core::error::Error for Error {}
