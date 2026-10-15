# Contributing to FleetOps

## Licensing

- Code outside `ee/` is Apache-2.0. Sign off each commit (`git commit -s`) to certify the [Developer Certificate of Origin](https://developercertificate.org/).
- Code under `ee/` is under the [FleetOps Enterprise License](ee/LICENSE). By contributing there you license your change under those terms.
- Community code never imports Enterprise code. `make check-editions` (run in CI) enforces this. Enterprise features plug in through `fleetops_core::edition::Entitlements`, `fleetops_api::app_with` and the UI slots in `web/src/ee-contract.ts`.

## Checks

`make lint test test-web` for Community, `make test-ee test-web-ee` for Enterprise, `make e2e` and `make e2e-ee` end to end.
