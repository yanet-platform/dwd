use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// Command-line interface.
///
/// Two mutually exclusive ways to run:
///   * `--config <path>` — full config-driven mode (HTTP/HTTPS/HTTP3 per the
///     TOML file);
///   * `--mode <proto>` — standalone single-protocol mode configured entirely
///     from CLI flags (`--port`, `--cert`, `--key`), ignoring the config file.
///
/// With neither flag, the config path falls back to `DWD_BACKEND_CONFIG` or the
/// default location.
#[derive(Debug, Parser)]
#[command(name = "dwd-backend", version, about)]
pub struct Cli {
    /// Path to the TOML config (config-driven mode).
    #[arg(short, long, value_name = "PATH", conflicts_with = "mode")]
    pub config: Option<PathBuf>,

    /// Run a single protocol standalone, configured from CLI flags.
    #[arg(short, long, value_name = "PROTO")]
    pub mode: Option<Mode>,

    /// Listen port (standalone mode; defaults to 80 for http, 443 otherwise).
    #[arg(short, long, requires = "mode")]
    pub port: Option<u16>,

    /// Certificate path (standalone https/http3; generated if omitted).
    /// Meaningless alone, so it requires --key.
    #[arg(long, value_name = "PATH", requires = "mode", requires = "key")]
    pub cert: Option<PathBuf>,

    /// Private key path (standalone https/http3; generated if omitted).
    /// Meaningless alone, so it requires --cert.
    #[arg(long, value_name = "PATH", requires = "mode", requires = "cert")]
    pub key: Option<PathBuf>,
}

/// Protocol selected by `--mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    Http,
    Https,
    Http3,
}

impl Mode {
    /// Default listen port when `--port` is not given.
    pub fn default_port(self) -> u16 {
        match self {
            Mode::Http => 80,
            Mode::Https | Mode::Http3 => 443,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn uses_renamed_binary_name() {
        assert_eq!(Cli::command().get_name(), "dwd-backend");
    }

    #[test]
    fn config_and_standalone_modes_conflict() {
        let args = ["dwd-backend", "--config", "config.toml", "--mode", "http"];
        assert!(Cli::try_parse_from(args).is_err());
    }
}
