//! Server half of the gRPC API seam.
//!
//! [`server`] implements the [`Dwd`](dwd_proto::dwd_server::Dwd) service over the
//! running core; [`snapshot`] maps the core statistics traits onto the wire
//! snapshot. The service is hosted in two ways: [`serve`] over an in-memory pipe
//! for the built-in TUI, and [`serve_tcp`] on a network address for remote
//! clients. The client half (TUI) lives in the `dwd` binary.

pub mod server;
pub mod snapshot;

pub use self::{
    server::DwdService,
    snapshot::{EngineDescriptor, SnapshotSource},
};

use core::net::SocketAddr;

use tokio::{io::DuplexStream, net::TcpListener, task::JoinHandle};
use tokio_stream::{wrappers::TcpListenerStream, StreamExt as _};
use tonic::transport::Server;

use dwd_proto::dwd_server::DwdServer;

/// Hosts the [`DwdService`] over a single in-memory connection.
///
/// The server exchanges HTTP/2 frames with the client over the given
/// [`DuplexStream`] — no OS socket is opened. The returned [`JoinHandle`] owns the
/// server task and should be aborted on shutdown.
pub fn serve(service: DwdService, server_io: DuplexStream) -> JoinHandle<Result<(), tonic::transport::Error>> {
    // Yield the one in-memory connection, then never end: an exhausted `incoming`
    // stream would let tonic tear the server down and drop our live connection.
    let incoming = tokio_stream::once(Ok::<_, std::io::Error>(server_io)).chain(tokio_stream::pending());

    tokio::spawn(async move {
        Server::builder()
            .add_service(DwdServer::new(service))
            .serve_with_incoming(incoming)
            .await
    })
}

/// Hosts the [`DwdService`] on a TCP address for remote control.
///
/// Binds eagerly so misconfiguration (port in use, bad address) fails startup
/// instead of surfacing later in a background task, and returns the bound
/// address (useful with port 0). Alongside the [`Dwd`](dwd_proto::dwd_server::Dwd)
/// service, gRPC reflection (v1 + v1alpha) is served so tools like `grpcurl`
/// can discover the API without proto files. The returned [`JoinHandle`] owns
/// the server task and should be aborted on shutdown.
pub async fn serve_tcp(
    service: DwdService,
    addr: SocketAddr,
) -> Result<(SocketAddr, JoinHandle<Result<(), tonic::transport::Error>>), anyhow::Error> {
    let listener = TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    log::info!("gRPC API server listening on {addr}");

    let reflection_v1 = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(dwd_proto::FILE_DESCRIPTOR_SET)
        .build_v1()?;
    let reflection_v1alpha = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(dwd_proto::FILE_DESCRIPTOR_SET)
        .build_v1alpha()?;

    let handle = tokio::spawn(async move {
        Server::builder()
            .add_service(reflection_v1)
            .add_service(reflection_v1alpha)
            .add_service(DwdServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
    });

    Ok((addr, handle))
}
