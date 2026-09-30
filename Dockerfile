# syntax=docker/dockerfile:1
#
# The FleetOps image: one static musl binary, with the web UI embedded, on
# distroless/static, running as a non-root user. Build it with `make image`.
#
# Every build stage runs on the build machine's own platform. The binary is
# cross-compiled for each target platform with cargo-zigbuild, so a
# multi-arch build needs no emulation.

ARG RUST_VERSION=1.98
ARG NODE_VERSION=24
ARG ZIGBUILD_VERSION=0.23.4

# The web UI, which the server binary embeds at compile time.
FROM --platform=$BUILDPLATFORM node:${NODE_VERSION}-trixie-slim AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

# zig and cargo-zigbuild, taken from the cargo-zigbuild project's image.
FROM --platform=$BUILDPLATFORM ghcr.io/rust-cross/cargo-zigbuild:${ZIGBUILD_VERSION} AS zigbuild

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim-trixie AS build
COPY --from=zigbuild /usr/local/cargo/bin/cargo-zigbuild /usr/local/cargo/bin/
COPY --from=zigbuild /usr/local/zig-* /usr/local/zig/
ENV PATH=/usr/local/zig:$PATH
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY --from=web /src/web/dist web/dist
ARG TARGETARCH
# Each platform gets its own cache mounts. The two builds run at once, and
# cargo's package lock lives outside the registry mount, so a shared
# registry lets both unpack the same crate at the same time.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=fleetops-cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/src/target,id=fleetops-target-${TARGETARCH} \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-musl ;; \
      arm64) target=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac && \
    # The target cache is shared by every checkout that builds on this
    # builder, and cargo judges workspace crates fresh by mtime. Touching
    # them makes cargo rebuild this checkout's crates (dependencies stay
    # cached) instead of reusing another checkout's newer build.
    find crates -type f -exec touch {} + && \
    cargo zigbuild --locked --release -p fleetops-server --target "$target" && \
    cp "target/$target/release/fleetops" /fleetops
# The data directory, owned by distroless's nonroot user (65532).
RUN mkdir -p /data

FROM gcr.io/distroless/static-debian13:nonroot
COPY --from=build /fleetops /fleetops
COPY --from=build --chown=65532:65532 /data /data
VOLUME /data
EXPOSE 9000
USER nonroot
ENTRYPOINT ["/fleetops"]
