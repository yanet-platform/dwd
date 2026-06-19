use core::{
    future,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use std::sync::Arc;

use anyhow::Error;
use dwd_proto::StatGroup;
use tokio::sync::mpsc::Receiver;

#[cfg(feature = "dpdk")]
use crate::worker::dpdk::DpdkEngine;
use crate::{
    api::StatSource,
    cfg::{BoxedGeneratorNew, ModeConfig},
    engine::{
        http::{Engine as HttpEngine, EngineRaw as HttpEngineRaw},
        udp::Engine as UdpEngine,
    },
    generator::{Generator, SuspendableGenerator},
    grpc::snapshot::{EngineDescriptor, SnapshotSource},
    stat::SharedGenerator,
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

/// A load-generating engine — the seam between the orchestrator and the modes.
pub trait Engine: Send {
    fn generator(&self) -> SharedGenerator;
    fn limits(&self) -> Vec<Arc<AtomicU64>>;
    /// Describes which statistic groups (i.e. TUI widgets) this engine exposes.
    fn describe(&self) -> EngineDescriptor;
    /// Source of wire snapshots for the gRPC stats stream.
    fn snapshot_source(&self) -> Arc<dyn SnapshotSource>;
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

    fn describe(&self) -> EngineDescriptor {
        EngineDescriptor {
            engine_kind: "http",
            groups: vec![
                StatGroup::Common,
                StatGroup::Tx,
                StatGroup::Rx,
                StatGroup::Http,
                StatGroup::Socket,
                StatGroup::RxTimings,
            ],
        }
    }

    fn snapshot_source(&self) -> Arc<dyn SnapshotSource> {
        self.stat()
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

    fn describe(&self) -> EngineDescriptor {
        EngineDescriptor {
            engine_kind: "http/raw",
            groups: vec![
                StatGroup::Common,
                StatGroup::Tx,
                StatGroup::Rx,
                StatGroup::Http,
                StatGroup::Socket,
                StatGroup::RxTimings,
            ],
        }
    }

    fn snapshot_source(&self) -> Arc<dyn SnapshotSource> {
        self.stat()
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

    fn describe(&self) -> EngineDescriptor {
        EngineDescriptor {
            engine_kind: "udp",
            groups: vec![StatGroup::Common, StatGroup::Tx, StatGroup::Socket],
        }
    }

    fn snapshot_source(&self) -> Arc<dyn SnapshotSource> {
        self.stat()
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

    fn describe(&self) -> EngineDescriptor {
        EngineDescriptor {
            engine_kind: "dpdk",
            groups: vec![StatGroup::Common, StatGroup::Tx, StatGroup::BurstTx],
        }
    }

    fn snapshot_source(&self) -> Arc<dyn SnapshotSource> {
        self.stat()
    }

    fn stat_source(&self) -> Arc<dyn StatSource> {
        self.stat()
    }

    fn run(self: Box<Self>, is_running: Arc<AtomicBool>) -> Result<(), Error> {
        Self::run(*self, is_running)
    }
}

/// Builds the engine for the given mode.
pub fn build(mode: ModeConfig) -> Result<Box<dyn Engine>, Error> {
    let engine: Box<dyn Engine> = match mode {
        ModeConfig::Http(cfg) => Box::new(HttpEngine::new(cfg)),
        ModeConfig::HttpRaw(cfg) => Box::new(HttpEngineRaw::new(cfg)),
        ModeConfig::Udp(cfg) => Box::new(UdpEngine::new(cfg)),
        #[cfg(feature = "dpdk")]
        ModeConfig::Dpdk(cfg) => Box::new(DpdkEngine::new(cfg.into_inner())?),
    };

    Ok(engine)
}

/// The shared rate-control loop.
///
/// Polls the generator every 10ms, distributes the target RPS across the
/// per-worker `limits` (integer division + remainder), applies control events
/// arriving on `control_rx`, and mirrors the value into `generator` for display
/// and metrics. Runs until the profile is exhausted or `is_running` flips false.
pub async fn run_generator(
    generator_fn: &BoxedGeneratorNew,
    limits: Vec<Arc<AtomicU64>>,
    generator: SharedGenerator,
    mut control_rx: Receiver<GeneratorEvent>,
    is_running: Arc<AtomicBool>,
) -> Result<(), Error> {
    let g = generator_fn.create()?;
    let mut g = SuspendableGenerator::new(g);
    g.activate();

    while let Some(v) = g.current() {
        if !is_running.load(Ordering::SeqCst) {
            break;
        }
        if let Ok(ev) = control_rx.try_recv() {
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

    is_running.store(false, Ordering::SeqCst);
    Ok(())
}
