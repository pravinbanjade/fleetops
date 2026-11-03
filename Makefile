.PHONY: dev dev-server dev-web web test test-docker kind-up test-kube e2e-kube agent-kube-smoke test-web e2e rss-bench rss-bench-scan lint fmt build image image-agent image-smoke agent-smoke openapi ldap-up test-ldap minio-up test-s3 check-editions

# The API on :9000 and the Vite dev server on :5173, which proxies /api to it.
dev:
	$(MAKE) -j2 dev-server dev-web

dev-web:
	cd web && pnpm install --frozen-lockfile
	cd web && pnpm dev

dev-server:
	DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null)} \
		cargo run -p fleetops-server -- --data ./.dev-data --bind 127.0.0.1:9000

# Every crate (default-members), and the license-key tests that need the
# committed test key.
test:
	cargo test
	cargo test -p fleetops-ee --features test-license-key keys::

# Typecheck, lint and unit tests for the web UI, and a check that the
# committed API client matches the committed OpenAPI document.
test-web:
	cd web && pnpm install --frozen-lockfile
	cd web && pnpm typecheck
	cd web && pnpm lint
	cd web && pnpm test
	cd web && pnpm check:api

# Integration tests against the Docker daemon of the current docker context;
# the Docker suite runs twice, the second time through an agent tunnel.
test-docker:
	DOCKER_HOST=$$(docker context inspect --format '{{.Endpoints.docker.Host}}') \
		sh -c 'cargo test -p fleetops-docker --features docker-it --test docker_it && \
		FLEETOPS_IT_TUNNEL=1 cargo test -p fleetops-docker --features docker-it --test docker_it && \
		cargo test -p fleetops-server --features docker-it --test live'

# The Playwright end-to-end test: a release build with the UI embedded,
# driven in Chromium against the current docker context's daemon. It pulls
# busybox and creates (then removes) containers named fleetops-e2e-*; the
# second spec enrolls a real fleetops-agent against the same daemon.
e2e: web
	cargo build --release -p fleetops-server --features fleetops-server/ee-test-license-key
	cargo build --release -p fleetops-agent
	cd web && pnpm exec playwright install chromium
	cd web && DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}')} \
		FLEETOPS_BIN=$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops \
		FLEETOPS_AGENT_BIN=$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops-agent \
		pnpm exec playwright test
	cd web && DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}')} \
		FLEETOPS_BIN=$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops \
		pnpm exec playwright test -c playwright.license.config.ts

# The idle memory targets: a release build managing 50 containers on the
# current docker context's daemon stays under 50 MiB of RSS, plus 1 MiB per
# agent with 10 agents connected; one idle agent stays under 15 MiB.
KUBE_IT ?= $(CURDIR)/target/kube-it

kind-up:
	scripts/kind-up.sh $(KUBE_IT)

test-kube: kind-up
	PATH=$(CURDIR)/scripts:$$PATH FLEETOPS_KUBE_IT=$(KUBE_IT) cargo test -p fleetops-kube --features kube-it --test kube_it --test helm_it -- --test-threads=4
	PATH=$(CURDIR)/scripts:$$PATH FLEETOPS_KUBE_IT=$(KUBE_IT) FLEETOPS_IT_TUNNEL=1 cargo test -p fleetops-kube --features kube-it --test kube_it --test helm_it -- --test-threads=4

# Set to a kind fixtures directory (make kind-up) to add the kube phase.
KUBE_IT_RSS ?=

rss-bench: build
	cargo build --release -p fleetops-agent
	KUBE_IT="$(KUBE_IT_RSS)" scripts/rss-bench.sh $(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops \
		$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops-agent

# The idle memory target with image scanning on (Enterprise, test license):
# pulls Trivy and its database on the current docker context's daemon.
rss-bench-scan: web
	cargo build --release -p fleetops-server --features fleetops-server/ee-test-license-key
	SCAN=1 AGENTS=0 scripts/rss-bench.sh $(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops

e2e-kube: build kind-up
	cd web && pnpm exec playwright install chromium
	cd web && FLEETOPS_KIND=1 FLEETOPS_KUBE_IT=$(KUBE_IT) \
		FLEETOPS_BIN=$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops \
		pnpm exec playwright test e2e/kube.spec.ts

agent-kube-smoke: kind-up
	scripts/agent-kube-smoke.sh $(IMAGE) $(AGENT_IMAGE)

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

# Builds the web UI into web/dist, which the server binary embeds.
web:
	cd web && pnpm install --frozen-lockfile
	cd web && pnpm build

build: web
	cargo build --release -p fleetops-server

# Regenerates the committed OpenAPI document after an API change, and the
# web client from it; `cargo test` and `make test-web` fail until both match.
openapi:
	cargo run -q -p fleetops-server -- --openapi > crates/fleetops-api/openapi.json
	cd web && pnpm gen:api

# The container image: a static musl binary on distroless/static, for
# linux/amd64 and linux/arm64, built entirely inside BuildKit (no local zig
# or cross toolchain). It is loaded into the local image store as $(IMAGE);
# a multi-platform load needs Docker's containerd image store. Build one
# platform with `make image PLATFORMS=linux/amd64`.
IMAGE ?= fleetops:dev
PLATFORMS ?= linux/amd64,linux/arm64

image:
	docker buildx build --platform $(PLATFORMS) --target server --tag $(IMAGE) --load .

# The agent image, from the same build as the server's.
AGENT_IMAGE ?= fleetops-agent:dev

image-agent:
	docker buildx build --platform $(PLATFORMS) --target agent --tag $(AGENT_IMAGE) --load .

# Runs $(IMAGE) against the local daemon's socket and checks that it
# serves the UI and reaches Docker as its non-root user.
image-smoke:
	scripts/image-smoke.sh $(IMAGE)

# Runs $(IMAGE) and $(AGENT_IMAGE) together on the local daemon and checks
# that the agent enrolls and its environment reaches Docker.
agent-smoke:
	scripts/agent-smoke.sh $(IMAGE) $(AGENT_IMAGE)

# ---- LDAP and S3 integration tests (ee/) -------------------------------------

# The OpenLDAP container the LDAP tests sign in against (127.0.0.1:3389).
ldap-up:
	scripts/ldap-up.sh

test-ldap: ldap-up
	FLEETOPS_LDAP_IT=ldap://127.0.0.1:3389 cargo test -p fleetops-ee --features ldap-it --test ldap_it

# The S3-compatible container the backup tests write to (127.0.0.1:9100).
minio-up:
	scripts/minio-up.sh

test-s3: minio-up
	FLEETOPS_S3_IT=http://127.0.0.1:9100 cargo test -p fleetops-ee --features s3-it --test s3_it

# The Enterprise Playwright suite, on a build that trusts the test key.
# Code outside ee/ (Apache-2.0) reaches ee/ only through its declared seams.
check-editions:
	scripts/check-editions.sh
