use core::{
    fmt::{self, Display, Formatter},
    time::Duration,
};

use super::Metric;

pub struct Quantile {
    v: u64,
    f: Box<dyn Fn() -> u64 + Send>,
}

impl Quantile {
    pub fn new<F, S>(f: F, stat: S) -> Self
    where
        F: Fn(&S) -> u64 + Send + 'static,
        S: Send + 'static,
    {
        Self { v: 0, f: Box::new(move || f(&stat)) }
    }
}

impl Display for Quantile {
    #[inline]
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:.2?}", Duration::from_micros(self.v))
    }
}

impl Metric for Quantile {
    fn update(&mut self) {
        self.v = (self.f)();
    }
}
