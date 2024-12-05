pub mod cfg;
pub mod cmd;
mod generator;
pub mod runtime;
mod shaper;
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

    /// Advances the producer and returns the next value.
    fn next(&self) -> &Self::Item;
}

pub enum GeneratorEvent {
    Suspend,
    Resume,
    Set(u64),
}
