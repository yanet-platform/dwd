//! CLI-facing application config.
//!
//! Builds the core [`ModeConfig`] and the generator factory from the parsed clap
//! [`Cmd`]. The heavy config types live in [`dwd_core::cfg`]; this is just the
//! CLI → core bridge plus the top-level [`Config`] the orchestrator consumes.

use core::{error::Error, net::SocketAddr, time::Duration};

use dwd_core::{
    cfg::{BoxedGeneratorNew, ModeConfig},
    generator::{self, Generator, LineGenerator},
};

use crate::cmd::Cmd;

/// Top-level application config.
#[derive(Debug)]
pub struct Config {
    /// The selected load mode and its resolved config.
    pub mode: ModeConfig,
    /// Deferred generator factory.
    pub generator_fn: BoxedGeneratorNew,
    /// Address to expose the Prometheus API on.
    pub api_addr: Option<SocketAddr>,
}

impl TryFrom<Cmd> for Config {
    type Error = Box<dyn Error>;

    fn try_from(v: Cmd) -> Result<Self, Self::Error> {
        let mode = v.mode.into_config()?;
        let api_addr = v.api_addr;
        let generator_fn = {
            let path = v.generator.clone();

            BoxedGeneratorNew::new(Box::new(move || -> Result<Box<dyn Generator>, anyhow::Error> {
                match &path {
                    Some(path) => generator::load(path),
                    None => {
                        const CENTURY: Duration = Duration::from_secs(86400 * 365 * 100);
                        let generator = LineGenerator::new(1000, 1000, CENTURY);

                        Ok(Box::new(generator))
                    }
                }
            }))
        };

        Ok(Self { mode, generator_fn, api_addr })
    }
}
