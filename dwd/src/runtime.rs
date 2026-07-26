//! The composition root: wires the core engine, the in-process gRPC seam, and
//! the TUI client together into the single bundled process.
//!
//! This is the only place that knows about *both* the core (server) and the TUI
//! (client). The boundary between them is the [`dwd_proto`] gRPC contract: the
//! TUI never touches the engine directly.

use core::{
    future,
    sync::atomic::{AtomicBool, Ordering},
};
use std::{
    sync::Arc,
    thread::{Builder, JoinHandle},
};

use anyhow::Error;
use dwd_core::{engine, grpc::DwdService};
use dwd_proto::DescribeRequest;
use tokio::sync::mpsc;

use crate::{
    cfg::Config,
    grpc::client,
    ui::{self, Ui},
};

/// In-memory pipe buffer size for the gRPC transport (per direction).
const DUPLEX_BUFFER: usize = 64 * 1024;

/// Orchestrates a single run: the engine, the generator loop, the gRPC server,
/// the gRPC client, the TUI, and the optional Prometheus endpoint.
#[derive(Debug)]
pub struct Runtime {
    cfg: Config,
    is_running: Arc<AtomicBool>,
}

impl Runtime {
    pub fn new(cfg: Config) -> Self {
        let is_running = Arc::new(AtomicBool::new(true));

        Self { cfg, is_running }
    }

    pub async fn run(self) -> Result<(), Error> {
        let engine = engine::build(self.cfg.mode.clone())?;

        let limits = engine.limits();
        let generator = engine.generator();
        let stat_source = engine.stat_source();
        let snapshot_source = engine.snapshot_source();
        let descriptor = engine.describe().into_response();

        // Start the Prometheus API server if configured — available both with
        // the TUI and headless.
        let api_handle = if let Some(addr) = self.cfg.api_addr {
            let metrics_state = Arc::new(dwd_core::api::MetricsState::new(stat_source));
            let server = dwd_core::api::Server::new(addr, metrics_state);
            Some(tokio::spawn(async move {
                if let Err(e) = server.run().await {
                    log::error!("API server error: {e}");
                }
            }))
        } else {
            None
        };

        // The control channel that `run_generator` polls; the gRPC `Control`
        // handler is now the only writer (the TUI reaches it through the API).
        let (control_tx, control_rx) = mpsc::channel::<dwd_core::GeneratorEvent>(1);

        // The client half of the process exists only when the TUI runs. Headless
        // (`--no-ui`) there is no client to serve, so the in-process gRPC seam is
        // not stood up at all, and shutdown comes from process signals instead of
        // the UI loop.
        let seam = if self.cfg.no_ui {
            drop(control_tx);
            self.spawn_signal_handler();

            None
        } else {
            // Stand up the in-process gRPC server over the core and connect a client
            // over the same in-memory pipe — no OS socket is opened.
            let service = DwdService::new(control_tx, snapshot_source, descriptor, self.is_running.clone());
            let (client_io, server_io) = tokio::io::duplex(DUPLEX_BUFFER);
            let server_handle = dwd_core::grpc::serve(service, server_io);
            let grpc_client = client::connect(client_io)?;

            // Build the TUI purely as a gRPC client: ask the server which widgets to
            // render, mirror the streamed stats behind `RemoteStat`, and route key
            // presses out as control RPCs.
            let groups = {
                let mut client = grpc_client.clone();
                client.describe(DescribeRequest {}).await?.into_inner().groups
            };
            let remote = Arc::new(client::RemoteStat::new());
            let (ui_tx, ui_rx) = mpsc::channel::<dwd_core::GeneratorEvent>(1);
            let ui = client::build_ui(ui_tx, &groups, remote.clone());

            let stats_handle = client::spawn_stats_consumer(grpc_client.clone(), remote);
            let control_handle = client::spawn_control_forwarder(grpc_client, ui_rx);

            Some((ui, stats_handle, control_handle, server_handle))
        };

        let engine = {
            let is_running = self.is_running.clone();
            Builder::new()
                .name("engine".into())
                .spawn(move || engine.run(is_running))?
        };

        let ui = match seam {
            Some((ui, stats_handle, control_handle, server_handle)) => {
                Some((self.run_ui(ui)?, stats_handle, control_handle, server_handle))
            }
            None => None,
        };

        let shaper = engine::run_generator(
            &self.cfg.generator_fn,
            limits,
            generator,
            control_rx,
            self.is_running.clone(),
        )
        .await;
        shaper?;

        if let Some((ui, stats_handle, control_handle, server_handle)) = ui {
            ui.join().expect("no self join").unwrap();

            // Tear down the gRPC seam when shutting down.
            stats_handle.abort();
            control_handle.abort();
            server_handle.abort();
        }
        engine.join().expect("no self join")?;

        if let Some(handle) = api_handle {
            handle.abort();
        }

        Ok(())
    }

    /// Flips `is_running` when the process receives a shutdown signal.
    ///
    /// Headless runs have no UI loop to translate key presses into shutdown, so
    /// SIGINT (Ctrl-C) and SIGTERM take that role instead: the engine and the
    /// generator loop drain gracefully, exactly as if the TUI had exited.
    fn spawn_signal_handler(&self) {
        let is_running = self.is_running.clone();

        tokio::spawn(async move {
            shutdown_signal().await;
            log::info!("shutdown signal received, stopping");
            is_running.store(false, Ordering::SeqCst);
        });
    }

    #[allow(clippy::type_complexity)]
    fn run_ui(&self, ui: Ui) -> Result<JoinHandle<Result<(), Box<dyn core::error::Error + Send + Sync>>>, Error> {
        let is_running = self.is_running.clone();

        let thread = Builder::new().name("ui".into()).spawn(move || {
            let rc = ui::run(ui);
            is_running.store(false, Ordering::SeqCst);
            rc
        })?;

        Ok(thread)
    }
}

/// Completes when SIGINT (Ctrl-C) or SIGTERM is received.
///
/// If a handler cannot be installed the corresponding branch pends forever
/// rather than resolving — the run then ends on profile exhaustion only.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            log::error!("failed to install SIGINT handler: {err}");
            future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};

        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(err) => {
                log::error!("failed to install SIGTERM handler: {err}");
                future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
