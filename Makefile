.PHONY: dev-server test test-docker lint fmt build

dev-server:
	DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null)} \
		cargo run -p fleetops-server -- --data ./.dev-data --bind 127.0.0.1:9000

test:
	cargo test --workspace

# Integration tests against the Docker daemon of the current docker context.
test-docker:
	DOCKER_HOST=$$(docker context inspect --format '{{.Endpoints.docker.Host}}') \
		sh -c 'cargo test -p fleetops-docker --features docker-it --test docker_it && cargo test -p fleetops-server --features docker-it --test live'

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

build:
	cargo build --release -p fleetops-server
