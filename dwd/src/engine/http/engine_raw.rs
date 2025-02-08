use core::{
    future::Future,
    net::SocketAddr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use std::{sync::Arc, thread, time::Instant};

use anyhow::Error;
use bytes::{Bytes, BytesMut};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpSocket, TcpStream},
    runtime::Builder,
};

use super::cfg::Config;
use crate::{
    shaper::Shaper,
    stat::{HttpWorkerStat, PerCpuStat, RxWorkerStat, SockWorkerStat, Stat, TxWorkerStat},
    Produce, VecProduce,
};

type WorkerStat = PerCpuStat<TxWorkerStat, RxWorkerStat, SockWorkerStat, HttpWorkerStat>;
type EngineStat = Stat<TxWorkerStat, RxWorkerStat, SockWorkerStat, HttpWorkerStat>;

// TODO: it is possible to unify with `Engine`.
#[derive(Debug)]
pub struct EngineRaw {
    cfg: Config<Bytes>,
    limits: Vec<Vec<Arc<AtomicU64>>>,
    stat: Arc<EngineStat>,
}

impl EngineRaw {
    pub fn new(cfg: Config<Bytes>) -> Self {
        let num_jobs = cfg.concurrency.get();
        let mut limits = vec![Vec::new(); cfg.native.threads.get()];
        let mut stats = Vec::new();
        for _ in 0..cfg.native.threads.get() {
            stats.push(Arc::new(WorkerStat::default()));
        }

        let mut idx = 0;
        let len = limits.len();
        while idx < num_jobs {
            limits[idx % len].push(Arc::new(AtomicU64::new(0)));
            idx += 1;
        }

        let stat = Arc::new(EngineStat::new(stats));

        Self { cfg, limits, stat }
    }

    #[inline]
    pub fn limits(&self) -> Vec<Vec<Arc<AtomicU64>>> {
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
        let num_threads = self.cfg.native.threads.into();
        let mut threads = Vec::with_capacity(num_threads);

        let bind = self.cfg.native.bind_endpoints.clone();
        let data = Arc::new(VecProduce::new(self.cfg.requests.clone()));

        for (idx, thread_limits) in self.limits.clone().into_iter().enumerate() {
            let thread = {
                let mut worker = Worker::new(
                    self.cfg.clone(),
                    bind.clone(),
                    data.clone(),
                    thread_limits,
                    is_running.clone(),
                    self.stat.stats[idx].clone(),
                );

                thread::Builder::new()
                    .name("dwd:worker".into())
                    .spawn(move || worker.run())?
            };

            threads.push(thread);
        }

        for thread in threads {
            thread.join().expect("no self join")?;
        }

        Ok(())
    }
}

/// Per-thread worker.
#[derive(Debug)]
struct Worker<B, D> {
    cfg: Config<Bytes>,
    /// Bind endpoints.
    bind: B,
    /// Data to send.
    data: D,
    limits: Vec<Arc<AtomicU64>>,
    is_running: Arc<AtomicBool>,
    stat: Arc<WorkerStat>,
}

impl<B, D> Worker<B, D> {
    pub fn new(
        cfg: Config<Bytes>,
        bind: B,
        data: D,
        limits: Vec<Arc<AtomicU64>>,
        is_running: Arc<AtomicBool>,
        stat: Arc<WorkerStat>,
    ) -> Self {
        Self {
            cfg,
            bind,
            data,
            limits,
            is_running,
            stat,
        }
    }
}

impl<B, D> Worker<B, D>
where
    B: Produce<Item = SocketAddr> + Clone + Send + Sync + 'static,
    D: Produce<Item = Bytes> + Clone + Send + Sync + 'static,
{
    pub fn run(&mut self) -> Result<(), Error> {
        let runtime = Builder::new_current_thread().enable_all().build()?;

        let mut jobs = Vec::new();
        for limit in &self.limits {
            let job = CoroWorker::new(
                self.cfg.addr,
                self.bind.clone(),
                self.data.clone(),
                self.cfg.native.requests_per_socket(),
                self.cfg.tcp_linger,
                self.cfg.tcp_no_delay,
                self.stat.clone(),
            );
            let mut job = ShapedCoroWorker::new(job, Shaper::new(0, limit.clone()), self.is_running.clone());

            let job = runtime.spawn(async move {
                job.run().await;
            });

            jobs.push(job);
        }

        runtime.block_on(async move {
            for job in jobs {
                job.await.unwrap();
            }
            Ok(())
        })
    }
}

/// Shaped per-task worker.
#[derive(Debug)]
struct ShapedCoroWorker<B, D> {
    /// Per-task worker.
    worker: CoroWorker<B, D>,
    /// Whether this worker is still active.
    is_running: Arc<AtomicBool>,
    /// The shaper.
    shaper: Shaper,
}

impl<B, D> ShapedCoroWorker<B, D> {
    pub fn new(worker: CoroWorker<B, D>, shaper: Shaper, is_running: Arc<AtomicBool>) -> Self {
        Self { worker, shaper, is_running }
    }
}

impl<B, D> ShapedCoroWorker<B, D>
where
    B: Produce<Item = SocketAddr>,
    D: Produce<Item = Bytes>,
{
    pub async fn run(&mut self) {
        while self.is_running.load(Ordering::Relaxed) {
            match self.shaper.tick() {
                0 => Self::wait().await,
                mut n => {
                    n = n.min(32);

                    for _ in 0..n {
                        self.worker.execute().await;
                    }

                    self.shaper.consume(n);
                }
            }
        }
    }

    #[inline]
    async fn wait() {
        // TODO: park/unpark.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// Per-task worker.
#[derive(Debug)]
struct CoroWorker<B, D> {
    /// Target endpoint.
    addr: SocketAddr,
    /// Bind endpoints.
    bind: B,
    /// Data to send.
    data: D,
    /// Current TCP socket.
    stream: Option<TcpStream>,
    /// Response buffer.
    buf: BytesMut,
    /// The number of requests after which the socket will be recreated.
    requests_per_sock: u64,
    /// Number of requests done for the currently active socket.
    ///
    /// Must be reset to zero when a new socket is created.
    requests_per_sock_done: u64,
    /// Request timeout.
    timeout: Duration,
    /// Set linger TCP option with specified value.
    tcp_linger: Option<u64>,
    /// Enable SOCK_NODELAY socket option.
    tcp_no_delay: bool,
    /// Runtime statistics.
    stat: Arc<WorkerStat>,
}

impl<B, D> CoroWorker<B, D> {
    pub fn new(
        addr: SocketAddr,
        bind: B,
        data: D,
        requests_per_sock: u64,
        tcp_linger: Option<u64>,
        tcp_no_delay: bool,
        stat: Arc<WorkerStat>,
    ) -> Self {
        Self {
            addr,
            bind,
            data,
            stream: None,
            buf: BytesMut::with_capacity(4096),
            requests_per_sock,
            requests_per_sock_done: 0,
            timeout: Duration::from_secs(4),
            tcp_linger,
            tcp_no_delay,
            stat,
        }
    }
}

impl<B, D> CoroWorker<B, D>
where
    B: Produce<Item = SocketAddr>,
    D: Produce<Item = Bytes>,
{
    #[inline]
    pub async fn execute(&mut self) {
        let now = Instant::now();

        if tokio::time::timeout(self.timeout, self.do_execute(&now)).await.is_err() {
            self.stat.on_timeout(&now);
        }
    }

    #[inline]
    async fn do_execute(&mut self, now: &Instant) {
        let mut sock = match self.curr_sock().await {
            Ok(sock) => sock,
            Err(..) => {
                self.stat.on_sock_err();
                return;
            }
        };

        let code = match self.perform_request(&mut sock).await {
            Ok(c) => c,
            Err(..) => {
                self.stat.on_sock_err();
                return;
            }
        };

        self.stat.on_response(now);
        match code {
            c if (200..300).contains(&c) => self.stat.on_2xx(),
            c if (300..400).contains(&c) => self.stat.on_3xx(),
            c if (400..500).contains(&c) => self.stat.on_4xx(),
            c if (500..600).contains(&c) => self.stat.on_5xx(),
            c => log::error!("unexpected code: {}", c),
        }

        self.requests_per_sock_done += 1;
        if self.requests_per_sock_done < self.requests_per_sock {
            self.stream = Some(sock);
        } else {
            self.requests_per_sock_done = 0;
        }
    }

    #[inline]
    async fn perform_request(&mut self, stream: &mut TcpStream) -> Result<u16, Error> {
        let request = self.data.next();
        stream.write_all(request).await?;
        self.stat.on_requests(1);
        self.stat.on_send(request.len() as u64);

        self.buf.clear();
        loop {
            let n_read = stream.read_buf(&mut self.buf).await?;

            let mut headers = [httparse::EMPTY_HEADER; 8];
            let mut resp = httparse::Response::new(&mut headers);
            let parsed = resp.parse(&self.buf)?;
            if !parsed.is_complete() {
                continue;
            }
            // todo: read body, otherwise it hangs if the body doesn't fit in a single
            // packet.

            let code = resp.code.unwrap_or(0);
            self.stat.on_recv(n_read as u64);

            return Ok(code);
        }
    }

    #[inline]
    async fn curr_sock(&mut self) -> Result<TcpStream, Error> {
        let stream = match self.stream.take() {
            Some(stream) => stream,
            None => self.reconnect().await?,
        };

        Ok(stream)
    }

    #[inline]
    async fn reconnect(&mut self) -> Result<TcpStream, Error> {
        let addr = self.bind.next();
        let sock = match addr {
            SocketAddr::V4(..) => TcpSocket::new_v4()?,
            SocketAddr::V6(..) => TcpSocket::new_v6()?,
        };
        sock.bind(*addr)?;

        let stream = sock.connect(self.addr).await?;
        if self.tcp_no_delay {
            stream.set_nodelay(self.tcp_no_delay)?;
        }
        if let Some(linger) = self.tcp_linger {
            stream.set_linger(Some(Duration::from_secs(linger)))?;
        }
        self.stat.on_sock_created();

        Ok(stream)
    }
}
