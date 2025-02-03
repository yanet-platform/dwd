use core::{
    future::Future,
    net::SocketAddr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use std::{sync::Arc, thread, time::Instant};

use anyhow::Error;
use bytes::Bytes;
use http::Request;
use http_body_util::Empty;
use hyper::client::conn::http1::{self, SendRequest};
use tokio::{net::TcpSocket, runtime::Builder};

use super::{cfg::Config, stat::Stat};
use crate::{engine::http::io::TokioIo, shaper::Shaper, Produce, VecProduce};

#[derive(Debug)]
pub struct Engine {
    cfg: Config,
    limits: Vec<Vec<Arc<AtomicU64>>>,
    stat: Arc<Stat>,
}

impl Engine {
    pub fn new(cfg: Config) -> Self {
        let num_jobs = cfg.concurrency.get();
        let mut limits = vec![Vec::new(); cfg.native.threads.get()];

        let mut idx = 0;
        let len = limits.len();
        while idx < num_jobs {
            limits[idx % len].push(Arc::new(AtomicU64::new(0)));
            idx += 1;
        }

        let stat = Arc::new(Stat::default());

        Self { cfg, limits, stat }
    }

    #[inline]
    pub fn limits(&self) -> Vec<Vec<Arc<AtomicU64>>> {
        self.limits.clone()
    }

    #[inline]
    pub fn stat(&self) -> Arc<Stat> {
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
                    self.stat.clone(),
                );

                thread::Builder::new()
                    .name(format!("dwd:{idx:02}"))
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
    cfg: Config,
    /// Bind endpoints.
    bind: B,
    /// Data to send.
    data: D,
    limits: Vec<Arc<AtomicU64>>,
    is_running: Arc<AtomicBool>,
    stat: Arc<Stat>,
}

impl<B, D> Worker<B, D> {
    pub fn new(
        cfg: Config,
        bind: B,
        data: D,
        limits: Vec<Arc<AtomicU64>>,
        is_running: Arc<AtomicBool>,
        stat: Arc<Stat>,
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
    D: Produce<Item = Request<Empty<Bytes>>> + Clone + Send + Sync + 'static,
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
    D: Produce<Item = Request<Empty<Bytes>>>,
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
        thread::sleep(Duration::from_micros(1));
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
    // stream: Option<TcpStream>,
    stream: Option<SendRequest<Empty<Bytes>>>,
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
    stat: Arc<Stat>,
}

impl<B, D> CoroWorker<B, D> {
    pub fn new(
        addr: SocketAddr,
        bind: B,
        data: D,
        requests_per_sock: u64,
        tcp_linger: Option<u64>,
        tcp_no_delay: bool,
        stat: Arc<Stat>,
    ) -> Self {
        Self {
            addr,
            bind,
            data,
            stream: None,
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
    D: Produce<Item = Request<Empty<Bytes>>>,
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
            Err(err) => {
                self.stat.on_sock_err(&err);
                return;
            }
        };

        // self.buf.clear();

        match self.perform_request(&mut sock).await {
            Ok(c) if (200..300).contains(&c) => self.stat.on_2xx(now),
            Ok(c) if (300..400).contains(&c) => self.stat.on_3xx(now),
            Ok(c) if (400..500).contains(&c) => self.stat.on_4xx(now),
            Ok(c) if (500..600).contains(&c) => self.stat.on_5xx(now),
            Ok(c) => log::error!("unexpected code: {}", c),
            Err(err) => {
                self.stat.on_sock_err(&err);
                return;
            }
        }

        self.requests_per_sock_done += 1;
        if self.requests_per_sock_done < self.requests_per_sock {
            self.stream = Some(sock);
        } else {
            self.requests_per_sock_done = 0;
        }
    }

    #[inline]
    async fn perform_request(&mut self, stream: &mut SendRequest<Empty<Bytes>>) -> Result<u16, Error> {
        let req = self.data.next();
        let resp = stream.send_request(req.clone()).await?;
        self.stat.on_requests(1);
        // self.stat.on_send(self.request.len() as u64);
        // self.stat.on_recv(self.buf.len() as u64);

        let code = resp.status().as_u16();

        Ok(code)
    }

    #[inline]
    async fn curr_sock(&mut self) -> Result<SendRequest<Empty<Bytes>>, Error> {
        let stream = match self.stream.take() {
            Some(stream) => stream,
            None => self.reconnect().await?,
        };

        Ok(stream)
    }

    #[inline]
    async fn reconnect(&mut self) -> Result<SendRequest<Empty<Bytes>>, Error> {
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

        let io = TokioIo::new(stream);
        let (sender, conn) = http1::handshake(io).await?;
        tokio::task::spawn(async move {
            if let Err(err) = conn.await {
                log::error!("connection failed: {err}");
            }
        });

        Ok(sender)
    }
}
