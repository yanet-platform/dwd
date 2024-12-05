use core::{
    fmt,
    fmt::{Display, Formatter},
};
use std::time::Instant;

use super::Metric;

pub struct Throughput<S> {
    f: Box<dyn Fn(&S) -> u64>,
    prev_v: u64,
    prev_ts: Instant,
    value: (f64, char),
}

impl<S> Throughput<S> {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&S) -> u64 + 'static,
    {
        Self {
            f: Box::new(f),
            prev_v: 0,
            prev_ts: Instant::now(),
            value: (0.0, ' '),
        }
    }

    fn fmt_si(v: f64) -> (f64, char) {
        match v {
            v if v >= 1e9 => (v / 1e9, 'G'),
            v if v >= 1e6 => (v / 1e6, 'M'),
            v if v >= 1e3 => (v / 1e3, 'K'),
            _ => (v, ' '),
        }
    }
}

impl<S> Display for Throughput<S> {
    #[inline]
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        let (rate, prefix) = self.value;

        write!(f, "{:.2} {}bit/s", rate, prefix)
    }
}

impl<S> Metric<S> for Throughput<S> {
    fn update(&mut self, stat: &S) {
        let now = Instant::now();

        let v = (self.f)(stat);
        let dv = (v.saturating_sub(self.prev_v) * 8) as f64;
        let dt = now.duration_since(self.prev_ts).as_secs_f64();
        let rate = dv / dt;

        self.prev_v = v;
        self.prev_ts = now;

        self.value = Self::fmt_si(rate);
    }
}
