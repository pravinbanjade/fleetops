# Contributing to FleetOps

## Licensing

- Code outside `ee/` is Apache-2.0. Sign off each commit (`git commit -s`) to certify the [Developer Certificate of Origin](https://developercertificate.org/).
- Code under `ee/` is under the [FleetOps Enterprise License](ee/LICENSE). By contributing there you license your change under those terms.
- Code outside `ee/` reaches Enterprise code only through its declared seams; `make check-editions` (run in CI) enforces this. Enterprise features plug in through `fleetops_core::edition::Entitlements`, `fleetops_api::app_with` and the UI slots in `web/src/ee-contract.ts`.

## Checks

`make lint test test-web check-editions`, and `make e2e` end to end. There is one build: it runs as Community until a license is installed.
