use core::{
    future,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use std::{
    sync::Arc,
    thread::{Builder, JoinHandle},
};

use anyhow::Error;
use tokio::sync::mpsc::{self, Receiver};

#[cfg(feature = "dpdk")]
use crate::worker::dpdk::DpdkEngine;
use crate::{
    api::{MetricsState, Server, StatSource},
    cfg::{Config, ModeConfig},
    engine::{
        http::{Engine as HttpEngine, EngineRaw as HttpEngineRaw},
        udp::Engine as UdpEngine,
    },
    generator::{Generator, SuspendableGenerator},
    stat::SharedGenerator,
    ui::{self, Ui},
    GeneratorEvent,
};

mod coro;
pub mod http;
mod runtime;
pub mod udp;

/// Task unit.
trait Task {
    /// Executes this task once.
    async fn execute(&mut self);
}

trait Engine: Send {
    fn generator(&self) -> SharedGenerator;
    fn limits(&self) -> Vec<Arc<AtomicU64>>;
    fn ui(&self) -> (Ui, Receiver<GeneratorEvent>);
    fn stat_source(&self) -> Arc<dyn StatSource>;
    fn run(self: Box<Self>, is_running: Arc<AtomicBool>) -> Result<(), Error>;
}

impl Engine for HttpEngine {
    fn generator(&self) -> SharedGenerator {
        self.stat().generator()
    }

    fn limits(&self) -> Vec<Arc<AtomicU64>> {
        self.limits().into_iter().flatten().collect()
    }

    fn ui(&self) -> (Ui, Receiver<GeneratorEvent>) {
        let (tx, rx) = mpsc::channel(1);

        let stat = self.stat();
        let ui = Ui::new(tx)
            .with_common(stat.clone())
            .with_tx(stat.clone())
            .with_rx(stat.clone())
            .with_http(stat.clone())
            .with_sock(stat.clone())
            .with_rx_timings(stat.clone());

        (ui, rx)
    }

    fn stat_source(&self) -> Arc<dyn StatSource> {
        self.stat()
    }

    fn run(self: Box<Self>, is_running: Arc<AtomicBool>) -> Result<(), Error> {
        Self::run(*self, future::pending(), is_running)
    }
}

impl Engine for HttpEngineRaw {
    fn generator(&self) -> SharedGenerator {
        self.stat().generator()
    }

    fn limits(&self) -> Vec<Arc<AtomicU64>> {
        self.limits().into_iter().flatten().collect()
    }

    fn ui(&self) -> (Ui, Receiver<GeneratorEvent>) {
        let (tx, rx) = mpsc::channel(1);
        let stat = self.stat();

        let ui = Ui::new(tx)
            .with_common(stat.clone())
            .with_tx(stat.clone())
            .with_rx(stat.clone())
            .with_http(stat.clone())
            .with_sock(stat.clone())
            .with_rx_timings(stat.clone());

        (ui, rx)
    }

    fn stat_source(&self) -> Arc<dyn StatSource> {
        self.stat()
    }

    fn run(self: Box<Self>, is_running: Arc<AtomicBool>) -> Result<(), Error> {
        Self::run(*self, future::pending(), is_running)
    }
}

impl Engine for UdpEngine {
    fn generator(&self) -> SharedGenerator {
        self.stat().generator()
    }

    fn limits(&self) -> Vec<Arc<AtomicU64>> {
        Self::limits(self)
    }

    fn ui(&self) -> (Ui, Receiver<GeneratorEvent>) {
        let (tx, rx) = mpsc::channel(1);
        let stat = self.stat();

        let ui = Ui::new(tx)
            .with_common(stat.clone())
            .with_tx(stat.clone())
            .with_sock(stat.clone());

        (ui, rx)
    }

    fn stat_source(&self) -> Arc<dyn StatSource> {
        self.stat()
    }

    fn run(self: Box<Self>, is_running: Arc<AtomicBool>) -> Result<(), Error> {
        Self::run(*self, future::pending(), is_running)
    }
}

#[cfg(feature = "dpdk")]
impl Engine for DpdkEngine {
    fn generator(&self) -> SharedGenerator {
        self.stat().generator()
    }

    fn limits(&self) -> Vec<Arc<AtomicU64>> {
        self.pps_limits()
    }

    fn ui(&self) -> (Ui, Receiver<GeneratorEvent>) {
        let (tx, rx) = mpsc::channel(1);
        let stat = self.stat();

        let ui = Ui::new(tx)
            .with_common(stat.clone())
            .with_tx(stat.clone())
            .with_burst_tx(stat.clone());

        (ui, rx)
    }

    fn stat_source(&self) -> Arc<dyn StatSource> {
        self.stat()
    }

    fn run(self: Box<Self>, is_running: Arc<AtomicBool>) -> Result<(), Error> {
        Self::run(*self, is_running)
    }
}

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
        let engine: Box<dyn Engine> = match self.cfg.mode.clone() {
            ModeConfig::Http(cfg) => Box::new(HttpEngine::new(cfg)),
            ModeConfig::HttpRaw(cfg) => Box::new(HttpEngineRaw::new(cfg)),
            ModeConfig::Udp(cfg) => Box::new(UdpEngine::new(cfg)),
            #[cfg(feature = "dpdk")]
            ModeConfig::Dpdk(cfg) => Box::new(DpdkEngine::new(cfg.into_inner())?),
        };

        let limits = engine.limits();
        let generator = engine.generator();
        let (ui, rx) = engine.ui();
        let stat_source = engine.stat_source();

        // Start API server if configured.
        let api_handle = if let Some(addr) = self.cfg.api_addr {
            let metrics_state = Arc::new(MetricsState::new(stat_source));
            let server = Server::new(addr, metrics_state);
            Some(tokio::spawn(async move {
                if let Err(e) = server.run().await {
                    log::error!("API server error: {e}");
                }
            }))
        } else {
            None
        };

        let engine = {
            let is_running = self.is_running.clone();
            Builder::new()
                .name("engine".into())
                .spawn(move || engine.run(is_running))?
        };

        let ui = self.run_ui(ui)?;

        let (shaper,) = tokio::join!(self.run_generator(limits, generator, rx));
        shaper?;

        ui.join().expect("no self join").unwrap();
        engine.join().expect("no self join")?;

        // Abort API server when shutting down.
        if let Some(handle) = api_handle {
            handle.abort();
        }

        Ok(())
    }

    async fn run_generator(
        &self,
        limits: Vec<Arc<AtomicU64>>,
        generator: SharedGenerator,
        mut rx: Receiver<GeneratorEvent>,
    ) -> Result<(), Error> {
        let g = self.cfg.generator_fn.create()?;
        let mut g = SuspendableGenerator::new(g);
        g.activate();

        while let Some(v) = g.current() {
            if !self.is_running.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(ev) = rx.try_recv() {
                match ev {
                    GeneratorEvent::Suspend => g.suspend(),
                    GeneratorEvent::Resume => g.resume(),
                    GeneratorEvent::Set(v) => g.set(v),
                }
            }

            let lb = v / limits.len() as u64;
            let ub = v % limits.len() as u64;

            for (idx, limit) in limits.iter().enumerate() {
                let mut lb = lb;
                if (idx as u64) < ub {
                    lb += 1;
                }

                limit.store(lb, Ordering::Relaxed);
            }

            generator.store(v);

            // TODO: configurable sleep time.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        self.is_running.store(false, Ordering::SeqCst);
        Ok(())
    }

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
