use core::{error::Error, net::SocketAddr, num::NonZero, time::Duration};
use std::{
    net::{IpAddr, Ipv6Addr},
    path::PathBuf,
};

use clap::{ArgAction, Parser};
#[cfg(feature = "dpdk")]
use dwd_core::{
    cfg::DpdkConfig,
    worker::dpdk::{Config as DpdkWorkerConfig, CoreId, PciDeviceName, PortConfig},
};
use dwd_core::{
    cfg::{ModeConfig, NativeLoadConfig},
    engine::{
        http::{payload::jsonline::JsonLineRecord, Config as HttpConfig},
        udp::Config as UdpConfig,
    },
};
use pnet::ipnetwork::IpNetwork;

/// The traffic generator we deserve.
#[derive(Debug, Clone, Parser)]
#[command(version, about)]
#[command(flatten_help = true)]
pub struct Cmd {
    #[clap(subcommand)]
    pub mode: ModeCmd,
    /// Path to the generator file.
    /// See /etc/dwd/generator.yaml for details.
    #[clap(long, global = true)]
    pub generator: Option<PathBuf>,
    /// Be verbose in terms of logging.
    #[clap(short, action = ArgAction::Count, global = true)]
    pub verbose: u8,
    /// Address to expose API on.
    ///
    /// When specified, starts an HTTP server that exposes API that can be used
    /// to observe the generator state and metrics.
    #[clap(long, global = true, value_name = "IP:PORT")]
    pub api_addr: Option<SocketAddr>,
}

#[derive(Debug, Clone, Parser)]
pub enum ModeCmd {
    /// HTTP mode.
    Http(HttpCmd),
    /// Fast, but restricted HTTP mode.
    #[command(name = "http/raw")]
    HttpRaw(HttpRawCmd),
    /// UDP mode.
    ///
    /// Response packets (if any) will be ignored.
    Udp(UdpCmd),
    #[cfg(feature = "dpdk")]
    /// DPDK mode.
    ///
    /// This mode is capable of generating very intensive workload by utilizing
    /// the DPDK library.
    /// The core idea is kernel bypass and working with NIC entirely in
    /// userspace.
    ///
    /// Note, that this mode requires the application to be run with CAP_ADMIN
    /// capabilities.
    Dpdk(DpdkCmd),
}

impl ModeCmd {
    /// Resolves the selected mode into its core configuration.
    pub fn into_config(self) -> Result<ModeConfig, Box<dyn Error>> {
        let m = match self {
            ModeCmd::Http(v) => ModeConfig::Http(v.into_config()?),
            ModeCmd::HttpRaw(v) => ModeConfig::HttpRaw(v.cmd.into_config()?),
            ModeCmd::Udp(v) => ModeConfig::Udp(v.into_config()?),
            #[cfg(feature = "dpdk")]
            ModeCmd::Dpdk(v) => ModeConfig::Dpdk(v.into_config()?),
        };

        Ok(m)
    }
}

#[derive(Debug, Clone, Parser)]
pub struct HttpRawCmd {
    #[clap(flatten)]
    pub cmd: HttpCmd,
}

#[derive(Debug, Clone, Parser)]
pub struct HttpCmd {
    /// Target endpoint.
    #[clap(required = true, value_name = "IPv4:PORT or [IPv6]:PORT")]
    pub addr: SocketAddr,
    /// Native workload settings.
    #[clap(flatten)]
    pub native: NativeLoadCmd,
    /// Number of parallel jobs.
    ///
    /// This also limits the maximum concurrent requests in flight. To achieve
    /// better runtime characteristics this value should be the multiple of
    /// the number of threads.
    #[clap(short, long, default_value_t = std::thread::available_parallelism().unwrap_or(NonZero::<usize>::MIN))]
    pub concurrency: NonZero<usize>,
    /// Path to the JSON payload file.
    #[clap(long, value_name = "PATH")]
    pub payload_json: PathBuf,
    /// Set linger TCP option with specified value.
    #[clap(long)]
    pub tcp_linger: Option<u64>,
    /// Enable SOCK_NODELAY socket option.
    #[clap(long)]
    pub tcp_no_delay: bool,
    /// Request timeout in seconds with fractional part.
    #[clap(long, default_value_t = 4.0)]
    pub timeout: f64,
}

impl HttpCmd {
    /// Loads the payloads and resolves this command into an HTTP engine config.
    pub fn into_config<T>(self) -> Result<HttpConfig<T>, Box<dyn Error>>
    where
        T: TryFrom<JsonLineRecord, Error = Box<dyn Error>>,
    {
        let HttpCmd {
            addr,
            native,
            concurrency,
            payload_json,
            timeout,
            tcp_linger,
            tcp_no_delay,
        } = self;

        let requests = JsonLineRecord::from_fs(payload_json)?;

        Ok(HttpConfig {
            addr,
            native: native.into_config()?,
            concurrency,
            timeout: Duration::try_from_secs_f64(timeout)?,
            tcp_linger,
            tcp_no_delay,
            requests,
        })
    }
}

#[derive(Debug, Clone, Parser)]
pub struct UdpCmd {
    /// Target endpoint.
    #[clap(required = true, value_name = "IPv4:PORT or [IPv6]:PORT")]
    pub addr: SocketAddr,
    /// Native workload settings.
    #[clap(flatten)]
    pub native: NativeLoadCmd,
}

impl UdpCmd {
    /// Resolves this command into a UDP engine config.
    pub fn into_config(self) -> Result<UdpConfig, Box<dyn Error>> {
        let UdpCmd { addr, native } = self;

        Ok(UdpConfig { addr, native: native.into_config()? })
    }
}

/// Native workload config.
#[derive(Debug, Clone, Parser)]
pub struct NativeLoadCmd {
    /// Number of threads.
    #[clap(short, long, default_value_t = std::thread::available_parallelism().unwrap_or(NonZero::<usize>::MIN))]
    pub threads: NonZero<usize>,
    /// Maximum number of requests executed per socket before reconnection.
    ///
    /// If none given (the default) sockets renew is disabled.
    #[clap(long)]
    pub requests_per_socket: Option<u64>,
    /// Deviation of the number of requests per socket.
    ///
    /// For example, if the number of requests per socket is 1000 and the
    /// deviation is 100, then the number of requests per socket will be
    /// between 900 and 1100.
    #[clap(long)]
    pub requests_per_socket_deviation: Option<u64>,
    /// IP prefix used to collect this machine's global unicast IP addresses and
    /// use them as bind addresses.
    ///
    /// For example, specifying "2a02:6b8:0:320:1111:1111:1111::/112" will scan
    /// all interfaces, collect all global unicast IP addresses and filter
    /// them by the given prefix. This option conflicts with the "bind-ips"
    /// argument.
    #[clap(long)]
    pub bind_network: Option<IpNetwork>,
}

impl NativeLoadCmd {
    /// Resolves bind addresses (scanning interfaces if `--bind-network` is given)
    /// into a native workload config.
    pub fn into_config(self) -> Result<NativeLoadConfig, Box<dyn Error>> {
        let NativeLoadCmd {
            threads,
            requests_per_socket,
            requests_per_socket_deviation,
            bind_network,
        } = self;

        let mut bind_endpoints = Vec::new();
        match bind_network {
            Some(net) => {
                for link in pnet::datalink::interfaces() {
                    if !link.is_up() || link.is_loopback() || link.ips.is_empty() {
                        continue;
                    }

                    bind_endpoints.extend(
                        link.ips
                            .into_iter()
                            .filter(|v| net.contains(v.ip()))
                            .map(|v| SocketAddr::new(v.ip(), 0)),
                    );
                }
            }
            None => {
                bind_endpoints.push(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0));
            }
        }

        log::debug!("bind endpoints: {:?}", bind_endpoints);

        Ok(NativeLoadConfig::new(
            threads,
            requests_per_socket,
            requests_per_socket_deviation,
            bind_endpoints,
        ))
    }
}

#[derive(Debug, Clone, Parser)]
pub struct DpdkCmd {
    /// Path to the DPDK configuration file in YAML format.
    #[clap(long, required = true)]
    pub dpdk_path: PathBuf,
    /// Path to the PCAP file (not pcapng!).
    ///
    /// Packets containing in this file will be used as a workload, looping
    /// infinitely until profile exhaustion.
    #[clap(long, required = true)]
    pub pcap_path: PathBuf,
}

#[cfg(feature = "dpdk")]
impl DpdkCmd {
    /// Parses the DPDK YAML config and resolves this command into a DPDK config.
    pub fn into_config(self) -> Result<DpdkConfig, Box<dyn Error>> {
        use std::{collections::HashMap, fs};

        use serde::Deserialize;

        #[derive(Deserialize)]
        struct Cfg {
            master_lcore: CoreId,
            ports: HashMap<PciDeviceName, PortConfig>,
        }

        let data = fs::read(&self.dpdk_path)?;
        let cfg: Cfg = serde_yaml::from_slice(&data)?;

        Ok(DpdkConfig::new(DpdkWorkerConfig::new(
            cfg.master_lcore,
            cfg.ports,
            self.pcap_path,
        )))
    }
}
