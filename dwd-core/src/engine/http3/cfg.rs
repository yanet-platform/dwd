use core::{net::SocketAddr, num::NonZero, time::Duration};

use crate::cfg::NativeLoadConfig;

/// HTTP/3 (QUIC) engine config.
///
/// Mirrors the HTTP/1 [`Config`](crate::engine::http::Config) but drops the
/// TCP-specific knobs (there is no TCP socket) and adds the TLS server name used
/// for SNI, since QUIC mandates TLS 1.3.
#[derive(Debug, Clone)]
pub struct Config<T> {
    /// Target endpoint.
    pub addr: SocketAddr,
    /// TLS server name (SNI) presented during the QUIC handshake.
    ///
    /// Certificate validation is disabled, so this only affects the SNI
    /// extension the server may route on.
    pub server_name: String,
    /// Number of parallel jobs.
    ///
    /// This also limits the maximum concurrent connections. To achieve better
    /// runtime characteristics this value should be a multiple of the number of
    /// threads.
    pub concurrency: NonZero<usize>,
    /// Native workload settings.
    pub native: NativeLoadConfig,
    /// Request timeout.
    pub timeout: Duration,
    /// Requests to send.
    pub requests: Vec<T>,
}
