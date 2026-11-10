# Multi-target build (auth design §9 rollout 6b): one builder stage,
# two runtime images selected with --target:
#   csi-driver → ghcr.io/<owner>/skynbd-csi-driver
#   skynbdd    → ghcr.io/<owner>/skynbdd
# Built by .github/workflows/images.yml for linux/amd64 and linux/arm64,
# and by the k3s smoke jobs for amd64.
#
# The builder always runs on the build host's architecture
# ($BUILDPLATFORM) and cross-compiles for arm64 — only the small runtime
# apt layers run under QEMU, not the Rust build.
#
# Layering is arranged for the GHA layer cache (type=gha, scopes in
# images.yml and the k3s-cluster action): everything above the final
# `COPY . .` — toolchain, apt, cargo-chef's dependency cook — is keyed
# on the manifests alone, so a source-only change (every PR) recompiles
# just the workspace crates instead of the whole release dep graph.

FROM --platform=$BUILDPLATFORM rust:1-bookworm AS chef
# The install layer caches until the base image moves. (Runs before the
# toolchain pin below on purpose: chef is a host binary, any toolchain
# will do.)
RUN cargo install cargo-chef --locked
WORKDIR /src
# Pin the toolchain FIRST, in its own layer. rust-toolchain.toml says
# `channel = "stable"`, and rustup resolves that per invocation — so
# when the file only arrived with the final `COPY . .`, the last RUN
# installed a *different* stable than every earlier layer used: the
# arm64 target added earlier was missing from it (the E0463 that broke
# main), and the cook layer's artifacts were silently invalid on every
# arch (different compiler). Installing here makes one toolchain span
# target-add, cook, and build alike.
COPY rust-toolchain.toml ./
RUN rustup toolchain install

# The recipe is a manifest fingerprint: it changes only when the
# Cargo.toml/Cargo.lock shape changes, never on source edits — which is
# what keeps the cook layer below warm across PRs.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
ARG TARGETARCH
# The CSI driver's build.rs carries its own protoc (protoc-bin-vendored).
# The driver links the agent crate's "dm" feature (thin-stack staging,
# CSI design 6h), which needs libdevmapper/libudev headers — multiarch
# apt provides the arm64 copies for the cross build. cmake covers
# aws-lc-sys in skynbdd's s3 dependency graph.
#
# devicemapper 0.34 takes devicemapper-sys as both a normal and a build
# dependency, so the cross build compiles it twice: once for aarch64 and
# once for the build host. The host copy probes pkg-config with
# HOST == TARGET, which never consults PKG_CONFIG_PATH_aarch64_*, and it
# links libdevmapper for real — so the arm64 branch needs the native dev
# packages next to the :arm64 ones, not instead of them.
RUN set -eux; \
    if [ "$TARGETARCH" = "arm64" ]; then \
        rustup target add aarch64-unknown-linux-gnu; \
        dpkg --add-architecture arm64; \
        apt-get update; \
        apt-get install -y --no-install-recommends \
            gcc-aarch64-linux-gnu libc6-dev-arm64-cross cmake pkg-config \
            libclang-dev libdevmapper-dev libudev-dev \
            libdevmapper-dev:arm64 libudev-dev:arm64; \
    else \
        apt-get update; \
        apt-get install -y --no-install-recommends \
            cmake pkg-config libclang-dev libdevmapper-dev libudev-dev; \
    fi

# Dependency cook: compiles the release dep graph from the recipe alone.
# Flags must mirror the real build below — a mismatch quietly recompiles
# everything there.
COPY --from=planner /src/recipe.json recipe.json
RUN set -eux; \
    if [ "$TARGETARCH" = "arm64" ]; then \
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
               CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
               AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
               PKG_CONFIG_ALLOW_CROSS=1 \
               PKG_CONFIG_PATH_aarch64_unknown_linux_gnu=/usr/lib/aarch64-linux-gnu/pkgconfig; \
        cargo chef cook --release --recipe-path recipe.json \
            --target aarch64-unknown-linux-gnu \
            -p skynbd-csi-driver -p skynbdd --features skynbdd/s3; \
    else \
        cargo chef cook --release --recipe-path recipe.json \
            -p skynbd-csi-driver -p skynbdd --features skynbdd/s3; \
    fi

COPY . .
# The cook above compiled the workspace skeleton with cargo-chef's stub
# build.rs, recording an empty OUT_DIR for skynbd-csi-driver. COPY keeps
# the context's checkout mtimes — older than the cook layer's artifacts —
# and on the --target build cargo then trusts the stub's build-script run:
# include_proto! finds no csi.v1.rs (what broke main's arm64 leg after
# the toolchain pin fixed E0463). Touching build.rs makes it newer than
# its cooked artifact, forcing a recompile and re-run of the real script
# on every arch; registry deps don't key on source mtimes, so the cook
# stays warm.
RUN find bins crates -name build.rs -exec touch {} +
RUN set -eux; \
    if [ "$TARGETARCH" = "arm64" ]; then \
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
               CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
               AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
               PKG_CONFIG_ALLOW_CROSS=1 \
               PKG_CONFIG_PATH_aarch64_unknown_linux_gnu=/usr/lib/aarch64-linux-gnu/pkgconfig; \
        rustup target add aarch64-unknown-linux-gnu; \
        cargo build --release --target aarch64-unknown-linux-gnu \
            -p skynbd-csi-driver -p skynbdd --features skynbdd/s3; \
        install -D target/aarch64-unknown-linux-gnu/release/skynbd-csi-driver \
            target/aarch64-unknown-linux-gnu/release/skynbdd -t /out; \
    else \
        cargo build --release \
            -p skynbd-csi-driver -p skynbdd --features skynbdd/s3; \
        install -D target/release/skynbd-csi-driver \
            target/release/skynbdd -t /out; \
    fi

# ---------------------------------------------------------------- csi-driver
FROM debian:bookworm-slim AS csi-driver
# nbd-client's TLS mode leaves a daemonized GnuTLS proxy per attached
# device — tini adopts and reaps those.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        nbd-client e2fsprogs xfsprogs util-linux ca-certificates tini kmod \
        libdevmapper1.02.1 libudev1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/skynbd-csi-driver /usr/local/bin/skynbd-csi-driver
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/skynbd-csi-driver"]

# ------------------------------------------------------------------- skynbdd
FROM debian:bookworm-slim AS skynbdd
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 skynbd
COPY --from=build /out/skynbdd /usr/local/bin/skynbdd
USER 10001
ENTRYPOINT ["/usr/local/bin/skynbdd"]
