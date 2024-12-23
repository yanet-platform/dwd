use core::{net::SocketAddr, num::NonZero};
use std::path::PathBuf;

use clap::{ArgAction, Parser};

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
pub struct UdpCmd {
    /// Target endpoint.
    #[clap(required = true)]
    pub addr: SocketAddr,
    /// Native workload settings.
    #[clap(flatten)]
    pub native: NativeLoadCmd,
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