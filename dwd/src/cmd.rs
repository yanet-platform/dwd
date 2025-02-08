use core::{error::Error, net::SocketAddr, num::NonZero};
use std::path::PathBuf;

use clap::{ArgAction, Parser};
use pnet::ipnetwork::IpNetwork;

use crate::engine::{
    http::{payload::jsonline::JsonLineRecord, Config as HttpConfig},
    udp::Config as UdpConfig,
};

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

#[derive(Debug, Clone, Parser)]
pub struct HttpRawCmd {
    #[clap(flatten)]
    pub cmd: HttpCmd,
}

#[derive(Debug, Clone, Parser)]
pub struct HttpCmd {
    /// Target endpoint.
    #[clap(required = true)]
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
    #[clap(long, value_name = "PATH", required = true)]
    pub payload_json: Option<PathBuf>,
    /// Set linger TCP option with specified value.
    #[clap(long)]
    pub tcp_linger: Option<u64>,
    /// Enable SOCK_NODELAY socket option.
    #[clap(long)]
    pub tcp_no_delay: bool,
}

impl<T> TryFrom<HttpCmd> for HttpConfig<T>
where
    T: TryFrom<JsonLineRecord, Error = Box<dyn Error>>,
{
    type Error = Box<dyn Error>;

    fn try_from(cmd: HttpCmd) -> Result<Self, Self::Error> {
        let HttpCmd {
            addr,
            native,
            concurrency,
            payload_json,
            tcp_linger,
            tcp_no_delay,
        } = cmd;

        let requests = {
            if let Some(path) = payload_json {
                JsonLineRecord::from_fs(path)?
            } else {
                todo!();
            }
        };

        let m = Self {
            addr,
            native: native.try_into()?,
            concurrency,
            tcp_linger,
            tcp_no_delay,
            requests,
        };

        Ok(m)
    }
}

#[derive(Debug, Clone, Parser)]
pub struct UdpCmd {
    /// Target endpoint.
    #[clap(required = true)]
    pub addr: SocketAddr,
    /// Native workload settings.
    #[clap(flatten)]
    pub native: NativeLoadCmd,
}

impl TryFrom<UdpCmd> for UdpConfig {
    type Error = Box<dyn Error>;

    fn try_from(v: UdpCmd) -> Result<Self, Self::Error> {
        let UdpCmd { addr, native } = v;

        let native = native.try_into()?;

        let m = Self { addr, native };

        Ok(m)
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
