//! Benchmarks for log-bucketed latency: [`PerCpuLogHistogram::record`] (the
//! per-request write side) and [`LogHistogram::quantile`] (the read side used
//! by the TUI / metrics).
//!
//! `record(us)` computes `log_FACTOR(us)`, clamps to the bucket count, and
//! bumps a counter — one `log()` call per request. We feed a fixed, cycling
//! set of microsecond values (set up outside the loop) so the measured work is
//! deterministic and allocation-free.
//!
//! `quantile(q)` scans the snapshot and interpolates. We build a realistic
//! filled snapshot once and query a few representative quantiles.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use dwd_core::histogram::{LogHistogram, PerCpuLogHistogram};

fn bench_histogram(c: &mut Criterion) {
    let mut group = c.benchmark_group("histogram");

    // Representative latency samples in microseconds spanning the bucket range
    // (sub-microsecond up to multi-second). Precomputed, no allocation in loop.
    let samples: [u64; 8] = [1, 10, 100, 1_000, 5_000, 50_000, 500_000, 5_000_000];

    group.bench_function("record", |b| {
        let hist = PerCpuLogHistogram::default();
        let mut i = 0usize;
        b.iter(|| {
            // Cycle through the fixed samples branchlessly via masking-free mod;
            // the index update is cheap relative to the log() in record().
            let us = samples[i % samples.len()];
            hist.record(black_box(us));
            i = i.wrapping_add(1);
        });
    });

    // Build a filled snapshot once: distribute counts across the buckets so
    // quantile() walks a realistic distribution rather than a single spike.
    let n_buckets = PerCpuLogHistogram::default().buckets().len();
    let snapshot: Vec<u64> = (0..n_buckets).map(|i| (i as u64 % 7) + 1).collect();
    let hist = LogHistogram::new(snapshot);

    for &q in &[0.5_f64, 0.9, 0.99, 0.999] {
        group.bench_function(format!("quantile_p{}", (q * 1000.0) as u64), |b| {
            b.iter(|| black_box(hist.quantile(black_box(q))));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_histogram);
criterion_main!(benches);
