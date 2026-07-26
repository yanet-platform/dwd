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
///
/// Cloning is cheap (channels and `Arc`s), so the same service can back both
/// the in-memory TUI seam and the network endpoint at once.
#[derive(Clone)]
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
                Kind::Stop(_) => {
                    // Stop bypasses the generator channel: flipping the shared
                    // run flag drains the engine and the generator loop exactly
                    // as a TUI exit or SIGTERM would.
                    log::info!("stop requested via the API");
                    self.is_running.store(false, Ordering::SeqCst);

                    return Ok(Response::new(ControlResponse {}));
                }
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

#[cfg(test)]
mod tests {
    use dwd_proto::{SetControl, StatGroup, StopControl, SuspendControl};

    use super::*;
    use crate::grpc::snapshot::EngineDescriptor;

    struct EmptySource;

    impl SnapshotSource for EmptySource {
        fn snapshot(&self) -> StatsSnapshot {
            StatsSnapshot::default()
        }
    }

    fn service() -> (DwdService, mpsc::Receiver<GeneratorEvent>, Arc<AtomicBool>) {
        let (control_tx, control_rx) = mpsc::channel(1);
        let descriptor = EngineDescriptor {
            engine_kind: "udp",
            groups: vec![StatGroup::Common],
        }
        .into_response();
        let is_running = Arc::new(AtomicBool::new(true));
        let service = DwdService::new(control_tx, Arc::new(EmptySource), descriptor, is_running.clone());

        (service, control_rx, is_running)
    }

    #[tokio::test]
    async fn control_forwards_generator_events() {
        let (service, mut control_rx, is_running) = service();

        service
            .control(Request::new(ControlRequest {
                kind: Some(Kind::Set(SetControl { rps: 1234 })),
            }))
            .await
            .expect("set control");
        assert!(matches!(control_rx.try_recv(), Ok(GeneratorEvent::Set(1234))));

        service
            .control(Request::new(ControlRequest {
                kind: Some(Kind::Suspend(SuspendControl {})),
            }))
            .await
            .expect("suspend control");
        assert!(matches!(control_rx.try_recv(), Ok(GeneratorEvent::Suspend)));

        // Generator control never touches the run flag.
        assert!(is_running.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn control_stop_flips_run_flag() {
        let (service, mut control_rx, is_running) = service();

        service
            .control(Request::new(ControlRequest {
                kind: Some(Kind::Stop(StopControl {})),
            }))
            .await
            .expect("stop control");

        // Stop is not a generator event: it acts on the run flag directly.
        assert!(!is_running.load(Ordering::SeqCst));
        assert!(control_rx.try_recv().is_err());
    }
}
