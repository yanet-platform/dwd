use core::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// A token-bucket shaper implementation.
#[derive(Debug)]
pub struct TokenBucket {
    bucket: AtomicU64,
    /// Minimum number of tokens we take.
    burst_size: u64,
}

impl TokenBucket {
    pub fn new(burst_size: u64) -> Self {
        let bucket = AtomicU64::new(0);

        Self { bucket, burst_size }
    }

    /// Takes at most `n` tokens from the bucket.
    pub fn take(&self, n: u64) -> u64 {
        let mut t = self.bucket.load(Ordering::Acquire);

        loop {
            if t == 0 || t < self.burst_size {
                return 0;
            }

            if n <= t {
                match self
                    .bucket
                    .compare_exchange(t, t - n, Ordering::SeqCst, Ordering::SeqCst)
                {
                    Ok(..) => {
                        return n;
                    }
                    Err(n) => {
                        t = n;
                        continue;
                    }
                }
            }

            // The requested number is greater than the number of tokens available.
            match self.bucket.compare_exchange(t, 0, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(..) => {
                    // So return the entire number of tokens available.
                    return t;
                }
                Err(n) => {
                    t = n;
                    continue;
                }
            }
        }
    }

    #[inline]
    pub fn put(&self, n: u64, cap: u64) -> u64 {
        if n == 0 {
            return 0;
        }

        let mut t = self.bucket.load(Ordering::Acquire);
        if t > cap {
            loop {
                match self.bucket.compare_exchange(t, cap, Ordering::SeqCst, Ordering::SeqCst) {
                    Ok(..) => {
                        break;
                    }
                    Err(n) => {
                        t = n;
                        continue;
                    }
                }
            }
        }

        loop {
            if t == cap {
                return 0;
            }

            let left = cap - t;
            if n <= left {
                match self
                    .bucket
                    .compare_exchange(t, t + n, Ordering::SeqCst, Ordering::SeqCst)
                {
                    Ok(..) => {
                        return n;
                    }
                    Err(n) => {
                        t = n;
                        continue;
                    }
                }
            }

            match self.bucket.compare_exchange(t, cap, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(..) => {
                    return left;
                }
                Err(n) => {
                    t = n;
                    continue;
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct Shaper {
    tokens: f64,
    prev_ts: Instant,
}

impl Shaper {
    #[inline]
    pub fn new() -> Self {
        Self { tokens: 0.0, prev_ts: Instant::now() }
    }

    /// Called on each loop tick.
    ///
    /// Returns the number of tokens available to consume.
    #[inline]
    pub fn tick(&mut self, rps: u64) -> u64 {
        let now = Instant::now();
        let elapsed = now.duration_since(self.prev_ts);

        self.tokens += rps as f64 * elapsed.as_secs_f64();
        self.prev_ts = now;

        self.tokens = self.tokens.min(rps as f64);
        self.tokens as u64
    }

    /// Consume specified amount of tokens.
    ///
    /// Must be called after actual token consumption (i.e. by performing
    /// requests or updating token bucket) to maintain this shaper.
    #[inline]
    pub fn consume(&mut self, num: u64) {
        self.tokens -= num as f64;
    }
}
