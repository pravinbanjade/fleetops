import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ImageDetail, ImageRow, SecuritySettings, SecuritySummary } from "@/api/gen/types.gen";
import { mockFetch } from "@/test/fetch";
import { ADMIN, ENVS, loggedIn, renderApp } from "@/test/app";

const row = (over: Partial<ImageRow>): ImageRow => ({
  image: "nginx:1.25.3",
  digests: 1,
  critical: 4,
  high: 11,
  critical_fixable: 4,
  high_fixable: 9,
  total: 40,
  running: 12,
  envs: ["local"],
  state: "scanned",
  has_result: true,
  last_scan_at: Math.floor(Date.now() / 1000) - 600,
  error: null,
  ...over,
});

const summary = (rows: ImageRow[], over: Partial<SecuritySummary> = {}): SecuritySummary => ({
  critical_fixable_running: 1,
  scanned: 1,
  failed: 0,
  unscanned: 0,
  rows,
  ...over,
});

const settingsOn: SecuritySettings = {
  global: { enabled: true, mirror: null, default_trivy_image: "aquasec/trivy:0.76.1" },
  envs: [{ env: "local", kind: "docker-local", enabled: null, effective_enabled: true, mirror: null, socket_path: null, status: { state: "ready" } }],
};

const routes = (s: SecuritySummary, extra: Record<string, unknown> = {}) => ({
  ...loggedIn,
  "GET /api/v1/edition": { json: { edition: "enterprise", features: ["vuln_scan"] } },
  "GET /api/v1/ee/security/images": { json: s },
  "GET /api/v1/ee/security/settings": { json: settingsOn },
  ...extra,
});

const table = () => screen.findByRole("table", { name: "Patch queue" });

describe("security page", () => {
  it("opens with Running only and Fix available on, and keeps them in the URL", async () => {
    const calls = mockFetch(routes(summary([row({})])));
    const { history } = renderApp("/security");
    await table();
    expect(screen.getByRole("checkbox", { name: "Running only" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Fix available" })).toBeChecked();
    const first = calls.find((c) => c.path.startsWith("/api/v1/ee/security/images"));
    expect(first?.path).toContain("running=true");
    expect(first?.path).toContain("fixable=true");
    await userEvent.click(screen.getByRole("checkbox", { name: "Fix available" }));
    await waitFor(() => expect(history.location.search).toContain("fixable=false"));
    await waitFor(() => expect(calls.some((c) => c.path.includes("fixable=false"))).toBe(true));
  });

  it("always shows failed and unscanned counts", async () => {
    mockFetch(routes(summary([row({})], { failed: 0, unscanned: 0 })));
    renderApp("/security");
    const figures = await screen.findByRole("group", { name: "Totals" });
    expect(within(figures).getByText("scan failed")).toBeInTheDocument();
    expect(within(figures).getByText("not scanned")).toBeInTheDocument();
  });

  it("never shows a failed row as clean", async () => {
    mockFetch(
      routes(
        summary([
          row({ image: "bad:1", state: "failed", has_result: false, critical: 0, high: 0, critical_fixable: 0, high_fixable: 0, error: "trivy exited 1: boom" }),
        ]),
      ),
    );
    renderApp("/security");
    const t = await table();
    await within(t).findByText("bad:1");
    const r = within(t).getAllByRole("row")[1];
    expect(r).toHaveTextContent("Scan failed");
    expect(within(r).queryByText("0")).toBeNull();
  });

  it("opens an image's detail from its row", async () => {
    const detail: ImageDetail = {
      image: "nginx:1.25.3",
      digests: [
        {
          key: "sha256:abc",
          state: "scanned",
          last_scan_at: Math.floor(Date.now() / 1000) - 60,
          trivy_image: "aquasec/trivy:0.76.1",
          error: null,
          counts: { critical: 1, high: 0, medium: 0, low: 0, unknown: 0, critical_fixable: 1, high_fixable: 0 },
          findings: [
            {
              vuln_id: "CVE-2024-5535",
              severity: "critical",
              pkg_name: "libssl3",
              installed_version: "3.0.11",
              fixed_version: "3.0.14",
              title: "openssl: SSL_select_next_proto buffer overread",
              url: "https://avd.aquasec.com/nvd/cve-2024-5535",
              first_seen_at: 1,
            },
          ],
          used_by: [{ env: "local", node: "", namespace: "", workload: "", container: "web", running: true }],
        },
      ],
    };
    mockFetch(routes(summary([row({})]), { "GET /api/v1/ee/security/image": { json: detail } }));
    const { history } = renderApp("/security");
    await userEvent.click(await screen.findByRole("link", { name: "nginx:1.25.3" }));
    await waitFor(() => expect(history.location.pathname).toBe("/security/image"));
    expect(await screen.findByRole("link", { name: "CVE-2024-5535" })).toHaveAttribute("href", "https://avd.aquasec.com/nvd/cve-2024-5535");
    expect(screen.getByText("3.0.14")).toBeInTheDocument();
    expect(screen.getByText("web")).toBeInTheDocument();
  });

  it("hides Scan now from viewers", async () => {
    mockFetch(
      routes(summary([row({})]), {
        "GET /api/v1/auth/me": { json: { ...ADMIN, user: { ...ADMIN.user, role: "user" } } },
        "GET /api/v1/envs": { json: [{ ...ENVS[0], access: { role: "viewer", grants: [{ permissions: ["images.view"], namespaces: null }] } }] },
      }),
    );
    renderApp("/security?envs=local");
    await table();
    expect(screen.queryByRole("button", { name: "Scan environment" })).toBeNull();
  });

  it("points admins at the settings when scanning is off", async () => {
    mockFetch(
      routes(summary([], { critical_fixable_running: 0, scanned: 0 }), {
        "GET /api/v1/ee/security/settings": { json: { ...settingsOn, global: { ...settingsOn.global, enabled: false }, envs: [{ ...settingsOn.envs[0], effective_enabled: false }] } },
      }),
    );
    renderApp("/security");
    expect(await screen.findByText("Security scanning is off.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Turn it on" })).toHaveAttribute("href", "/admin/security");
  });

  it("is in the sidebar", async () => {
    mockFetch(routes(summary([])));
    renderApp("/security");
    expect(await screen.findByRole("link", { name: "Security" })).toBeInTheDocument();
  });
});
