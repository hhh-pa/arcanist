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
# System level: some tests override HOME, which would hide a global entry.
RUN git config --system --add safe.directory /work

# Provision every pinned gate tool. Prebuilt downloads that cannot execute
# on this libc compile here once (see ensure_tool), so runtime provisioning
# inside CI is always a fast no-op.
COPY scripts/ci/lib/ /tmp/gates-lib/
RUN mkdir -p /tmp/prov-work \
    && python3 /tmp/gates-lib/rust-quality-gates.py /tmp/prov-work --provision-only \
    && rm -rf /tmp/prov-work /tmp/gates-lib /usr/local/cargo/registry

# Prebuilt dependency tree ("builder" layer): compiling the workspace's
# dependencies once here means gate runs rebuild arc-rs itself only.
# CARGO_TARGET_DIR lives outside the /work mount so the prebuilt tree
# survives it; the gates script already honors CARGO_TARGET_DIR, and its
# llvm-cov seed copies this debug tree so coverage deps stay fresh too.
# The nightly udeps tree warms with the script's exact command and target
# dir (pin extracted from the script: no drift). The warm stub must COMPILE
# as a normal bin: an empty main.rs passes `cargo test --no-run` (the test
# harness supplies main) but fails udeps with E0601, hence `fn main() {}`.
# A nonzero udeps exit on the stub is EXPECTED (the stub uses no
# dependencies, so every dep reports unused): only a genuine build failure
# ("could not compile" in the log, or zero fingerprints) fails the image,
# otherwise the log stays quiet and the verdict stays a runtime one.
# Registry sources stay extracted: cargo fingerprints compare mtimes, and
# re-extraction would stale every prebuilt unit.
ENV CARGO_TARGET_DIR=/opt/target
COPY rust/Cargo.toml rust/Cargo.lock /tmp/warm/rust/
COPY scripts/ci/lib/rust-quality-gates.py /tmp/warm/pins.py
RUN mkdir -p /tmp/warm/rust/src && printf 'fn main() {}\n' > /tmp/warm/rust/src/main.rs \
    && cd /tmp/warm/rust && cargo fetch \
    && cargo test --no-run \
    && NIGHTLY=$(grep -E '^UDEPS_NIGHTLY = ' /tmp/warm/pins.py | cut -d'"' -f2) \
    && [ -n "$NIGHTLY" ] \
    && UDEPS_LOG=/tmp/warm/udeps.log \
    && { NIGHTLY_TARGET="${CARGO_TARGET_DIR}/nightly"; \
        CARGO_TARGET_DIR="$NIGHTLY_TARGET" cargo +"$NIGHTLY" udeps --all-targets >"$UDEPS_LOG" 2>&1; \
        STATUS=$?; \
        if [ $STATUS -ne 0 ] \
            && { grep -q "could not compile" "$UDEPS_LOG" \
                || [ -z "$(ls -A "$NIGHTLY_TARGET/debug/.fingerprint" 2>/dev/null)" ]; }; then \
            echo "udeps warm: nightly build FAILED:"; cat "$UDEPS_LOG"; exit 1; \
        fi; \
        echo "udeps warm: nightly tree prebuilt (udeps exit $STATUS; unused-deps verdict deferred to runtime)"; } \
    && rm -rf /tmp/warm

WORKDIR /work
