use core::{net::SocketAddr, num::NonZero, time::Duration};

use crate::cfg::NativeLoadConfig;

/// HTTP engine config.
#[derive(Debug, Clone)]
pub struct Config<T> {
    /// Target endpoint.
    pub addr: SocketAddr,
    /// Number of parallel jobs.
    ///
    /// This also limits the maximum concurrent requests in flight. To achieve
    /// better runtime characteristics this value should be the multiple of
    /// the number of threads.
    pub concurrency: NonZero<usize>,
    /// Speak HTTPS (TLS) instead of plaintext HTTP.
    ///
    /// Certificate verification is disabled, which suits load testing against
    /// staging hosts and bare IPs. Only honored by the hyper `http` engine.
    pub tls: bool,
    /// TLS server name (SNI) presented during the handshake.
    ///
    /// Only meaningful with [`tls`](Self::tls). When `None`, the target IP is
    /// used and the (hostname-only) SNI extension is suppressed.
    pub server_name: Option<String>,
    /// Native workload settings.
    pub native: NativeLoadConfig,
    /// Request timeout.
    pub timeout: Duration,
    /// Set linger TCP option with specified value.
    pub tcp_linger: Option<u64>,
    /// Enable SOCK_NODELAY socket option.
    pub tcp_no_delay: bool,
    /// Requests to send.
    pub requests: Vec<T>,
}
