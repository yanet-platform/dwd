//! Server half of the in-process gRPC API seam.
//!
//! [`server`] implements the [`Dwd`](dwd_proto::dwd_server::Dwd) service over the
//! running core; [`snapshot`] maps the core statistics traits onto the wire
//! snapshot. The client half (TUI) lives in the `dwd` binary.

pub mod server;
pub mod snapshot;

pub use self::{
    server::DwdService,
    snapshot::{EngineDescriptor, SnapshotSource},
};

use tokio::{io::DuplexStream, task::JoinHandle};
use tokio_stream::StreamExt as _;
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
