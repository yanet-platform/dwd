use core::{
    fmt::Display,
    sync::atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use crate::{
    histogram::LogHistogram,
    stat::{CommonStat, RxStat, SocketStat, TxStat},
};

#[derive(Debug, Default)]
pub struct Stat {
    generator: AtomicU64,
    num_requests: AtomicU64,
    num_responses: AtomicU64,
    /// The number of sockets created and initialized.
    ///
    /// By initialization we mean, for example, socket creation, binding,
    /// successful connection establishing for TCP and other syscalls made
    /// before a socket can be used for communication.
    num_sock_created: AtomicU64,
    /// The number of errors occurred during socket usage.
    ///
    /// This can be either initialization errors like resource exhaustion, or
    /// communication errors.
    num_sock_errors: AtomicU64,
    num_timeouts: AtomicU64,
    num_2xx: AtomicU64,
    num_3xx: AtomicU64,
    num_4xx: AtomicU64,
    num_5xx: AtomicU64,
    bytes_tx: AtomicU64,
    bytes_rx: AtomicU64,
    hist: LogHistogram,
}

impl Stat {
    /// Increases the number of requests made by the given value.
    ///
    /// Should be called after each successful request transmitted.
    #[inline]
    pub fn on_requests(&self, v: u64) {
        self.num_requests.fetch_add(v, Ordering::Relaxed);
    }

    #[inline]
    pub fn on_xxx(&self, now: &Instant) {
        self.num_responses.fetch_add(1, Ordering::Relaxed);
        self.hist.record(now.elapsed().as_micros() as u64);
    }

    #[inline]
    pub fn on_2xx(&self, now: &Instant) {
        self.on_xxx(now);
        self.num_2xx.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn on_3xx(&self, now: &Instant) {
        self.on_xxx(now);
        self.num_3xx.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn on_4xx(&self, now: &Instant) {
        self.on_xxx(now);
        self.num_4xx.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn on_5xx(&self, now: &Instant) {
        self.on_xxx(now);
        self.num_5xx.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn on_send(&self, n: u64) {
        self.bytes_tx.fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    pub fn on_recv(&self, n: u64) {
        self.bytes_rx.fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    pub fn on_sock_created(&self) {
        self.num_sock_created.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn on_sock_err<E: Display>(&self, err: &E) {
        log::error!("{}", err);
        self.num_sock_errors.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn on_timeout(&self, now: &Instant) {
        self.on_xxx(now);
        self.num_timeouts.fetch_add(1, Ordering::Relaxed);
    }
}

impl TxStat for Stat {
    #[inline]
    fn num_requests(&self) -> u64 {
        self.num_requests.load(Ordering::Relaxed)
    }

    #[inline]
    fn bytes_tx(&self) -> u64 {
        self.bytes_tx.load(Ordering::Relaxed)
    }
}

impl RxStat for Stat {
    #[inline]
    fn num_responses(&self) -> u64 {
        self.num_responses.load(Ordering::Relaxed)
    }

    #[inline]
    fn num_timeouts(&self) -> u64 {
        self.num_timeouts.load(Ordering::Relaxed)
    }

    #[inline]
    fn bytes_rx(&self) -> u64 {
        self.bytes_rx.load(Ordering::Relaxed)
    }

    #[inline]
    fn hist(&self) -> &LogHistogram {
        &self.hist
    }
}

impl SocketStat for Stat {
    #[inline]
    fn num_sock_created(&self) -> u64 {
        self.num_sock_created.load(Ordering::Relaxed)
    }

    #[inline]
    fn num_sock_errors(&self) -> u64 {
        self.num_sock_errors.load(Ordering::Relaxed)
    }
}

// impl HttpStat for Stat {
//     #[inline]
//     fn num_2xx(&self) -> u64 {
//         self.num_2xx.load(Ordering::Relaxed)
//     }

//     #[inline]
//     fn num_3xx(&self) -> u64 {
//         self.num_3xx.load(Ordering::Relaxed)
//     }

//     #[inline]
//     fn num_4xx(&self) -> u64 {
//         self.num_4xx.load(Ordering::Relaxed)
//     }

//     #[inline]
//     fn num_5xx(&self) -> u64 {
//         self.num_5xx.load(Ordering::Relaxed)
//     }
// }

impl CommonStat for Stat {
    #[inline]
    fn generator(&self) -> u64 {
        self.generator.load(Ordering::Relaxed)
    }

    #[inline]
    fn on_generator(&self, v: u64) {
        self.generator.store(v, Ordering::Relaxed);
    }
}
