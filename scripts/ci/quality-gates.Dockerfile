# Quality-gates image for the `quality gates` job (.github/workflows/rust.yml).
#
# Cached on Docker Hub content-addressed by dependency inputs (see the job's
# key): a cache hit pulls this image and runs the gates inside it; a miss
# rebuilds here and pushes, then runs the gates. Pinned tool versions come
# from scripts/ci/lib/rust-quality-gates.py via --provision-only, so the
# image and the run cannot disagree; only the base stable toolchain floats
# (same as the dtolnay stable toolchain this replaces, lagged by rebuilds).
FROM rust:slim-trixie

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_TERM_COLOR=always \
    CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:$PATH

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        libssl-dev \
        pkg-config \
        python3 \
    && rm -rf /var/lib/apt/lists/*

# The CI workspace mounts at /work owned by the host user; mark it safe so
# git works as root inside the container (else "dubious ownership" errors).
RUN git config --global --add safe.directory /work

# Provision every pinned gate tool. Prebuilt downloads that cannot execute
# on this libc compile here once (see ensure_tool), so runtime provisioning
# inside CI is always a fast no-op.
COPY scripts/ci/lib/ /tmp/gates-lib/
RUN mkdir -p /tmp/prov-work \
    && python3 /tmp/gates-lib/rust-quality-gates.py /tmp/prov-work --provision-only \
    && rm -rf /tmp/prov-work /tmp/gates-lib /usr/local/cargo/registry

# Warm the registry cache for the workspace dependencies. Cargo requires at
# least one target to resolve a manifest, hence the dummy main.
COPY rust/Cargo.toml rust/Cargo.lock /tmp/fetch/rust/
RUN mkdir -p /tmp/fetch/rust/src && touch /tmp/fetch/rust/src/main.rs \
    && cd /tmp/fetch/rust && cargo fetch && rm -rf /tmp/fetch /usr/local/cargo/registry/src

WORKDIR /work
