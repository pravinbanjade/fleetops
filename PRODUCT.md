# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

**Primary: ops and DevOps engineers on small and mid-size teams.** They run shared Docker hosts at work, and later Kubernetes clusters and multiple hosts. They use FleetOps often during the working day for routine changes: deploying, restarting, pulling images and checking resource use. They also use it under pressure during incidents, reading logs, opening a shell in a container and finding what changed. Team needs follow from this: accounts, roles, and an audit trail of who did what.

Other roles exist in the product and are secondary:

- **Admins** set up the server, users and environments.
- **Operators and viewers** are the roles planned for per-environment RBAC in slice 3c.

## Product Purpose

FleetOps is an open-source container management platform and an alternative to Portainer. It is one Rust binary with the web UI and a SQLite database embedded. Pointed at a Docker or Podman socket, it lets a team manage containers, images, volumes, networks and Compose projects entirely from the browser.

Success for sub-project 1, which is done:

- setup, login and full resource management from one binary;
- lists and detail pages that update live from Docker events;
- a log viewer that stays responsive at 100k+ lines;
- a terminal with resize;
- idle RSS under 50 MB on a host with about 50 containers.

## Positioning

**Real-time UX is the lead differentiator, and it wins when priorities compete.** FleetOps's claim is that the interface itself is better than Portainer's:

- event-driven lists with no polling or manual refresh;
- a first-class log viewer (virtualized, with search, resume without repeats, and ANSI colour);
- an honest terminal that never silently respawns a shell;
- live stats and sparklines.

The supporting differentiators, which must not be traded away lightly, are:

- a lightweight single binary (small image, low memory, fast start);
- no paywalled features: RBAC, SSO, GitOps and audit are free;
- Compose/GitOps-first stacks (sub-project 4).

## Operating Context

- It runs as a binary or a distroless container image next to the Docker daemon, reached over the socket.
- The same UI manages remote hosts through an agent (sub-project 2) and Kubernetes clusters (sub-project 3).
- It is used in a desktop browser during working sessions, often side by side with a terminal, an editor or an incident channel.
- **Roadmap:**
  1. core and local Docker (done);
  2. agent and multi-host (done);
  3. Kubernetes (done: core, live watches and YAML editing; next: 3c RBAC for Docker and Kubernetes with namespace scoping, 3d Helm);
  4. stacks and GitOps;
  5. teams and enterprise features (free).

## Capabilities and Constraints

- **Resources:** containers, including create, recreate with rollback, duplicate, logs, terminal, stats and files. Also images (pull and build with streamed progress), volumes, networks, Compose projects, a dashboard and a command palette.
- **Administration:** users, environments and the audit log.
- **Security constraints:**
  - Docker socket access is root-equivalent. Every Docker route needs a session, and mutations need CSRF.
  - Every exec is audited, so the terminal starts nothing until the user connects.
  - Secret-looking values are masked in the UI; this is display-only.
- **Real-time channel:** one websocket carries events, stats, logs, exec and task topics.
- **Frontend:**
  - React, TypeScript and Vite.
  - The API client is generated from OpenAPI.
  - A strict CSP allows self-only scripts.
  - Heavy libraries (xterm.js, uPlot) load lazily per tab.
- **Themes:** dark, light and system.
- **Terminology:** Docker's own terms are used exactly (container, image, volume, network, Compose project), as is "environment" for a managed Docker endpoint.
- **Undecided:**
  - hosting or publishing of the container image (no registry yet);
  - pricing or sponsorship model beyond "no paywalled features".

## Brand Commitments

- **The name "FleetOps" is final.**
- The only brand asset is `web/public/favicon.svg`.

## Evidence on Hand

- **Documentation:** `README.md`, the design spec (`docs/superpowers/specs/2026-09-29-fleetops-core-docker-design.md`) and one implementation plan per phase (`docs/superpowers/plans/`).
- **Tests:** Rust unit and integration tests, vitest component tests, and one Playwright end-to-end test of the core flow (`web/e2e/core-flow.spec.ts`).
- **Measured facts:** static binaries of 15–17 MB, and an image of about 8 MB compressed per platform.
- **None yet:**
  - no users, testimonials, case studies, adoption numbers or press;
  - no published benchmarks against Portainer, since the RSS target has not been measured.

Future work must not invent any of these.

## Product Principles

1. **Live by default.** The screen reflects Docker's current state without refresh. When it cannot, for example on a dropped connection or a stopped container, it says so plainly rather than showing stale data as current.
2. **Honest under pressure.** In an incident the UI must not surprise the user. There are no silent retries that start processes, no hidden destructive defaults, and Docker's own error messages are shown as they are.
3. **Weight is a feature.** Every addition earns its bundle size, memory and startup cost, and heavy tools load only when opened.
4. **Everything is free and auditable.** Team features are never gated. Every mutating action leaves an audit record.
5. **Built to grow past one host.** Environments, roles and the real-time channel are designed so that agents, Kubernetes and RBAC extend the product rather than fork it.

## Accessibility & Inclusion

- **Target: WCAG 2.2 AA.** This covers keyboard operability, contrast in both themes, visible focus, and accessible names and roles on every control.
- Tests find UI by role and name, and new UI should keep that working.
- Live regions are used deliberately. For example, the log view has `aria-live` turned off so a streaming log does not flood screen readers.
