# Development

See [CONTRIBUTING.md](../CONTRIBUTING.md) for licensing and sign-off. This page covers the toolchain, the make targets and how the tests and CI run.

## Requirements

| Tool | Version |
|---|---|
| Rust | stable, 1.86 or newer (`rust-toolchain.toml` pins stable with rustfmt and clippy) |
| Node.js | 24 or newer |
| pnpm | 12 (`packageManager` in `web/package.json`) |
| Docker or Podman | A reachable Docker-compatible socket |
| git | The CLI, for the git-backed stack tests: they build fixture repositories and serve them with `git http-backend` |

## Run it locally

```sh
make dev          # API on :9000 plus the Vite dev server on :5173 (proxies /api and the websocket)
```

Open <http://localhost:5173>. `make dev-server` keeps its data in `./.dev-data`, which git ignores. It reads `DOCKER_HOST` from the environment, or from the current docker context when the variable is unset.

## Make targets

| Target | What it does |
|---|---|
| `make dev` | Runs `dev-server` and `dev-web` together |
| `make test` | `cargo test --workspace`, which uses mocks and needs no Docker |
| `make test-docker` | Integration tests against the current docker context's daemon (`docker-it` feature); the Docker suite runs a second time through an agent tunnel |
| `make test-web` | Web typecheck, lint, vitest, and a check that the generated API client matches the OpenAPI document |
| `make e2e` | `make build` and the agent, then the Playwright end-to-end tests in Chromium against the current docker context's daemon |
| `make rss-bench` | `make build`, then checks idle RSS stays under 50 MiB with 50 containers, under 50 MiB + 1 MiB per agent with 10 agents, and under 15 MiB per agent (`COUNT`, `LIMIT_MIB`, `AGENTS`, `AGENT_LIMIT_MIB` override). With `KUBE_IT_RSS=target/kube-it`, it also checks the server stays under 100 MiB (`KUBE_LIMIT_MIB`) with 5 kubeconfig clusters being listed and watched |
| `make test-kube` | Creates or reuses the `kind` cluster `fleetops-test` (`brew install kind`), then runs the Kubernetes suite through a kubeconfig and again through the agent tunnel |
| `make e2e-kube` | The Playwright Kubernetes test against that cluster |
| `make agent-kube-smoke` | Loads `$(AGENT_IMAGE)` into the cluster, runs `$(IMAGE)` on kind's network, applies the generated manifest, and checks that the cluster environment comes up |
| `make lint` | `cargo fmt --check` and `clippy -D warnings` |
| `make fmt` | `cargo fmt --all` |
| `make web` | Builds the UI into `web/dist` |
| `make build` | `make web`, then a release build of the `fleetops` binary |
| `make image` | Builds the `fleetops:dev` container image for linux/amd64 and linux/arm64 (`IMAGE` and `PLATFORMS` override) |
| `make image-agent` | Builds the `fleetops-agent:dev` image (`AGENT_IMAGE` and `PLATFORMS` override) |
| `make image-smoke` | Runs `$(IMAGE)` against the local daemon and checks the UI, the non-root user and Docker access |
| `make agent-smoke` | Runs `$(IMAGE)` and `$(AGENT_IMAGE)` together and checks that the agent enrolls and its environment reaches Docker |
| `make openapi` | Regenerates `crates/fleetops-api/openapi.json` and the TypeScript client after an API change |

## Notes

- **Building without the UI.** `cargo build` works without `web/dist`. The server's `build.rs` writes a placeholder page, and the API still works under `/api/v1`.
- **After changing the API,** run `make openapi` and commit both generated files. `cargo test` and `make test-web` fail until the committed document and client match the code.
- **The integration tests** (`make test-docker`) create and remove their own labelled resources. Run them against a daemon where that's acceptable.
- **The end-to-end test** (`make e2e`) starts the server on `127.0.0.1:9123` with an empty data directory (`web/e2e-data`). It pulls `busybox:1.37`, and creates and then removes containers named `fleetops-e2e-*`; the second spec enrolls a real `fleetops-agent` against the same daemon. It leaves the image in place. On failure, `cd web && pnpm exec playwright show-trace` opens the trace saved under `web/test-results`.
- **CI** (`.github/workflows/ci.yml`) runs on pushes to `main` and on pull requests: `make lint test`, `make test-ee`, `make test-web`, `make test-web-ee`, `make check-editions`, `make test-docker`, `make e2e`, `make e2e-ee`, `make rss-bench`, the kind suite (`make test-kube e2e-kube`, with the server's footprint while listing clusters), `make agent-kube-smoke` with real images, and the Community, Enterprise and agent images built for both platforms followed by `make image-smoke` and `make agent-smoke`. Jobs run on GitHub's Ubuntu runners and use their Docker daemon; `.github/actions/setup` installs the toolchain, and kind, kubectl and helm at pinned versions.
- **Releases** (`.github/workflows/release.yml`) run on a `vX.Y.Z` tag that matches the workspace version in `Cargo.toml`. They push the Community, Enterprise and agent images (`docker.io/fleets/fleetops`, `fleetops-ee` and `fleetops-agent`, tagged `X.Y.Z`, `X.Y` and `latest`) to Docker Hub with the `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` secrets of the `Production` environment, and create a GitHub release with static linux binaries of each and SHA-256 checksums.
- **TDD.** New behaviour starts with a failing test.
