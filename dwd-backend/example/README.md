# dwd-backend example

Example of running locally with HTTP, HTTPS, and HTTP/3.

## Run

```sh
cargo run -- --config example/config.toml
# or via environment variable:
DWD_BACKEND_CONFIG=example/config.toml cargo run
```

Listens on:
- `8080/tcp` (HTTP)
- `8443/tcp` (HTTPS)
- `8443/udp` (HTTP/3)

## Test

```sh
curl http://localhost:8080/
curl -k https://localhost:8443/
cargo run --example h3_client -- https://127.0.0.1:8443/
```

## Configuration

All settings are described in `config.toml`. The listen address (`address`) is
set once globally. HTTPS and HTTP/3 are enabled by default and independent of
each other; either can be turned off with `enabled = false`. If a section has no
certificate paths (`cert` and `key`), the server generates a temporary in-memory
certificate at startup. When `[http3]` has no paths of its own, it reuses the
`[https]` certificate if HTTPS is enabled.
