# AGENTS.md

Guide for agents working with this package.

## Overview

`dwd-backend` is a simple HTTP server in Rust that serves content from a file or a
hardcoded string. It supports three protocols simultaneously:

- **HTTP** (TCP) via actix-web
- **HTTPS** (TCP) via actix-web + rustls
- **HTTP/3** (UDP/QUIC) via a separate server on `quinn` + `h3`

Everything is configured through a TOML config. The binary is named `dwd-backend`.
Certificates for HTTPS/HTTP3 are either read from files or generated on the fly
via `rcgen` (see `get_cert_and_key`).

## Workspace placement

- The package lives at `~/dwd/dwd-backend` inside the `~/dwd` Git repository.
- It is intentionally listed in the parent `Cargo.toml` under
  `workspace.exclude`, so it remains a standalone Cargo project with its own
  `Cargo.lock`, release profile, `target/`, and Debian package paths.
- Run Cargo commands from `~/dwd/dwd-backend` or pass
  `--manifest-path dwd-backend/Cargo.toml`; running them at `~/dwd` operates on
  the parent workspace instead.
- Git commands resolve to the parent `~/dwd` repository. Do not modify sibling
  packages or commit changes without an explicit request.

## Structure

- `README.md` — project overview and build instructions (standard, musl, deb).
- `src/main.rs` — entry point, mode dispatch, and listener orchestration.
- `src/server.rs` — shared response construction, Actix HTTP/HTTPS listeners,
  QUIC endpoint binding, and HTTP/3 connection/request handlers.
- `src/tls.rs` — certificate loading/generation and shared rustls/QUIC setup.
  `CertKey` preserves the parsed private-key format and can be cloned safely
  when HTTPS and HTTP/3 share a certificate.
- `src/cli.rs` — clap CLI definition (`Cli`, `Mode`) with the flag rules
  (see "CLI" below).
- `src/config.rs` — TOML config parsing (`Config`, `HttpConfig`, `HttpsConfig`,
  `Http3Config`, `ContentConfig`) and content resolution.
- `example/config.toml` — example config with all sections.
- `example/content` — example content file.
- `example/README.md` — user-facing docs for the example.
- `example/h3_client.rs` — test HTTP/3 client (cargo example) for checking the
  server; it does not verify the certificate.

## Build and run

```sh
cargo build                         # build the binary
cargo build --bins --examples       # + the test h3 client
cargo run -- -c example/config.toml # run with the example config
```

Static musl release build:

```sh
cargo build --release --target x86_64-unknown-linux-musl
```

The config path is resolved in this order:
1. the `--config`/`-c` argument;
2. the `DWD_BACKEND_CONFIG` environment variable;
3. the default `/etc/dwd-backend/config.toml`.

## CLI

Two mutually exclusive run modes (see `src/cli.rs`):

- `--config`/`-c <PATH>` — config-driven mode (HTTP/HTTPS/HTTP3 per the TOML
  file). This is the default mode; without the flag the path falls back to
  `DWD_BACKEND_CONFIG`, then `/etc/dwd-backend/config.toml`.
- `--mode`/`-m <http|https|http3>` — standalone single-protocol mode configured
  entirely from CLI flags; the config file is ignored and the default content
  is served.

Flag rules, enforced by clap (`conflicts_with`/`requires` in `src/cli.rs`):

- `--mode` conflicts with `--config`.
- `--port`/`-p`, `--cert`, `--key` require `--mode` (defaults: port 80 for
  `http`, 443 otherwise; certificate generated if not given).
- `--cert` and `--key` require each other — either is useless alone.

## Debian package

Built with `cargo-deb` (config in `[package.metadata.deb]` of `Cargo.toml`).
The package ships a statically linked glibc binary built with `jemalloc`
(jemalloc is incompatible with musl, so the deb cannot use the static musl
binary; static glibc via `-C target-feature=+crt-static` gives a static binary
that keeps jemalloc). Build it first and reuse it:

```sh
RUSTFLAGS="-C target-feature=+crt-static" \
    cargo build --release --features jemalloc --target x86_64-unknown-linux-gnu
cargo deb --target x86_64-unknown-linux-gnu --no-build
```

`cargo-deb` rewrites the `target/release/` asset paths to
`target/x86_64-unknown-linux-gnu/release/` when `--target` is passed, so the
`[package.metadata.deb]` assets need no change. The `.deb` lands in
`target/debian/`. Contents:

- `/usr/bin/dwd-backend` — the statically linked glibc binary (built with
  `jemalloc`). Static glibc uses NSS at runtime, so a `getaddrinfo` link-time
  warning is expected and harmless (the server binds numeric addresses or
  `localhost`).
- `/etc/dwd-backend/config.toml` — default config (a dpkg conffile).
- `/lib/systemd/system/dwd-backend.service` — systemd unit (enabled on
  install, not started; runs as the `dwd-backend` system user with
  `CAP_NET_BIND_SERVICE` for ports < 1024).
- `/usr/share/doc/dwd-backend/{README,copyright,changelog.Debian.gz}`.

Packaging assets live in `pkg/`: `config.toml`, `dwd-backend.service`, the
`postinst`/`postrm` maintainer scripts (they contain `#DEBHELPER#`, which
cargo-deb expands into the systemd enable/stop hooks), `changelog` (the
Debian changelog, wired in via `changelog = "pkg/changelog"` in
`[package.metadata.deb]`; bump it per release), and `make-changes.sh`. The
`postinst` also creates the `dwd-backend` system user/group; `postrm` removes
them on purge.

`cargo-deb` builds only the `.deb`, not the `.changes` upload-control file (it
is a binary build, not a source build). `pkg/make-changes.sh <path-to.deb>`
derives a `.changes` from the finished `.deb`'s control fields plus the latest
`pkg/changelog` entry (Format 1.8, with size/MD5/SHA1/SHA256), writing it next
to the `.deb`. Sign with `debsign`, upload with `dput`.

## Config

- `address` — **global** at the config root, one for all servers
  (HTTP/HTTPS/HTTP3). Sections have no address, only `port`.
- `[http]` — optional (default port 80).
- `[https]` — **enabled by default** (default port 443); disable with
  `enabled = false`. If `cert` and `key` are not set, generates a certificate on
  the fly.
- `[http3]` — **enabled by default** (default port 443); disable with
  `enabled = false`. Works independently of HTTPS (generates its own certificate
  if `cert` and `key` are not set, or reuses the one from `[https]` when that is
  enabled). Runs over QUIC/UDP, so it can share a port number with HTTPS
  (TCP vs UDP).
- The config file itself is optional: if it is missing, default values are used.
- `[content]` — `file` (served if it exists) or `payload`.

## Performance

- **The response body is built once at startup** as a shared `Bytes` buffer
  (`AppState.body`); every handler answers with a refcount clone, so the hot
  path allocates and formats nothing.
- **The content-type header is never parsed per request**: both handlers use
  `HeaderValue::from_static` on the shared `CONTENT_TYPE_TEXT` constant.
- **`main` runs on the multi-threaded `#[tokio::main]` runtime**, not
  `#[actix_web::main]` (actix-rt is current-thread). actix spawns its own
  worker threads either way, but the QUIC/HTTP3 tasks run on the main runtime —
  with actix-rt they would all share one core. Do not switch back.
- **`[profile.release]`** enables fat LTO, one codegen unit, abort-on-panic, and `strip`.
- **actix-web is built with `default-features = false`** (only `macros` and
  `rustls-0_23`, which enables HTTP/2): compression, cookies, and unicode
  routing are unused and only add dependencies and binary size.
- **`resolve_content` reads the file directly** and falls back to the payload
  on `NotFound` — no separate existence check (extra stat + check-then-read
  race).
- `jemalloc` is behind an optional `jemalloc` feature (it is incompatible with
  musl). Enable it on glibc with `cargo build --features jemalloc`. Default
  builds and musl builds use the system allocator.

## Important technical facts / pitfalls

- **actix-web does NOT support HTTP/3.** HTTP/3 is implemented by a separate
  server on `quinn` + `h3` + `h3-quinn`, spawned via `tokio::spawn` alongside
  actix. Do not look for HTTP/3 inside actix.
- **Two crypto providers in the tree:** both `ring` (via quinn) and `aws-lc-rs`
  (via actix rustls). The default provider is therefore ambiguous —
  `CryptoProvider::get_default()` would panic/error. The rustls config is always
  created EXPLICITLY via
  `builder_with_provider(rustls::crypto::ring::default_provider())`. Do not
  switch to `ServerConfig::builder()` without an explicit provider.
- **QUIC requires** TLS 1.3, the `h3` ALPN, and `max_early_data_size = u32::MAX` —
  see `build_quic_config` in `src/tls.rs`.
- Crate versions are interdependent: `h3 = 0.0.8`, `h3-quinn = 0.0.10`,
  `quinn = 0.11`, `rustls = 0.23`, `http = 1`, `rcgen = 0.13`. The h3 0.0.8 API
  differs from older versions (`accept()` → `RequestResolver` →
  `resolve_request()`).
- **`build_quic_config` reuses `build_rustls_config`**, adding the `h3` ALPN and
  `max_early_data_size`. The base config also enables TLS 1.2, but quinn uses
  only TLS 1.3, so that is harmless.
- **Two `http` crate versions in the tree:** actix-web 4 uses `http` 0.2
  internally, while the h3 stack uses `http` 1. Their `HeaderName`/`HeaderValue`
  types are not interchangeable — a header value cannot be shared between the
  actix and h3 handlers (hence the shared `&str` constant instead).
- **The certificate is generated/loaded once.** If both `[https]` and `[http3]`
  are enabled without their own cert/key, HTTP/3 reuses the HTTPS certificate
  (the "Generating..." line appears only once in the log).

## Testing (manual)

The server blocks the terminal, and background tasks in a persistent shell tend
to hang tool calls. A reliable approach is to start the server via `setsid` with
a log file, then read the log and send requests in separate calls, and finally
kill the process:

```sh
setsid ./target/debug/dwd-backend -c example/config.toml >/tmp/bh.log 2>&1 &
# then separately:
curl -s http://127.0.0.1:8080/          # HTTP
curl -sk https://127.0.0.1:8443/         # HTTPS
./target/debug/examples/h3_client https://127.0.0.1:8443/   # HTTP/3
pkill -9 -f 'target/debug/dwd-backend'
```

The expected response for all three: `Content: <payload or file contents>`.

The system `curl` without an HTTP/3 QUIC backend cannot test HTTP/3 — use
`examples/h3_client`.

## Conventions

- Comments and user-facing messages (logs, errors) are in English, in the
  existing style (concise, without explaining the obvious).
- Do NOT commit without an explicit request. Do NOT create unnecessary temporary
  files in the repo (use `/tmp`).
- Artifacts in `target/` with stale names (if something was renamed) are rebuilt
  automatically; run `cargo clean` if needed.
