use core::{
    error::Error,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    ops::Deref,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use std::{
    sync::{atomic::AtomicU64, Arc},
    thread::{self, Builder, JoinHandle},
};

use tokio::sync::mpsc::{self, Receiver};

#[cfg(feature = "dpdk")]
use crate::{
    cfg::DpdkConfig,
    worker::dpdk::{DpdkWorkerGroup, Stat as DpdkStat},
};
use crate::{
    cfg::{Config, ModeConfig, UdpConfig},
    generator::{Generator, SuspendableGenerator},
    stat::CommonStat,
    ui::{self, Ui},
    worker::udp::{LocalStat, Stat as UdpStat, UdpWorker},
    GeneratorEvent, Produce,
};

struct LocalAddrIter {
    addrs: Vec<SocketAddr>,
    idx: AtomicUsize,
}

impl Produce for Arc<LocalAddrIter> {
    type Item = SocketAddr;

    #[inline]
    fn next(&self) -> &Self::Item {
        let idx = self.idx.fetch_add(1, Ordering::Relaxed);
        &self.addrs[idx % self.addrs.len()]
    }
}

struct Buffer(Vec<u8>);

impl Produce for Arc<Buffer> {
    type Item = [u8];

    #[inline]
    fn next(&self) -> &Self::Item {
        match self.deref() {
            Buffer(buf) => buf,
        }
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

    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        match self.cfg.mode.clone() {
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

    pub async fn run_udp(self, cfg: UdpConfig) -> Result<(), Box<dyn Error>> {
        let num_threads = cfg.native.threads.into();
        let mut threads = Vec::with_capacity(num_threads);

        let bind = Arc::new(LocalAddrIter {
            addrs: vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)],
            idx: AtomicUsize::new(0),
        });
        let data = Arc::new(Buffer(b"GET / HTTP/1.1\r\n\r\n".to_vec()));
        let mut stats = Vec::new();
        let mut limits = Vec::new();

        for idx in 0..num_threads {
            let stat = Arc::new(LocalStat::default());
            let limit = Arc::new(AtomicU64::new(0));

            let thread = {
                let worker = UdpWorker::new(
                    cfg.addr,
                    bind.clone(),
                    data.clone(),
                    self.is_running.clone(),
                    limit.clone(),
                    stat.clone(),
                )
                .with_requests_per_sock(cfg.native.requests_per_socket());
                Builder::new()
                    .name(format!("dwd:{idx:02}"))
                    .spawn(move || worker.run())?
            };

            stats.push(stat);
            limits.push(limit);
            threads.push(thread);
        }

        let stat = Arc::new(UdpStat::new(stats));
        let (tx, rx) = mpsc::channel(1);

        let ui = Ui::new(stat.clone(), tx).with_sock(stat.clone());
        let ui = self.run_ui(ui)?;

        let (shaper,) = tokio::join!(self.run_generator(limits, stat.clone(), rx));
        shaper?;

        ui.join().expect("no self join").unwrap();

        for thread in threads {
            thread.join().expect("no self join");
        }

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

                    let ui = Ui::new(stat.clone(), tx);
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
