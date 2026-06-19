//! Benchmarks for the RPS lookup hot path: [`Generator::current_at`].
//!
//! `run_generator` polls the active generator every 10ms, but the lookup math
//! itself is the inner-loop-adjacent cost we care about. We bench the concrete
//! `LineGenerator` (covers `const`, which is a degenerate line) and
//! `SinGenerator` (the interesting phase/`sin` math).
//!
//! Time is pinned: we `activate_at(base)` once at setup and call
//! `current_at(base + offset)` with a fixed offset so no `Instant::now()` is
//! read inside the measured closure. The offset sits well inside the
//! generator's duration so `current_at` returns `Some(..)` (the live branch).

use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use criterion::{criterion_group, criterion_main, Criterion};
use dwd_core::generator::{Generator, LineGenerator, SinGenerator};

fn bench_generator(c: &mut Criterion) {
    let mut group = c.benchmark_group("generator");

    let base = Instant::now();
    // Fixed query point, half-way through a 100s generation window.
    let at = base + Duration::from_secs(50);

    // Constant: a line with equal endpoints.
    group.bench_function("const_current_at", |b| {
        let mut g = LineGenerator::new(1_000, 1_000, Duration::from_secs(100));
        g.activate_at(base);
        b.iter(|| black_box(g.current_at(black_box(at))));
    });

    // Ramping line: distinct endpoints exercise the slope arithmetic.
    group.bench_function("line_current_at", |b| {
        let mut g = LineGenerator::new(1_000, 1_000_000, Duration::from_secs(100));
        g.activate_at(base);
        b.iter(|| black_box(g.current_at(black_box(at))));
    });

    // Sin: midpoint + amplitude * sin(k * t + phase). The transcendental sin
    // call dominates and is the reason this path is benched separately.
    group.bench_function("sin_current_at", |b| {
        // pps_mid = 500_500, a = 499_500, one radian/sec, zero phase.
        let mut g = SinGenerator::new(500_500.0, 499_500.0, 1.0, 0.0, Duration::from_secs(100));
        g.activate_at(base);
        b.iter(|| black_box(g.current_at(black_box(at))));
    });

    group.finish();
}

criterion_group!(benches, bench_generator);
criterion_main!(benches);
