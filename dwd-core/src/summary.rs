//! End-of-run summary statistics.
//!
//! When the shooting finishes DWD prints a compact report to the logs: totals,
//! achieved vs. target rate, throughput, latency quantiles and — most
//! importantly for unattended runs — anomaly indicators (timeouts, socket
//! errors, HTTP 4xx/5xx, retransmits, unanswered requests, an underachieved
//! target) with an explicit final verdict.
//!
//! A [`Recorder`] runs on a dedicated OS thread for the whole shooting,
//! sampling the engine's cumulative counters once per second: totals alone
//! yield only the average rate, while the per-second deltas give the
//! median/min/max of the *achieved* RPS. The thread watches `is_running` and
//! freezes the load-window duration the moment the shooting stops, so time
//! spent afterwards in a still-open TUI does not skew the averages. The final
//! counters are read later, in [`Recorder::finish`], once the engine has fully
//! drained.
//!
//! Reading the per-CPU counters from this extra thread follows the same
//! pattern the Prometheus endpoint already uses: rare, read-only, off the hot
//! path.

use core::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use std::{
    io,
    sync::Arc,
    thread::{self, Builder, JoinHandle},
    time::Instant,
};

use dwd_proto::StatsSnapshot;

use crate::{grpc::snapshot::SnapshotSource, histogram::LogHistogram};

/// How often the achieved rate is sampled.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// How often the sampler polls `is_running` between samples.
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Achieved/target ratio below which the run is flagged as underachieved.
const UNDERACHIEVED_THRESHOLD: f64 = 0.95;
/// Share of requests left unanswered above which the run is flagged.
///
/// A handful of requests in flight at shutdown is normal; a large share points
/// at a stuck target or lost responses.
const UNANSWERED_THRESHOLD_PCT: f64 = 1.0;

/// Samples the engine's counters in the background for the end-of-run summary.
pub struct Recorder {
    thread: JoinHandle<Sampler>,
    source: Arc<dyn SnapshotSource>,
    engine_kind: &'static str,
}

impl Recorder {
    /// Spawns the sampling thread; call right before the engine starts.
    pub fn spawn(
        engine_kind: &'static str,
        source: Arc<dyn SnapshotSource>,
        is_running: Arc<AtomicBool>,
    ) -> Result<Self, io::Error> {
        let sampler = Sampler::new(source.clone());
        let thread = Builder::new()
            .name("summary".into())
            .spawn(move || sampler.run(&is_running))?;

        Ok(Self { thread, source, engine_kind })
    }

    /// Joins the sampling thread and folds its samples plus a final snapshot
    /// into a [`RunSummary`]. Call after the engine has stopped.
    pub fn finish(self) -> RunSummary {
        let sampler = self.thread.join().expect("summary sampler never panics");
        let snapshot = self.source.snapshot();

        RunSummary {
            engine_kind: self.engine_kind,
            duration: sampler.stopped.duration_since(sampler.started),
            rates: sampler.rates,
            targets: sampler.targets,
            snapshot,
        }
    }
}

/// The sampling loop state; lives entirely on the `summary` thread.
struct Sampler {
    source: Arc<dyn SnapshotSource>,
    started: Instant,
    stopped: Instant,
    prev_at: Instant,
    prev_requests: u64,
    /// Achieved rate per sampling interval, requests (packets) per second.
    rates: Vec<f64>,
    /// Target RPS as reported by the generator at each sample.
    targets: Vec<u64>,
}

impl Sampler {
    fn new(source: Arc<dyn SnapshotSource>) -> Self {
        let now = Instant::now();

        Self {
            source,
            started: now,
            stopped: now,
            prev_at: now,
            prev_requests: 0,
            rates: Vec::new(),
            targets: Vec::new(),
        }
    }

    fn run(mut self, is_running: &AtomicBool) -> Self {
        let mut next_sample = self.started + SAMPLE_INTERVAL;

        while is_running.load(Ordering::SeqCst) {
            thread::sleep(SHUTDOWN_POLL_INTERVAL);

            let now = Instant::now();
            if now >= next_sample {
                self.sample(now);
                next_sample = now + SAMPLE_INTERVAL;
            }
        }
        self.stopped = Instant::now();

        self
    }

    fn sample(&mut self, now: Instant) {
        let snapshot = self.source.snapshot();
        let requests = snapshot.tx.as_ref().map_or(0, |tx| tx.num_requests);

        let elapsed = now.duration_since(self.prev_at).as_secs_f64();
        if elapsed > 0.0 {
            self.rates
                .push(requests.saturating_sub(self.prev_requests) as f64 / elapsed);
        }
        self.targets.push(snapshot.generator_rps);

        self.prev_at = now;
        self.prev_requests = requests;
    }
}

/// The folded result of a finished shooting, ready to be logged.
#[derive(Debug)]
pub struct RunSummary {
    engine_kind: &'static str,
    /// Load-window duration: engine start to `is_running` flipping false.
    duration: Duration,
    rates: Vec<f64>,
    targets: Vec<u64>,
    snapshot: StatsSnapshot,
}

impl RunSummary {
    /// Prints the report to the logs: `info` for the figures, `warn` for every
    /// detected anomaly and the non-clean verdict.
    pub fn log(&self) {
        let secs = self.duration.as_secs_f64().max(f64::EPSILON);
        let (noun, unit) = self.wording();
        let requests = self.requests();

        log::info!(
            "shooting finished: engine={}, duration={}",
            self.engine_kind,
            fmt_duration(self.duration)
        );

        if let Some(tx) = &self.snapshot.tx {
            let avg = (tx.num_requests as f64 / secs).round() as u64;
            match distribution(&self.rates) {
                Some((median, min, max)) => log::info!(
                    "{noun}: total={}, avg={avg} {unit}, median={median} {unit}, min={min} {unit}, max={max} {unit}",
                    tx.num_requests
                ),
                None => log::info!("{noun}: total={}, avg={avg} {unit}", tx.num_requests),
            }
            log::info!(
                "tx: bytes={} ({})",
                fmt_bytes(tx.bytes_tx),
                fmt_bitrate(tx.bytes_tx, secs)
            );
        }

        if let Some(target) = mean(&self.targets) {
            if target > 0 {
                let achieved = 100.0 * (requests as f64 / secs) / target as f64;
                log::info!("target: avg={target} {unit}, achieved={achieved:.1}%");
            }
        }

        if let Some(rx) = &self.snapshot.rx {
            let unanswered = requests.saturating_sub(rx.num_responses + rx.num_timeouts);
            log::info!(
                "rx: responses={} ({:.2}% of {noun}), timeouts={}, unanswered={}, bytes={} ({})",
                rx.num_responses,
                pct(rx.num_responses, requests),
                rx.num_timeouts,
                unanswered,
                fmt_bytes(rx.bytes_rx),
                fmt_bitrate(rx.bytes_rx, secs),
            );

            // Timeouts are recorded into the same histogram, exactly as the
            // TUI reports it.
            if rx.histogram_buckets.iter().sum::<u64>() > 0 {
                let hist = LogHistogram::new(rx.histogram_buckets.clone());
                log::info!(
                    "latency: p50={}, p90={}, p95={}, p99={}, p99.9={}, max={}",
                    fmt_us(hist.quantile(0.5)),
                    fmt_us(hist.quantile(0.9)),
                    fmt_us(hist.quantile(0.95)),
                    fmt_us(hist.quantile(0.99)),
                    fmt_us(hist.quantile(0.999)),
                    fmt_us(hist.quantile(1.0)),
                );
            }
        }

        if let Some(http) = &self.snapshot.http {
            let total = http.num_2xx + http.num_3xx + http.num_4xx + http.num_5xx;
            if total > 0 {
                log::info!(
                    "http: 2xx={} ({:.2}%), 3xx={}, 4xx={}, 5xx={}",
                    http.num_2xx,
                    pct(http.num_2xx, total),
                    http.num_3xx,
                    http.num_4xx,
                    http.num_5xx
                );
            }
        }

        if let Some(sock) = &self.snapshot.socket {
            log::info!(
                "sockets: created={}, errors={}, retransmits={}",
                sock.num_sock_created,
                sock.num_sock_errors,
                sock.num_retransmits
            );
        }

        if let Some(bursts) = &self.snapshot.burst_tx {
            if let Some((total, avg_size, full_pct)) = burst_stats(&bursts.num_bursts_tx) {
                log::info!("bursts: total={total}, avg-size={avg_size:.1}, full={full_pct:.1}%");
            }
        }

        let issues = self.issues();
        if issues.is_empty() {
            log::info!("verdict: OK (no anomalies detected)");
        } else {
            for issue in &issues {
                log::warn!("anomaly: {issue}");
            }
            log::warn!("verdict: {} anomaly kind(s) detected", issues.len());
        }
    }

    /// Everything a performance engineer should be alarmed by, one entry per
    /// anomaly kind.
    fn issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        let (noun, unit) = self.wording();
        let requests = self.requests();

        if let Some(rx) = &self.snapshot.rx {
            if rx.num_timeouts > 0 {
                issues.push(format!(
                    "{} timeouts ({:.3}% of {noun})",
                    rx.num_timeouts,
                    pct(rx.num_timeouts, requests)
                ));
            }
            let unanswered = requests.saturating_sub(rx.num_responses + rx.num_timeouts);
            if pct(unanswered, requests) > UNANSWERED_THRESHOLD_PCT {
                issues.push(format!(
                    "{unanswered} {noun} got no response ({:.2}%, stuck target or lost responses)",
                    pct(unanswered, requests)
                ));
            }
        }

        if let Some(http) = &self.snapshot.http {
            if http.num_5xx > 0 {
                issues.push(format!("{} HTTP 5xx responses", http.num_5xx));
            }
            if http.num_4xx > 0 {
                issues.push(format!("{} HTTP 4xx responses", http.num_4xx));
            }
        }

        if let Some(sock) = &self.snapshot.socket {
            if sock.num_sock_errors > 0 {
                issues.push(format!("{} socket errors", sock.num_sock_errors));
            }
            if sock.num_retransmits > 0 {
                issues.push(format!(
                    "{} TCP retransmits (network or target under pressure)",
                    sock.num_retransmits
                ));
            }
        }

        if let Some(target) = mean(&self.targets) {
            let secs = self.duration.as_secs_f64().max(f64::EPSILON);
            let avg = requests as f64 / secs;
            if target > 0 && avg < UNDERACHIEVED_THRESHOLD * target as f64 {
                issues.push(format!(
                    "target underachieved: avg {} of {target} {unit} ({:.1}%)",
                    avg.round() as u64,
                    100.0 * avg / target as f64
                ));
            }
        }

        issues
    }

    /// What to call the load units: DPDK pushes packets, everything else
    /// requests.
    fn wording(&self) -> (&'static str, &'static str) {
        if self.engine_kind == "dpdk" {
            ("packets", "pps")
        } else {
            ("requests", "rps")
        }
    }

    fn requests(&self) -> u64 {
        self.snapshot.tx.as_ref().map_or(0, |tx| tx.num_requests)
    }
}

/// Returns `(median, min, max)` of the achieved-rate samples, rounded.
fn distribution(rates: &[f64]) -> Option<(u64, u64, u64)> {
    if rates.is_empty() {
        return None;
    }

    let mut sorted = rates.to_vec();
    sorted.sort_by(f64::total_cmp);

    let mid = sorted.len() / 2;
    let median = if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    };

    Some((
        median.round() as u64,
        sorted[0].round() as u64,
        sorted[sorted.len() - 1].round() as u64,
    ))
}

/// Integer mean of the samples, `None` when there are none.
fn mean(samples: &[u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }

    Some(samples.iter().sum::<u64>() / samples.len() as u64)
}

/// Returns `(total, average burst size, share of full bursts %)`.
fn burst_stats(counts: &[u64]) -> Option<(u64, f64, f64)> {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return None;
    }

    let weighted: u64 = counts.iter().enumerate().map(|(idx, &c)| (idx as u64 + 1) * c).sum();
    let full = *counts.last().expect("total > 0 implies non-empty");

    Some((
        total,
        weighted as f64 / total as f64,
        100.0 * full as f64 / total as f64,
    ))
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        return 0.0;
    }

    100.0 * part as f64 / whole as f64
}

fn fmt_duration(d: Duration) -> String {
    let total = d.as_secs_f64();
    if total < 60.0 {
        return format!("{total:.1}s");
    }

    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else {
        format!("{m}m{s:02}s")
    }
}

fn fmt_bytes(v: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

    let mut value = v as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{v} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn fmt_bitrate(bytes: u64, secs: f64) -> String {
    const UNITS: [&str; 5] = ["bit/s", "Kbit/s", "Mbit/s", "Gbit/s", "Tbit/s"];

    let mut value = bytes as f64 * 8.0 / secs;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }

    format!("{value:.1} {}", UNITS[unit])
}

/// Formats a latency in microseconds, scaling the unit for readability.
fn fmt_us(us: u64) -> String {
    if us == u64::MAX {
        // `LogHistogram::quantile` overflow marker.
        return "n/a".into();
    }

    if us < 1_000 {
        format!("{us}us")
    } else if us < 1_000_000 {
        format!("{:.2}ms", us as f64 / 1e3)
    } else {
        format!("{:.2}s", us as f64 / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use dwd_proto::{HttpStats, RxStats, SocketStats, TxStats};

    use super::*;

    /// A summary over the given snapshot with a 10s load window and flat
    /// 1000-RPS target.
    fn summary(snapshot: StatsSnapshot) -> RunSummary {
        RunSummary {
            engine_kind: "http",
            duration: Duration::from_secs(10),
            rates: vec![1000.0; 10],
            targets: vec![1000; 10],
            snapshot,
        }
    }

    /// A snapshot of a clean 10k-requests run matching the `summary` fixture.
    fn clean_snapshot() -> StatsSnapshot {
        StatsSnapshot {
            generator_rps: 1000,
            tx: Some(TxStats {
                num_requests: 10_000,
                bytes_tx: 1_000_000,
            }),
            rx: Some(RxStats {
                num_responses: 10_000,
                num_timeouts: 0,
                bytes_rx: 2_000_000,
                histogram_buckets: vec![0; 40],
            }),
            socket: Some(SocketStats {
                num_sock_created: 8,
                num_sock_errors: 0,
                num_retransmits: 0,
            }),
            http: Some(HttpStats {
                num_2xx: 10_000,
                num_3xx: 0,
                num_4xx: 0,
                num_5xx: 0,
            }),
            burst_tx: None,
        }
    }

    #[test]
    fn clean_run_has_no_issues() {
        assert!(summary(clean_snapshot()).issues().is_empty());
    }

    #[test]
    fn issues_detect_anomalies() {
        let mut snapshot = clean_snapshot();
        {
            let rx = snapshot.rx.as_mut().unwrap();
            rx.num_timeouts = 100;
            rx.num_responses = 9_000; // 900 unanswered on top of the timeouts.
        }
        {
            let http = snapshot.http.as_mut().unwrap();
            http.num_2xx = 8_800;
            http.num_4xx = 50;
            http.num_5xx = 150;
        }
        {
            let sock = snapshot.socket.as_mut().unwrap();
            sock.num_sock_errors = 3;
            sock.num_retransmits = 7;
        }

        let issues = summary(snapshot).issues();
        // Timeouts, unanswered, 5xx, 4xx, socket errors and retransmits.
        assert_eq!(issues.len(), 6);
    }

    #[test]
    fn unanswered_below_threshold_is_tolerated() {
        let mut snapshot = clean_snapshot();
        // 50 of 10k (0.5%) in flight at shutdown: normal, not an anomaly.
        snapshot.rx.as_mut().unwrap().num_responses = 9_950;

        assert!(summary(snapshot).issues().is_empty());
    }

    #[test]
    fn underachieved_target_is_flagged() {
        let mut snapshot = clean_snapshot();
        // 5k requests in 10s against a 1000-RPS target: 50% achieved.
        snapshot.tx.as_mut().unwrap().num_requests = 5_000;
        snapshot.rx.as_mut().unwrap().num_responses = 5_000;

        let issues = summary(snapshot).issues();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("underachieved"), "{}", issues[0]);
    }

    #[test]
    fn distribution_median_odd_and_even() {
        assert_eq!(distribution(&[3.0, 1.0, 2.0]), Some((2, 1, 3)));
        assert_eq!(distribution(&[4.0, 1.0, 2.0, 3.0]), Some((3, 1, 4))); // 2.5 rounds up.
        assert_eq!(distribution(&[]), None);
    }

    #[test]
    fn mean_of_samples() {
        assert_eq!(mean(&[1, 2, 3]), Some(2));
        assert_eq!(mean(&[]), None);
    }

    #[test]
    fn burst_stats_math() {
        // 10 bursts of size 1, 10 of size 32 (full).
        let mut counts = vec![0; 32];
        counts[0] = 10;
        counts[31] = 10;

        let (total, avg, full) = burst_stats(&counts).unwrap();
        assert_eq!(total, 20);
        assert!((avg - 16.5).abs() < 1e-9);
        assert!((full - 50.0).abs() < 1e-9);

        assert_eq!(burst_stats(&[0; 32]), None);
    }

    #[test]
    fn pct_handles_zero_denominator() {
        assert_eq!(pct(1, 0), 0.0);
        assert_eq!(pct(1, 4), 25.0);
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2.00 KiB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024 * 1024), "5.00 GiB");

        assert_eq!(fmt_bitrate(1_000_000, 8.0), "1.0 Mbit/s");

        assert_eq!(fmt_us(999), "999us");
        assert_eq!(fmt_us(1_500), "1.50ms");
        assert_eq!(fmt_us(2_000_000), "2.00s");
        assert_eq!(fmt_us(u64::MAX), "n/a");

        assert_eq!(fmt_duration(Duration::from_millis(59_940)), "59.9s");
        assert_eq!(fmt_duration(Duration::from_secs(61)), "1m01s");
        assert_eq!(fmt_duration(Duration::from_secs(3723)), "1h02m03s");
    }
}
