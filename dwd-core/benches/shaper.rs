//! Benchmarks for the token-bucket [`Shaper`] — the rate limiter every engine
//! drives once per worker loop tick.
//!
//! The hot pattern in a worker is `tick()` (mint tokens from elapsed time) →
//! execute up to N tasks → `consume(n)` (debit). We bench `tick`, `consume`,
//! and the combined `tick + consume` cycle.
//!
//! `Shaper::tick()` reads `Instant::now()` internally, so the measured closure
//! includes one clock read per call (intrinsic to the function under test).
//! `consume()` is pure arithmetic and is benched in isolation.

use std::{
    hint::black_box,
    sync::{atomic::AtomicU64, Arc},
};

use criterion::{criterion_group, criterion_main, Criterion};
use dwd_core::shaper::Shaper;

fn bench_shaper(c: &mut Criterion) {
    let mut group = c.benchmark_group("shaper");

    // tick() in isolation: mints tokens from the elapsed wall-clock time and
    // returns the number available. Includes one Instant::now() read.
    group.bench_function("tick", |b| {
        let limit = Arc::new(AtomicU64::new(1_000_000));
        let mut shaper = Shaper::new(black_box(1), limit);
        b.iter(|| black_box(shaper.tick()));
    });

    // consume() in isolation: pure floating-point debit, no clock read.
    // The internal token count is an f64 that simply drifts negative over the
    // loop; consume() never branches on it, so the work per call is constant
    // and underflow is harmless (no panic, no behavior change).
    group.bench_function("consume", |b| {
        let limit = Arc::new(AtomicU64::new(1_000_000));
        let mut shaper = Shaper::new(1, limit);
        b.iter(|| shaper.consume(black_box(1)));
    });

    // The realistic worker cycle: tick then consume what we got. Burst size 1
    // keeps tick() returning a positive token count for steady-state behavior.
    group.bench_function("tick_consume", |b| {
        let limit = Arc::new(AtomicU64::new(1_000_000));
        let mut shaper = Shaper::new(1, limit);
        b.iter(|| {
            let n = black_box(shaper.tick());
            shaper.consume(black_box(n));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_shaper);
criterion_main!(benches);
