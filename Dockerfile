FROM ubuntu:24.04 AS builder

ENV RUST_VERSION=1.87
ENV DPDK_VERSION=24.11

ARG DEBIAN_FRONTEND=noninteractive

RUN apt update -y

# Setup dependencies:
# - curl - for downloading DPDK.
# - tar, xz-utils - for extracting DPDK.
RUN apt install -y curl tar xz-utils

# Download and unpack DPDK.
RUN mkdir -p /build/dpdk
WORKDIR /build
RUN curl -s -o dpdk-${DPDK_VERSION}.tar.xz https://fast.dpdk.org/rel/dpdk-${DPDK_VERSION}.tar.xz
RUN tar -xvJf dpdk-${DPDK_VERSION}.tar.xz -C dpdk --strip-components=1

# Build dependencies:
# - clang - compiler.
# - libclang-dev - required for Rust bindgen.
# - make - required for jemalloc-sys build.
# - meson, ninja-build - build system.
# - pkg-config - helps DPDK to find dependencies.
# - python3-pyelftools - for whatever DPDK reasons.
RUN apt install -y --no-install-recommends \
    clang \
    libclang-dev \
    make \
    meson \
    ninja-build \
    pkg-config \
    python3-pyelftools

# MLX5 dependencies (dynamic linking required due to GPL license).
RUN apt install -y --no-install-recommends \
    libnuma-dev \
    libmlx5-1 \
    libibverbs1 \
    libibverbs-dev

# Build DPDK with static libraries.
WORKDIR /build/dpdk
RUN meson setup \
    --prefix=/usr/local \
    --libdir=lib64 \
    -Ddefault_library=static \
    -Ddisable_drivers=net/mlx4 \
    -Ddisable_apps=* \
    -Denable_apps= \
    -Dtests=false \
    build

RUN ninja -C build
RUN ninja -C build install

# Update library paths for linker.
RUN echo "/usr/local/lib64" > /etc/ld.so.conf.d/dpdk.conf && ldconfig

WORKDIR /
RUN rm -rf /build/dpdk /build/*.tar.xz

# Install Rust.
RUN curl -f -sSf https://sh.rustup.rs | bash -s -- -y --default-toolchain none
RUN /root/.cargo/bin/rustup toolchain install $RUST_VERSION --profile minimal -c clippy -c rustfmt

# Install cargo-deb for debian package building.
RUN /root/.cargo/bin/cargo install cargo-deb

# Copy source code.
WORKDIR /src
COPY . .

# Verify versions.
RUN clang --version
RUN /root/.cargo/bin/cargo --version

# Build with DPDK support.
RUN /root/.cargo/bin/cargo build --release --features=dpdk

# Run clippy.
RUN /root/.cargo/bin/cargo clippy --features=dpdk -- -D warnings

# Build debian package.
RUN /root/.cargo/bin/cargo deb -p dwd --no-build
