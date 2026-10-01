ARG UBUNTU_VERSION=24.04
FROM ubuntu:${UBUNTU_VERSION} AS builder

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    curl && \
    rm -rf /var/lib/apt/lists/*

ENV RUST_VERSION=1.96
ENV PATH="/root/.cargo/bin:${PATH}"

RUN curl -fsSL https://sh.rustup.rs | bash -s -- \
    -y --profile minimal --default-toolchain "$RUST_VERSION"
RUN cargo install cargo-deb --locked

WORKDIR /src
COPY . .

RUN RUSTFLAGS="-C target-feature=+crt-static" \
    cargo build --locked --release --features jemalloc --target x86_64-unknown-linux-gnu

ARG DWD_VERSION
RUN set -- && \
    if [ -n "$DWD_VERSION" ]; then \
        release_version="${DWD_VERSION%%+*}"; \
        build_metadata="${DWD_VERSION#"$release_version"}"; \
        deb_version=$(printf '%s' "$release_version" | sed 's/-/~/'); \
        set -- --deb-version "${deb_version}${build_metadata}"; \
    fi && \
    cargo deb --target x86_64-unknown-linux-gnu --no-build "$@"
