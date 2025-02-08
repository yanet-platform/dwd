use core::{
    cell::UnsafeCell,
    fmt::Display,
    net::SocketAddr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use std::{io::Error, net::UdpSocket, sync::Arc};

use crate::{
    shaper::Shaper,
    stat::{CommonStat, SocketStat, TxStat},
    Produce,
};

#[derive(Debug)]
pub struct UdpWorker<B, D> {
    /// Target endpoint.
    addr: SocketAddr,
    /// Bind endpoints.
    bind: B,
    /// Data to send.
    data: D,
    /// Currently active socket.
    sock: Option<UdpSocket>,
    /// The number of requests after which the socket will be recreated.
    requests_per_sock: u64,
    /// Number of requests done for the currently active socket.
    ///
    /// Must be reset to zero when a new socket is created.
    requests_per_sock_done: u64,
    /// Whether this worker is still active.
    is_running: Arc<AtomicBool>,
    /// The shaper.
    shaper: Shaper,
    /// Runtime statistics.
    stat: Arc<LocalStat>,
}

impl<B, D> UdpWorker<B, D> {
    pub fn new(addr: SocketAddr, bind: B, data: D, is_running: Arc<AtomicBool>, limit: Arc<AtomicU64>) -> Self {
        let shaper = Shaper::new(0, limit);

        Self {
            addr,
            bind,
            data,
            sock: None,
            requests_per_sock: u64::MAX,
            requests_per_sock_done: 0,
            is_running,
            shaper,
            stat: Arc::new(LocalStat::default()),
        }
    }

    pub fn stat(&self) -> Arc<LocalStat> {
        self.stat.clone()
    }

    pub fn with_requests_per_sock(mut self, requests_per_sock: u64) -> Self {
        self.requests_per_sock = requests_per_sock;
        self
    }
}

impl<B, D> UdpWorker<B, D>
where
    B: Produce<Item = SocketAddr>,
    D: Produce<Item = [u8]>,
{
    pub fn run(mut self) {
        while self.is_running.load(Ordering::Relaxed) {
            match self.shaper.tick() {
                0 => Self::wait(),
                n => {
                    for _ in 0..n {
                        self.execute();
                    }

                    self.shaper.consume(n);
                }
            }
        }
    }

    /// Performs a single UDP request.
    // Note: this is the rare beneficial case of using the always-inline
    // attribute.
    #[inline(always)]
    fn execute(&mut self) {
        // Obtain either the current or a new socket to be used.
        //
        // Note, that at this moment the corresponding struct's field is set
        // to `None`, meaning that we must return it back to be reused, if we
        // want to.
        let sock = match self.curr_sock() {
            Ok(sock) => sock,
            Err(err) => {
                self.stat.on_sock_err(&err);
                return;
            }
        };

        let data = self.data.next();
        match sock.send(data) {
            Ok(..) => {
                self.requests_per_sock_done += 1;
                if self.requests_per_sock_done < self.requests_per_sock {
                    // Reuse the socket if we're still under the max-requests-per-socket
                    // condition.
                    self.sock = Some(sock);
                }

                self.stat.on_requests(1);
                self.stat.on_send(data.len() as u64);
            }
            Err(err) => {
                self.stat.on_sock_err(&err);
            }
        }
    }

    /// Takes and returns the currently active socket, if it exists. Otherwise,
    /// creates and binds a new socket.
    #[inline]
    fn curr_sock(&mut self) -> Result<UdpSocket, Error> {
        let sock = match self.sock.take() {
            Some(sock) => sock,
            None => {
                let bind = self.bind.next();
                let sock = UdpSocket::bind(bind)?;
                // We "connect" the socket to avoid passing address each time
                // we send a datagram.
                sock.connect(self.addr)?;
                self.requests_per_sock_done = 0;
                self.stat.on_sock_created();

                sock
            }
        };

        Ok(sock)
    }

    #[inline]
    fn wait() {
        std::thread::sleep(Duration::from_micros(1));
    }
}

#[derive(Debug, Default)]
pub struct LocalStat {
    num_requests: UnsafeCell<u64>,
    bytes_tx: UnsafeCell<u64>,
    num_sock_created: UnsafeCell<u64>,
    num_sock_errors: UnsafeCell<u64>,
}

unsafe impl Sync for LocalStat {}

impl LocalStat {
    #[inline]
    pub fn on_requests(&self, n: u64) {
        unsafe { *self.num_requests.get() += n };
    }

    #[inline]
    pub fn on_send(&self, n: u64) {
        unsafe { *self.bytes_tx.get() += n };
    }

    #[inline]
    pub fn on_sock_created(&self) {
        unsafe { *self.num_sock_created.get() += 1 };
    }

    #[inline]
    pub fn on_sock_err<E: Display>(&self, err: &E) {
        log::error!("{}", err);
        unsafe { *self.num_sock_errors.get() += 1 };
    }
}

#[derive(Debug, Default)]
pub struct Stat {
    generator: AtomicU64,
    stats: Vec<Arc<LocalStat>>,
}

impl Stat {
    pub fn new(stats: Vec<Arc<LocalStat>>) -> Self {
        Self { stats, ..Default::default() }
    }
}

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

impl TxStat for Stat {
    #[inline]
    fn num_requests(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.num_requests.get() }).sum()
    }

    #[inline]
    fn bytes_tx(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.bytes_tx.get() }).sum()
    }
}

impl SocketStat for Stat {
    #[inline]
    fn num_sock_created(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.num_sock_created.get() }).sum()
    }

    #[inline]
    fn num_sock_errors(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.num_sock_errors.get() }).sum()
    }
}
