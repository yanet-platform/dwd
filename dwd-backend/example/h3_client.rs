//! Simple HTTP/3 client for testing the dwd-backend server.
//!
//! Usage:
//!   cargo run --example h3_client -- https://127.0.0.1:8443/
//!
//! The client does not verify the server certificate (for self-signed certs).

use bytes::Buf;
use quinn::crypto::rustls::QuicClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use std::error::Error;
use std::net::ToSocketAddrs;
use std::sync::Arc;

/// A verifier that accepts any certificate (for testing only!).
#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://127.0.0.1:8443/".to_string());
    let uri: http::Uri = url.parse()?;
    let host = uri.host().ok_or("no host in URL")?.to_string();
    let port = uri.port_u16().unwrap_or(443);
    let addr = (host.as_str(), port)
        .to_socket_addrs()?
        .next()
        .ok_or("failed to resolve address")?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"h3".to_vec()];

    let client_config = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));

    let mut endpoint = quinn::Endpoint::client("[::]:0".parse()?)?;
    endpoint.set_default_client_config(client_config);

    let conn = endpoint.connect(addr, &host)?.await?;
    let quinn_conn = h3_quinn::Connection::new(conn);

    let (mut driver, mut send_request) = h3::client::new(quinn_conn).await?;

    let drive = async move { std::future::poll_fn(|cx| driver.poll_close(cx)).await };

    let request = async move {
        let req = http::Request::builder().uri(uri).body(())?;
        let mut stream = send_request.send_request(req).await?;
        stream.finish().await?;

        let resp = stream.recv_response().await?;
        println!("Status: {}", resp.status());

        let mut body = Vec::new();
        while let Some(mut chunk) = stream.recv_data().await? {
            while chunk.has_remaining() {
                let b = chunk.chunk();
                body.extend_from_slice(b);
                let n = b.len();
                chunk.advance(n);
            }
        }
        println!("Body: {}", String::from_utf8_lossy(&body));
        Ok::<_, Box<dyn Error>>(())
    };

    tokio::select! {
        res = request => res?,
        err = drive => {
            return Err(Box::<dyn Error>::from(err.to_string()));
        }
    }

    endpoint.wait_idle().await;
    Ok(())
}
