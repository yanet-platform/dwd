//! TLS configuration for the HTTP/3 client.
//!
//! Load testing usually targets bare IPs / staging hosts without valid
//! certificates, so certificate verification is **disabled by default**: the
//! verifier below accepts any presented chain. This is deliberate and matches
//! the tool's purpose; it must never be reused in a security context.

use std::sync::Arc;

use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{ring, verify_tls12_signature, verify_tls13_signature, CryptoProvider},
    pki_types::{CertificateDer, ServerName, UnixTime},
    ClientConfig, DigitallySignedStruct, SignatureScheme,
};

/// ALPN protocol identifier for HTTP/3.
pub const ALPN_H3: &[u8] = b"h3";

/// A [`ServerCertVerifier`] that accepts any certificate without verification.
#[derive(Debug)]
struct NoVerifier(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Builds a rustls [`ClientConfig`] for HTTP/3 that skips certificate
/// verification and advertises the `h3` ALPN protocol.
pub fn insecure_client_config() -> ClientConfig {
    let provider = Arc::new(ring::default_provider());

    // QUIC mandates TLS 1.3; pin it explicitly so the config stays valid for
    // `QuicClientConfig` even when the `tls12` feature is enabled crate-wide
    // (the HTTP/1 engine turns it on for broader server compatibility).
    let mut config = ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("ring provider supports TLS 1.3")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifier(provider)))
        .with_no_client_auth();

    config.alpn_protocols = vec![ALPN_H3.to_vec()];
    config
}

#[cfg(test)]
mod tests {
    use super::{insecure_client_config, ALPN_H3};

    #[test]
    fn advertises_h3_alpn() {
        let config = insecure_client_config();
        assert_eq!(config.alpn_protocols, vec![ALPN_H3.to_vec()]);
    }

    #[test]
    fn convertible_to_quic_client_config() {
        let config = insecure_client_config();
        // QUIC requires TLS 1.3; conversion fails if the config disallows it.
        quinn::crypto::rustls::QuicClientConfig::try_from(config).expect("valid QUIC TLS config");
    }
}
