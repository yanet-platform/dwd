//! Client side of the gRPC seam: the TUI's view of the core.
//!
//! The render loop stays a plain synchronous thread. It reads statistics from a
//! [`RemoteStat`] (an atomically-swapped snapshot mirrored behind the existing
//! stat traits, so every widget is reused verbatim) and writes control intents
//! into an mpsc the [driver](spawn_control_forwarder) turns into unary RPCs. Two
//! driver tasks run on the main runtime and are the only things that touch gRPC.

use core::future::ready;
use std::sync::Arc;

use arc_swap::ArcSwap;
use dwd_core::{
    histogram::LogHistogram,
    stat::{BurstTxStat, CommonStat, HttpStat, RxStat, SocketStat, TxStat},
    GeneratorEvent,
};
use dwd_proto::{
    control_request::Kind, dwd_client::DwdClient, ControlRequest, ResumeControl, SetControl, StatGroup, StatsSnapshot,
    StreamStatsRequest, SuspendControl,
};
use hyper_util::rt::TokioIo;
use tokio::{
    io::DuplexStream,
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::ui::Ui;

/// A client-side mirror of the latest statistics snapshot.
///
/// Implements all six stat traits by reading the most recent [`StatsSnapshot`],
/// returning zero / empty values for groups the engine does not provide. Because
/// the TUI widgets are generic over `Arc<S: Trait>`, passing `Arc<RemoteStat>`
/// reuses every existing widget unchanged.
#[derive(Debug)]
pub struct RemoteStat {
    snapshot: ArcSwap<StatsSnapshot>,
}

impl RemoteStat {
    /// Creates a stat seeded with an empty snapshot.
    pub fn new() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(StatsSnapshot::default()),
        }
    }

    /// Publishes a freshly received snapshot for the render loop to read.
    pub fn store(&self, snapshot: StatsSnapshot) {
        self.snapshot.store(Arc::new(snapshot));
    }
}

impl Default for RemoteStat {
    fn default() -> Self {
        Self::new()
    }
}

impl CommonStat for RemoteStat {
    fn generator(&self) -> u64 {
        self.snapshot.load().generator_rps
    }
}

impl TxStat for RemoteStat {
    fn num_requests(&self) -> u64 {
        self.snapshot.load().tx.as_ref().map_or(0, |tx| tx.num_requests)
    }

    fn bytes_tx(&self) -> u64 {
        self.snapshot.load().tx.as_ref().map_or(0, |tx| tx.bytes_tx)
    }
}

impl RxStat for RemoteStat {
    fn num_responses(&self) -> u64 {
        self.snapshot.load().rx.as_ref().map_or(0, |rx| rx.num_responses)
    }

    fn num_timeouts(&self) -> u64 {
        self.snapshot.load().rx.as_ref().map_or(0, |rx| rx.num_timeouts)
    }

    fn bytes_rx(&self) -> u64 {
        self.snapshot.load().rx.as_ref().map_or(0, |rx| rx.bytes_rx)
    }

    fn hist(&self) -> LogHistogram {
        let snapshot = self.snapshot.load();
        let buckets = snapshot
            .rx
            .as_ref()
            .map(|rx| rx.histogram_buckets.clone())
            .unwrap_or_default();

        LogHistogram::new(buckets)
    }
}

impl SocketStat for RemoteStat {
    fn num_sock_created(&self) -> u64 {
        self.snapshot.load().socket.as_ref().map_or(0, |s| s.num_sock_created)
    }

    fn num_sock_errors(&self) -> u64 {
        self.snapshot.load().socket.as_ref().map_or(0, |s| s.num_sock_errors)
    }

    fn num_retransmits(&self) -> u64 {
        self.snapshot.load().socket.as_ref().map_or(0, |s| s.num_retransmits)
    }
}

impl HttpStat for RemoteStat {
    fn num_2xx(&self) -> u64 {
        self.snapshot.load().http.as_ref().map_or(0, |h| h.num_2xx)
    }

    fn num_3xx(&self) -> u64 {
        self.snapshot.load().http.as_ref().map_or(0, |h| h.num_3xx)
    }

    fn num_4xx(&self) -> u64 {
        self.snapshot.load().http.as_ref().map_or(0, |h| h.num_4xx)
    }

    fn num_5xx(&self) -> u64 {
        self.snapshot.load().http.as_ref().map_or(0, |h| h.num_5xx)
    }
}

impl BurstTxStat for RemoteStat {
    fn num_bursts_tx(&self, idx: usize) -> u64 {
        self.snapshot
            .load()
            .burst_tx
            .as_ref()
            .and_then(|b| b.num_bursts_tx.get(idx))
            .copied()
            .unwrap_or(0)
    }
}

/// Connects a gRPC client over the client half of an in-memory pipe.
///
/// The server half is hosted by [`dwd_core::grpc::serve`]; no OS socket is opened.
pub fn connect(client_io: DuplexStream) -> Result<DwdClient<Channel>, anyhow::Error> {
    // A single connection: the connector hands over the client half exactly once.
    let mut client_io = Some(TokioIo::new(client_io));
    let connector = service_fn(move |_: Uri| {
        let io = client_io.take().expect("in-process connector invoked once");
        ready(Ok::<_, std::io::Error>(io))
    });

    // `http://` (not `https://`) avoids tonic's TLS requirement; the authority is
    // irrelevant because the connector ignores the URI.
    let channel = Endpoint::try_from("http://dwd.invalid")?.connect_with_connector_lazy(connector);

    Ok(DwdClient::new(channel))
}

/// Drives the statistics stream, publishing each snapshot into `remote`.
pub fn spawn_stats_consumer(mut client: DwdClient<Channel>, remote: Arc<RemoteStat>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut stream = match client.stream_stats(StreamStatsRequest {}).await {
            Ok(response) => response.into_inner(),
            Err(status) => {
                log::error!("failed to subscribe to statistics: {status}");
                return;
            }
        };

        loop {
            match stream.message().await {
                Ok(Some(snapshot)) => remote.store(snapshot),
                Ok(None) => break,
                Err(status) => {
                    log::debug!("statistics stream ended: {status}");
                    break;
                }
            }
        }
    })
}

/// Forwards control intents from the TUI as unary `Control` RPCs.
pub fn spawn_control_forwarder(mut client: DwdClient<Channel>, mut rx: Receiver<GeneratorEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Err(status) = client.control(into_control_request(event)).await {
                log::error!("control request failed: {status}");
            }
        }
    })
}

/// Maps a UI control intent onto its wire request.
fn into_control_request(event: GeneratorEvent) -> ControlRequest {
    let kind = match event {
        GeneratorEvent::Suspend => Kind::Suspend(SuspendControl {}),
        GeneratorEvent::Resume => Kind::Resume(ResumeControl {}),
        GeneratorEvent::Set(rps) => Kind::Set(SetControl { rps }),
    };

    ControlRequest { kind: Some(kind) }
}

/// Builds the TUI from the engine's described stat groups, wiring every widget
/// against the shared [`RemoteStat`]. The group order mirrors the old per-engine
/// `Ui::with_*` selection, preserving the exact layout.
pub fn build_ui(tx: Sender<GeneratorEvent>, groups: &[i32], stat: Arc<RemoteStat>) -> Ui {
    let mut ui = Ui::new(tx);

    for &group in groups {
        let Ok(group) = StatGroup::try_from(group) else {
            continue;
        };

        ui = match group {
            StatGroup::Common => ui.with_common(stat.clone()),
            StatGroup::Tx => ui.with_tx(stat.clone()),
            StatGroup::Rx => ui.with_rx(stat.clone()),
            StatGroup::RxTimings => ui.with_rx_timings(stat.clone()),
            StatGroup::Socket => ui.with_sock(stat.clone()),
            StatGroup::Http => ui.with_http(stat.clone()),
            StatGroup::BurstTx => ui.with_burst_tx(stat.clone()),
            StatGroup::Unspecified => ui,
        };
    }

    ui
}

#[cfg(test)]
mod tests {
    use dwd_proto::{HttpStats, RxStats, SocketStats, TxStats};

    use super::*;

    fn sample_snapshot() -> StatsSnapshot {
        StatsSnapshot {
            generator_rps: 1000,
            tx: Some(TxStats { num_requests: 42, bytes_tx: 4096 }),
            rx: Some(RxStats {
                num_responses: 40,
                num_timeouts: 2,
                bytes_rx: 8192,
                histogram_buckets: vec![0, 5, 10, 3],
            }),
            socket: Some(SocketStats {
                num_sock_created: 7,
                num_sock_errors: 1,
                num_retransmits: 3,
            }),
            http: Some(HttpStats {
                num_2xx: 30,
                num_3xx: 4,
                num_4xx: 5,
                num_5xx: 1,
            }),
            burst_tx: None,
        }
    }

    #[test]
    fn remote_stat_mirrors_snapshot() {
        let stat = RemoteStat::new();
        stat.store(sample_snapshot());

        assert_eq!(stat.generator(), 1000);
        assert_eq!(stat.num_requests(), 42);
        assert_eq!(stat.bytes_tx(), 4096);
        assert_eq!(stat.num_responses(), 40);
        assert_eq!(stat.num_timeouts(), 2);
        assert_eq!(stat.bytes_rx(), 8192);
        assert_eq!(stat.num_sock_created(), 7);
        assert_eq!(stat.num_sock_errors(), 1);
        assert_eq!(stat.num_retransmits(), 3);
        assert_eq!(stat.num_2xx(), 30);
        assert_eq!(stat.num_5xx(), 1);
    }

    #[test]
    fn remote_stat_reconstructs_histogram() {
        let stat = RemoteStat::new();
        stat.store(sample_snapshot());

        // The reconstructed histogram must match a direct one built from the same
        // buckets, so client-side quantiles render identically.
        let direct = LogHistogram::new(vec![0, 5, 10, 3]);
        for q in [0.5, 0.9, 0.99, 1.0] {
            assert_eq!(stat.hist().quantile(q), direct.quantile(q));
        }
    }

    #[test]
    fn remote_stat_defaults_to_zero_for_absent_groups() {
        let stat = RemoteStat::new();

        assert_eq!(stat.generator(), 0);
        assert_eq!(stat.num_requests(), 0);
        assert_eq!(stat.num_responses(), 0);
        assert_eq!(stat.num_bursts_tx(0), 0);
        assert!(stat.hist().snapshot().is_empty());
    }

    /// Exercises the whole in-process seam — describe, control and a streamed
    /// snapshot — over the real duplex transport on a current-thread runtime,
    /// mirroring how the binary drives it. Validates that a unary call and a
    /// long-lived server stream multiplex over the single in-memory connection.
    #[tokio::test]
    async fn in_process_seam_roundtrip() {
        use core::sync::atomic::{AtomicBool, Ordering};

        use dwd_core::grpc::{DwdService, EngineDescriptor, SnapshotSource};
        use dwd_proto::{control_request::Kind, ControlRequest, DescribeRequest, SetControl};

        struct FixedSource;
        impl SnapshotSource for FixedSource {
            fn snapshot(&self) -> StatsSnapshot {
                StatsSnapshot {
                    generator_rps: 777,
                    ..Default::default()
                }
            }
        }

        let (control_tx, mut control_rx) = tokio::sync::mpsc::channel::<GeneratorEvent>(1);
        let descriptor = EngineDescriptor {
            engine_kind: "udp",
            groups: vec![StatGroup::Common, StatGroup::Tx, StatGroup::Socket],
        }
        .into_response();
        let is_running = Arc::new(AtomicBool::new(true));
        let service = DwdService::new(control_tx, Arc::new(FixedSource), descriptor, is_running.clone());

        // Host the server (core) and connect the client (bin) over one in-memory pipe.
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let server = dwd_core::grpc::serve(service, server_io);
        let mut client = connect(client_io).expect("connect");

        // Describe: the client learns the engine kind and widget groups.
        let described = client
            .describe(DescribeRequest {})
            .await
            .expect("describe")
            .into_inner();
        assert_eq!(described.engine_kind, "udp");
        assert_eq!(
            described.groups,
            vec![StatGroup::Common as i32, StatGroup::Tx as i32, StatGroup::Socket as i32],
        );

        // Control: a unary RPC is forwarded to the generator's control channel.
        client
            .control(ControlRequest {
                kind: Some(Kind::Set(SetControl { rps: 500 })),
            })
            .await
            .expect("control");
        let event = control_rx.recv().await.expect("control event delivered");
        assert!(matches!(event, GeneratorEvent::Set(500)));

        // Stream: the first snapshot mirrors the source (multiplexed alongside the
        // unary call on the same connection).
        let mut stream = client
            .stream_stats(StreamStatsRequest {})
            .await
            .expect("stream")
            .into_inner();
        let snapshot = stream.message().await.expect("stream item").expect("snapshot present");
        assert_eq!(snapshot.generator_rps, 777);

        is_running.store(false, Ordering::SeqCst);
        server.abort();
    }
}
