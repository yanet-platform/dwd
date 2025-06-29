use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub mod cfg;
pub mod cmd;
pub mod engine;
mod generator;
mod histogram;
pub mod logging;
mod shaper;
mod sockstat;
mod stat;
mod ui;
mod worker;

/// Thread-safe producing iterators.
///
/// Unlike the [`Iterator`] this trait accepts `self` by reference and returns a
/// reference to the next item, not an [`Option`].
///
/// Think of it as an infinite thread-safe iterator.
trait Produce {
    /// The type of the elements being produced.
    type Item: ?Sized;

    /// Advances this producer and returns the next value.
    fn next(&self) -> &Self::Item;
}

/// Infinice cycle producing iterator, that yields the same value.
#[derive(Debug)]
pub struct OneProduce<T> {
    v: T,
}

impl<T> OneProduce<T> {
    /// Constructs a new [`OneProduce`] from the given value.
    #[inline]
    pub const fn new(v: T) -> Self {
        Self { v }
    }
}

impl<T> Produce for Arc<OneProduce<T>> {
    type Item = T;

    #[inline]
    fn next(&self) -> &Self::Item {
        &self.v
    }
}

/// Thread-safe infinite cycle producing iterator over the given vector.
#[derive(Debug)]
pub struct VecProduce<T> {
    vec: Vec<T>,
    idx: AtomicUsize,
}

impl<T> VecProduce<T> {
    /// Constructs a new [`VecProduce`] from the given vector.
    #[inline]
    pub const fn new(vec: Vec<T>) -> Self {
        Self { vec, idx: AtomicUsize::new(0) }
    }
}

impl<T> Produce for Arc<VecProduce<T>> {
    type Item = T;

    #[inline]
    fn next(&self) -> &Self::Item {
        // Increment the current value, returning the previous one.
        let idx = self.idx.fetch_add(1, Ordering::Relaxed);
        let idx = idx % self.vec.len();

        &self.vec[idx]
    }
}

pub enum GeneratorEvent {
    Suspend,
    Resume,
    Set(u64),
}
