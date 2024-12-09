use core::{
    fmt,
    fmt::{Display, Formatter},
};
use std::time::Instant;

use super::Metric;

pub struct Meter {
    f: Box<dyn Fn() -> u64 + Send>,
    rate: f64,
    prev_v: u64,
    prev_ts: Instant,
}

impl Meter {
    pub fn new<F, S>(f: F, stat: S) -> Self
    where
        F: Fn(&S) -> u64 + Send + 'static,
        S: Send + 'static,
    {
        Self {
            f: Box::new(move || f(&stat)),
            rate: 0.0,
            prev_v: 0,
            prev_ts: Instant::now(),
        }
    }
}

impl Display for Meter {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:.0}", self.rate)
    }
}

impl Metric for Meter {
    fn update(&mut self) {
        let now = Instant::now();

        let dt = now.duration_since(self.prev_ts).as_secs_f64();
        if dt < 0.05 {
            return;
        }

        let v = (self.f)();
        let dv = v.saturating_sub(self.prev_v) as f64;
        self.rate = dv / dt;

        self.prev_v = v;
        self.prev_ts = now;
    }
}
