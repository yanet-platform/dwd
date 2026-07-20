//! TLS support for the HTTP/1 engine (HTTPS).
//!
//! Load testing usually targets bare IPs / staging hosts without valid
//! certificates, so certificate verification is **disabled**: the verifier
//! below accepts any presented chain. This mirrors the HTTP/3 engine's
//! [`tls`](crate::engine::http3) behaviour and is deliberate — it must never be
//! reused in a security context.

use core::net::IpAddr;
use std::sync::Arc;

use anyhow::Error;
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{ring, verify_tls12_signature, verify_tls13_signature, CryptoProvider},
    pki_types::{CertificateDer, ServerName, UnixTime},
    ClientConfig, DigitallySignedStruct, SignatureScheme,
};
use tokio::net::TcpStream;
use tokio_rustls::{client::TlsStream, TlsConnector};

/// ALPN protocol identifier for HTTP/1.1.
const ALPN_HTTP11: &[u8] = b"http/1.1";

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

/// Builds a rustls [`ClientConfig`] for HTTP/1 that skips certificate
/// verification and advertises the `http/1.1` ALPN protocol.
fn insecure_client_config() -> ClientConfig {
    let provider = Arc::new(ring::default_provider());

    let mut config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("ring provider supports the default TLS versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifier(provider)))
        .with_no_client_auth();

    config.alpn_protocols = vec![ALPN_HTTP11.to_vec()];
    config
}

/// TLS client for the HTTP/1 engine.
///
/// Wraps a shared [`TlsConnector`] and the SNI server name presented on every
/// handshake. Cloning is cheap (the connector is reference-counted internally),
/// so every worker keeps its own handle.
#[derive(Clone)]
pub struct Tls {
    connector: TlsConnector,
    server_name: ServerName<'static>,
}

impl Tls {
    /// Builds a TLS client. Certificate verification is disabled.
    ///
    /// `server_name` sets the SNI hostname; when `None`, the target `ip` is
    /// used, in which case rustls omits the (hostname-only) SNI extension.
    pub fn new(server_name: Option<&str>, ip: IpAddr) -> Result<Self, Error> {
        let server_name = match server_name {
            Some(name) => ServerName::try_from(name.to_owned())?,
            None => ServerName::IpAddress(ip.into()),
        };
        let connector = TlsConnector::from(Arc::new(insecure_client_config()));

        Ok(Self { connector, server_name })
    }

    /// Performs the TLS handshake over an already-established TCP stream.
    #[inline]
    pub async fn connect(&self, stream: TcpStream) -> Result<TlsStream<TcpStream>, Error> {
        Ok(self.connector.connect(self.server_name.clone(), stream).await?)
    }
}

impl core::fmt::Debug for Tls {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Tls")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use core::net::{IpAddr, Ipv4Addr};

    use super::{insecure_client_config, Tls, ALPN_HTTP11};

    #[test]
    fn advertises_http11_alpn() {
        let config = insecure_client_config();
        assert_eq!(config.alpn_protocols, vec![ALPN_HTTP11.to_vec()]);
    }

    #[test]
    fn builds_with_hostname_sni() {
        Tls::new(Some("example.org"), IpAddr::V4(Ipv4Addr::LOCALHOST)).expect("valid TLS client");
    }

    #[test]
    fn builds_with_ip_fallback() {
        Tls::new(None, IpAddr::V4(Ipv4Addr::LOCALHOST)).expect("valid TLS client");
    }

    /// Full HTTPS round-trip over the exact transport path the HTTP/1 engine
    /// uses: [`Tls::connect`] → [`TokioIo`] → hyper `http1` handshake → request.
    /// The server presents a throwaway self-signed cert; the client accepts it
    /// because verification is disabled.
    #[tokio::test]
    async fn https_round_trip() {
        use std::sync::Arc;

        use bytes::Bytes;
        use http::Request;
        use http_body_util::{BodyExt, Empty};
        use hyper::client::conn::http1;
        use rustls::{
            crypto::ring,
            pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
            ServerConfig,
        };
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::{TcpListener, TcpStream},
        };
        use tokio_rustls::TlsAcceptor;

        use crate::engine::http::io::TokioIo;

        // Self-signed server identity for 127.0.0.1 / localhost.
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("self-signed cert");
        let cert = certified.cert.der().clone();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der()));

        let server_config = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring provider supports the default TLS versions")
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .expect("valid server cert");
        let acceptor = TlsAcceptor::from(Arc::new(server_config));

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        // Minimal TLS + HTTP/1.1 origin server: read the request, reply `200 ok`.
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            let mut tls = acceptor.accept(tcp).await.expect("tls accept");
            let mut buf = [0u8; 1024];
            let _ = tls.read(&mut buf).await.expect("read request");
            tls.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                .await
                .expect("write response");
            tls.flush().await.expect("flush");
        });

        // Client: drive the engine's transport path by hand.
        let tls = Tls::new(None, addr.ip()).expect("tls client");
        let tcp = TcpStream::connect(addr).await.expect("connect");
        let stream = tls.connect(tcp).await.expect("tls handshake");

        let (mut sender, conn) = http1::handshake(TokioIo::new(stream)).await.expect("http handshake");
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let req = Request::builder()
            .uri("/")
            .header("host", "localhost")
            .body(Empty::<Bytes>::new())
            .expect("request");
        let mut resp = sender.send_request(req).await.expect("send request");
        assert_eq!(resp.status().as_u16(), 200);

        while let Some(frame) = resp.frame().await {
            frame.expect("body frame");
        }

        server.await.expect("server task");
    }
}
