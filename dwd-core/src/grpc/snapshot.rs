//! Mapping between the core statistics traits and the gRPC wire snapshot.
//!
//! The TUI never sees the live per-CPU [`Stat`] anymore; instead the server turns
//! it into a [`StatsSnapshot`] of absolute cumulative counters on each tick. Every
//! engine exposes exactly the subset of the six stat traits it supports, so we
//! mirror the per-combination [`StatSource`](crate::api::StatSource) impls and fill
//! only the matching sub-messages.

use dwd_proto::{BurstTxStats, DescribeResponse, HttpStats, RxStats, SocketStats, StatGroup, StatsSnapshot, TxStats};

use crate::stat::{
    BurstTxStat, BurstTxWorkerStat, CommonStat, HttpStat, HttpWorkerStat, RxStat, RxWorkerStat, SockWorkerStat,
    SocketStat, Stat, TxStat, TxWorkerStat,
};

/// Number of per-burst-size counters tracked by the DPDK engine.
const BURST_TX_BUCKETS: usize = 32;

/// A source that can produce a [`StatsSnapshot`] of its current values.
///
/// Object-safe so the server can hold an `Arc<dyn SnapshotSource>` regardless of
/// which engine produced it — exactly like [`StatSource`](crate::api::StatSource).
pub trait SnapshotSource: Send + Sync {
    /// Builds a snapshot of the current cumulative statistics.
    fn snapshot(&self) -> StatsSnapshot;
}

/// Describes a running engine for the [`Describe`](dwd_proto::dwd_server::Dwd::describe)
/// RPC: its kind and the ordered set of stat groups (TUI widgets) to render.
#[derive(Debug, Clone)]
pub struct EngineDescriptor {
    /// Human-readable engine kind, e.g. `"udp"`.
    pub engine_kind: &'static str,
    /// Stat groups in display order; mirrors the old `Ui::with_*` selection.
    pub groups: Vec<StatGroup>,
}

impl EngineDescriptor {
    /// Converts this descriptor into its wire representation.
    pub fn into_response(self) -> DescribeResponse {
        DescribeResponse {
            engine_kind: self.engine_kind.to_owned(),
            groups: self.groups.into_iter().map(|g| g as i32).collect(),
        }
    }
}

// Routed through the `CommonStat` bound because `Stat` also has an inherent
// `generator()` returning a `SharedGenerator` that would otherwise shadow it.
fn common_rps(s: &impl CommonStat) -> u64 {
    s.generator()
}

fn tx_stats(s: &impl TxStat) -> TxStats {
    TxStats {
        num_requests: s.num_requests(),
        bytes_tx: s.bytes_tx(),
    }
}

fn rx_stats(s: &impl RxStat) -> RxStats {
    RxStats {
        num_responses: s.num_responses(),
        num_timeouts: s.num_timeouts(),
        bytes_rx: s.bytes_rx(),
        histogram_buckets: s.hist().snapshot().to_vec(),
    }
}

fn socket_stats(s: &impl SocketStat) -> SocketStats {
    SocketStats {
        num_sock_created: s.num_sock_created(),
        num_sock_errors: s.num_sock_errors(),
        num_retransmits: s.num_retransmits(),
    }
}

fn http_stats(s: &impl HttpStat) -> HttpStats {
    HttpStats {
        num_2xx: s.num_2xx(),
        num_3xx: s.num_3xx(),
        num_4xx: s.num_4xx(),
        num_5xx: s.num_5xx(),
    }
}

fn burst_tx_stats(s: &impl BurstTxStat) -> BurstTxStats {
    BurstTxStats {
        num_bursts_tx: (0..BURST_TX_BUCKETS).map(|idx| s.num_bursts_tx(idx)).collect(),
    }
}

/// HTTP / HTTP-raw engine stat: TX, RX, Socket and HTTP with histogram.
impl<B> SnapshotSource for Stat<TxWorkerStat, RxWorkerStat, SockWorkerStat, HttpWorkerStat, B>
where
    B: Send + Sync,
{
    fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            generator_rps: common_rps(self),
            tx: Some(tx_stats(self)),
            rx: Some(rx_stats(self)),
            socket: Some(socket_stats(self)),
            http: Some(http_stats(self)),
            burst_tx: None,
        }
    }
}

/// UDP engine stat: TX and Socket, no RX/HTTP/histogram.
impl<H, B> SnapshotSource for Stat<TxWorkerStat, (), SockWorkerStat, H, B>
where
    H: Send + Sync,
    B: Send + Sync,
{
    fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            generator_rps: common_rps(self),
            tx: Some(tx_stats(self)),
            socket: Some(socket_stats(self)),
            ..Default::default()
        }
    }
}

/// DPDK engine stat: TX with the burst-size histogram only.
impl SnapshotSource for Stat<TxWorkerStat, (), (), (), BurstTxWorkerStat> {
    fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            generator_rps: common_rps(self),
            tx: Some(tx_stats(self)),
            burst_tx: Some(burst_tx_stats(self)),
            ..Default::default()
        }
    }
}
