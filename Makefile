.PHONY: dev dev-server dev-web web test test-docker kind-up test-kube e2e-kube agent-kube-smoke test-web e2e rss-bench lint fmt build image image-agent image-smoke agent-smoke openapi ldap-up test-ldap minio-up test-s3 test-ee test-web-ee web-ee build-ee e2e-ee check-editions image-ee

# The API on :9000 and the Vite dev server on :5173, which proxies /api to it.
dev:
	$(MAKE) -j2 dev-server dev-web

dev-web:
	cd web && pnpm install --frozen-lockfile
	cd web && pnpm dev

dev-server:
	DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null)} \
		cargo run -p fleetops-server -- --data ./.dev-data --bind 127.0.0.1:9000

# Community crates (default-members); make test-ee covers ee/.
test:
	cargo test

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
e2e: build
	cargo build --release -p fleetops-agent
	cd web && pnpm exec playwright install chromium
	cd web && DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}')} \
		FLEETOPS_BIN=$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops \
		FLEETOPS_AGENT_BIN=$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops-agent \
		pnpm exec playwright test

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
	cargo clippy -p fleetops-server -p fleetops-ee -p fleetops-license --features fleetops-server/enterprise --all-targets -- -D warnings

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

# ---- Enterprise edition (ee/LICENSE) ----------------------------------------

EE_FEATURES = --features fleetops-server/enterprise
EE_IMAGE ?= fleetops-ee:dev

# The OpenLDAP container the LDAP tests sign in against (127.0.0.1:3389).
ldap-up:
	scripts/ldap-up.sh

test-ldap: ldap-up
	FLEETOPS_LDAP_IT=ldap://127.0.0.1:3389 cargo test -p fleetops-ee --features ldap-it --test ldap_it

# The MinIO container the S3 backup tests write to (127.0.0.1:9100).
minio-up:
	scripts/minio-up.sh

test-s3: minio-up
	FLEETOPS_S3_IT=http://127.0.0.1:9100 cargo test -p fleetops-ee --features s3-it --test s3_it

# The Enterprise crates, and the server built with them.
test-ee:
	cargo test -p fleetops-ee -p fleetops-license
	cargo test -p fleetops-ee --features test-license-key keys::
	cargo test -p fleetops-server $(EE_FEATURES)

test-web-ee:
	cd web && pnpm install --frozen-lockfile
	cd web && pnpm typecheck:ee
	cd web && pnpm test:ee

web-ee:
	cd web && pnpm install --frozen-lockfile
	cd web && pnpm build:ee

build-ee: web-ee
	cargo build --release -p fleetops-server $(EE_FEATURES)

# The Enterprise Playwright suite, on a build that trusts the test key.
e2e-ee: web-ee
	cargo build --release -p fleetops-server --features fleetops-server/ee-test-license-key
	cd web && pnpm exec playwright install chromium
	cd web && DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}')} \
		FLEETOPS_BIN=$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops \
		pnpm exec playwright test -c playwright.ee.config.ts

# Community code must not reach into ee/ (spec §6).
check-editions: web
	scripts/check-editions.sh

image-ee:
	docker buildx build --platform $(PLATFORMS) --build-arg EDITION=ee --target server --tag $(EE_IMAGE) --load .
