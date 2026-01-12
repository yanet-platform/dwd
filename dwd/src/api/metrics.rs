//! Prometheus metrics collector and HTTP handler.

use std::{fmt::Write, sync::Arc};

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Router};
use prometheus_client::{
    encoding::text::encode,
    metrics::{counter::Counter, gauge::Gauge},
    registry::Registry,
};

use crate::{
    histogram::LogHistogram,
    stat::{CommonStat, HttpStat, RxStat, SocketStat, TxStat},
};

/// Prometheus histogram buckets for latency (in seconds).
/// Range: 5us to 10s with logarithmic distribution.
const LATENCY_BUCKETS: [f64; 20] = [
    0.000_005, // 5us
    0.000_010, // 10us
    0.000_025, // 25us
    0.000_050, // 50us
    0.000_100, // 100us
    0.000_250, // 250us
    0.000_500, // 500us
    0.001,     // 1ms
    0.002_5,   // 2.5ms
    0.005,     // 5ms
    0.010,     // 10ms
    0.025,     // 25ms
    0.050,     // 50ms
    0.100,     // 100ms
    0.250,     // 250ms
    0.500,     // 500ms
    1.0,       // 1s
    2.5,       // 2.5s
    5.0,       // 5s
    10.0,      // 10s
];

/// Collector that gathers metrics from stat sources and exports them to
/// Prometheus.
pub struct MetricsCollector {
    registry: Registry,
    generator_rps: Gauge,
    requests_total: Counter,
    responses_total: Counter,
    timeouts_total: Counter,
    bytes_tx_total: Counter,
    bytes_rx_total: Counter,
    http_2xx_total: Counter,
    http_3xx_total: Counter,
    http_4xx_total: Counter,
    http_5xx_total: Counter,
    sockets_created_total: Counter,
    socket_errors_total: Counter,
    retransmits_total: Counter,
}

impl MetricsCollector {
    /// Creates a new metrics collector and registers all metrics.
    pub fn new() -> Self {
        let mut registry = Registry::default();

        let generator_rps = Gauge::default();
        registry.register(
            "dwd_generator_rps",
            "Target RPS from the generator",
            generator_rps.clone(),
        );

        let requests_total = Counter::default();
        registry.register(
            "dwd_requests_total",
            "Total number of requests sent",
            requests_total.clone(),
        );

        let responses_total = Counter::default();
        registry.register(
            "dwd_responses_total",
            "Total number of responses received",
            responses_total.clone(),
        );

        let timeouts_total = Counter::default();
        registry.register(
            "dwd_timeouts_total",
            "Total number of request timeouts",
            timeouts_total.clone(),
        );

        let bytes_tx_total = Counter::default();
        registry.register("dwd_bytes_tx_total", "Total bytes transmitted", bytes_tx_total.clone());

        let bytes_rx_total = Counter::default();
        registry.register("dwd_bytes_rx_total", "Total bytes received", bytes_rx_total.clone());

        let http_2xx_total = Counter::default();
        registry.register("dwd_http_2xx_total", "Total HTTP 2xx responses", http_2xx_total.clone());

        let http_3xx_total = Counter::default();
        registry.register("dwd_http_3xx_total", "Total HTTP 3xx responses", http_3xx_total.clone());

        let http_4xx_total = Counter::default();
        registry.register("dwd_http_4xx_total", "Total HTTP 4xx responses", http_4xx_total.clone());

        let http_5xx_total = Counter::default();
        registry.register("dwd_http_5xx_total", "Total HTTP 5xx responses", http_5xx_total.clone());

        let sockets_created_total = Counter::default();
        registry.register(
            "dwd_sockets_created_total",
            "Total number of sockets created",
            sockets_created_total.clone(),
        );

        let socket_errors_total = Counter::default();
        registry.register(
            "dwd_socket_errors_total",
            "Total number of socket errors",
            socket_errors_total.clone(),
        );

        let retransmits_total = Counter::default();
        registry.register(
            "dwd_retransmits_total",
            "Total number of TCP retransmits",
            retransmits_total.clone(),
        );

        Self {
            registry,
            generator_rps,
            requests_total,
            responses_total,
            timeouts_total,
            bytes_tx_total,
            bytes_rx_total,
            http_2xx_total,
            http_3xx_total,
            http_4xx_total,
            http_5xx_total,
            sockets_created_total,
            socket_errors_total,
            retransmits_total,
        }
    }

    /// Updates common stats (generator RPS).
    pub fn update_common<S: CommonStat>(&self, stat: &S) {
        self.generator_rps.set(stat.generator() as i64);
    }

    /// Updates TX stats.
    pub fn update_tx<S: TxStat>(&self, stat: &S) {
        let requests = stat.num_requests();
        let bytes_tx = stat.bytes_tx();

        // Calculate delta from current counter value.
        let current_requests = self.requests_total.get();
        if requests > current_requests {
            self.requests_total.inc_by(requests - current_requests);
        }

        let current_bytes_tx = self.bytes_tx_total.get();
        if bytes_tx > current_bytes_tx {
            self.bytes_tx_total.inc_by(bytes_tx - current_bytes_tx);
        }
    }

    /// Updates RX stats.
    pub fn update_rx<S: RxStat>(&self, stat: &S) {
        let responses = stat.num_responses();
        let timeouts = stat.num_timeouts();
        let bytes_rx = stat.bytes_rx();

        let current_responses = self.responses_total.get();
        if responses > current_responses {
            self.responses_total.inc_by(responses - current_responses);
        }

        let current_timeouts = self.timeouts_total.get();
        if timeouts > current_timeouts {
            self.timeouts_total.inc_by(timeouts - current_timeouts);
        }

        let current_bytes_rx = self.bytes_rx_total.get();
        if bytes_rx > current_bytes_rx {
            self.bytes_rx_total.inc_by(bytes_rx - current_bytes_rx);
        }
    }

    /// Updates HTTP stats.
    pub fn update_http<S: HttpStat>(&self, stat: &S) {
        let num_2xx = stat.num_2xx();
        let num_3xx = stat.num_3xx();
        let num_4xx = stat.num_4xx();
        let num_5xx = stat.num_5xx();

        let current_2xx = self.http_2xx_total.get();
        if num_2xx > current_2xx {
            self.http_2xx_total.inc_by(num_2xx - current_2xx);
        }

        let current_3xx = self.http_3xx_total.get();
        if num_3xx > current_3xx {
            self.http_3xx_total.inc_by(num_3xx - current_3xx);
        }

        let current_4xx = self.http_4xx_total.get();
        if num_4xx > current_4xx {
            self.http_4xx_total.inc_by(num_4xx - current_4xx);
        }

        let current_5xx = self.http_5xx_total.get();
        if num_5xx > current_5xx {
            self.http_5xx_total.inc_by(num_5xx - current_5xx);
        }
    }

    /// Updates socket stats.
    pub fn update_socket<S: SocketStat>(&self, stat: &S) {
        let created = stat.num_sock_created();
        let errors = stat.num_sock_errors();
        let retransmits = stat.num_retransmits();

        let current_created = self.sockets_created_total.get();
        if created > current_created {
            self.sockets_created_total.inc_by(created - current_created);
        }

        let current_errors = self.socket_errors_total.get();
        if errors > current_errors {
            self.socket_errors_total.inc_by(errors - current_errors);
        }

        let current_retransmits = self.retransmits_total.get();
        if retransmits > current_retransmits {
            self.retransmits_total.inc_by(retransmits - current_retransmits);
        }
    }

    /// Encodes all metrics to Prometheus text format.
    pub fn encode(&self) -> String {
        let mut buffer = String::new();
        encode(&mut buffer, &self.registry).expect("encoding should not fail");
        buffer
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Encodes a LogHistogram to Prometheus histogram format.
///
/// The histogram is encoded manually because prometheus-client's Histogram
/// uses observe() which accumulates values, but we need to export absolute
/// cumulative bucket counts from the log-histogram.
fn encode_histogram(hist: &LogHistogram) -> String {
    let snapshot = hist.snapshot();
    let factor = LogHistogram::factor();

    // Calculate cumulative counts for prometheus buckets.
    // For each prometheus bucket with upper bound B (in seconds),
    // we sum all log-bucket counts where the upper bound <= B.
    let mut bucket_counts = vec![0u64; LATENCY_BUCKETS.len()];
    let mut total_count = 0u64;
    let mut total_sum = 0.0f64;

    for (idx, &count) in snapshot.iter().enumerate() {
        if count == 0 {
            continue;
        }

        total_count += count;

        // Upper bound of this log-bucket in microseconds.
        let upper_us = factor.powi(idx as i32);
        // Lower bound for sum calculation.
        let lower_us = if idx == 0 { 0.0 } else { factor.powi(idx as i32 - 1) };
        // Midpoint in seconds for sum calculation.
        let midpoint_sec = (lower_us + upper_us) / 2.0 / 1_000_000.0;
        total_sum += midpoint_sec * count as f64;

        // Upper bound in seconds.
        let upper_sec = upper_us / 1_000_000.0;

        // Add count to all prometheus buckets whose upper bound >= upper_sec.
        for (bucket_idx, &bucket_bound) in LATENCY_BUCKETS.iter().enumerate() {
            if bucket_bound >= upper_sec {
                bucket_counts[bucket_idx] += count;
            }
        }
    }

    let mut output = String::new();
    writeln!(
        output,
        "# HELP dwd_latency_seconds Response latency histogram in seconds"
    )
    .unwrap();
    writeln!(output, "# TYPE dwd_latency_seconds histogram").unwrap();

    // Buckets must be cumulative.
    let mut cumulative = 0u64;
    for (idx, &bound) in LATENCY_BUCKETS.iter().enumerate() {
        cumulative += bucket_counts[idx];
        writeln!(
            output,
            "dwd_latency_seconds_bucket{{le=\"{:.6}\"}} {}",
            bound, cumulative
        )
        .unwrap();
    }
    writeln!(output, "dwd_latency_seconds_bucket{{le=\"+Inf\"}} {}", total_count).unwrap();
    writeln!(output, "dwd_latency_seconds_sum {:.6}", total_sum).unwrap();
    writeln!(output, "dwd_latency_seconds_count {}", total_count).unwrap();

    output
}

/// Trait for stat sources that can be collected.
pub trait StatSource: Send + Sync {
    /// Updates the metrics collector with current stats.
    fn collect(&self, collector: &MetricsCollector);

    /// Returns the latency histogram if available.
    fn histogram(&self) -> Option<LogHistogram> {
        None
    }
}

/// Shared state for the metrics handler.
pub struct MetricsState {
    collector: MetricsCollector,
    stat_source: Arc<dyn StatSource>,
}

impl MetricsState {
    /// Creates a new metrics state.
    pub fn new(stat_source: Arc<dyn StatSource>) -> Self {
        Self {
            collector: MetricsCollector::new(),
            stat_source,
        }
    }
}

/// Creates a router for metrics endpoints.
pub fn router(state: Arc<MetricsState>) -> Router {
    Router::new()
        .route("/api/v1/metrics", get(metrics_handler))
        .with_state(state)
}

async fn metrics_handler(State(state): State<Arc<MetricsState>>) -> impl IntoResponse {
    // Update metrics from stat source.
    state.stat_source.collect(&state.collector);

    // Encode to prometheus format.
    let mut body = state.collector.encode();

    // Add histogram if available (encoded separately due to its special nature).
    if let Some(hist) = state.stat_source.histogram() {
        body.push_str(&encode_histogram(&hist));
    }

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}
