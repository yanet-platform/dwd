mod cli;
mod config;
mod server;
mod tls;

use clap::Parser;
use cli::{Cli, Mode};
use config::Config;
use server::{bind_http3, build_body, resolve_socket_addr, run_http3_server, run_tcp_servers};
use std::io;
use std::path::Path;
use tls::{build_quic_config, build_rustls_config, get_cert_and_key};

/// Default listen address for standalone mode (matches the config default).
const DEFAULT_ADDRESS: &str = "::";
const DEFAULT_CONFIG_PATH: &str = "/etc/dwd-backend/config.toml";
const CONFIG_ENV: &str = "DWD_BACKEND_CONFIG";

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;

/// Config-driven mode: run HTTP/HTTPS/HTTP3 as described by the TOML file.
async fn run_from_config(config_path: &Path) -> io::Result<()> {
    let cfg = Config::load(config_path)?;
    let body = build_body(cfg.resolve_content()?);

    let https_keys = if cfg.https.enabled {
        Some(get_cert_and_key(cfg.https.cert.as_deref(), cfg.https.key.as_deref())?)
    } else {
        None
    };

    if cfg.http3.enabled {
        let http3 = &cfg.http3;
        let cert_key = match (http3.cert.as_deref(), http3.key.as_deref(), &https_keys) {
            (None, None, Some(keys)) => keys.clone(),
            (cert, key, _) => get_cert_and_key(cert, key)?,
        };

        let quic_config = build_quic_config(cert_key)?;
        let addr = resolve_socket_addr(&cfg.address, http3.port)?;
        let endpoint = bind_http3(addr, quic_config)?;
        let http3_body = body.clone();
        tokio::spawn(run_http3_server(endpoint, http3_body));

        println!("HTTP3 listening on {}:{} (UDP/QUIC)", cfg.address, http3.port);
    }

    let https = https_keys
        .map(build_rustls_config)
        .transpose()?
        .map(|tls_config| (cfg.https.port, tls_config));

    run_tcp_servers(&cfg.address, Some(cfg.http.port), https, body).await
}

/// Standalone mode: run one protocol configured entirely from CLI flags.
async fn run_single(cli: &Cli, mode: Mode) -> io::Result<()> {
    let port = cli.port.unwrap_or_else(|| mode.default_port());
    let body = build_body(config::default_content_payload());

    match mode {
        Mode::Http => run_tcp_servers(DEFAULT_ADDRESS, Some(port), None, body).await,
        Mode::Https => {
            let keys = get_cert_and_key(cli.cert.as_deref(), cli.key.as_deref())?;
            let tls_config = build_rustls_config(keys)?;
            run_tcp_servers(DEFAULT_ADDRESS, None, Some((port, tls_config)), body).await
        }
        Mode::Http3 => {
            let cert_key = get_cert_and_key(cli.cert.as_deref(), cli.key.as_deref())?;
            let quic_config = build_quic_config(cert_key)?;
            let addr = resolve_socket_addr(DEFAULT_ADDRESS, port)?;
            let endpoint = bind_http3(addr, quic_config)?;
            println!("HTTP3 listening on {DEFAULT_ADDRESS}:{port} (UDP/QUIC)");
            run_http3_server(endpoint, body).await;
            Ok(())
        }
    }
}

// The multi-threaded tokio runtime lets QUIC/HTTP3 tasks use all cores; Actix
// manages its own worker threads.
#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();

    if let Some(mode) = cli.mode {
        return run_single(&cli, mode).await;
    }

    let config_path = cli
        .config
        .clone()
        .or_else(|| std::env::var_os(CONFIG_ENV).map(Into::into))
        .unwrap_or_else(|| DEFAULT_CONFIG_PATH.into());

    run_from_config(&config_path).await
}
