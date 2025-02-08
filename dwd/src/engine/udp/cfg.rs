use core::net::SocketAddr;

use crate::cfg::NativeLoadConfig;

/// UDP engine config.
#[derive(Debug, Clone)]
pub struct Config {
    /// Target endpoint.
    pub addr: SocketAddr,
    /// Native workload settings.
    pub native: NativeLoadConfig,
}
