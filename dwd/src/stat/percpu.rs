use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU64, Ordering},
};
use std::{sync::Arc, time::Instant};

use super::{CommonStat, HttpStat, RxStat, SocketStat, TxStat};
use crate::histogram::{LogHistogram, PerCpuLogHistogram};

#[derive(Debug)]
pub struct Stat<T, R, S, H> {
    generator: AtomicU64,
    pub stats: Vec<Arc<PerCpuStat<T, R, S, H>>>,
}

impl<T, R, S, H> Stat<T, R, S, H>
where
    T: Default,
    R: Default,
    S: Default,
    H: Default,
{
    pub fn new(stats: Vec<Arc<PerCpuStat<T, R, S, H>>>) -> Self {
        Self { generator: AtomicU64::new(0), stats }
    }
}

impl<T, R, S, H> CommonStat for Stat<T, R, S, H> {
    #[inline]
    fn generator(&self) -> u64 {
        self.generator.load(Ordering::Relaxed)
    }

    #[inline]
    fn on_generator(&self, v: u64) {
        self.generator.store(v, Ordering::Relaxed);
    }
}

impl<R, S, H> TxStat for Stat<TxWorkerStat, R, S, H> {
    #[inline]
    fn num_requests(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.tx.num_requests.get() }).sum()
    }

    #[inline]
    fn bytes_tx(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.tx.bytes_tx.get() }).sum()
    }
}

impl<T, S, H> RxStat for Stat<T, RxWorkerStat, S, H> {
    #[inline]
    fn num_responses(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.rx.num_responses.get() }).sum()
    }

    #[inline]
    fn num_timeouts(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.rx.num_timeouts.get() }).sum()
    }

    #[inline]
    fn bytes_rx(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.rx.bytes_rx.get() }).sum()
    }

    #[inline]
    fn hist(&self) -> LogHistogram {
        let mut snapshot = vec![0u64; self.stats.len()];
        for s in &self.stats {
            for (idx, b) in s.rx.hist.buckets().iter().enumerate() {
                snapshot[idx] += unsafe { *b.get() };
            }
        }

        LogHistogram::new(snapshot)
    }
}

impl<T, R, H> SocketStat for Stat<T, R, SockWorkerStat, H> {
    #[inline]
    fn num_sock_created(&self) -> u64 {
        self.stats
            .iter()
            .map(|v| unsafe { *v.sock.num_sock_created.get() })
            .sum()
    }

    #[inline]
    fn num_sock_errors(&self) -> u64 {
        self.stats
            .iter()
            .map(|v| unsafe { *v.sock.num_sock_errors.get() })
            .sum()
    }
}

impl<T, R, S> HttpStat for Stat<T, R, S, HttpWorkerStat> {
    #[inline]
    fn num_2xx(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.http.num_2xx.get() }).sum()
    }

    #[inline]
    fn num_3xx(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.http.num_3xx.get() }).sum()
    }

    #[inline]
    fn num_4xx(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.http.num_4xx.get() }).sum()
    }

    #[inline]
    fn num_5xx(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.http.num_5xx.get() }).sum()
    }
}

#[derive(Debug, Default)]
pub struct PerCpuStat<T = (), R = (), S = (), H = ()> {
    tx: T,
    rx: R,
    sock: S,
    http: H,
}

impl<R, S, H> PerCpuStat<TxWorkerStat, R, S, H> {
    /// Increases the number of requests made by the given value.
    ///
    /// Should be called after each successful request transmitted.
    #[inline]
    pub fn on_requests(&self, v: u64) {
        unsafe { *self.tx.num_requests.get() += v };
    }

    #[inline]
    pub fn on_send(&self, n: u64) {
        unsafe { *self.tx.bytes_tx.get() += n };
    }
}

impl<T, S, H> PerCpuStat<T, RxWorkerStat, S, H> {
    /// Increases the number of responses.
    ///
    /// Should be called after each successful response received.
    #[inline]
    pub fn on_response(&self, now: &Instant) {
        unsafe { *self.rx.num_responses.get() += 1 };
        self.rx.hist.record(now.elapsed().as_micros() as u64);
    }

    #[inline]
    pub fn on_recv(&self, n: u64) {
        unsafe { *self.rx.bytes_rx.get() += n };
    }

    #[inline]
    pub fn on_timeout(&self, now: &Instant) {
        unsafe { *self.rx.num_timeouts.get() += 1 };
        self.rx.hist.record(now.elapsed().as_micros() as u64);
    }
}

impl<T, R, H> PerCpuStat<T, R, SockWorkerStat, H> {
    /// Increases the number of sockets created.
    #[inline]
    pub fn on_sock_created(&self) {
        unsafe { *self.sock.num_sock_created.get() += 1 };
    }

    /// Increases the number of socket errors.
    #[inline]
    pub fn on_sock_err(&self) {
        unsafe { *self.sock.num_sock_errors.get() += 1 };
    }
}

impl<T, R, S> PerCpuStat<T, R, S, HttpWorkerStat> {
    /// Increases the number of 2xx responses by the given value.
    #[inline]
    pub fn on_2xx(&self) {
        unsafe { *self.http.num_2xx.get() += 1 };
    }

    /// Increases the number of 3xx responses by the given value.
    #[inline]
    pub fn on_3xx(&self) {
        unsafe { *self.http.num_3xx.get() += 1 };
    }

    /// Increases the number of 4xx responses by the given value.
    #[inline]
    pub fn on_4xx(&self) {
        unsafe { *self.http.num_4xx.get() += 1 };
    }

    /// Increases the number of 5xx responses by the given value.
    #[inline]
    pub fn on_5xx(&self) {
        unsafe { *self.http.num_5xx.get() += 1 };
    }
}

unsafe impl<T, R, S, H> Sync for PerCpuStat<T, R, S, H>
where
    T: Sync,
    R: Sync,
    S: Sync,
    H: Sync,
{
}

/// Per-worker transmission statistics.
#[derive(Debug, Default)]
pub struct TxWorkerStat {
    /// Number of requests made.
    num_requests: UnsafeCell<u64>,
    /// Number of bytes transmitted.
    bytes_tx: UnsafeCell<u64>,
}

unsafe impl Sync for TxWorkerStat {}

/// Per-worker reception statistics.
#[derive(Debug, Default)]
pub struct RxWorkerStat {
    /// Number of responses received.
    num_responses: UnsafeCell<u64>,
    /// Number of bytes received.
    bytes_rx: UnsafeCell<u64>,
    /// Number of timeouts.
    num_timeouts: UnsafeCell<u64>,
    /// Response times histogram.
    hist: PerCpuLogHistogram,
}

unsafe impl Sync for RxWorkerStat {}

/// Per-worker socket statistics.
#[derive(Debug, Default)]
pub struct SockWorkerStat {
    /// Number of sockets created.
    num_sock_created: UnsafeCell<u64>,
    /// Number of socket errors.
    num_sock_errors: UnsafeCell<u64>,
}

unsafe impl Sync for SockWorkerStat {}

/// Per-worker HTTP statistics.
#[derive(Debug, Default)]
pub struct HttpWorkerStat {
    /// Number of 2xx responses.
    num_2xx: UnsafeCell<u64>,
    /// Number of 3xx responses.
    num_3xx: UnsafeCell<u64>,
    /// Number of 4xx responses.
    num_4xx: UnsafeCell<u64>,
    /// Number of 5xx responses.
    num_5xx: UnsafeCell<u64>,
}

unsafe impl Sync for HttpWorkerStat {}
