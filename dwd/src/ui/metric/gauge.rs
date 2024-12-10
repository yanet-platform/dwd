use core::{
    fmt,
    fmt::{Display, Formatter},
};

use super::Metric;

pub struct Gauge {
    f: Box<dyn Fn() -> u64 + Send>,
    value: u64,
}

impl Gauge {
    pub fn new<F, S>(f: F, stat: S) -> Self
    where
        F: Fn(&S) -> u64 + Send + 'static,
        S: Send + 'static,
    {
        let f = move || f(&stat);
        Self { f: Box::new(f), value: 0 }
    }

    #[inline]
    pub const fn get(&self) -> u64 {
        self.value
    }
}

impl Display for Gauge {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.value)
    }
}

impl Metric for Gauge {
    fn update(&mut self) {
        self.value = (self.f)();
    }
}
