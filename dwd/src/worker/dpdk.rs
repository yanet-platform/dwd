use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

pub use dpdk::cpu::CoreId;
use dpdk::{
    self as dpdk,
    boxed::RteBox,
    cpu::CoreMask,
    eal::{Eal, EalBuilder},
};
use pcap_parser::parse_pcap;
use pnet::{packet::ethernet::MutableEthernetPacket, util::MacAddr};
use serde::Deserialize;
use thiserror::Error;

use crate::{
    shaper::Shaper,
    stat::{BurstTxStat, CommonStat, TxStat},
};

const MBUFS_COUNT: u32 = 256 * 1024;
const MBUF_SIZE: u32 = 10 * 1024;
const MBUFS_BURST_SIZE: u16 = 32;
const MTU: u16 = 9 * 1024;
const PORT_RX_QUEUE_SIZE: u16 = 1024;
const PORT_TX_QUEUE_SIZE: u16 = 1024;

pub type PciDeviceName = String;

#[derive(Debug, Error)]
pub enum Error {
    #[error("i/o error")]
    Io(#[from] io::Error),
    #[error("failed to load pcap file")]
    PcapLoad(io::Error),
    #[error("failed to parse pcap file")]
    PcapParse,
    #[error("invalid PCI device")]
    InvalidPciDevice,
    #[error("invalid ports count")]
    InvalidPortsCount,
    #[error("invalid cores count")]
    InvalidCoresCount,
    #[error("out of memory")]
    OutOfMemory,
    #[error("dpdk error: {0}")]
    DpdkError(#[from] dpdk::error::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Core ID that is used as master.
    master_lcore: CoreId,
    /// Port settings, united by PCI device.
    ///
    /// The PCI device name is expected to be in format "domain:bus:devid.func".
    ports: HashMap<PciDeviceName, PortConfig>,
    /// Path to the pcap file.
    pcap_path: PathBuf,
}

impl Config {
    /// Constructs a new `Config`.
    pub const fn new(master_lcore: CoreId, ports: HashMap<PciDeviceName, PortConfig>, pcap_path: PathBuf) -> Self {
        Self { master_lcore, ports, pcap_path }
    }

    /// Returns the core ID that is used as master.
    #[inline]
    pub fn master_lcore(&self) -> CoreId {
        self.master_lcore
    }

    /// Returns the iterator over CPU cores specified in this config.
    #[inline]
    pub fn cores(&self) -> impl Iterator<Item = CoreId> + '_ {
        self.ports.values().flat_map(|v| v.cores.iter()).copied()
    }

    /// Returns the total number of distinct CPU cores specified in this
    /// config.
    #[inline]
    pub fn cores_count(&self) -> usize {
        let mut cores = HashSet::new();
        for core in self.cores() {
            cores.insert(core);
        }

        cores.len()
    }

    /// Constructs and returns the CPU core mask to be used in EAL.
    pub fn core_mask(&self) -> CoreMask {
        let mut mask = CoreMask::default();
        mask.add(self.master_lcore);
        for id in self.cores() {
            mask.add(id);
        }

        mask
    }

    #[inline]
    pub fn pcap_path(&self) -> &Path {
        &self.pcap_path
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PortConfig {
    /// List of CPU cores to run on.
    cores: Vec<CoreId>,
}

#[no_mangle]
extern "C" fn run_worker(data: *mut core::ffi::c_void) -> i32 {
    // This function can be run simultaneously in multiple threads. We must
    // take care of concurrent access.
    let wg = unsafe { &mut *(data as *mut DpdkEngine) };

    let core_id = dpdk::rte_lcore_id();

    // Main thread.
    if core_id == wg.cfg.master_lcore() {
        return 0;
    }

    let worker = wg.workers.get_mut(&core_id).expect("must be initialized");
    worker.run();

    0
}

/// Represents a group of DPDK workers that are capable of load generation.
///
/// Due to the specifics of DPDK in how multi-threaded applications are
/// organized, for example CPU core pinning, we have to encapsulate the logic
/// in its separate domain.
pub struct DpdkEngine {
    /// Configuration.
    cfg: Config,
    /// Loaded pcap (not pcapng).
    pcap: Vec<u8>,
    /// PPS limits for each worker.
    pps_limits: HashMap<CoreId, Arc<AtomicU64>>,
    /// Runtime statistics.
    stats: HashMap<CoreId, Arc<LocalStat>>,
    ///
    workers: HashMap<CoreId, RteBox<Worker>>,
}

impl DpdkEngine {
    pub fn new(cfg: Config) -> Result<Self, Error> {
        let pcap = fs::read(cfg.pcap_path()).map_err(Error::PcapLoad)?;

        let mut pps_limits = HashMap::new();
        for core in cfg.cores() {
            pps_limits.insert(core, Arc::new(AtomicU64::new(0)));
        }

        let stats = Default::default();
        let workers = HashMap::new();

        // 3. Validate.
        if dpdk::ethdev::eth_dev_count() as usize != cfg.ports.len() {
            return Err(Error::InvalidPortsCount);
        }
        if dpdk::lcore::lcore_count() as usize != cfg.cores_count() + 1 {
            return Err(Error::InvalidCoresCount);
        }

        let m = Self {
            cfg,
            pcap,
            pps_limits,
            stats,
            workers,
        };

        Ok(m)
    }

    /// Returns the copy of shared PPS limits.    
    pub fn pps_limits(&self) -> Vec<Arc<AtomicU64>> {
        self.pps_limits.values().cloned().collect()
    }

    pub fn stats(&self) -> Vec<Arc<LocalStat>> {
        self.stats.values().cloned().collect()
    }

    pub fn run(mut self, is_running: Arc<AtomicBool>) -> Result<(), anyhow::Error> {
        // Init DPDK EAL (Environment Abstraction Layer).
        let _eal = Self::init_eal(&self.cfg)?;

        self.init_workers(is_running)?;
        self.init_ports()?;
        self.load_pcap()?;

        for port_id in dpdk::ethdev::eth_dev_iter() {
            unsafe {
                dpdk::ffi::rte_eth_stats_reset(port_id);
                dpdk::ffi::rte_eth_dev_start(port_id);
                dpdk::ffi::rte_eth_promiscuous_enable(port_id);
            }
        }

        // 6. Spawn threads.
        unsafe {
            dpdk::ffi::rte_eal_mp_remote_launch(
                Some(run_worker),
                &mut self as *mut DpdkEngine as *mut core::ffi::c_void,
                // dpdk::ffi::rte_rmt_call_main_t::SKIP_MAIN,
                dpdk::ffi::rte_rmt_call_master_t::SKIP_MASTER,
            )
        };

        // 7. Join.
        unsafe { dpdk::ffi::rte_eal_mp_wait_lcore() };

        Ok(())
    }

    fn init_eal(cfg: &Config) -> Result<Eal, Error> {
        let name = env!("CARGO_CRATE_NAME");
        let mut eal = EalBuilder::from_coremask(name.into(), cfg.core_mask())
            .with_master_lcore(cfg.master_lcore())
            .with_proc_type("primary")
            .with_in_memory()
            .with_log_capture()?;

        for pci in cfg.ports.keys() {
            eal = eal.with_pci_whitelist(pci);
        }

        log::debug!("EAL: {:?}", eal);
        let eal = eal.init()?;

        Ok(eal)
    }

    fn init_workers(&mut self, is_running: Arc<AtomicBool>) -> Result<(), Error> {
        for core in self.cfg.cores() {
            let socket_id = dpdk::rte_lcore_to_socket_id(core);

            let mempool_name = CString::new(format!("mp::{}", core)).unwrap();

            log::debug!("constructing mempool of size {MBUFS_COUNT} ...");
            let mempool = unsafe {
                dpdk::ffi::rte_pktmbuf_pool_create(
                    mempool_name.as_ptr(),
                    MBUFS_COUNT,
                    64,
                    0,
                    dpdk::ffi::RTE_MBUF_DEFAULT_BUF_SIZE as u16,
                    socket_id as i32,
                )
            };
            if mempool.is_null() {
                return Err(dpdk::error::Error::from_errno().into());
            }

            let pps_limit = self.pps_limits.get(&core).expect("must exist").clone();
            let stat = Arc::new(LocalStat::default());
            let shaper = Shaper::new(0, pps_limit);
            let worker = RteBox::new(
                Worker::new(mempool, shaper, stat.clone(), is_running.clone()),
                socket_id,
            )?;

            self.stats.insert(core, stat);
            self.workers.insert(core, worker);
        }

        Ok(())
    }

    fn init_ports(&mut self) -> Result<HashMap<u16, String>, Error> {
        let mut ports = HashMap::new();

        for (pci, port) in &self.cfg.ports {
            let port_id = dpdk::ethdev::rte_eth_dev_get_port_by_name(&pci)?;
            ports.insert(port_id, pci.clone());

            let socket_id = dpdk::rte_lcore_to_socket_id(CoreId::new(port_id));
            log::debug!("PCI: {pci}, port ID: {port_id}, socket ID: {socket_id}");

            let mut dev_info: dpdk::ffi::rte_eth_dev_info = Default::default();
            if unsafe { dpdk::ffi::rte_eth_dev_info_get(port_id, &mut dev_info) } != 0 {
                return Err(Error::InvalidPciDevice);
            }

            log::debug!("PCI: {pci}, device info: {dev_info:?}");

            // Device MAC address.
            let mut mac = Default::default();
            unsafe { dpdk::ffi::rte_eth_macaddr_get(port_id, &mut mac) };
            let device_mac = MacAddr::from(mac.addr_bytes);
            log::debug!("PCI: {pci}, device MAC: {device_mac}");

            // Neighbour MAC address.
            let neighbours = nl::get_neighbors(dev_info.if_index)?;
            let neighbour_mac = if neighbours.is_empty() {
                log::warn!("PCI: {pci}, no neighbour MAC address found");
                Default::default()
            } else {
                neighbours[0]
            };
            log::debug!("PCI: {pci}, neighbour MAC: {neighbour_mac}");

            let mut port_cfg = dpdk::ffi::rte_eth_conf::default();
            // port_cfg.rxmode.mq_mode = dpdk::ffi::rte_eth_rx_mq_mode::RTE_ETH_MQ_RX_RSS;
            port_cfg.rxmode.mq_mode = dpdk::ffi::rte_eth_rx_mq_mode::ETH_MQ_RX_RSS;
            // port_cfg.rx_adv_conf.rss_conf.rss_hf = 41868; //unsafe {
            // dpdk::ffi::rte_eth_rss_ip().into() };
            port_cfg.rx_adv_conf.rss_conf.rss_hf = dpdk::ffi::ETH_RSS_IP.into();
            // port_cfg.rxmode.max_lro_pkt_size = core::cmp::min(
            //     MBUF_SIZE - 2 * dpdk::ffi::RTE_PKTMBUF_HEADROOM,
            //     dev_info.max_rx_pktlen - 2 * dpdk::ffi::RTE_PKTMBUF_HEADROOM,
            // );
            port_cfg.rxmode.max_rx_pkt_len = core::cmp::min(
                MBUF_SIZE - 2 * dpdk::ffi::RTE_PKTMBUF_HEADROOM,
                dev_info.max_rx_pktlen - 2 * dpdk::ffi::RTE_PKTMBUF_HEADROOM,
            );

            let mtu = MTU;
            log::debug!("max_rx_pkt_len: {}, MTU: {mtu}", port_cfg.rxmode.max_lro_pkt_size);

            let rx_queues_count = port.cores.len() as u16;
            let tx_queues_count = self.cfg.cores_count() as u16 + 1; // +1 for control thread.

            log::debug!("rx_queues_count: {rx_queues_count}, tx_queues_count: {tx_queues_count}");
            let rc = unsafe { dpdk::ffi::rte_eth_dev_configure(port_id, rx_queues_count, tx_queues_count, &port_cfg) };
            if rc < 0 {
                return Err(Error::DpdkError(rc.into()));
            }

            let rc = unsafe { dpdk::ffi::rte_eth_dev_set_mtu(port_id, mtu) };
            if rc != 0 {
                return Err(Error::DpdkError(rc.into()));
            }

            // Init queues.
            for (queue_id, core_id) in port.cores.iter().enumerate() {
                let queue_id = queue_id as u16;
                let worker = self.workers.get_mut(core_id).expect("worker must be initialized");

                let rc = unsafe {
                    dpdk::ffi::rte_eth_rx_queue_setup(
                        port_id,
                        queue_id,
                        PORT_RX_QUEUE_SIZE,
                        dpdk::ffi::rte_eth_dev_socket_id(port_id) as u32,
                        core::ptr::null(),
                        worker.mempool,
                    )
                };
                if rc < 0 {
                    return Err(Error::DpdkError(rc.into()));
                }

                worker.port_id = port_id;
                worker.rx_queue_id = queue_id;
                worker.tx_queue_id = queue_id + 1; // tx queue "0" for slow worker.
                worker.src_mac = device_mac;
            }

            for queue_id in 0..tx_queues_count {
                let rc = unsafe {
                    dpdk::ffi::rte_eth_tx_queue_setup(
                        port_id,
                        queue_id,
                        PORT_TX_QUEUE_SIZE,
                        dpdk::ffi::rte_eth_dev_socket_id(port_id) as u32,
                        core::ptr::null(),
                    )
                };
                if rc < 0 {
                    return Err(Error::DpdkError(rc.into()));
                }
            }
        }

        Ok(ports)
    }

    fn load_pcap(&mut self) -> Result<(), Error> {
        let pcap = match parse_pcap(&self.pcap) {
            Ok((.., pcap)) => pcap,
            Err(..) => {
                return Err(Error::PcapParse);
            }
        };

        let mut packets_count = 0;
        let cores: Vec<CoreId> = self.cfg.cores().collect();
        for (idx, block) in pcap.blocks.iter().enumerate() {
            let mut d = Vec::with_capacity(block.caplen as usize);
            d.resize(block.caplen as usize, 0);
            d.copy_from_slice(block.data);
            let _p = MutableEthernetPacket::new(&mut d).unwrap();
            // todo: fix L2 headers.

            let worker = self.workers.get_mut(&cores[idx % cores.len()]).unwrap();
            let mbuf = unsafe { dpdk::rte_pktmbuf_alloc(worker.mempool) };
            if mbuf.is_null() {
                return Err(Error::OutOfMemory);
            }

            let mbuf_ptr = unsafe { dpdk::rte_pktmbuf_append(mbuf, block.caplen as u16) };
            if mbuf_ptr.is_null() {
                return Err(Error::OutOfMemory);
            }

            unsafe { core::ptr::copy_nonoverlapping(block.data.as_ptr(), mbuf_ptr as *mut u8, block.caplen as usize) };
            let mbufs_count = worker.mbufs_count;
            worker.mbufs[mbufs_count] = mbuf;
            worker.mbufs_count += 1;
            packets_count += 1;
        }
        for (core, worker) in &self.workers {
            log::info!("core {core}: {} packets", worker.mbufs_count);
        }
        log::info!("packets count: {packets_count}");

        Ok(())
    }
}

#[derive(Debug)]
struct Worker {
    port_id: u16,
    rx_queue_id: u16,
    tx_queue_id: u16,
    src_mac: MacAddr,
    mempool: *mut dpdk::ffi::rte_mempool,
    mbufs_count: usize,
    mbufs: [*mut dpdk::ffi::rte_mbuf; MBUFS_COUNT as usize],
    recv_mbufs: [*mut dpdk::ffi::rte_mbuf; MBUFS_BURST_SIZE as usize],
    shaper: Shaper,
    stat: Arc<LocalStat>,
    is_running: Arc<AtomicBool>,
    packets_count_tx: usize,
}

impl Worker {
    pub fn new(
        mempool: *mut dpdk::ffi::rte_mempool,
        shaper: Shaper,
        stat: Arc<LocalStat>,
        is_running: Arc<AtomicBool>,
    ) -> Self {
        Self {
            port_id: 0,
            rx_queue_id: 0,
            tx_queue_id: 0,
            src_mac: Default::default(),
            mempool,
            mbufs_count: 0,
            mbufs: [core::ptr::null_mut(); MBUFS_COUNT as usize],
            recv_mbufs: [core::ptr::null_mut(); MBUFS_BURST_SIZE as usize],
            shaper,
            stat,
            is_running,
            packets_count_tx: 0,
        }
    }

    pub fn run(&mut self) {
        while self.is_running.load(Ordering::Relaxed) {
            // todo: stat.
            let _rx_size = unsafe {
                dpdk::rte_eth_rx_burst(
                    self.port_id,
                    self.rx_queue_id,
                    self.recv_mbufs.as_mut_ptr(),
                    MBUFS_BURST_SIZE,
                )
            };

            if self.packets_count_tx % (self.mbufs_count * 16 * 1024) == 0 {
                for mbuf in &mut self.mbufs[..self.mbufs_count] {
                    unsafe { dpdk::rte_mbuf_refcnt_set(*mbuf, 16 * 1024 + 1024) };
                }
            }

            let tokens = self.shaper.tick();
            if tokens > 0 {
                let count = core::cmp::min(
                    core::cmp::min(32, self.mbufs_count - self.packets_count_tx % self.mbufs_count),
                    tokens as usize,
                ) as u16;

                let tx_size = unsafe {
                    dpdk::rte_eth_tx_burst(
                        self.port_id,
                        self.tx_queue_id,
                        self.mbufs[self.packets_count_tx % self.mbufs_count..].as_mut_ptr(),
                        count,
                    )
                };

                if tx_size > 0 {
                    let mut size = 0u64;
                    for mbuf in &self.mbufs[self.packets_count_tx % self.mbufs_count..][..tx_size as usize] {
                        // size += unsafe { (**mbuf).__bindgen_anon_2.__bindgen_anon_1.pkt_len } as u64;
                        size += unsafe { (**mbuf).pkt_len } as u64;
                    }

                    self.packets_count_tx += tx_size as usize;
                    self.stat.on_requests(tx_size as u64);
                    self.stat.on_send(size);

                    self.shaper.consume(tokens);
                }
            }
        }
    }
}

mod nl {
    use std::io::Error;

    use netlink_packet_core::{NetlinkHeader, NetlinkMessage, NetlinkPayload, NLM_F_DUMP, NLM_F_REQUEST};
    use netlink_packet_route::{
        neighbour::{NeighbourAttribute, NeighbourMessage, NeighbourState},
        AddressFamily, RouteNetlinkMessage,
    };
    use netlink_sys::{protocols::NETLINK_ROUTE, Socket, SocketAddr};
    use pnet::util::MacAddr;

    pub fn get_neighbors(if_index: u32) -> Result<Vec<MacAddr>, Error> {
        let mut socket = Socket::new(NETLINK_ROUTE)?;
        let _port_number = socket.bind_auto()?.port_number();
        socket.connect(&SocketAddr::new(0, 0))?;

        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_DUMP | NLM_F_REQUEST;

        let mut nd_msg = NeighbourMessage::default();
        nd_msg.header.ifindex = if_index;
        nd_msg.header.state = NeighbourState::Reachable;
        let mut req = NetlinkMessage::new(nl_hdr, NetlinkPayload::from(RouteNetlinkMessage::GetNeighbour(nd_msg)));
        req.finalize();

        let mut buf = vec![0; req.header.length as usize];
        req.serialize(&mut buf[..]);

        socket.send(&buf[..], 0)?;

        let mut receive_buffer = vec![0; 4096];
        let mut offset = 0;

        let mut out = Vec::new();
        'outer: loop {
            let size = socket.recv(&mut &mut receive_buffer[..], 0)?;

            loop {
                let bytes = &receive_buffer[offset..];
                let msg: NetlinkMessage<RouteNetlinkMessage> = NetlinkMessage::deserialize(bytes).unwrap();

                match msg.payload {
                    NetlinkPayload::Done(_) => break 'outer,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewNeighbour(entry)) => {
                        let address_family = entry.header.family;
                        if (address_family == AddressFamily::Inet || address_family == AddressFamily::Inet6)
                            && entry.header.state == NeighbourState::Reachable
                            && entry.header.ifindex == if_index
                        {
                            entry.attributes.iter().for_each(|nla| {
                                if let NeighbourAttribute::LinkLocalAddress(addr) = nla {
                                    if addr.len() == 6 {
                                        let mut buf = [0u8; 6];
                                        buf.copy_from_slice(&addr);
                                        out.push(MacAddr::from(buf));
                                    }
                                };
                            });
                        };
                    }
                    NetlinkPayload::Error(err) => return Err(err.into()),
                    _ => {}
                }

                offset += msg.header.length as usize;
                if offset == size || msg.header.length == 0 {
                    offset = 0;
                    break;
                }
            }
        }

        Ok(out)
    }
}

#[derive(Debug, Default)]
pub struct LocalStat {
    num_requests: UnsafeCell<u64>,
    bytes_tx: UnsafeCell<u64>,
    bursts_tx: [UnsafeCell<u64>; 32],
}

unsafe impl Sync for LocalStat {}

#[derive(Debug, Default)]
pub struct Stat {
    generator: AtomicU64,
    stats: Vec<Arc<LocalStat>>,
}

impl Stat {
    pub fn new(stats: Vec<Arc<LocalStat>>) -> Self {
        Self { stats, ..Default::default() }
    }
}

impl CommonStat for Stat {
    #[inline]
    fn generator(&self) -> u64 {
        self.generator.load(Ordering::Relaxed)
    }

    #[inline]
    fn on_generator(&self, v: u64) {
        self.generator.store(v, Ordering::Relaxed);
    }
}

impl TxStat for Stat {
    #[inline]
    fn num_requests(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.num_requests.get() }).sum()
    }

    #[inline]
    fn bytes_tx(&self) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.bytes_tx.get() }).sum()
    }
}

impl BurstTxStat for Stat {
    #[inline]
    fn num_bursts_tx(&self, idx: usize) -> u64 {
        self.stats.iter().map(|v| unsafe { *v.bursts_tx[idx].get() }).sum()
    }
}

impl LocalStat {
    #[inline]
    pub fn on_requests(&self, n: u64) {
        unsafe { *self.num_requests.get() += n };
        unsafe { *self.bursts_tx[n as usize - 1].get() += 1 };
    }

    #[inline]
    pub fn on_send(&self, n: u64) {
        unsafe { *self.bytes_tx.get() += n };
    }
}
