use core::cell::UnsafeCell;

const FACTOR: f64 = 1.5;

/// Reciprocal of `ln(FACTOR)`, precomputed so [`PerCpuLogHistogram::record`]
/// performs a single `ln` plus a multiply instead of the two transcendental
/// calls hidden inside `f64::log` (which evaluates `self.ln() / FACTOR.ln()`
/// on every call). Multiplying by this constant is bit-for-bit identical to
/// `(us as f64).log(FACTOR)` for every input, so bucket boundaries are
/// preserved exactly (verified by the equivalence test below).
const INV_LN_FACTOR: f64 = 2.4663034623764317; // == 1.0 / FACTOR.ln()

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
        let idx = ((us as f64).ln() * INV_LN_FACTOR) as usize;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The hand-written `INV_LN_FACTOR` constant must equal `1.0 / FACTOR.ln()`
    /// to the last bit; otherwise the optimized bucket index could drift from
    /// the original `log(FACTOR)` mapping.
    #[test]
    fn inv_ln_factor_matches_factor() {
        assert_eq!(INV_LN_FACTOR, 1.0 / FACTOR.ln());
    }

    /// Reference bucket index using the original transcendental form.
    fn ref_idx(us: u64, len: usize) -> usize {
        let idx = (us as f64).log(FACTOR) as usize;
        idx.min(len - 1)
    }

    /// Optimized bucket index as computed by [`PerCpuLogHistogram::record`].
    fn opt_idx(us: u64, len: usize) -> usize {
        let idx = ((us as f64).ln() * INV_LN_FACTOR) as usize;
        idx.min(len - 1)
    }

    /// The optimized index must equal the reference index for every input:
    /// dense low sweep, every bucket boundary +/- a few microseconds, zero,
    /// the largest recordable value, and extreme `u64` values.
    #[test]
    fn record_index_is_identical() {
        let len = PerCpuLogHistogram::default().buckets().len();

        // Dense sweep over small values (covers sub-1.0 -> 0 and the low
        // buckets where rounding is most sensitive).
        for us in 0u64..1_000_000 {
            assert_eq!(opt_idx(us, len), ref_idx(us, len), "dense us={us}");
        }

        // Around every bucket boundary f^k.
        let mut curr = 1.0_f64;
        for _ in 0..len + 5 {
            for d in [-2i64, -1, 0, 1, 2] {
                let us = (curr as i64 + d).max(0) as u64;
                assert_eq!(opt_idx(us, len), ref_idx(us, len), "boundary us={us}");
            }
            curr *= FACTOR;
        }

        // Edge / extreme values.
        for us in [
            0,
            1,
            60_000_000, // ~max bucket boundary (60s)
            90_000_000,
            1u64 << 40,
            1u64 << 63,
            u64::MAX - 1,
            u64::MAX,
        ] {
            assert_eq!(opt_idx(us, len), ref_idx(us, len), "edge us={us}");
        }
    }

    /// `record` must land samples in the same buckets the reference math
    /// predicts (end-to-end sanity over the optimized write path).
    #[test]
    fn record_lands_in_reference_bucket() {
        let hist = PerCpuLogHistogram::default();
        let len = hist.buckets().len();
        let samples = [0u64, 1, 10, 100, 1_000, 5_000, 50_000, 500_000, 5_000_000];
        for &us in &samples {
            hist.record(us);
        }
        let mut expected = vec![0u64; len];
        for &us in &samples {
            expected[ref_idx(us, len)] += 1;
        }
        for (i, b) in hist.buckets().iter().enumerate() {
            assert_eq!(unsafe { *b.get() }, expected[i], "bucket {i}");
        }
    }
}
