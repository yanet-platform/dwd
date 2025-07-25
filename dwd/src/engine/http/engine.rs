use core::{
    future::Future,
    net::SocketAddr,
    num::NonZero,
    sync::atomic::{AtomicBool, AtomicU64},
    time::Duration,
};
use std::{
    os::fd::{AsFd, AsRawFd, RawFd},
    sync::Arc,
    time::Instant,
};

use anyhow::Error;
use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Empty};
use hyper::client::conn::http1::{self, SendRequest};
use tokio::net::TcpSocket;

use super::cfg::Config;
use crate::{
    engine::{
        coro::ShapedCoroWorker,
        http::io::TokioIo,
        runtime::{TaskSet, WorkerRuntime},
        Task,
    },
    shaper::Shaper,
    sockstat::TcpInfo,
    stat::{HttpWorkerStat, PerCpuStat, RxWorkerStat, SockWorkerStat, Stat, TxWorkerStat},
    Produce, VecProduce,
};

type WorkerStat = PerCpuStat<TxWorkerStat, RxWorkerStat, SockWorkerStat, HttpWorkerStat, ()>;
type EngineStat = Stat<TxWorkerStat, RxWorkerStat, SockWorkerStat, HttpWorkerStat, ()>;

#[derive(Debug)]
pub struct Engine {
    cfg: Config<Request<Empty<Bytes>>>,
    limits: Vec<Vec<Arc<AtomicU64>>>,
    stat: Arc<EngineStat>,
}

impl Engine {
    pub fn new(cfg: Config<Request<Empty<Bytes>>>) -> Self {
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
        let num_threads = self.cfg.native.threads;

        let bind = self.cfg.native.bind_endpoints.clone();
        let data = Arc::new(VecProduce::new(self.cfg.requests.clone()));

        let rt = WorkerRuntime::new(num_threads, |tid: usize| {
            let bind = bind.clone();
            let data = data.clone();
            let requests_per_socket = self.cfg.native.requests_per_socket();
            let stat = self.stat.stats[tid].clone();
            let is_running = is_running.clone();
            let limits = self.limits[tid].clone();
            let num_tasks = NonZero::new(limits.len()).unwrap();

            let set = TaskSet::new(num_tasks, move |idx: usize| {
                let job = CoroWorker::new(
                    self.cfg.addr,
                    bind.clone(),
                    data.clone(),
                    requests_per_socket,
                    self.cfg.timeout,
                    self.cfg.tcp_linger,
                    self.cfg.tcp_no_delay,
                    stat.clone(),
                );

                let shaper = Shaper::new(0, limits[idx].clone());
                let job = ShapedCoroWorker::new(job, shaper, is_running.clone());

                job.run()
            });

            || set.run()
        });

        rt.run()?;

        Ok(())
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
        timeout: Duration,
        tcp_linger: Option<u64>,
        tcp_no_delay: bool,
        stat: Arc<WorkerStat>,
    ) -> Self {
        Self {
            addr,
            bind,
            data,
            stream: None,
            requests_per_sock,
            requests_per_sock_done: 0,
            timeout,
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
            if self.requests_per_sock_done % 32 == 0 {
                sock.stat.update();
            }
            self.stream = Some(sock);
        } else {
            self.requests_per_sock_done = 0;
        }
    }

    #[inline]
    async fn perform_request(&mut self, stream: &mut TcpStream) -> Result<u16, Error> {
        let req = self.data.next();
        let mut resp = stream.sender.send_request(req.clone()).await?;
        self.stat.on_requests(1);

        let code = resp.status().as_u16();
        while let Some(next) = resp.frame().await {
            next?;
        }

        Ok(code)
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
        // TODO: if addr is 4/6 - then filter bind addresses to v4/v6 only.
        let addr = self.bind.next();
        let sock = match addr {
            SocketAddr::V4(..) => TcpSocket::new_v4()?,
            SocketAddr::V6(..) => TcpSocket::new_v6()?,
        };
        sock.bind(*addr)?;

        let fd = sock.as_fd().as_raw_fd();
        let stat = TcpStatMonitor::new(fd, self.stat.clone());

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

        let stream = TcpStream::new(stat, sender);

        Ok(stream)
    }
}

impl<B, D> Task for CoroWorker<B, D>
where
    B: Produce<Item = SocketAddr>,
    D: Produce<Item = Request<Empty<Bytes>>>,
{
    #[inline]
    async fn execute(&mut self) {
        Self::execute(self).await
    }
}

#[derive(Debug)]
struct TcpStatMonitor {
    fd: RawFd,
    tcp_info: TcpInfo,
    stat: Arc<WorkerStat>,
}

impl TcpStatMonitor {
    pub fn new(fd: RawFd, stat: Arc<WorkerStat>) -> Self {
        Self {
            fd,
            tcp_info: TcpInfo::default(),
            stat,
        }
    }

    #[inline]
    pub fn update(&mut self) {
        let tcpi = TcpInfo::new(self.fd);

        self.stat
            .on_send(tcpi.tcpi_bytes_acked.saturating_sub(self.tcp_info.tcpi_bytes_acked));
        self.stat.on_recv(
            tcpi.tcpi_bytes_received
                .saturating_sub(self.tcp_info.tcpi_bytes_received),
        );
        self.stat
            .on_retransmits(tcpi.tcpi_total_retrans.saturating_sub(self.tcp_info.tcpi_total_retrans));

        self.tcp_info = tcpi;
    }
}

impl Drop for TcpStatMonitor {
    fn drop(&mut self) {
        self.update();
    }
}

#[derive(Debug)]
struct TcpStream {
    stat: TcpStatMonitor,
    sender: SendRequest<Empty<Bytes>>,
}

impl TcpStream {
    pub fn new(stat: TcpStatMonitor, sender: SendRequest<Empty<Bytes>>) -> Self {
        Self { stat, sender }
    }
}
