//! Core of the DWD traffic generator: the load engines, generators, shaper,
//! per-CPU statistics, the `Produce` cycling iterators, and the server half of
//! the in-process gRPC API seam.
//!
//! This crate deliberately depends on **no CLI or TUI** (`clap`, `ratatui`,
//! `crossterm`): the frontend talks to it only through [`dwd_proto`]. That
//! boundary is enforced by the compiler — the engine cannot reach into the UI.

use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub mod api;
pub mod cfg;
pub mod engine;
pub mod generator;
pub mod grpc;
pub mod histogram;
pub mod shaper;
pub mod sockstat;
pub mod stat;
pub mod worker;

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

/// A control event for the generator: suspend, resume, or set a fixed RPS.
///
/// Flows from the gRPC `Control` handler into the rate-control loop.
pub enum GeneratorEvent {
    Suspend,
    Resume,
    Set(u64),
}
