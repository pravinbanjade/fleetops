.PHONY: dev dev-server dev-web web test test-docker test-web lint fmt build openapi

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

# Integration tests against the Docker daemon of the current docker context.
test-docker:
	DOCKER_HOST=$$(docker context inspect --format '{{.Endpoints.docker.Host}}') \
		sh -c 'cargo test -p fleetops-docker --features docker-it --test docker_it && cargo test -p fleetops-server --features docker-it --test live'

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
