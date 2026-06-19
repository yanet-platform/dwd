//! Engine-level configuration types shared by the orchestrator and the engines.
//!
//! These are produced from the CLI in the `dwd` binary, but live here because the
//! engines consume them and the orchestrator dispatches on [`ModeConfig`].

use core::{
    fmt::{self, Debug, Formatter},
    net::SocketAddr,
    num::NonZero,
};
use std::sync::Arc;

use bytes::Bytes;
use http::Request;
use http_body_util::Empty;

#[cfg(feature = "dpdk")]
use crate::worker::dpdk::Config as DpdkWorkerConfig;
use crate::{
    engine::{http::Config as HttpConfig, udp::Config as UdpConfig},
    generator::Generator,
    VecProduce,
};

/// The selected load mode together with its fully-resolved configuration.
#[derive(Debug, Clone)]
pub enum ModeConfig {
    Http(HttpConfig<Request<Empty<Bytes>>>),
    HttpRaw(HttpConfig<Bytes>),
    Udp(UdpConfig),
    #[cfg(feature = "dpdk")]
    Dpdk(DpdkConfig),
}

/// Native (kernel-stack) workload config shared by the HTTP and UDP engines.
#[derive(Debug, Clone)]
pub struct NativeLoadConfig {
    /// Number of threads.
    pub threads: NonZero<usize>,
    /// Maximum number of requests executed per socket before reconnection.
    /// If none given (default) sockets renew is disabled.
    requests_per_socket: Option<u64>,
    /// Deviation of the number of requests per socket.
    requests_per_socket_deviation: Option<u64>,
    /// Socket addresses to bind on.
    pub bind_endpoints: Arc<VecProduce<SocketAddr>>,
}

impl NativeLoadConfig {
    /// Builds a native workload config from already-resolved bind addresses.
    pub fn new(
        threads: NonZero<usize>,
        requests_per_socket: Option<u64>,
        requests_per_socket_deviation: Option<u64>,
        bind_endpoints: Vec<SocketAddr>,
    ) -> Self {
        Self {
            threads,
            requests_per_socket,
            requests_per_socket_deviation,
            bind_endpoints: Arc::new(VecProduce::new(bind_endpoints)),
        }
    }

    /// Returns the maximum number of requests executed per socket before
    /// reconnection.
    #[inline]
    pub fn requests_per_socket(&self) -> u64 {
        self.requests_per_socket.unwrap_or(u64::MAX)
    }

    /// Returns the deviation of the number of requests per socket.
    #[inline]
    pub fn requests_per_socket_deviation(&self) -> u64 {
        self.requests_per_socket_deviation.unwrap_or(0)
    }
}

/// DPDK workload config (a thin wrapper around the worker config).
#[cfg(feature = "dpdk")]
#[derive(Debug, Clone)]
pub struct DpdkConfig(DpdkWorkerConfig);

#[cfg(feature = "dpdk")]
impl DpdkConfig {
    /// Wraps a fully-built DPDK worker config.
    #[inline]
    pub fn new(inner: DpdkWorkerConfig) -> Self {
        Self(inner)
    }

    #[inline]
    pub fn into_inner(self) -> DpdkWorkerConfig {
        self.0
    }
}

/// A boxed runtime generator.
pub type BoxedGenerator = Box<dyn Generator>;

/// A deferred generator factory (built once at startup).
pub struct BoxedGeneratorNew(Box<dyn Fn() -> Result<BoxedGenerator, anyhow::Error>>);

impl BoxedGeneratorNew {
    /// Wraps a generator factory closure.
    #[inline]
    pub fn new(f: Box<dyn Fn() -> Result<BoxedGenerator, anyhow::Error>>) -> Self {
        Self(f)
    }

    /// Instantiates a fresh generator.
    #[inline]
    pub fn create(&self) -> Result<BoxedGenerator, anyhow::Error> {
        match self {
            Self(f) => f(),
        }
    }
}

impl Debug for BoxedGeneratorNew {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), fmt::Error> {
        fmt.debug_tuple("GeneratorFn").finish()
    }
}
