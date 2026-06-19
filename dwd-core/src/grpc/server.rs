//! Server side of the gRPC seam: the [`Dwd`] service backed by the running core.

use core::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use std::sync::Arc;

use dwd_proto::{
    control_request::Kind, dwd_server::Dwd, ControlRequest, ControlResponse, DescribeRequest, DescribeResponse,
    StatsSnapshot, StreamStatsRequest,
};
use tokio::sync::mpsc::{self, Sender};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::{grpc::snapshot::SnapshotSource, GeneratorEvent};

/// Cadence at which statistic snapshots are pushed to subscribers. Matches the
/// TUI's ~40 FPS frame interval so the rate/throughput math is unchanged.
const STATS_INTERVAL: Duration = Duration::from_millis(25);

/// Capacity of the per-subscriber snapshot channel. A tiny buffer keeps the
/// producer slightly ahead of the consumer without unbounded growth.
const STATS_CHANNEL_CAP: usize = 4;

/// The [`Dwd`] service implementation over the in-process core.
pub struct DwdService {
    /// Forwards control RPCs to `run_generator` (mirrors the old UI mpsc).
    control_tx: Sender<GeneratorEvent>,
    /// Produces statistic snapshots for the stream.
    snapshot_source: Arc<dyn SnapshotSource>,
    /// Cached describe response (engine kind + widget groups).
    descriptor: DescribeResponse,
    /// Shared run flag; the stats stream ends when it flips to `false`.
    is_running: Arc<AtomicBool>,
}

impl DwdService {
    /// Creates a new service over the given core handles.
    pub fn new(
        control_tx: Sender<GeneratorEvent>,
        snapshot_source: Arc<dyn SnapshotSource>,
        descriptor: DescribeResponse,
        is_running: Arc<AtomicBool>,
    ) -> Self {
        Self {
            control_tx,
            snapshot_source,
            descriptor,
            is_running,
        }
    }
}

#[tonic::async_trait]
impl Dwd for DwdService {
    async fn describe(&self, _request: Request<DescribeRequest>) -> Result<Response<DescribeResponse>, Status> {
        Ok(Response::new(self.descriptor.clone()))
    }

    async fn control(&self, request: Request<ControlRequest>) -> Result<Response<ControlResponse>, Status> {
        if let Some(kind) = request.into_inner().kind {
            let event = match kind {
                Kind::Suspend(_) => GeneratorEvent::Suspend,
                Kind::Resume(_) => GeneratorEvent::Resume,
                Kind::Set(set) => GeneratorEvent::Set(set.rps),
            };

            // Non-blocking, coalescing send — mirrors the TUI's `try_send`: if the
            // generator is momentarily behind, dropping a redundant control event
            // is harmless (the next one supersedes it).
            let _ = self.control_tx.try_send(event);
        }

        Ok(Response::new(ControlResponse {}))
    }

    type StreamStatsStream = ReceiverStream<Result<StatsSnapshot, Status>>;

    async fn stream_stats(
        &self,
        _request: Request<StreamStatsRequest>,
    ) -> Result<Response<Self::StreamStatsStream>, Status> {
        let (tx, rx) = mpsc::channel(STATS_CHANNEL_CAP);
        let source = self.snapshot_source.clone();
        let is_running = self.is_running.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(STATS_INTERVAL);
            loop {
                interval.tick().await;
                if !is_running.load(Ordering::SeqCst) {
                    break;
                }
                if tx.send(Ok(source.snapshot())).await.is_err() {
                    // Subscriber dropped the stream.
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
