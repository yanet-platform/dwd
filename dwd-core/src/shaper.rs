use core::sync::atomic::{AtomicU64, Ordering};
use std::{sync::Arc, time::Instant};

/// A token-bucket shaper implementation.
#[derive(Debug)]
pub struct Shaper {
    limit: Arc<AtomicU64>,
    tokens: f64,
    burst_size: u64,
    prev_ts: Instant,
}

impl Shaper {
    pub fn new(burst_size: u64, limit: Arc<AtomicU64>) -> Self {
        Self {
            limit,
            tokens: 0.0,
            burst_size,
            prev_ts: Instant::now(),
        }
    }

    /// Called on each loop tick in a worker.
    ///
    /// Returns the number of tokens available to consume.
    pub fn tick(&mut self) -> u64 {
        let now = Instant::now();
        let elapsed = now.duration_since(self.prev_ts);

        let limit = self.limit.load(Ordering::Relaxed);
        self.tokens += limit as f64 * elapsed.as_secs_f64();
        self.prev_ts = now;
        if (self.tokens as u64) < self.burst_size {
            return 0;
        }

        self.tokens = self.tokens.min(limit as f64);
        self.tokens as u64
    }

    /// Consume specified amount of tokens.
    ///
    /// Must be called after actual token consumption (i.e. performing requests)
    /// to maintain this shaper.
    #[inline]
    pub fn consume(&mut self, num: u64) {
        self.tokens -= num as f64;
    }
}
