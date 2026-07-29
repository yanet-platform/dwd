# dwd-backend

A simple HTTP server in Rust that serves content from a file or a hardcoded
string. It speaks three protocols simultaneously:

- **HTTP** (TCP) via actix-web
- **HTTPS** (TCP) via actix-web + rustls
- **HTTP/3** (UDP/QUIC) via a separate server on `quinn` + `h3`

HTTPS and HTTP/3 are enabled by default; either can be disabled with
`enabled = false`. If no certificate is provided, a temporary self-signed one is
generated on the fly.

## Build

### Standard build

```sh
cargo build                       # debug binary
cargo build --release             # optimized binary
cargo build --bins --examples     # + the test HTTP/3 client
```

The binary is `target/{debug,release}/dwd-backend`. The release profile enables
fat LTO, a single codegen unit, abort-on-panic, and symbol stripping
(`[profile.release]` in `Cargo.toml`) for a faster and smaller binary.

### Static musl build

Produces a fully static binary with no dynamic dependencies:

```sh
rustup target add x86_64-unknown-linux-musl   # once
cargo build --release --target x86_64-unknown-linux-musl
```

The binary is `target/x86_64-unknown-linux-musl/release/dwd-backend`.

### Memory allocator (jemalloc)

The response body is built once at startup and shared across every request as a
reference-counted `Bytes` buffer, so the hot request path performs no
allocations or formatting. For allocation-heavy workloads the global allocator
still matters, so `jemalloc` is available behind an optional `jemalloc` feature:

```sh
cargo build --release --features jemalloc
```

> **Note:** `jemalloc` is incompatible with musl. Default builds and the static
> musl build therefore use the system allocator; enable `jemalloc` only on glibc
> (it must **not** be combined with `--target x86_64-unknown-linux-musl`).

### Debian package

Built with [`cargo-deb`](https://github.com/kornelski/cargo-deb). The package
ships a statically linked glibc binary built with `jemalloc`
(`-C target-feature=+crt-static`), so build it first and reuse it:

```sh
cargo install cargo-deb                        # once
RUSTFLAGS="-C target-feature=+crt-static" \
    cargo build --release --features jemalloc --target x86_64-unknown-linux-gnu
cargo deb --target x86_64-unknown-linux-gnu --no-build
```

The `.deb` lands in `target/debian/`. It installs:

- `/usr/bin/dwd-backend` — the statically linked glibc binary (built with
  `jemalloc`)
- `/etc/dwd-backend/config.toml` — default config (a dpkg conffile)
- `/lib/systemd/system/dwd-backend.service` — systemd unit (enabled on
  install, not started; runs as the `dwd-backend` system user with
  `CAP_NET_BIND_SERVICE` for ports < 1024)
- `/usr/share/doc/dwd-backend/{README,copyright,changelog.Debian.gz}`

The Debian changelog is `pkg/changelog`; bump it before releasing a new
version.

> **Note:** static glibc resolves host names via NSS, which is loaded at
> runtime, so a `Using 'getaddrinfo' in statically linked applications...`
> warning at link time is expected. The server binds numeric addresses or
> `localhost`, so this does not affect normal operation. For a fully portable
> static binary with no glibc runtime coupling, use the musl build instead.

#### `.changes` file

`cargo-deb` produces only the `.deb` (it is not a source build), not the
`.changes` file used to upload into an APT repository. Generate one from the
finished `.deb` and the changelog with the bundled helper:

```sh
pkg/make-changes.sh target/debian/dwd-backend_0.1.0-3_amd64.deb
```

It writes `dwd-backend_<version>_<arch>.changes` next to the `.deb` with the
control fields, the latest changelog entry, and the `.deb` size/MD5/SHA1/SHA256.
Sign it with `debsign` and upload with `dput` as usual.

## Run

The server has two mutually exclusive modes.

### Config-driven mode

Runs HTTP/HTTPS/HTTP3 as described by a TOML file:

```sh
cargo run -- --config example/config.toml
# short form:
cargo run -- -c example/config.toml
# or via environment variable:
DWD_BACKEND_CONFIG=example/config.toml cargo run
```

The config path is resolved in this order:

1. the `--config`/`-c` argument;
2. the `DWD_BACKEND_CONFIG` environment variable;
3. the default `/etc/dwd-backend/config.toml`.

The config file itself is optional: if it is missing, default values are used.

### Standalone single-protocol mode

`--mode`/`-m` runs exactly one protocol configured entirely from the command
line, ignoring the config file (the default content is served):

```sh
# HTTP on the default port (80)
cargo run -- --mode http
# HTTPS on a custom port with a generated certificate
cargo run -- -m https -p 8443
# HTTP/3 with an explicit certificate and key
cargo run -- -m http3 -p 8443 --cert cert.pem --key key.pem
```

Flags:

- `--mode`/`-m <http|https|http3>` — the protocol to run.
- `--port`/`-p <PORT>` — listen port (defaults to 80 for `http`, 443 otherwise).
- `--cert <PATH>` / `--key <PATH>` — certificate and key for `https`/`http3`;
  one is useless without the other, so they must be given together. If both are
  omitted, a temporary self-signed certificate is generated.

`--mode` cannot be combined with `--config`, and `--port`/`--cert`/`--key`
require `--mode`.

See [`example/README.md`](example/README.md) for a runnable example and the
[`example/config.toml`](example/config.toml) for all config options.
