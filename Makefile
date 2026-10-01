.PHONY: dev dev-server dev-web web test test-docker test-web e2e lint fmt build image image-smoke openapi

# The API on :9000 and the Vite dev server on :5173, which proxies /api to it.
dev:
	$(MAKE) -j2 dev-server dev-web

dev-web:
	pnpm -C web install --frozen-lockfile
	pnpm -C web dev

dev-server:
	DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null)} \
		cargo run -p fleetops-server -- --data ./.dev-data --bind 127.0.0.1:9000

test:
	cargo test --workspace

# Typecheck, lint and unit tests for the web UI, and a check that the
# committed API client matches the committed OpenAPI document.
test-web:
	pnpm -C web install --frozen-lockfile
	pnpm -C web typecheck
	pnpm -C web lint
	pnpm -C web test
	pnpm -C web check:api

# Integration tests against the Docker daemon of the current docker context;
# the Docker suite runs twice, the second time through an agent tunnel.
test-docker:
	DOCKER_HOST=$$(docker context inspect --format '{{.Endpoints.docker.Host}}') \
		sh -c 'cargo test -p fleetops-docker --features docker-it --test docker_it && \
		FLEETOPS_IT_TUNNEL=1 cargo test -p fleetops-docker --features docker-it --test docker_it && \
		cargo test -p fleetops-server --features docker-it --test live'

# The Playwright end-to-end test: a release build with the UI embedded,
# driven in Chromium against the current docker context's daemon. It pulls
# busybox and creates (then removes) one container named fleetops-e2e-*.
e2e: build
	pnpm -C web exec playwright install chromium
	DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}')} \
		FLEETOPS_BIN=$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops \
		pnpm -C web exec playwright test

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

# Builds the web UI into web/dist, which the server binary embeds.
web:
	pnpm -C web install --frozen-lockfile
	pnpm -C web build

build: web
	cargo build --release -p fleetops-server

# Regenerates the committed OpenAPI document after an API change, and the
# web client from it; `cargo test` and `make test-web` fail until both match.
openapi:
	cargo run -q -p fleetops-server -- --openapi > crates/fleetops-api/openapi.json
	pnpm -C web gen:api

# The container image: a static musl binary on distroless/static, for
# linux/amd64 and linux/arm64, built entirely inside BuildKit (no local zig
# or cross toolchain). It is loaded into the local image store as $(IMAGE);
# a multi-platform load needs Docker's containerd image store. Build one
# platform with `make image PLATFORMS=linux/amd64`.
IMAGE ?= fleetops:dev
PLATFORMS ?= linux/amd64,linux/arm64

image:
	docker buildx build --platform $(PLATFORMS) --tag $(IMAGE) --load .

# Runs $(IMAGE) against the local daemon's socket and checks that it
# serves the UI and reaches Docker as its non-root user.
image-smoke:
	scripts/image-smoke.sh $(IMAGE)
