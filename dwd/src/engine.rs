use core::{
    future,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use std::{
    error::Error,
    sync::Arc,
    thread::{self, Builder, JoinHandle},
};

use bytes::Bytes;
use http_body_util::Empty;
use hyper::Request;
use tokio::sync::mpsc::{self, Receiver};

#[cfg(feature = "dpdk")]
use crate::{
    cfg::DpdkConfig,
    worker::dpdk::{DpdkWorkerGroup, Stat as DpdkStat},
};
use crate::{
    cfg::{Config, ModeConfig},
    engine::{
        http::{Engine as HttpEngine, EngineRaw as HttpEngineRaw},
        udp::Engine as UdpEngine,
    },
    generator::{Generator, SuspendableGenerator},
    stat::CommonStat,
    ui::{self, Ui},
    GeneratorEvent,
};

mod coro;
pub mod http;
pub mod udp;

/// Task unit.
trait Task {
    /// Executes this task once.
    async fn execute(&mut self);
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

    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        match self.cfg.mode.clone() {
            ModeConfig::Http(cfg) => {
                self.run_http(cfg).await?;
            }
            ModeConfig::HttpRaw(cfg) => {
                self.run_http_raw(cfg).await?;
            }
            ModeConfig::Udp(cfg) => {
                self.run_udp(cfg).await?;
            }
            #[cfg(feature = "dpdk")]
            ModeConfig::Dpdk(cfg) => {
                self.run_dpdk(cfg.clone()).await?;
            }
        };

        Ok(())
    }

    pub async fn run_http(self, cfg: http::Config<Request<Empty<Bytes>>>) -> Result<(), Box<dyn Error>> {
        let engine = HttpEngine::new(cfg);
        let stat = engine.stat();
        let limits = engine.limits().into_iter().flatten().collect();

        let engine = {
            let is_running = self.is_running.clone();
            Builder::new().spawn(move || engine.run(future::pending(), is_running))?
        };

        let (tx, rx) = mpsc::channel(1);

        let ui = Ui::new(tx)
            .with_tx(stat.clone())
            .with_rx(stat.clone())
            .with_http(stat.clone())
            .with_sock(stat.clone())
            .with_rx_timings(stat.clone());
        let ui = self.run_ui(ui)?;

        let (shaper,) = tokio::join!(self.run_generator(limits, stat.clone(), rx));
        shaper?;

        ui.join().expect("no self join").unwrap();
        engine.join().expect("no self join")?;

        Ok(())
    }

    pub async fn run_http_raw(self, cfg: http::Config<Bytes>) -> Result<(), Box<dyn Error>> {
        let engine = HttpEngineRaw::new(cfg);
        let stat = engine.stat();
        let limits = engine.limits().into_iter().flatten().collect();

        let engine = {
            let is_running = self.is_running.clone();
            Builder::new().spawn(move || engine.run(future::pending(), is_running))?
        };

        let (tx, rx) = mpsc::channel(1);

        let ui = Ui::new(tx)
            .with_tx(stat.clone())
            .with_rx(stat.clone())
            .with_http(stat.clone())
            .with_sock(stat.clone())
            .with_rx_timings(stat.clone());
        let ui = self.run_ui(ui)?;

        let (shaper,) = tokio::join!(self.run_generator(limits, stat.clone(), rx));
        shaper?;

        ui.join().expect("no self join").unwrap();
        engine.join().expect("no self join")?;

        Ok(())
    }

    pub async fn run_udp(self, cfg: udp::Config) -> Result<(), Box<dyn Error>> {
        let engine = UdpEngine::new(cfg);
        let stat = engine.stat();
        let limits = engine.limits();

        let engine = {
            let is_running = self.is_running.clone();
            Builder::new().spawn(move || engine.run(future::pending(), is_running))?
        };

        let (tx, rx) = mpsc::channel(1);

        let ui = Ui::new(tx).with_tx(stat.clone()).with_sock(stat.clone());
        let ui = self.run_ui(ui)?;

        let (shaper,) = tokio::join!(self.run_generator(limits, stat.clone(), rx));
        shaper?;

        ui.join().expect("no self join").unwrap();
        engine.join().expect("no self join")?;

        Ok(())
    }

    // TODO: refactor
    #[cfg(feature = "dpdk")]
    async fn run_dpdk(self, cfg: DpdkConfig) -> Result<(), Box<dyn Error>> {
        let cfg = cfg.into_inner();
        let dpdk = DpdkWorkerGroup::new(cfg, self.is_running.clone())?;
        let stat = Arc::new(DpdkStat::new(dpdk.stats()));
        let limits = dpdk.pps_limits();

        let thread: JoinHandle<()> = Builder::new().spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime
                .block_on(async move {
                    let (tx, rx) = mpsc::channel(1);

                    let ui = Ui::new(tx).with_tx(stat.clone()).with_burst_tx(stat.clone());
                    let ui = self.run_ui(ui)?;

                    let (shaper,) = tokio::join!(self.run_generator(limits, stat, rx));
                    shaper?;

                    ui.join().expect("no self join").unwrap();

                    Ok::<(), Box<dyn Error>>(())
                })
                .unwrap();

            // Ok(())
        })?;

        dpdk.run()?;
        thread.join().expect("no self join");

        Ok(())
    }

    async fn run_generator<S>(
        &self,
        limits: Vec<Arc<AtomicU64>>,
        stat: Arc<S>,
        mut rx: Receiver<GeneratorEvent>,
    ) -> Result<(), Box<dyn Error>>
    where
        S: CommonStat,
    {
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

            stat.on_generator(v);

            // TODO: configurable sleep time.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        self.is_running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn run_ui(&self, ui: Ui) -> Result<JoinHandle<Result<(), Box<dyn Error + Send + Sync>>>, Box<dyn Error>> {
        let is_running = self.is_running.clone();

        let thread = thread::Builder::new().spawn(move || {
            let rc = ui::run(ui);
            is_running.store(false, Ordering::SeqCst);
            rc
        })?;

        Ok(thread)
    }
}
