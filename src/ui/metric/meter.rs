use core::{
    fmt,
    fmt::{Display, Formatter},
};
use std::time::Instant;

use super::Metric;

pub struct Meter<S> {
    f: Box<dyn Fn(&S) -> u64>,
    rate: f64,
    prev_v: u64,
    prev_ts: Instant,
}

impl<S> Meter<S> {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&S) -> u64 + 'static,
    {
        Self {
            f: Box::new(f),
            rate: 0.0,
            prev_v: 0,
            prev_ts: Instant::now(),
        }
    }
}

impl<S> Display for Meter<S> {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:.0}", self.rate)
    }
}

impl<S> Metric<S> for Meter<S> {
    fn update(&mut self, stat: &S) {
        let now = Instant::now();

        let dt = now.duration_since(self.prev_ts).as_secs_f64();
        if dt < 0.05 {
            return;
        }

        let v = (self.f)(stat);
        let dv = v.saturating_sub(self.prev_v) as f64;
        self.rate = dv / dt;

        self.prev_v = v;
        self.prev_ts = now;
    }
}
