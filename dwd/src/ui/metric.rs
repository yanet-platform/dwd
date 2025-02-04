use core::fmt::Display;

pub use self::{gauge::Gauge, meter::Meter, quantile::Quantile, throughput::Throughput};

mod gauge;
mod meter;
mod quantile;
mod throughput;

/// Stateful metric that can display itself.
pub trait Metric
where
    Self: Display,
{
    /// Updates this metric.
    fn update(&mut self);
}
