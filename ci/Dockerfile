# The image every CI job runs in: Rust, Node and pnpm, Playwright's system
# libraries, kind, kubectl and helm (tests only), and a Docker daemon the job starts itself
# (ci/start-docker.sh). The daemon runs in the job's own container, so the
# socket, published ports on 127.0.0.1 and kind's API server are all local,
# as on a developer machine. GitLab's hosted runners are privileged, which
# the daemon needs.
#
# .gitlab-ci.yml builds it at the start of every pipeline; unchanged layers
# come from the registry cache.

ARG RUST_VERSION=1.98
ARG NODE_VERSION=24
ARG DOCKER_VERSION=29

FROM node:${NODE_VERSION}-trixie-slim AS node
FROM docker:${DOCKER_VERSION}-dind AS docker

FROM rust:${RUST_VERSION}-trixie
ARG KIND_VERSION=0.33.0
ARG KUBECTL_VERSION=1.37.1
# Tests only (the Helm interop suite); no image ships helm.
ARG HELM_VERSION=3.19.0
ARG PLAYWRIGHT_VERSION=1.63.0

RUN apt-get update && \
    apt-get install -y --no-install-recommends iptables procps python3 xz-utils && \
    rm -rf /var/lib/apt/lists/*

# rust-toolchain.toml asks for the stable channel by name.
RUN rustup toolchain install stable --profile minimal --component rustfmt,clippy && \
    rustup default stable

COPY --from=node /usr/local/bin/node /usr/local/bin/
COPY --from=node /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm && \
    ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx && \
    ln -s ../lib/node_modules/corepack/dist/corepack.js /usr/local/bin/corepack && \
    corepack enable
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0

# The static Docker binaries (dockerd, containerd, runc, the CLI and its
# buildx plugin) and the dind wrapper that sets up nested cgroups.
COPY --from=docker /usr/local/bin/ /usr/local/bin/
COPY --from=docker /usr/local/libexec/docker/cli-plugins /usr/local/libexec/docker/cli-plugins
VOLUME /var/lib/docker

# A stalled connection retries rather than waiting out curl's five-minute
# default.
RUN arch=$(dpkg --print-architecture) && \
    fetch() { curl -fsSL --connect-timeout 20 --retry 5 --retry-all-errors -o "$1" "$2"; } && \
    fetch /usr/local/bin/kind "https://kind.sigs.k8s.io/dl/v${KIND_VERSION}/kind-linux-${arch}" && \
    fetch /usr/local/bin/kubectl "https://dl.k8s.io/release/v${KUBECTL_VERSION}/bin/linux/${arch}/kubectl" && \
    fetch /tmp/helm.tgz "https://get.helm.sh/helm-v${HELM_VERSION}-linux-${arch}.tar.gz" && \
    tar -xzf /tmp/helm.tgz -C /tmp && mv "/tmp/linux-${arch}/helm" /usr/local/bin/helm && \
    rm -rf /tmp/helm.tgz "/tmp/linux-${arch}" && \
    chmod +x /usr/local/bin/kind /usr/local/bin/kubectl

# Chromium's system libraries; the browser itself is cached per project.
RUN npx -y "playwright@${PLAYWRIGHT_VERSION}" install-deps chromium && \
    rm -rf /var/lib/apt/lists/* /root/.npm
