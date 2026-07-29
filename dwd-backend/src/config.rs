use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Single listen address for all protocols ("::" or "0.0.0.0").
    pub address: String,

    pub http: HttpConfig,

    /// Enabled by default. The certificate/key are taken from cert/key or generated.
    pub https: HttpsConfig,

    /// Enabled by default. Does not require [https]: the certificate is taken from
    /// cert/key, reused from [https], or generated.
    pub http3: Http3Config,

    pub content: ContentConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            address: default_address(),
            http: HttpConfig::default(),
            https: HttpsConfig::default(),
            http3: Http3Config::default(),
            content: ContentConfig::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct HttpConfig {
    pub port: u16,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self { port: default_http_port() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct HttpsConfig {
    /// Enabled by default. Disable with `enabled = false`.
    pub enabled: bool,
    pub port: u16,
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
}

impl Default for HttpsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: default_https_port(),
            cert: None,
            key: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Http3Config {
    /// Enabled by default. Disable with `enabled = false`.
    pub enabled: bool,
    pub port: u16,
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
}

impl Default for Http3Config {
    fn default() -> Self {
        Self {
            enabled: true,
            port: default_http3_port(),
            cert: None,
            key: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ContentConfig {
    pub file: PathBuf,
    pub payload: String,
}

impl Default for ContentConfig {
    fn default() -> Self {
        Self {
            file: default_content_file(),
            payload: default_content_payload(),
        }
    }
}

fn default_address() -> String {
    "::".to_string()
}

fn default_http_port() -> u16 {
    80
}

fn default_https_port() -> u16 {
    443
}

fn default_http3_port() -> u16 {
    443
}

fn default_content_file() -> PathBuf {
    "/etc/dwd-backend/content".into()
}

pub fn default_content_payload() -> String {
    "default content".to_string()
}

impl Config {
    /// Loads the configuration from a TOML file at the given path.
    /// If the file is missing, returns a config with default values.
    pub fn load<P: AsRef<Path>>(path: P) -> std::io::Result<Config> {
        let path = path.as_ref();
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("Config {} not found, using default values", path.display());
                return Ok(Config::default());
            }
            Err(e) => return Err(e),
        };
        toml::from_str(&raw).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }

    /// Returns the effective content: the file contents if the file exists,
    /// otherwise the payload from the config. A single read (no separate
    /// existence check) avoids the extra stat and the check-then-read race.
    pub fn resolve_content(&self) -> std::io::Result<String> {
        match fs::read_to_string(&self.content.file) {
            Ok(content) => Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(self.content.payload.clone()),
            Err(e) => Err(e),
        }
    }
}
