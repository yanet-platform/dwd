use core::{
    error::Error,
    fmt::{self, Debug, Formatter},
    net::SocketAddr,
    num::NonZero,
};

#[cfg(feature = "dpdk")]
use {
    crate::{
        cmd::DpdkCmd,
        worker::dpdk::{Config as DpdkWorkerConfig, PciDeviceName, PortConfig},
    },
    dpdk::cpu::CoreId,
    serde::Deserialize,
    std::{collections::HashMap, fs},
};

use crate::{
    cmd::{Cmd, ModeCmd, NativeLoadCmd, UdpCmd},
    generator::{self, Generator, LineGenerator},
};

#[derive(Debug)]
pub struct Config {
    pub mode: ModeConfig,
    pub generator_fn: BoxedGeneratorNew,
}

impl TryFrom<Cmd> for Config {
    type Error = Box<dyn Error>;

    fn try_from(v: Cmd) -> Result<Self, Self::Error> {
        let mode = v.mode.try_into()?;
        let generator_fn = {
            let generator = v.generator.clone();

            Box::new(move || -> Result<Box<dyn Generator>, Box<dyn Error>> {
                match &generator {
                    Some(path) => generator::load(path),
                    None => Ok(Box::new(LineGenerator::infinite_max())),
                }
            })
        };

        let m = Self {
            mode,
            generator_fn: BoxedGeneratorNew(generator_fn),
        };

        Ok(m)
    }
}

#[derive(Debug, Clone)]
pub enum ModeConfig {
    Udp(UdpConfig),
    #[cfg(feature = "dpdk")]
    Dpdk(DpdkConfig),
}

impl TryFrom<ModeCmd> for ModeConfig {
    type Error = Box<dyn Error>;

    fn try_from(v: ModeCmd) -> Result<Self, Self::Error> {
        let m = match v {
            ModeCmd::Udp(v) => Self::Udp(v.try_into()?),
            #[cfg(feature = "dpdk")]
            ModeCmd::Dpdk(v) => Self::Dpdk(v.try_into()?),
        };

        Ok(m)
    }
}

#[derive(Debug, Clone)]
pub struct UdpConfig {
    /// Target endpoint.
    pub addr: SocketAddr,
    /// Native workload settings.
    pub native: NativeLoadConfig,
}

impl TryFrom<UdpCmd> for UdpConfig {
    type Error = Box<dyn Error>;

    fn try_from(v: UdpCmd) -> Result<Self, Self::Error> {
        let native = v.native.try_into()?;

        let m = Self { addr: v.addr, native };

        Ok(m)
    }
}

/// Native workload config.
#[derive(Debug, Clone)]
pub struct NativeLoadConfig {
    /// Number of threads.
    pub threads: NonZero<usize>,
    /// Maximum number of requests executed per socket before reconnection.
    /// If none given (default) sockets renew is disabled.
    requests_per_socket: Option<u64>,
}

impl NativeLoadConfig {
    /// Returns the maximum number of requests executed per socket before
    /// reconnection.
    #[inline]
    pub fn requests_per_socket(&self) -> u64 {
        self.requests_per_socket.unwrap_or(u64::MAX)
    }
}

impl TryFrom<NativeLoadCmd> for NativeLoadConfig {
    type Error = Box<dyn Error>;

    fn try_from(cmd: NativeLoadCmd) -> Result<Self, Self::Error> {
        let NativeLoadCmd { threads, requests_per_socket } = cmd;

        let m = Self { threads, requests_per_socket };

        Ok(m)
    }
}

#[cfg(feature = "dpdk")]
#[derive(Debug, Clone)]
pub struct DpdkConfig(DpdkWorkerConfig);

#[cfg(feature = "dpdk")]
impl DpdkConfig {
    #[inline]
    pub fn into_inner(self) -> DpdkWorkerConfig {
        self.0
    }
}

#[cfg(feature = "dpdk")]
impl TryFrom<DpdkCmd> for DpdkConfig {
    type Error = Box<dyn Error>;

    fn try_from(v: DpdkCmd) -> Result<Self, Self::Error> {
        #[derive(Deserialize)]
        struct Cfg {
            master_lcore: CoreId,
            ports: HashMap<PciDeviceName, PortConfig>,
        }

        let data = fs::read(&v.dpdk_path)?;
        let cfg: Cfg = serde_yaml::from_slice(&data)?;

        let m = Self(DpdkWorkerConfig::new(cfg.master_lcore, cfg.ports, v.pcap_path));

        Ok(m)
    }
}

pub type BoxedGenerator = Box<dyn Generator>;
pub struct BoxedGeneratorNew(Box<dyn Fn() -> Result<BoxedGenerator, Box<dyn Error>> + Send>);

impl BoxedGeneratorNew {
    #[inline]
    pub fn create(&self) -> Result<BoxedGenerator, Box<dyn Error>> {
        match self {
            Self(f) => f(),
        }
    }
}

impl Debug for BoxedGeneratorNew {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), fmt::Error> {
        fmt.debug_tuple("GeneratorFn").finish()
    }
}
