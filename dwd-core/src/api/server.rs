//! HTTP API server.

use core::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;

use super::metrics::{self, MetricsState};

/// HTTP API server.
pub struct Server {
    addr: SocketAddr,
    metrics_state: Arc<MetricsState>,
}

impl Server {
    /// Creates a new API server.
    pub fn new(addr: SocketAddr, metrics_state: Arc<MetricsState>) -> Self {
        Self { addr, metrics_state }
    }

    /// Runs the API server.
    pub async fn run(self) -> Result<(), std::io::Error> {
        let app = Router::new().merge(metrics::router(self.metrics_state));

        let listener = TcpListener::bind(self.addr).await?;
        let addr = listener.local_addr()?;
        log::info!("API server listening on {addr}");

        axum::serve(listener, app).await
    }
}
