use core::{net::SocketAddr, num::NonZero};

use bytes::Bytes;
use http::Request;
use http_body_util::Empty;

use crate::cfg::NativeLoadConfig;

/// HTTP engine config.
#[derive(Debug, Clone)]
pub struct Config {
    /// Target endpoint.
    pub addr: SocketAddr,
    /// Number of parallel jobs.
    ///
    /// This also limits the maximum concurrent requests in flight. To achieve
    /// better runtime characteristics this value should be the multiple of
    /// the number of threads.   
    pub concurrency: NonZero<usize>,
    /// Native workload settings.
    pub native: NativeLoadConfig,
    /// Set linger TCP option with specified value.
    pub tcp_linger: Option<u64>,
    /// Enable SOCK_NODELAY socket option.
    pub tcp_no_delay: bool,
    /// Requests to send.
    pub requests: Vec<Request<Empty<Bytes>>>,
}
