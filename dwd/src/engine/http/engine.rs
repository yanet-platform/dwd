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
use tokio::{net::TcpSocket, runtime::Builder};

use super::cfg::Config;
use crate::{
    engine::{
        coro::ShapedCoroWorker,
        http::io::TokioIo,
        runtime::{TaskSet, WorkerRuntime},
        Task,
    },
    shaper::Shaper,
    stat::{HttpWorkerStat, PerCpuStat, RxWorkerStat, SockWorkerStat, Stat, TxWorkerStat},
    Produce, VecProduce,
};

type WorkerStat = PerCpuStat<TxWorkerStat, RxWorkerStat, SockWorkerStat, HttpWorkerStat>;
type EngineStat = Stat<TxWorkerStat, RxWorkerStat, SockWorkerStat, HttpWorkerStat>;

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
                self.update_stats(&mut sock);
            }
            self.stream = Some(sock);
        } else {
            self.requests_per_sock_done = 0;
            self.update_stats(&mut sock);
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
    fn update_stats(&mut self, stream: &mut TcpStream) {
        let tcpi = get_tcp_info(stream.fd);
        self.stat.on_send(tcpi.bytes_acked - stream.tcp_info.bytes_acked);
        self.stat.on_recv(tcpi.bytes_received - stream.tcp_info.bytes_received);
        stream.tcp_info = tcpi;
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
        let fd = sock.as_fd().as_raw_fd();

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

        let stream = TcpStream::new(fd, sender);

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
struct TcpStream {
    fd: RawFd,
    sender: SendRequest<Empty<Bytes>>,
    tcp_info: TcpInfo,
}

impl TcpStream {
    pub fn new(fd: RawFd, sender: SendRequest<Empty<Bytes>>) -> Self {
        Self {
            fd,
            sender,
            tcp_info: TcpInfo::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TcpInfo {
    pub bytes_acked: u64,
    pub bytes_received: u64,
}

#[cfg(target_os = "linux")]
fn get_tcp_info(fd: RawFd) -> TcpInfo {
    #[repr(C)]
    #[derive(Debug, Clone, Default)]
    struct tcp_info {
        pub tcpi_state: u8,
        pub tcpi_ca_state: u8,
        pub tcpi_retransmits: u8,
        pub tcpi_probes: u8,
        pub tcpi_backoff: u8,
        pub tcpi_options: u8,
        /// This contains the bitfields `tcpi_snd_wscale` and
        /// `tcpi_rcv_wscale`. Each is 4 bits.
        pub tcpi_snd_rcv_wscale: u8,
        pub tcpi_rto: u32,
        pub tcpi_ato: u32,
        pub tcpi_snd_mss: u32,
        pub tcpi_rcv_mss: u32,
        pub tcpi_unacked: u32,
        pub tcpi_sacked: u32,
        pub tcpi_lost: u32,
        pub tcpi_retrans: u32,
        pub tcpi_fackets: u32,
        pub tcpi_last_data_sent: u32,
        pub tcpi_last_ack_sent: u32,
        pub tcpi_last_data_recv: u32,
        pub tcpi_last_ack_recv: u32,
        pub tcpi_pmtu: u32,
        pub tcpi_rcv_ssthresh: u32,
        pub tcpi_rtt: u32,
        pub tcpi_rttvar: u32,
        pub tcpi_snd_ssthresh: u32,
        pub tcpi_snd_cwnd: u32,
        pub tcpi_advmss: u32,
        pub tcpi_reordering: u32,
        pub tcpi_rcv_rtt: u32,
        pub tcpi_rcv_space: u32,
        pub tcpi_total_retrans: u32,

        pub tcpi_pacing_rate: u64,
        pub tcpi_max_pacing_rate: u64,
        pub tcpi_bytes_acked: u64,
        pub tcpi_bytes_received: u64,
        pub tcpi_segs_out: u32,
        pub tcpi_segs_in: u32,
        pub tcpi_notsent_bytes: u32,
        pub tcpi_min_rtt: u32,
        pub tcpi_data_segs_in: u32,
        pub tcpi_data_segs_out: u32,
        pub tcpi_delivery_rate: u64,
        pub tcpi_busy_time: u64,
        pub tcpi_rwnd_limited: u64,
        pub tcpi_sndbuf_limited: u64,
    }
    let mut info = tcp_info::default();
    let mut len = core::mem::size_of_val(&info) as libc::socklen_t;
    unsafe {
        _ = libc::getsockopt(
            fd,
            libc::SOL_TCP,
            libc::TCP_INFO,
            &mut info as *mut tcp_info as *mut libc::c_void,
            &mut len as *mut u32,
        );
    }

    let info = TcpInfo {
        bytes_acked: info.tcpi_bytes_acked,
        bytes_received: info.tcpi_bytes_received,
    };

    return info;
}

#[cfg(not(target_os = "linux"))]
fn get_tcp_info(_fd: RawFd) -> TcpInfo {
    TcpInfo::default()
}
