# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

DWD is a high-performance traffic generator written in Rust (Cargo workspace). It has two fundamentally different operating-mode families:

- **Native modes** (`udp`, `http`, `http/raw`): use the kernel network stack; performance comes from careful code, per-task coroutines, and multithreading.
- **DPDK mode** (`dpdk`): bypasses the kernel, driving the NIC directly from userspace. Compiled in only with `--features=dpdk` (Linux only).

Workspace crates: `dwd/` (the binary + library, all load logic) and `dpdk/` (package `dwd-dpdk`, an optional FFI wrapper). See `README.md` for end-user usage, the load-profile YAML reference, and DPDK host setup (hugepages, PCAP crafting).

**Engine maturity** (treat changes accordingly):
- `udp` (native) — production reference implementation; the most battle-tested path.
- `dpdk` — production for use, but internals are in flux (`main.rs` TODO: refactor worker + library); prefer localized changes.
- `http` (hyper-based) — production. *Note:* `README.md`'s "HTTP (right now actually not)" remark is **stale**; the hyper HTTP engine works.
- `http/raw` (raw-socket `EngineRaw`) — **WIP / experimental**; don't assume parity with the others.

## Active direction (read before any large change)

Work is converging on four fronts. New code should move *toward* these, and CLAUDE should propose changes that align with them. The bullets below describe intent — the code does not fully reflect them yet.

1. **Decouple CLI/TUI from the core via an API seam.** Target: extract a `dwd-core` crate (engine, generators, shaper, stats, `Produce`) and put a **tonic gRPC** service in front of it. The CLI and TUI become **clients that talk only through the API** — no reaching into the engine directly. *Single binary is fine; co-located in-process is fine for now* — the goal is the **code boundary**, not separate processes. The coupling to untangle is today's shared `Arc<Stat>`, the in-process `mpsc<GeneratorEvent>`, and the in-process TUI. Map them onto the API: **`GeneratorEvent` → unary control** (set / suspend / resume), **`StatSource` → streaming statistics**.
2. **Modernize.** Move to **edition 2024 + latest stable Rust**, and **drop the MSRV pin** and its CI job (binaries ship prebuilt, so build-from-source on old toolchains isn't a constraint). Upgrade dependencies (including breaking majors). Today the code is edition 2021 / `rust-version = 1.87`.
3. **Performance is paramount** — tuning everywhere. See **Performance contract** below; it is a hard gate, not advice.
4. **Build the test suite from near-zero** — unit + property + benchmarks. See **Testing**.

## Commands

```bash
# Build / run (native; works on all platforms)
cargo build -p dwd --release          # binary at target/release/dwd
cargo run -p dwd -- udp 127.0.0.1:80  # opens the TUI; type the desired RPS

# DPDK build (Linux only; needs DPDK 24.11 installed to /usr/local, see Dockerfile)
cargo build -p dwd --release --features=dpdk

# The enforced gate (CI) — there is currently NO test suite, so this is the bar:
cargo fmt --all -- --check
cargo clippy -p dwd --all-targets -- -D warnings   # native
cargo clippy --features=dpdk -- -D warnings        # DPDK (runs inside Docker in CI)
cargo build -p dwd --release

# Tests / benches (be the one to add them — see Testing):
cargo test  -p dwd                    # unit + property tests (proptest)
cargo bench -p dwd                    # criterion benches in benches/
# Profiling a hot path:
perf record -g -- target/release/dwd udp <addr>   # then: perf report

# Reproducible DPDK build + clippy + .deb (builds DPDK 24.11 from source first):
docker build -t dwd .
nix-shell                             # dev shell with dpdk/clang/rdma-core (shell.nix)
```

Build/runtime facts:
- `.cargo/config.toml` sets `--cfg tokio_unstable` globally — required because native engines use `tokio::runtime::LocalRuntime`.
- Global allocator is **jemalloc** (`main.rs`); release profile keeps `debug = true` but `panic = "abort"` (workspace `Cargo.toml`).
- Releases (`v*` tags, `.github/workflows/release.yml`) ship three binaries: no-DPDK, DPDK on Ubuntu 24.04 (`Dockerfile`), and a glibc-2.27 compat build (`Dockerfile.ubuntu18`), plus `.deb`s.
- These settings (jemalloc / `panic=abort` / `tokio_unstable` / the threading model) are deliberate but **not off-limits** — they may be revisited during perf/modernization work, with benchmark justification.

## Architecture

### Startup and dispatch
`main.rs` parses the CLI (`cmd.rs`, clap) into a `Config` (`cfg.rs`), then runs a **single-threaded** tokio runtime (`new_current_thread`). `Runtime::run` (`engine.rs`) selects an engine from `ModeConfig` (`Http` / `HttpRaw` / `Udp` / `Dpdk`).

Everything routes through the `Engine` trait (`engine.rs`) — the seam between `Runtime` and the modes:
`generator()`, `limits()`, `ui()`, `stat_source()`, `run()`. `Runtime` is mode-agnostic. (This trait is the natural place the future gRPC control/stats API will attach.)

### Three concurrency domains
`Runtime::run` fans out into three independently-scheduled places, coordinated only by a shared `Arc<AtomicBool>` `is_running`:
1. **Engine** — OS thread named `engine`; blocks doing the actual load.
2. **UI** — OS thread named `ui`; `ui::run` is a blocking ratatui/crossterm loop. Exiting (q / Ctrl-C) flips `is_running` false.
3. **Generator + API server** — on the main current-thread tokio runtime (`run_generator`; the axum metrics server, if `--api-addr` is set, is a `tokio::spawn` task).

### The rate-control pipeline (the heart of the system)
Understand this before touching pacing:
1. **Generators** (`generator.rs`) are time functions returning a target RPS at any `Instant`: `const`, `line`, `sin`, plus `seq` (sequential) / `sum` (parallel) combinators. Built from YAML (`--generator`) via a factory closure in `Config`; default is a ~constant 1000-RPS line when no file is given.
2. `run_generator` (`engine.rs`) wraps the generator in a `SuspendableGenerator`, polls its RPS every 10ms, and **distributes that one number across the engine's per-worker `limits` (`Vec<Arc<AtomicU64>>`)** by integer division + remainder. It also mirrors the value into a `SharedGenerator` snapshot used **only for display/metrics** (workers never read it).
3. **`Shaper`** (`shaper.rs`) — the single token-bucket rate limiter used by *all* engines. Each worker owns one and reads its `Arc<AtomicU64>` limit. `tick()` returns how many requests are owed; `consume(n)` debits.
4. **`ShapedCoroWorker`** (`engine/coro.rs`) — the per-task loop: `tick()` → execute up to 32 `Task`s → `consume`. Bridges rate-limiting and the async `Task` trait.

`GeneratorEvent` (Suspend / Resume / Set, in `lib.rs`) flows UI → `run_generator` over an mpsc channel and mutates **only** the `SuspendableGenerator` (`set(Some(v))` suspends+overrides; `set(None)` resumes) — it does *not* write shaper limits directly. **Changing the `Shaper` token-bucket contract requires updating every engine and `ShapedCoroWorker` together** — it's the shared coupling point.

### Native concurrency layering
`ThreadPool` (OS threads, `engine/runtime.rs`) → `LocalTaskPool` (one tokio `LocalRuntime` per thread, `spawn_local` tasks) → `ShapedCoroWorker` (one per concurrency slot) → `Shaper`. `--threads` (native) sets the thread count; `--concurrency` (HTTP) sets total in-flight tasks across threads. Each task picks its own bind IP (`--bind-network` filters interface IPs; default is `[::]:0`) and cycles ephemeral ports.

### Engines
- **HTTP** (`engine/http/`): two variants over one `Config<T>` — `Engine` (`engine.rs`, full hyper http1 handshake) and `EngineRaw` (`engine_raw.rs`, manual socket I/O + `httparse`; experimental). `io.rs` adapts tokio↔hyper I/O. Payloads load once from a JSONL file (`payload/jsonline.rs`, yandex-tank-style records: uri/method/host/headers).
- **UDP** (`engine/udp/`): datagrams with optional bursts; responses ignored.
- **DPDK** (`worker/dpdk.rs` + `dpdk/` crate): a separate non-tokio path. Pins one `Worker` per configured lcore via `rte_eal_mp_remote_launch`, loads a PCAP into preallocated mbufs, round-robins packets across cores. Driven by a YAML hardware config (`master_lcore` + per-PCI-device core lists).

### The `Produce` trait
`Produce` (`lib.rs`, with `OneProduce` / `VecProduce`) is the lock-free, thread-safe **cycling iterator** every native engine uses to hand out the next request payload and next bind address to concurrent workers. Returns `&Item` by reference, never ends — and never allocates on the hot path.

### Observability (the observer pattern — keep it)
Metrics are **statistics** built on an observer pattern that the maintainer is happy with; **extend it, don't redesign it**. The future gRPC stream attaches here.
- **Per-CPU stats** (`stat.rs`, `stat/percpu.rs`): each worker owns its own `Arc<PerCpuStat>` and updates `UnsafeCell<u64>` counters with no atomics/locks — sound **only because the instance is not shared between threads**. Aggregation sums across CPUs lazily on read. `StatSource` lets each engine expose the subset it has (UDP has no RX, DPDK only TX, …).
- **Histograms** (`histogram.rs`): log-bucketed latency (~40 buckets) with quantile interpolation.
- **TUI** (`ui/`): `ui/metric/` = Gauge/Meter/Quantile/Throughput; `ui/widget/` = input (digit entry → `GeneratorEvent::Set`), keymap, status; `s` toggles suspend/resume.
- **API** (`api/`): axum server exposing Prometheus metrics at `/api/v1/metrics` (`api/metrics.rs`).
- **Logging** (`logging.rs`): `log::*` call sites today, with a `tracing-subscriber` backend, to stderr; verbosity via `-v` repetition.

## Performance contract (hot-path rules — hard gate)

Perf is paramount; treat these as do-not-regress invariants for any per-request / per-packet (inner-loop) code:
- **No heap allocation and no syscalls in the inner loop.** Reuse buffers, batch via bursts, preallocate (mbuf pools; payloads/bind-addrs via `Produce`).
- **Lock-free per-CPU stats only.** Keep counters per-CPU (`UnsafeCell`, exactly one instance per thread), atomics `Relaxed` unless correctness demands more. Never add a shared lock or strong-ordering atomic on the hot path.
- **Static dispatch in the inner loop.** Keep engines generic (monomorphized over `Produce`/payload types); `Box<dyn Generator>` and friends are setup-time only. Watch for accidental virtualization after refactors.
- **Respect core-pinning / NUMA.** DPDK pins one worker per lcore with per-socket mempools — keep work local to its core/NUMA node; don't introduce cross-core sharing in workers. (Native core-affinity is a future perf lever.)
- **Prefer branchless code** in hot paths.

Validation methodology (required for perf changes):
- Add/extend **criterion** benchmarks and compare before/after.
- **Profile under `perf`** (`perf record -g` → `perf report`) and study the report; don't claim a win without numbers.

## Testing

There is currently **no test suite** — `cargo test` only compiles targets. The enforced bar is fmt + `clippy -D warnings` + build. Build the suite as you work:
- **Unit tests**: inline `#[cfg(test)] mod tests`.
- **Property tests**: **proptest**. High-value first targets: `generator.rs` (line/sin/seq/sum math, `find_closest_phase`), `shaper.rs` (token-bucket conservation), `histogram.rs` (bucketing + quantile monotonicity).
- **Benchmarks**: **criterion** in `benches/` (e.g. `Shaper::tick`, `Generator::current`, histogram record/quantile, DPDK tx loop).
- Runtime correctness is otherwise the author's manual responsibility. **DPDK is unverifiable without hardware** (bound NIC, hugepages, `CAP_ADMIN`) — when you can't run something, say what you couldn't verify.

## Conventions

- **Errors**: `anyhow::Error` / `Box<dyn Error>` throughout the binary; `thiserror` is available **only under the `dpdk` feature** and used for typed DPDK errors (`worker/dpdk.rs`). Don't add `thiserror` to non-DPDK code.
- **Logging**: use the `log::*` facade in new code. **Do not introduce `tracing` spans / instrumentation** — distributed tracing is explicitly not wanted here (the logging backend itself may change later, `main.rs` TODO #3). Never log inside the inner loop.
- **Metrics**: extend the existing `StatSource` + per-CPU `Stat` observer pattern; don't replace it.
- **`dpdk` feature gating is strict**: the feature pulls in `dwd-dpdk`, `thiserror`, `pcap-parser`, `etherparse`. Any code touching those must be behind `#[cfg(feature = "dpdk")]`, or the default build breaks. `dpdk/build.rs` and `worker/dpdk.rs` are Linux-only.
- **Doc comments**: the codebase uses dense `//!`/`///` docs and section comments — match that density.

## Commits & PRs

Commit messages **and** PR descriptions use the same shape:

```
<type>: <brief>

<description>
```

- `<type>` is a conventional-commit type — `feat`, `fix`, `perf`, `docs`, `refactor`, `test`, `chore`, `ci`, `build`, … — matching existing history (e.g. `feat: ci/cd`, `fix: ci`, `fix: CPU compatibility - use haswell target for DPDK`).
- `<brief>` is a short imperative summary on the subject line; after a blank line, `<description>` explains the what/why.
- **Never add AI attribution anywhere.** No `Co-Authored-By: Claude`, no "Co-developed with Claude", no "Generated with Claude Code" — not in commit messages, not in PR bodies.

## Recipes (the common feature work)

### Add a load-profile generator type
1. `generator.rs`: add `FooGeneratorConfig` (derive `Deserialize, Serialize, PartialEq, Clone`) and a variant in the `Config` enum (`#[serde(rename_all="lowercase", tag="type")]` → YAML `type: foo`).
2. `impl GeneratorFactory for FooGeneratorConfig` (`create` / `reduce` / `duration`) and add arms to `Config`'s `GeneratorFactory` match.
3. Implement the runtime `FooGenerator` + `Generator` trait (`activate` / `activate_at` / `current` / `current_at`). Contract: `current_at` returns `None` once `duration` elapses; it must answer at an **arbitrary `Instant`** (the `SuspendableGenerator` freezes time by subtracting suspended duration). `reduce(factor)` divides RPS for multi-worker scaling.
4. Add proptest + unit coverage. (`const` is just a `line` with equal endpoints — mirror that simplicity.)

### Add an engine (mode)
1. `cmd.rs`: add `XxxCmd`, a `ModeCmd` variant, and `TryFrom<XxxCmd> for XxxConfig`.
2. `cfg.rs`: add a `ModeConfig` variant and the arm in `TryFrom<ModeCmd> for ModeConfig`.
3. New module under `engine/`: implement the engine with `new()`, `stat()`, `limits()`, `run()`. Use `Shaper` + `ShapedCoroWorker` for pacing and `Produce` for payload/bind cycling; give each worker its own per-CPU `Stat`.
4. `engine.rs`: `impl Engine` for it (`generator` / `limits` / `ui` / `stat_source` / `run`), wire the TUI via `Ui::new(tx).with_*`, and add the `ModeConfig::Xxx => Box::new(..)` arm in `Runtime::run`.
5. Feature-gate it (like `dpdk`) if it needs heavy/optional dependencies.

## Handle with care

- **`worker/dpdk.rs` MLX5 shutdown ordering + mbuf refcnt cycling.** Subtle and double-free-prone: refcounts are inflated for zero-copy reuse and must be reset, with a DMA-flush delay, before port stop / EAL cleanup. It is **correctness-critical and hardware-unverifiable** — make only minimal, well-reasoned changes.

Nothing else is fenced off — perf and modernization may touch any part, provided changes stay correct, clippy-clean, and benchmarked where perf-relevant.

## Known drift / gotchas

- **DPDK version**: code targets **24.11** (`Dockerfile`, `dpdk/build.rs`); `README.md` was reconciled to 24.11.
- **DPDK FFI pattern**: hot-path DPDK calls (`rte_pktmbuf_alloc`, `rte_eth_rx/tx_burst`, …) are inline macros bindgen can't bind. They're re-exposed as `_rte_*` C functions in `dpdk/src/ffi/stub.c` (compiled by `cc`) and wrapped in `dpdk/src/lib.rs`; bindgen reads `dpdk/src/ffi/wrapper.h`. DPDK static libs are whole-archive linked; GPL deps (`numa`, `ibverbs`, `mlx5`) are linked dynamically.
- `README.md` install snippets omit `libmlx5-1` (needed at runtime for MLX5 NICs; present in the `Dockerfile` and the `.deb` `depends`).
