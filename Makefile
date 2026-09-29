.PHONY: dev-server test lint fmt build

dev-server:
	cargo run -p dockerer-server -- --data ./.dev-data --bind 127.0.0.1:9000

test:
	cargo test --workspace

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

build:
	cargo build --release -p dockerer-server
