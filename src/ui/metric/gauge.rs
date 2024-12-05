use core::{
    fmt,
    fmt::{Display, Formatter},
};

use super::Metric;

pub struct Gauge<S> {
    f: Box<dyn Fn(&S) -> u64>,
    value: u64,
}

impl<S> Gauge<S> {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&S) -> u64 + 'static,
    {
        Self { f: Box::new(f), value: 0 }
    }
}

impl<S> Display for Gauge<S> {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.value)
    }
}

impl<S> Metric<S> for Gauge<S> {
    fn update(&mut self, stat: &S) {
        self.value = (self.f)(stat);
    }
}
