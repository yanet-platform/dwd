use core::{
    future::Future,
    net::SocketAddr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use std::{net::UdpSocket, sync::Arc, thread};

use anyhow::Error;

use super::Config;
use crate::{
    engine::runtime::ThreadPool,
    shaper::Shaper,
    stat::{PerCpuStat, SockWorkerStat, Stat, TxWorkerStat},
    OneProduce, Produce,
};

type WorkerStat = PerCpuStat<TxWorkerStat, (), SockWorkerStat, (), ()>;
type EngineStat = Stat<TxWorkerStat, (), SockWorkerStat, (), ()>;

#[derive(Debug)]
pub struct Engine {
    cfg: Config,
    /// RPS limits, per CPU.
    limits: Vec<Arc<AtomicU64>>,
    stat: Arc<EngineStat>,
}

impl Engine {
    pub fn new(cfg: Config) -> Self {
        let num_threads = cfg.native.threads.get();

        let mut limits = Vec::with_capacity(num_threads);
        let mut stats = Vec::new();
        for _ in 0..num_threads {
            limits.push(Arc::new(AtomicU64::new(0)));
            stats.push(Arc::new(WorkerStat::default()));
        }

        let stat = Arc::new(EngineStat::new(stats));

        Self { cfg, limits, stat }
    }

    #[inline]
    pub fn limits(&self) -> Vec<Arc<AtomicU64>> {
        self.limits.clone()
    }

    #[inline]
    pub fn stat(&self) -> Arc<EngineStat> {
        self.stat.clone()
    }

    pub fn run<F>(self, _stop: F, is_running: Arc<AtomicBool>) -> Result<(), Error>
    where
        F: Future<Output = ()> + 'static,
    {
        let num_threads = self.cfg.native.threads;

        let bind = self.cfg.native.bind_endpoints.clone();
        let data = Arc::new(OneProduce::new(b"GET / HTTP/1.1\r\n\r\n".to_vec()));

        let rt = ThreadPool::new(num_threads, move |tid: usize| {
            let bind = bind.clone();
            let data = data.clone();
            let stat = self.stat.stats[tid].clone();
            let is_running = is_running.clone();
            let limits = self.limits[tid].clone();

            let mut worker = Worker::new(
                self.cfg.addr,
                bind.clone(),
                data.clone(),
                self.cfg.native.requests_per_socket(),
                limits,
                is_running.clone(),
                stat,
            );

            move || worker.run()
        });

        rt.run()?;

        Ok(())
    }
}

/// Per-thread worker.
#[derive(Debug)]
pub struct Worker<B, D> {
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
    stat: Arc<WorkerStat>,
}

impl<B, D> Worker<B, D> {
    pub fn new(
        addr: SocketAddr,
        bind: B,
        data: D,
        requests_per_sock: u64,
        limit: Arc<AtomicU64>,
        is_running: Arc<AtomicBool>,
        stat: Arc<WorkerStat>,
    ) -> Self {
        let shaper = Shaper::new(0, limit);

        Self {
            addr,
            bind,
            data,
            sock: None,
            requests_per_sock,
            requests_per_sock_done: 0,
            is_running,
            shaper,
            stat,
        }
    }
}

impl<B, D> Worker<B, D>
where
    B: Produce<Item = SocketAddr>,
    D: Produce<Item = Vec<u8>>,
{
    pub fn run(&mut self) -> Result<(), Error> {
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

        Ok(())
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
            Err(..) => {
                self.stat.on_sock_err();
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
            Err(..) => {
                self.stat.on_sock_err();
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
        thread::sleep(Duration::from_micros(1));
    }
}
