use core::cell::UnsafeCell;

const FACTOR: f64 = 1.5;

#[derive(Debug)]
pub struct PerCpuLogHistogram {
    buckets: Vec<UnsafeCell<u64>>,
}

impl PerCpuLogHistogram {
    pub fn buckets(&self) -> &[UnsafeCell<u64>] {
        &self.buckets
    }

    #[inline]
    pub fn record(&self, us: u64) {
        let idx = (us as f64).log(FACTOR) as usize;
        let idx = idx.min(self.buckets.len() - 1);

        unsafe { *self.buckets[idx].get() += 1 };
    }
}

impl Default for PerCpuLogHistogram {
    fn default() -> Self {
        let mut buckets = Vec::new();
        let max = 60000000.0; // 60s
        let mut curr = 1.0;
        loop {
            if curr >= max {
                break;
            }

            buckets.push(UnsafeCell::new(0));
            curr *= FACTOR;
        }

        Self { buckets }
    }
}

#[derive(Debug)]
pub struct LogHistogram {
    snapshot: Vec<u64>,
}

impl LogHistogram {
    #[inline]
    pub const fn new(snapshot: Vec<u64>) -> Self {
        Self { snapshot }
    }

    /// Returns the raw snapshot of bucket counts.
    #[inline]
    pub fn snapshot(&self) -> &[u64] {
        &self.snapshot
    }

    /// Returns the logarithmic factor used for bucket boundaries.
    #[inline]
    pub const fn factor() -> f64 {
        FACTOR
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

        let size: u64 = self.snapshot.iter().sum();

        let mut sum = 0;
        for (idx, &b) in self.snapshot.iter().enumerate() {
            if ((sum + b) as f64) >= q * (size as f64) {
                // Found upper bound, let's interpolate now.

                let idx = idx as f64;
                let b = b as f64;
                let sum = sum as f64;
                let size = size as f64;
                let c_inv = |q: f64| FACTOR.powf((q * size - sum) / b + idx);

                return c_inv(q) as u64;
            }
            sum += b;
        }

        u64::MAX
    }
}
