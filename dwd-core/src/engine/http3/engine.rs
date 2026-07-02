//! HTTP/3 (QUIC) load engine.
//!
//! Structurally a sibling of the HTTP/1 [`Engine`](crate::engine::http::Engine):
//! it reuses the same `ThreadPool → LocalTaskPool → ShapedCoroWorker → Shaper`
//! layering, the same [`Produce`] payload/bind cycling, and the very same
//! per-CPU [`Stat`] shape (so it inherits `StatSource`/`SnapshotSource` for
//! free). Only the transport differs: instead of a TCP socket + hyper HTTP/1
//! handshake, each worker drives a `quinn` QUIC endpoint and an `h3` client.
//!
//! One QUIC connection maps onto the HTTP "socket" abstraction: connection
//! establishment bumps `on_sock_created`, connection/stream errors bump
//! `on_sock_err`. QUIC has no TCP retransmit counter, so that stays zero.

use core::{
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    num::NonZero,
    sync::atomic::{AtomicBool, AtomicU64},
    time::Duration,
};
use std::{sync::Arc, time::Instant};

use anyhow::{anyhow, Error};
use bytes::Buf;
use h3::client::SendRequest;
use http::Request;
use quinn::{ClientConfig, Endpoint};
use rand::{rngs::ThreadRng, Rng};

use super::{cfg::Config, tls::insecure_client_config};
use crate::{
    engine::{
        coro::ShapedCoroWorker,
        runtime::{LocalTaskPool, ThreadPool},
        Task,
    },
    shaper::Shaper,
    stat::{HttpWorkerStat, PerCpuStat, RxWorkerStat, SockWorkerStat, Stat, TxWorkerStat},
    Produce, VecProduce,
};

type WorkerStat = PerCpuStat<TxWorkerStat, RxWorkerStat, SockWorkerStat, HttpWorkerStat, ()>;
type EngineStat = Stat<TxWorkerStat, RxWorkerStat, SockWorkerStat, HttpWorkerStat, ()>;

/// The h3-quinn open-streams handle backing a live connection.
type OpenStreams = h3_quinn::OpenStreams;

#[derive(Debug)]
pub struct Engine {
    cfg: Config<Request<()>>,
    limits: Vec<Vec<Arc<AtomicU64>>>,
    stat: Arc<EngineStat>,
}

impl Engine {
    pub fn new(cfg: Config<Request<()>>) -> Self {
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

        // The TLS config is immutable, so the QUIC client config is built once
        // at startup and shared across all workers/connections. This keeps the
        // reconnect path free of TLS/crypto allocations (perf contract).
        let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(insecure_client_config())
            .map_err(|err| anyhow!("invalid TLS config: {err}"))?;
        let client_config = ClientConfig::new(Arc::new(crypto));

        let thread_pool = ThreadPool::new(num_threads, |tid: usize| {
            let bind = bind.clone();
            let data = data.clone();
            let client_config = client_config.clone();
            let addr = self.cfg.addr;
            let server_name = self.cfg.server_name.clone();
            let requests_per_conn = self.cfg.native.requests_per_socket();
            let requests_per_conn_deviation = self.cfg.native.requests_per_socket_deviation();
            let timeout = self.cfg.timeout;
            let stat = self.stat.stats[tid].clone();
            let is_running = is_running.clone();
            let limits = self.limits[tid].clone();
            let num_tasks = NonZero::new(limits.len()).unwrap();

            let set = LocalTaskPool::new(num_tasks, move |idx: usize| {
                let requests_per_conn_gen = if requests_per_conn_deviation > 0 {
                    RequestsPerConn::uniform(requests_per_conn, requests_per_conn_deviation)
                } else {
                    RequestsPerConn::fixed(requests_per_conn)
                };

                let job = CoroWorker::new(
                    addr,
                    server_name.clone(),
                    bind.clone(),
                    data.clone(),
                    client_config.clone(),
                    requests_per_conn_gen,
                    timeout,
                    stat.clone(),
                );

                let shaper = Shaper::new(0, limits[idx].clone());
                let job = ShapedCoroWorker::new(job, shaper, is_running.clone());

                job.run()
            });

            || set.run()
        });

        thread_pool.run()?;

        Ok(())
    }
}

#[derive(Debug)]
enum RequestsPerConn {
    Fixed(u64),
    Uniform(ThreadRng, core::ops::RangeInclusive<u64>),
}

impl RequestsPerConn {
    pub fn fixed(requests_per_conn: u64) -> Self {
        Self::Fixed(requests_per_conn)
    }

    pub fn uniform(requests_per_conn: u64, deviation: u64) -> Self {
        let min = requests_per_conn.saturating_sub(deviation);
        let max = requests_per_conn.saturating_add(deviation);

        Self::Uniform(ThreadRng::default(), min..=max)
    }

    #[inline]
    pub fn next(&mut self) -> u64 {
        match self {
            Self::Fixed(v) => *v,
            Self::Uniform(rng, range) => rng.random_range(range.clone()),
        }
    }
}

/// A live HTTP/3 connection: the h3 request sender for an established QUIC
/// connection, plus the QUIC connection handle for graceful shutdown.
struct Http3Connection {
    quic_conn: quinn::Connection,
    send_request: SendRequest<OpenStreams, bytes::Bytes>,
}

impl Drop for Http3Connection {
    fn drop(&mut self) {
        // Explicitly close the connection to send a CONNECTION_CLOSE frame,
        // rather than letting the driver drop it ungracefully or time out.
        self.quic_conn.close(0u32.into(), b"");
    }
}

/// Per-task HTTP/3 worker.
struct CoroWorker<B, D> {
    /// Target endpoint.
    addr: SocketAddr,
    /// TLS server name (SNI).
    server_name: String,
    /// Bind endpoints.
    bind: B,
    /// Data to send.
    data: D,
    /// Prebuilt QUIC client config (TLS + crypto), shared across connections.
    client_config: ClientConfig,
    /// Reusable QUIC endpoint (bound lazily on first connect).
    endpoint: Option<Endpoint>,
    /// Current live connection.
    conn: Option<Http3Connection>,
    /// The number of requests left for the currently active connection.
    requests_per_conn_left: u64,
    /// Generator for the number of requests per connection.
    requests_per_conn_gen: RequestsPerConn,
    /// Request timeout.
    timeout: Duration,
    /// Runtime statistics.
    stat: Arc<WorkerStat>,
}

impl<B, D> CoroWorker<B, D> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        addr: SocketAddr,
        server_name: String,
        bind: B,
        data: D,
        client_config: ClientConfig,
        requests_per_conn_gen: RequestsPerConn,
        timeout: Duration,
        stat: Arc<WorkerStat>,
    ) -> Self {
        let mut requests_per_conn_gen = requests_per_conn_gen;
        let requests_per_conn_left = requests_per_conn_gen.next();

        Self {
            addr,
            server_name,
            bind,
            data,
            client_config,
            endpoint: None,
            conn: None,
            requests_per_conn_left,
            requests_per_conn_gen,
            timeout,
            stat,
        }
    }
}

impl<B, D> CoroWorker<B, D>
where
    B: Produce<Item = SocketAddr>,
    D: Produce<Item = Request<()>>,
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
        let mut conn = match self.curr_conn().await {
            Ok(conn) => conn,
            Err(..) => {
                self.stat.on_sock_err();
                return;
            }
        };

        let code = match self.perform_request(&mut conn).await {
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
            c => log::warn!("unexpected HTTP code: {c}"),
        }

        self.requests_per_conn_left = self.requests_per_conn_left.saturating_sub(1);
        if self.requests_per_conn_left > 0 {
            // Reuse the connection if we haven't reached the per-connection limit.
            self.conn = Some(conn);
        } else {
            self.requests_per_conn_left = self.requests_per_conn_gen.next();
        }
    }

    #[inline]
    async fn curr_conn(&mut self) -> Result<Http3Connection, Error> {
        let conn = match self.conn.take() {
            Some(conn) => conn,
            None => self.reconnect().await?,
        };

        Ok(conn)
    }

    #[inline]
    async fn perform_request(&mut self, conn: &mut Http3Connection) -> Result<u16, Error> {
        let req = self.data.next().clone();
        let mut stream = conn.send_request.send_request(req).await?;
        self.stat.on_requests(1);

        // Empty request body: close the sending side immediately.
        stream.finish().await?;

        let resp = stream.recv_response().await?;
        let code = resp.status().as_u16();

        // Drain the response body, accounting received bytes.
        let mut bytes_rx = 0u64;
        while let Some(chunk) = stream.recv_data().await? {
            bytes_rx += chunk.remaining() as u64;
        }
        if bytes_rx > 0 {
            self.stat.on_recv(bytes_rx);
        }

        Ok(code)
    }

    #[inline]
    async fn reconnect(&mut self) -> Result<Http3Connection, Error> {
        let endpoint = self.endpoint()?;

        let connecting = endpoint.connect(self.addr, &self.server_name)?;
        let conn = connecting.await?;
        self.stat.on_sock_created();

        let quinn_conn = h3_quinn::Connection::new(conn.clone());
        let (mut driver, send_request) = h3::client::new(quinn_conn).await?;

        // The connection is driven for as long as any `SendRequest` clone lives;
        // once the worker drops its connection, the driver resolves and exits.
        tokio::task::spawn(async move {
            let _ = core::future::poll_fn(|cx| driver.poll_close(cx)).await;
        });

        Ok(Http3Connection { quic_conn: conn, send_request })
    }

    /// Returns the worker's QUIC endpoint, creating and configuring it on first
    /// use. The endpoint is bound to the next configured bind address.
    #[inline]
    fn endpoint(&mut self) -> Result<Endpoint, Error> {
        if let Some(endpoint) = &self.endpoint {
            return Ok(endpoint.clone());
        }

        let mut bind = *self.bind.next();

        // The QUIC endpoint's local UDP socket must share the target's address
        // family, otherwise it cannot reach it. When the bind address is the
        // default unspecified one, align its family with the target.
        if bind.ip().is_unspecified() {
            match self.addr {
                SocketAddr::V4(_) => bind.set_ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
                SocketAddr::V6(_) => bind.set_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)),
            }
        }

        let mut endpoint =
            Endpoint::client(bind).map_err(|err| anyhow!("failed to create QUIC endpoint on {bind}: {err}"))?;
        endpoint.set_default_client_config(self.client_config.clone());

        self.endpoint = Some(endpoint.clone());
        Ok(endpoint)
    }
}

impl<B, D> Task for CoroWorker<B, D>
where
    B: Produce<Item = SocketAddr>,
    D: Produce<Item = Request<()>>,
{
    #[inline]
    async fn execute(&mut self) {
        Self::execute(self).await
    }
}

impl<B, D> core::fmt::Debug for CoroWorker<B, D> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CoroWorker")
            .field("addr", &self.addr)
            .field("server_name", &self.server_name)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}
