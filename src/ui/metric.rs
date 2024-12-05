use core::fmt::Display;

pub use self::{gauge::Gauge, meter::Meter, throughput::Throughput};

mod gauge;
mod meter;
mod throughput;

/// Stateful metric that can display itself.
pub trait Metric<S>
where
    Self: Display,
{
    /// Updates this metric using specified state.
    fn update(&mut self, stat: &S);
}
