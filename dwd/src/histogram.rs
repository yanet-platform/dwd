use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct LogHistogram {
    bounds: Vec<f64>,
    buckets: Vec<AtomicU64>,
    factor: f64,
}

impl Default for LogHistogram {
    fn default() -> Self {
        let mut bounds = Vec::new();
        let mut buckets = Vec::new();
        let max = 60000000.0; // 60s
        let factor = 1.5;
        let mut curr = 1.0;
        loop {
            if curr >= max {
                break;
            }

            bounds.push(curr);
            buckets.push(AtomicU64::new(0));
            curr *= factor;
        }

        Self { bounds, buckets, factor }
    }
}

impl LogHistogram {
    /// Returns buckets' upper bounds.
    #[inline]
    pub fn upper_bounds(&self) -> &[f64] {
        &self.bounds
    }

    #[inline]
    pub fn snapshot(&self) -> (&[f64], Vec<u64>) {
        (
            self.upper_bounds(),
            self.buckets.iter().map(|v| v.load(Ordering::Relaxed)).collect(),
        )
    }

    #[inline]
    pub fn record(&self, us: u64) {
        let idx = (us as f64).log(self.factor) as usize;
        let idx = idx.min(self.buckets.len() - 1);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Calculates the quantile.
    ///
    /// Suppose we have the following histogram, in linear coordinates:
    /// +---+------+--------+----------------+-----------+
    /// | i | b[i] | sum[i] | Duration range | Histogram |
    /// +---+------+--------+----------------+-----------+
    /// | 0 | 2    | 2      | [0; f^0)       | **        |
    /// | 1 | 1    | 3      | [f^0; f^1)     | *         |
    /// | 2 | 9    | 12     | [f^1; f^2)     | ********* |
    /// | 3 | 4    | 16     | [f^2; f^3)     | ****      |
    /// | 4 | 3    | 19     | [f^3; f^4)     | ***       |
    /// | 5 | 0    | 19     | [f^4; f^5)     |           |
    /// | 6 | 1    | 20     | [f^5; f^6)     | *         |
    /// | . | .... | ...... | ..........     |           |
    /// | N | 4    | 1000   | [f^(N-1); Inf  | ****      |
    /// +---+------+--------+----------------+-----------+
    ///
    /// For given quantile "q" have to find the first index "i",
    /// where sum[i] + b[i] >= q * sum[N].
    /// Thus, we can guarantee that sum[i] requests are under the given
    /// quantile.
    /// So we can return the corresponding duration range upper
    /// bound: "f^i".
    /// But we can go further and improve our result by performing linear
    /// interpolation in logarithmic coordinates by base "f":
    ///
    /// |        o sum[i+1]
    /// |       /|
    /// |      / |
    /// |     x  |
    /// |    /^--|-- q * sum[N]
    /// |   / |  |
    /// |  /  |  |
    /// | /   |  |
    /// |/    |  |
    /// o sum[i] |
    /// |     |  |
    /// i     x  i+1
    ///
    /// Let's build two linear equations with known boundary conditions:
    ///
    /// sum[i]   = k * i     + c
    /// sum[i+1] = k * (i+1) + c
    ///
    /// Solve them, find "k" and "c":
    ///
    /// c = sum[i] - k * i
    /// sum[i+1] = k * (i+1) + (sum[i] - k * i)
    /// k = (sum[i+1] - sum[i]) / ((i+1)-i)
    /// k = (sum[i+1] - sum[i])
    /// k = (sum[i] + b[i] - sum[i])
    /// k = b[i]
    ///
    /// Then, find the desired pseudo-index "x":
    ///
    /// q * sum[N] = b[i] * x + (sum[i] - b[i] * i)
    /// x = (q * sum[N] - (sum[i] - b[i] * i)) / b[i]
    ///
    /// The result will be "f^x", because we need to return from logarithmic
    /// coordinates to normal.
    pub fn quantile(&self, q: f64) -> u64 {
        assert!((0.0..=1.0).contains(&q));

        let snapshot: Vec<u64> = self.buckets.iter().map(|v| v.load(Ordering::Relaxed)).collect();
        let size: u64 = snapshot.iter().sum();

        let mut sum = 0;
        for (idx, &b) in snapshot.iter().enumerate() {
            if ((sum + b) as f64) >= q * (size as f64) {
                // Found upper bound, let's interpolate now.

                let idx = idx as f64;
                let b = b as f64;
                let sum = sum as f64;
                let size = size as f64;
                let c_inv = |q: f64| self.factor.powf((q * size - sum) / b + idx);

                return c_inv(q) as u64;
            }
            sum += b;
        }

        u64::MAX
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_zero_quantile() {
        let h = LogHistogram::default();
        h.record(1000);
        assert_eq!(0, h.quantile(0.0));
    }

    #[test]
    fn test_low_bound_quantile() {
        let cases: &[[u64; 45]] = &[
            [
                213, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 151, 0, 0, 0, 0, 0, 36, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            [
                319, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 57, 0, 0, 0, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            [
                182, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 12, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        ];
        for c in cases {
            let h = LogHistogram {
                buckets: c.iter().copied().map(AtomicU64::new).collect(),
                ..Default::default()
            };
            assert_eq!(h.quantile(0.10), 1);
            assert_eq!(h.quantile(0.50), 1);
        }
    }
}