use core::{
    error::Error,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    ops::Deref,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use std::{
    sync::Arc,
    thread::{self, Builder, JoinHandle},
};

use tokio::{
    sync::mpsc::{self, Receiver, Sender},
    time,
};

use crate::{
    cfg::{Config, ModeConfig},
    generator::{Generator, SuspendableGenerator},
    shaper::{Shaper, TokenBucket},
    stat::{CommonStat, SocketStat, TxStat},
    ui,
    worker::udp::{LocalStat, Stat, UdpWorker},
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
    bucket: Arc<TokenBucket>,
    is_running: Arc<AtomicBool>,
}

impl Runtime {
    pub fn new(cfg: Config) -> Self {
        let is_running = Arc::new(AtomicBool::new(true));
        let shaper = Arc::new(TokenBucket::new(0));

        Self { cfg, bucket: shaper, is_running }
    }

    pub async fn run(&self) -> Result<(), Box<dyn Error>> {
        let cfg = match &self.cfg.mode {
            ModeConfig::Udp(cfg) => cfg,
        };

        let num_threads = cfg.native.threads.into();
        let mut threads = Vec::with_capacity(num_threads);

        let bind = Arc::new(LocalAddrIter {
            addrs: vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)],
            idx: AtomicUsize::new(0),
        });
        let data = Arc::new(Buffer(b"GET / HTTP/1.1\r\n\r\n".to_vec()));
        let mut stats = Vec::new();

        for idx in 0..num_threads {
            let stat = Arc::new(LocalStat::default());
            let thread = {
                let worker = UdpWorker::new(
                    cfg.addr,
                    bind.clone(),
                    data.clone(),
                    self.is_running.clone(),
                    self.bucket.clone(),
                    stat.clone(),
                )
                .with_requests_per_sock(cfg.native.requests_per_socket());
                Builder::new().name(format!("b:{idx}")).spawn(move || worker.run())?
            };
            threads.push(thread);
            stats.push(stat);
        }

        let stat = Arc::new(Stat::new(stats));
        let (tx, rx) = mpsc::channel(1);

        let ui = self.run_ui(stat.clone(), tx)?;

        let (shaper,) = tokio::join!(self.run_shaper(stat.clone(), rx));
        shaper?;

        ui.join().expect("no self join").unwrap();

        for thread in threads {
            thread.join().expect("no self join");
        }

        Ok(())
    }

    #[allow(clippy::type_complexity)]
    fn run_ui<S>(
        &self,
        stat: Arc<S>,
        tx: Sender<GeneratorEvent>,
    ) -> Result<JoinHandle<Result<(), Box<dyn Error + Send + Sync>>>, Box<dyn Error>>
    where
        S: CommonStat + TxStat + SocketStat + Send + Sync + 'static,
    {
        let is_running = self.is_running.clone();

        let thread = thread::Builder::new().spawn(move || {
            let rc = ui::run(stat, tx);
            is_running.store(false, Ordering::SeqCst);
            rc
        })?;

        Ok(thread)
    }

    async fn run_shaper<S>(&self, stat: Arc<S>, mut rx: Receiver<GeneratorEvent>) -> Result<(), Box<dyn Error>>
    where
        S: CommonStat,
    {
        let g = self.cfg.generator_fn.create()?;
        let mut g = SuspendableGenerator::new(g);
        g.activate();

        let mut shaper = Shaper::new();

        while let Some(cap) = g.current() {
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
            stat.on_generator(cap);

            let tokens = shaper.tick(cap);
            let tokens = self.bucket.put(tokens, cap);
            shaper.consume(tokens);

            // TODO: configurable sleep time.
            time::sleep(Duration::from_millis(1)).await;
        }

        self.is_running.store(true, Ordering::SeqCst);

        Ok(())
    }
}
