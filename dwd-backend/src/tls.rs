use quinn::crypto::rustls::QuicServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;
use std::sync::Arc;

/// Certificate chain and private key shared by the TCP and QUIC TLS configs.
pub struct CertKey {
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

impl Clone for CertKey {
    fn clone(&self) -> Self {
        Self {
            cert_chain: self.cert_chain.clone(),
            key: self.key.clone_key(),
        }
    }
}

/// Wraps an arbitrary error into an io::Error(InvalidData).
fn invalid_data<E: std::fmt::Display>(error: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

/// Loads the certificate and key from files, or generates them on the fly.
pub fn get_cert_and_key(cert_path: Option<&Path>, key_path: Option<&Path>) -> io::Result<CertKey> {
    let (cert_path, key_path) = match (cert_path, key_path) {
        (None, None) => {
            println!("Generating a temporary self-signed certificate...");
            let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).map_err(invalid_data)?;
            return Ok(CertKey {
                cert_chain: vec![cert.cert.der().clone()],
                key: PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
            });
        }
        (Some(cert_path), Some(key_path)) => (cert_path, key_path),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "certificate and private key must be configured together",
            ));
        }
    };

    let mut cert_file = BufReader::new(File::open(cert_path)?);
    let mut key_file = BufReader::new(File::open(key_path)?);

    let cert_chain = rustls_pemfile::certs(&mut cert_file)
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid_data)?;
    if cert_chain.is_empty() {
        return Err(invalid_data(format!(
            "no certificates found in {}",
            cert_path.display()
        )));
    }

    let key = rustls_pemfile::private_key(&mut key_file)
        .map_err(invalid_data)?
        .ok_or_else(|| invalid_data(format!("no private key found in {}", key_path.display())))?;

    Ok(CertKey { cert_chain, key })
}

/// Builds a base rustls::ServerConfig.
///
/// The ring provider is selected explicitly because ring and aws-lc-rs are
/// both present in the dependency tree.
pub fn build_rustls_config(cert_key: CertKey) -> io::Result<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(invalid_data)?
        .with_no_client_auth()
        .with_single_cert(cert_key.cert_chain, cert_key.key)
        .map_err(invalid_data)
}

/// Builds a quinn::ServerConfig for HTTP/3 (QUIC).
pub fn build_quic_config(cert_key: CertKey) -> io::Result<quinn::ServerConfig> {
    let mut tls = build_rustls_config(cert_key)?;
    tls.alpn_protocols = vec![b"h3".to_vec()];
    tls.max_early_data_size = u32::MAX;

    let quic_tls = QuicServerConfig::try_from(tls).map_err(invalid_data)?;
    Ok(quinn::ServerConfig::with_crypto(Arc::new(quic_tls)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_incomplete_certificate_pair() {
        let error = get_cert_and_key(Some(Path::new("cert.pem")), None)
            .err()
            .expect("an incomplete certificate pair must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
