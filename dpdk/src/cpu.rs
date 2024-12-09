use core::fmt::{self, Display, Formatter};

use serde::Deserialize;

/// Represents a CPU core ID.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct CoreId(u16);

impl CoreId {
    /// Constructs a new `CoreId` using specified unsigned integer.
    ///
    /// # Panics
    ///
    /// Panics if `id < 128`.
    pub const fn new(id: u16) -> Self {
        assert!(id < 128);
        Self(id)
    }

    /// Returns the underlying core id value.
    #[inline]
    pub fn as_u16(&self) -> u16 {
        match self {
            Self(id) => *id,
        }
    }
}

impl Display for CoreId {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), fmt::Error> {
        let Self(id) = self;
        write!(fmt, "{}", id)
    }
}

/// Keeps CPU cores bitmask for further usage in EAL arguments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CoreMask(u128);

impl CoreMask {
    /// Adds a CPU id into the bitmask.
    #[inline]
    pub fn add(&mut self, id: CoreId) {
        let CoreId(id) = id;

        match self {
            Self(v) => {
                *v |= 1u128 << id as u128;
            }
        }
    }

    /// Yields the underlying bitmask value.
    #[inline]
    pub fn as_u128(&self) -> u128 {
        match self {
            Self(v) => *v,
        }
    }
}
